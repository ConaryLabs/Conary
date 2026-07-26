// conary-core/src/bootstrap/image/erofs_generation.rs

//! Exact composefs-native bootstrap generation output.

use super::{ImageBuilder, ImageError, ImageFormat, ImageResult, dir_size};
use crate::db::models::{ExistingDirectoryMaterialization, FileEntry, Trove, TroveType};
use crate::db::schema::ensure_current;
use crate::filesystem::CasStore;
use crate::generation::artifact::{
    ArtifactWriteInputs, BootAssetSources, CasObjectVerification, stage_boot_assets,
    write_generation_artifact,
};
use crate::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
use crate::generation::root_manifest::{build_erofs_image_from_root_manifest, scan_selected_root};
use std::fs;

impl ImageBuilder {
    /// Build composefs-native output: EROFS image + CAS store + SQLite DB.
    ///
    /// This produces the same artifact type as a runtime generation. The
    /// bootstrap output becomes "generation 1." The output directory contains:
    ///
    /// - `objects/` -- CAS store with all file content
    /// - `generations/1/root.erofs` -- EROFS image referencing CAS objects
    /// - `generations/1/.conary-gen.json` -- generation metadata
    /// - `generations/1/.conary-artifact.json` -- export artifact contract
    /// - `generations/1/cas-manifest.json` -- scoped CAS object list
    /// - `generations/1/boot-assets/` -- kernel, initramfs, and EFI bootloader
    /// - `db.sqlite3` -- SQLite database with trove + file records
    ///
    /// No external tools are needed -- composefs-rs builds EROFS in userspace.
    pub(super) fn build_erofs_generation(&mut self) -> Result<ImageResult, ImageError> {
        self.log_line("Building composefs-native output (EROFS + CAS + DB)");

        let output_dir = self.output.clone();
        fs::create_dir_all(&output_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create output dir: {e}")))?;

        let objects_dir = output_dir.join("objects");
        let generations_dir = output_dir.join("generations");
        let gen_dir = generations_dir.join("1");
        fs::create_dir_all(&gen_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create gen dir: {e}")))?;

        self.log_line("Capturing exact sysroot manifests and CAS content");
        let cas = CasStore::new(&objects_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create CAS store: {e}")))?;
        let captured = scan_selected_root(&self.sysroot, &cas).map_err(|e| {
            ImageError::CreationFailed(format!("Failed to capture exact sysroot: {e}"))
        })?;
        let file_count = captured.generation.entries.len() + captured.state.entries.len();
        self.log_line(&format!("Captured {file_count} payload nodes"));

        self.log_line("Creating SQLite database with schema");
        let db_path = output_dir.join("db.sqlite3");
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create database: {e}")))?;
        ensure_current(&conn)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to initialize schema: {e}")))?;

        self.log_line("Inserting trove and exact payload-node records");
        let mut trove = Trove::new(
            "conaryos-base".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.description = Some("conaryOS base system (bootstrapped from LFS)".to_string());
        trove.architecture = Some(self.config.target_arch.to_string());
        let trove_id = trove
            .insert(&conn)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to insert trove: {e}")))?;

        let installed_entries = captured
            .generation
            .entries
            .iter()
            .chain(&captured.state.entries)
            .map(|entry| {
                FileEntry::new(
                    entry.path.clone(),
                    entry.node.clone(),
                    entry.content.clone(),
                    trove_id,
                )
            })
            .collect::<Vec<_>>();
        FileEntry::batch_insert(
            &conn,
            &installed_entries,
            ExistingDirectoryMaterialization::ApplyIncoming,
        )
        .map_err(|e| {
            ImageError::CreationFailed(format!("Failed to insert exact file entries: {e}"))
        })?;

        self.log_line("Building EROFS image from exact root manifest");
        let build_result = build_erofs_image_from_root_manifest(&captured.generation, &gen_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to build EROFS image: {e}")))?;
        captured.state.write_to(&gen_dir).map_err(|e| {
            ImageError::CreationFailed(format!("Failed to write mutable-state manifest: {e}"))
        })?;

        self.log_line("Staging boot assets");
        let architecture = self.config.target_arch.to_string();
        let boot_kernel_version = crate::generation::metadata::detect_kernel_version(&self.sysroot)
            .map_err(|error| {
                ImageError::CreationFailed(format!(
                    "Failed to resolve exact bootstrap kernel release: {error}"
                ))
            })?
            .ok_or_else(|| {
                ImageError::CreationFailed(format!(
                    "bootstrap sysroot {} has no exact /usr/lib/modules/<release> kernel authority",
                    self.sysroot.display()
                ))
            })?;
        let kernel_src = self.sysroot.join("boot/vmlinuz");
        let initramfs_src = self.sysroot.join("boot/initramfs.img");
        let efi_bootloader_src = self.sysroot.join("boot/EFI/BOOT/BOOTX64.EFI");
        if !efi_bootloader_src.exists() {
            return Err(ImageError::CreationFailed(format!(
                "bootstrap sysroot is missing systemd-boot at {}; copy \
                 /usr/lib/systemd/boot/efi/systemd-bootx64.efi into \
                 /boot/EFI/BOOT/BOOTX64.EFI before building exportable EROFS output",
                efi_bootloader_src.display()
            )));
        }
        let boot_assets = stage_boot_assets(BootAssetSources {
            generation_dir: &gen_dir,
            generation: 1,
            architecture: &architecture,
            kernel_version: &boot_kernel_version,
            kernel: &kernel_src,
            initramfs: &initramfs_src,
            efi_bootloader: &efi_bootloader_src,
        })
        .map_err(|e| ImageError::CreationFailed(format!("Failed to stage boot assets: {e}")))?;

        self.log_line("Writing generation artifact manifests");
        let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
            generation_dir: &gen_dir,
            generation: 1,
            architecture: &architecture,
            erofs_path: &build_result.image_path,
            cas_base_rel: "../../objects",
            cas_verification: CasObjectVerification::AlreadyVerified,
            boot_assets,
        })
        .map_err(|e| {
            ImageError::CreationFailed(format!("Failed to write generation artifact: {e}"))
        })?;

        self.log_line("Writing generation metadata");
        #[allow(clippy::cast_possible_wrap)]
        let metadata = GenerationMetadata {
            generation: 1,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(build_result.image_size as i64),
            cas_objects_referenced: Some(build_result.cas_objects_referenced as i64),
            fsverity_enabled: false,
            erofs_verity_digest: build_result.erofs_verity_digest.clone(),
            artifact_manifest_sha256: Some(artifact_manifest_sha256),
            security_capability_xattr_count: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            package_count: 1,
            kernel_version: Some(boot_kernel_version),
            summary: "Bootstrap generation 1 (LFS base system)".to_string(),
        };
        metadata.write_to(&gen_dir).map_err(|e| {
            ImageError::CreationFailed(format!("Failed to write generation metadata: {e}"))
        })?;

        #[cfg(unix)]
        {
            let current_link = output_dir.join("current");
            let _ = fs::remove_file(&current_link);
            std::os::unix::fs::symlink("generations/1", &current_link).map_err(|e| {
                ImageError::CreationFailed(format!("Failed to create current symlink: {e}"))
            })?;
        }

        drop(conn);

        let erofs_size = build_result.image_size;
        let objects_size = dir_size(&objects_dir);
        let db_size = fs::metadata(&db_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let total_size = erofs_size + objects_size + db_size;

        self.log_line(&format!(
            "Generation 1 complete: EROFS={erofs_size} bytes, CAS={objects_size} bytes, \
             DB={db_size} bytes, files={file_count}"
        ));

        Ok(ImageResult {
            path: output_dir,
            format: ImageFormat::Erofs,
            size: total_size,
            efi_bootable: false,
            bios_bootable: false,
            method: "composefs-rs".to_string(),
            partitions: vec![
                format!("CAS objects ({objects_size} bytes)"),
                format!("EROFS image ({erofs_size} bytes)"),
                format!("SQLite DB ({db_size} bytes)"),
            ],
        })
    }
}
