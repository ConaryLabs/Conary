// conary-core/src/generation/builder.rs

//! Generation builder — creates EROFS images from system state.
//!
//! This module provides two levels of API:
//!
//! - [`build_erofs_image`]: Low-level function that takes slices of
//!   [`FileEntryRef`] and [`SymlinkEntryRef`] and produces an EROFS image
//!   at the given path. Uses composefs-rs for image building.
//!
//! - [`build_generation_from_db`]: Higher-level function that queries the
//!   database for installed troves and their files, creates a state
//!   snapshot, and builds a complete generation directory with EROFS
//!   image and metadata JSON.

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::db::models::{FileEntry, InstallSource, StateEngine, SystemState, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, BootAssetSources, CasObjectRef, stage_boot_assets,
    write_generation_artifact,
};
use crate::generation::metadata::{
    GENERATION_FORMAT, GenerationMetadata, clear_generation_pending, mark_generation_pending,
};
mod erofs;

pub use erofs::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image, hex_to_digest};

/// Build a complete generation from the current database state.
///
/// This is the high-level entry point that:
/// 1. Queries all installed troves and their file entries
/// 2. Builds the EROFS image via [`build_erofs_image`]
/// 3. Creates a system state snapshot (only after successful image build)
/// 4. Writes generation metadata JSON
///
/// The state snapshot is deliberately created *after* the EROFS image build
/// succeeds. Creating it before would leave an orphaned DB state record if
/// the image build fails.
///
/// Returns `(generation_number, BuildResult)`.
pub fn build_generation_from_db(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
) -> crate::Result<(i64, BuildResult)> {
    build_generation_from_db_with_boot_root(conn, generations_root, summary, Path::new("/boot"))
}

pub fn build_generation_from_db_with_boot_root(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
    boot_root: &Path,
) -> crate::Result<(i64, BuildResult)> {
    struct PendingGenerationGuard {
        gen_dir: PathBuf,
        armed: bool,
    }

    impl PendingGenerationGuard {
        fn new(gen_dir: PathBuf) -> Self {
            Self {
                gen_dir,
                armed: true,
            }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for PendingGenerationGuard {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }

            if let Err(error) = std::fs::remove_dir_all(&self.gen_dir) {
                warn!(
                    "Failed to clean up incomplete generation {}: {}",
                    self.gen_dir.display(),
                    error
                );
            }
        }
    }

    // Step 1: Ensure generations base directory exists
    std::fs::create_dir_all(generations_root).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generations directory {}: {e}",
            generations_root.display()
        ))
    })?;

    // Step 2: Reserve the generation number and create the directory.
    //
    // TOCTOU guard: hold an exclusive advisory lock on the generations
    // directory for the duration of number-allocation + directory-creation.
    // Without this, two concurrent `build_generation_from_db` calls could
    // read the same `next_state_number`, both try to create the same
    // directory, and one would silently overwrite the other's work.
    //
    // The lock is released automatically when `_gen_lock` is dropped at the
    // end of this function (or on any early-return error path).
    let lock_path = generations_root.join(".generation-build.lock");
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| {
            crate::error::Error::IoError(format!(
                "Failed to open generation lock file {}: {e}",
                lock_path.display()
            ))
        })?;
    use fs2::FileExt as _;
    lock_file.lock_exclusive().map_err(|e| {
        crate::error::Error::IoError(format!("Failed to acquire generation build lock: {e}"))
    })?;
    // RAII guard: lock is released when this drops.
    let _gen_lock = lock_file;

    let gen_number = SystemState::next_state_number(conn).map_err(|e| {
        crate::error::Error::InternalError(format!("Failed to determine next state number: {e}"))
    })?;
    let gen_dir = generations_root.join(gen_number.to_string());
    if gen_dir.exists() {
        return Err(crate::error::Error::AlreadyExists(format!(
            "Generation directory already exists: {}",
            gen_dir.display()
        )));
    }
    std::fs::create_dir_all(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generation directory {}: {e}",
            gen_dir.display()
        ))
    })?;
    mark_generation_pending(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to mark generation {} as pending: {e}",
            gen_dir.display()
        ))
    })?;
    let mut pending_guard = PendingGenerationGuard::new(gen_dir.clone());

    // Step 3: Collect file entries from all installed troves (single bulk query).
    // Exclude files belonging to adopted-track troves: those troves are metadata-
    // only and their file records use placeholder hashes that cannot be resolved
    // in the CAS. Filtering here makes the intent explicit and avoids silently
    // relying on hex parse failures to skip them.
    let troves = Trove::list_all(conn)?;
    // Build the adopted-track trove id set so we can exclude their files.
    let adopted_track_ids: std::collections::HashSet<i64> = troves
        .iter()
        .filter(|t| t.install_source == InstallSource::AdoptedTrack)
        .filter_map(|t| t.id)
        .collect();
    let all_files_raw = FileEntry::find_all_ordered(conn)?;
    let all_files: Vec<FileEntry> = all_files_raw
        .into_iter()
        .filter(|f| !adopted_track_ids.contains(&f.trove_id))
        .collect();

    let file_refs: Vec<FileEntryRef> = all_files
        .iter()
        .map(|file| {
            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;

            FileEntryRef {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size,
                permissions,
                owner: file.owner.clone(),
                group_name: file.group_name.clone(),
            }
        })
        .collect();

    // Step 4: Build EROFS image with symlinks from DB.
    // This must succeed before we commit state to the database.
    let symlink_refs = collect_symlink_refs(conn)?;
    let result = build_erofs_image(&file_refs, &symlink_refs, &gen_dir)?;

    // Step 5: Stage boot assets and write the export artifact contract before
    // committing metadata. Export must not scrape live /boot later.
    let architecture = runtime_generation_architecture()?;
    let kernel_version = detect_kernel_version_from_troves(&troves).ok_or_else(|| {
        crate::error::Error::NotFound(
            "could not determine kernel version for generation boot assets".to_string(),
        )
    })?;
    let boot_assets = stage_runtime_boot_assets_from_boot_root(
        &gen_dir,
        gen_number,
        architecture,
        &kernel_version,
        boot_root,
    )?;
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &gen_dir,
        generation: gen_number,
        architecture,
        erofs_path: &result.image_path,
        cas_base_rel: "../../objects",
        cas_objects: cas_objects_from_file_refs(&file_refs),
        boot_assets,
    })?;

    // Step 6: Create system state snapshot at the reserved number -- only
    // after successful image build so we never leave orphaned state records
    // on build failure. Using create_snapshot_at() ensures the DB state
    // number matches the directory number we already created.
    let engine = StateEngine::new(conn);
    let _state = engine
        .create_snapshot_at(gen_number, summary, None, None)
        .map_err(|e| {
            crate::error::Error::InternalError(format!(
                "Failed to create system state snapshot: {e}"
            ))
        })?;

    // Step 7: Write generation metadata
    #[allow(clippy::cast_possible_wrap)]
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(result.image_size as i64),
        cas_objects_referenced: Some(result.cas_objects_referenced as i64),
        fsverity_enabled: false, // Caller can enable separately
        erofs_verity_digest: result.erofs_verity_digest.clone(),
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: troves.len() as i64,
        kernel_version: Some(kernel_version),
        summary: summary.to_string(),
    };
    metadata.write_to(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!("Failed to write generation metadata: {e}"))
    })?;
    clear_generation_pending(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to clear pending marker for generation {}: {e}",
            gen_dir.display()
        ))
    })?;
    pending_guard.disarm();

    info!(
        "Generation {} built: {} CAS objects, {} packages, composefs format",
        gen_number,
        result.cas_objects_referenced,
        troves.len()
    );

    Ok((gen_number, result))
}

/// Rebuild the EROFS image for an existing generation without allocating a
/// new state number. Used by recovery to restore a generation that was already
/// recorded in the database.
///
/// Unlike [`build_generation_from_db`], this does NOT create a new system state
/// snapshot. It only rebuilds the EROFS image and metadata for the specified
/// generation number, using the current DB package state.
pub(crate) fn rebuild_generation_image(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    gen_number: i64,
    summary: &str,
) -> crate::Result<BuildResult> {
    rebuild_generation_image_with_boot_root(
        conn,
        generations_root,
        gen_number,
        summary,
        Path::new("/boot"),
    )
}

pub(crate) fn rebuild_generation_image_with_boot_root(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    gen_number: i64,
    summary: &str,
    boot_root: &Path,
) -> crate::Result<BuildResult> {
    let gen_dir = generations_root.join(gen_number.to_string());
    std::fs::create_dir_all(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generation directory {}: {e}",
            gen_dir.display()
        ))
    })?;

    let troves = Trove::list_all(conn)?;
    let all_files = FileEntry::find_all_ordered(conn)?;

    let file_refs: Vec<FileEntryRef> = all_files
        .iter()
        .map(|file| {
            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;
            FileEntryRef {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size,
                permissions,
                owner: file.owner.clone(),
                group_name: file.group_name.clone(),
            }
        })
        .collect();

    let symlink_refs = collect_symlink_refs(conn)?;
    let result = build_erofs_image(&file_refs, &symlink_refs, &gen_dir)?;
    let architecture = runtime_generation_architecture()?;
    let kernel_version = detect_kernel_version_from_troves(&troves).ok_or_else(|| {
        crate::error::Error::NotFound(
            "could not determine kernel version for generation boot assets".to_string(),
        )
    })?;
    let boot_assets = stage_runtime_boot_assets_from_boot_root(
        &gen_dir,
        gen_number,
        architecture,
        &kernel_version,
        boot_root,
    )?;
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &gen_dir,
        generation: gen_number,
        architecture,
        erofs_path: &result.image_path,
        cas_base_rel: "../../objects",
        cas_objects: cas_objects_from_file_refs(&file_refs),
        boot_assets,
    })?;

    #[allow(clippy::cast_possible_wrap)]
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(result.image_size as i64),
        cas_objects_referenced: Some(result.cas_objects_referenced as i64),
        fsverity_enabled: false,
        erofs_verity_digest: result.erofs_verity_digest.clone(),
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: troves.len() as i64,
        kernel_version: Some(kernel_version),
        summary: summary.to_string(),
    };
    metadata.write_to(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!("Failed to write generation metadata: {e}"))
    })?;

    info!(
        "Generation {} rebuilt in place: {} CAS objects, {} packages",
        gen_number,
        result.cas_objects_referenced,
        troves.len()
    );

    Ok(result)
}

/// Collect symlink entries from all installed troves.
///
/// Queries file entries that have a non-NULL symlink_target and returns them
/// as `SymlinkEntryRef` values suitable for EROFS image building.
///
/// Returns an empty vec if the `file_entries` table does not have a
/// `symlink_target` column (older schema or test databases).
fn collect_symlink_refs(conn: &rusqlite::Connection) -> crate::Result<Vec<SymlinkEntryRef>> {
    let mut stmt = match conn.prepare(
        "SELECT path, symlink_target FROM files \
         WHERE symlink_target IS NOT NULL AND symlink_target != ''",
    ) {
        Ok(s) => s,
        Err(e) => {
            // Column may not exist in pre-v60 schemas.
            debug!("Skipping symlink collection: {e}");
            return Ok(Vec::new());
        }
    };

    let refs = stmt
        .query_map([], |row| {
            Ok(SymlinkEntryRef {
                path: row.get(0)?,
                target: row.get(1)?,
            })
        })
        .map_err(|e| crate::error::Error::InternalError(format!("Failed to query symlinks: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(refs)
}

fn cas_objects_from_file_refs(file_refs: &[FileEntryRef]) -> Vec<CasObjectRef> {
    file_refs
        .iter()
        .map(|file| CasObjectRef {
            sha256: file.sha256_hash.clone(),
            size: file.size,
        })
        .collect()
}

fn runtime_generation_architecture() -> crate::Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Err(crate::error::Error::NotImplemented(
            "aarch64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        "riscv64" => Err(crate::error::Error::NotImplemented(
            "riscv64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        other => Err(crate::error::Error::NotImplemented(format!(
            "unsupported runtime architecture for generation export: {other}"
        ))),
    }
}

fn stage_runtime_boot_assets_from_boot_root(
    gen_dir: &Path,
    generation: i64,
    architecture: &str,
    kernel_version: &str,
    boot_root: &Path,
) -> crate::Result<crate::generation::artifact::BootAssetsManifest> {
    if kernel_version.contains('/') || kernel_version.contains('\\') {
        return Err(crate::error::Error::InvalidPath(format!(
            "kernel version must not contain path separators: {kernel_version}"
        )));
    }

    let kernel = boot_root.join(format!("vmlinuz-{kernel_version}"));
    let initramfs = boot_root.join(format!("initramfs-{kernel_version}.img"));
    let efi_bootloader = boot_root.join("EFI/BOOT/BOOTX64.EFI");
    stage_boot_assets(BootAssetSources {
        generation_dir: gen_dir,
        generation,
        architecture,
        kernel_version,
        kernel: &kernel,
        initramfs: &initramfs,
        efi_bootloader: &efi_bootloader,
    })
}

/// Get kernel version from an already-loaded trove list.
///
/// Looks for kernel-related packages in the trove list, falling back to
/// the running kernel version from `/proc/version`.
pub fn detect_kernel_version_from_troves(troves: &[Trove]) -> Option<String> {
    for trove in troves {
        if trove.name.starts_with("kernel") || trove.name.starts_with("linux-image") {
            return Some(trove.version.clone());
        }
    }
    // Fall back to running kernel version from /proc/sys/kernel/osrelease
    crate::generation::metadata::running_kernel_version()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn build_generation_from_db_writes_export_artifact_contract() {
        use crate::db::models::{FileEntry, Trove, TroveType};
        use crate::db::schema::migrate;
        use crate::filesystem::CasStore;

        let tmp = tempfile::TempDir::new().unwrap();
        let generations_root = tmp.path().join("generations");
        let objects_dir = tmp.path().join("objects");
        let boot_root = tmp.path().join("boot");
        std::fs::create_dir_all(&generations_root).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
        std::fs::write(boot_root.join("initramfs-6.19.8-conary.img"), b"initramfs").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let cas = CasStore::new(&objects_dir).unwrap();
        let hello_hash = cas.store(b"hello").unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut trove = Trove::new(
            "kernel".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = FileEntry::new(
            "/usr/bin/hello".to_string(),
            hello_hash,
            b"hello".len() as i64,
            0o755,
            trove_id,
        );
        file.insert(&conn).unwrap();

        let (generation, _result) = build_generation_from_db_with_boot_root(
            &conn,
            &generations_root,
            "runtime artifact test",
            &boot_root,
        )
        .unwrap();
        let gen_dir = generations_root.join(generation.to_string());

        assert!(gen_dir.join(".conary-artifact.json").is_file());
        assert!(gen_dir.join("cas-manifest.json").is_file());
        assert!(gen_dir.join("boot-assets/manifest.json").is_file());
        assert!(gen_dir.join("boot-assets/vmlinuz").is_file());
        assert!(gen_dir.join("boot-assets/initramfs.img").is_file());
        assert!(gen_dir.join("boot-assets/EFI/BOOT/BOOTX64.EFI").is_file());
        let metadata = GenerationMetadata::read_from(&gen_dir).unwrap();
        assert!(metadata.artifact_manifest_sha256.is_some());
        assert_eq!(metadata.kernel_version.as_deref(), Some("6.19.8-conary"));
        crate::generation::artifact::load_generation_artifact(&gen_dir).unwrap();
    }

    #[test]
    fn detect_kernel_version_does_not_panic() {
        let result = detect_kernel_version_from_troves(&[]);
        assert!(result.is_some() || result.is_none());
    }
}
