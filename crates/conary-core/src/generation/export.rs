// conary-core/src/generation/export.rs

use std::path::{Path, PathBuf};

use crate::generation::artifact::{
    ARTIFACT_MANIFEST_FILE, BOOT_ASSETS_DIR, CAS_MANIFEST_FILE, GenerationArtifact,
};
use crate::generation::metadata::{
    EXCLUDED_DIRS, GENERATION_METADATA_FILE, GENERATION_METADATA_SIGNATURE_FILE, ROOT_SYMLINKS,
};

const RUNTIME_ROOT_DIRS: &[&str] = &["usr", "etc", "boot"];

pub fn project_generation_rootfs(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
) -> crate::Result<PathBuf> {
    std::fs::create_dir_all(staging_dir)?;

    let generation_rel = PathBuf::from("conary")
        .join("generations")
        .join(artifact.generation.to_string());
    let generation_dest = staging_dir.join(&generation_rel);
    std::fs::create_dir_all(&generation_dest)?;

    copy_file(&artifact.erofs_path, &generation_dest.join("root.erofs"))?;
    copy_file(
        &artifact.generation_dir.join(GENERATION_METADATA_FILE),
        &generation_dest.join(GENERATION_METADATA_FILE),
    )?;
    let signature = artifact
        .generation_dir
        .join(GENERATION_METADATA_SIGNATURE_FILE);
    if signature.exists() {
        copy_file(
            &signature,
            &generation_dest.join(GENERATION_METADATA_SIGNATURE_FILE),
        )?;
    }
    copy_file(
        &artifact.generation_dir.join(ARTIFACT_MANIFEST_FILE),
        &generation_dest.join(ARTIFACT_MANIFEST_FILE),
    )?;
    copy_file(
        &artifact.generation_dir.join(CAS_MANIFEST_FILE),
        &generation_dest.join(CAS_MANIFEST_FILE),
    )?;
    copy_dir_recursive(
        &artifact.generation_dir.join(BOOT_ASSETS_DIR),
        &generation_dest.join(BOOT_ASSETS_DIR),
    )?;

    let objects_dest = staging_dir.join("conary/objects");
    for object in &artifact.cas_objects {
        let source = crate::filesystem::object_path(&artifact.cas_dir, &object.sha256)?;
        let dest = crate::filesystem::object_path(&objects_dest, &object.sha256)?;
        copy_file(&source, &dest)?;
    }

    create_current_symlink(staging_dir, &artifact.generation.to_string())?;
    std::fs::create_dir_all(staging_dir.join("conary/etc-state"))?;

    for dir in RUNTIME_ROOT_DIRS.iter().chain(EXCLUDED_DIRS.iter()) {
        std::fs::create_dir_all(staging_dir.join(dir))?;
    }
    create_root_symlinks(staging_dir)?;

    Ok(staging_dir.to_path_buf())
}

pub fn project_generation_esp(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
) -> crate::Result<PathBuf> {
    if artifact.artifact_manifest.architecture != "x86_64" {
        return Err(crate::Error::NotImplemented(format!(
            "generation export only supports x86_64 ESP projection, got {}",
            artifact.artifact_manifest.architecture
        )));
    }

    std::fs::create_dir_all(staging_dir)?;
    let boot_assets_dir = artifact.generation_dir.join(BOOT_ASSETS_DIR);
    copy_file(
        &boot_assets_dir.join(&artifact.boot_assets.efi_bootloader),
        &staging_dir.join("EFI/BOOT/BOOTX64.EFI"),
    )?;
    copy_file(
        &boot_assets_dir.join(&artifact.boot_assets.kernel),
        &staging_dir.join("vmlinuz"),
    )?;
    copy_file(
        &boot_assets_dir.join(&artifact.boot_assets.initramfs),
        &staging_dir.join("initramfs.img"),
    )?;

    let loader_dir = staging_dir.join("loader");
    let entries_dir = loader_dir.join("entries");
    std::fs::create_dir_all(&entries_dir)?;
    std::fs::write(
        loader_dir.join("loader.conf"),
        format!(
            "default conary-gen-{}\ntimeout 3\nconsole-mode max\neditor no\n",
            artifact.generation
        ),
    )?;
    std::fs::write(
        entries_dir.join(format!("conary-gen-{}.conf", artifact.generation)),
        format!(
            "title      Conary Generation {0}\n\
             linux      /vmlinuz\n\
             initrd     /initramfs.img\n\
             options    root=PARTLABEL=CONARY_ROOT rootfstype={1} rw conary.generation={0} console=tty0 console=ttyS0\n\
             sort-key   conary-{0}\n",
            artifact.generation,
            crate::image::repart::BLS_ROOTFSTYPE
        ),
    )?;

    Ok(staging_dir.to_path_buf())
}

fn copy_file(source: &Path, dest: &Path) -> crate::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, dest).map_err(|e| {
        crate::Error::IoError(format!(
            "failed to copy {} to {}: {e}",
            source.display(),
            dest.display()
        ))
    })?;
    Ok(())
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> crate::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            return Err(crate::Error::InvalidPath(format!(
                "refusing to project symlink from {}",
                source_path.display()
            )));
        }
        if metadata.is_dir() {
            copy_dir_recursive(&source_path, &dest_path)?;
        } else if metadata.is_file() {
            copy_file(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_current_symlink(staging_dir: &Path, generation: &str) -> crate::Result<()> {
    let link = staging_dir.join("conary/current");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(PathBuf::from("generations").join(generation), link)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_current_symlink(staging_dir: &Path, generation: &str) -> crate::Result<()> {
    std::fs::write(
        staging_dir.join("conary/current"),
        format!("generations/{generation}\n"),
    )?;
    Ok(())
}

#[cfg(unix)]
fn create_root_symlinks(staging_dir: &Path) -> crate::Result<()> {
    for (link, target) in ROOT_SYMLINKS {
        let link_path = staging_dir.join(link);
        let _ = std::fs::remove_file(&link_path);
        std::os::unix::fs::symlink(target, link_path)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_root_symlinks(staging_dir: &Path) -> crate::Result<()> {
    for (link, target) in ROOT_SYMLINKS {
        std::fs::write(staging_dir.join(link), format!("{target}\n"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::artifact::{
        ArtifactWriteInputs, BootAssetsManifest, CasObjectRef, write_generation_artifact,
    };
    use crate::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    struct Fixture {
        _tmp: TempDir,
        generation_dir: PathBuf,
        objects_dir: PathBuf,
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn write_cas_object(objects_dir: &Path, bytes: &[u8]) -> CasObjectRef {
        let sha256 = digest(bytes);
        let object_path = crate::filesystem::object_path(objects_dir, &sha256).unwrap();
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(object_path, bytes).unwrap();
        CasObjectRef {
            sha256,
            size: bytes.len() as u64,
        }
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let artifact_root = tmp.path().join("artifact");
            let generation_dir = artifact_root.join("generations/7");
            let objects_dir = artifact_root.join("objects");
            let boot_assets_dir = generation_dir.join(BOOT_ASSETS_DIR);
            std::fs::create_dir_all(boot_assets_dir.join("EFI/BOOT")).unwrap();
            std::fs::create_dir_all(&objects_dir).unwrap();
            std::fs::write(generation_dir.join("root.erofs"), b"root-erofs").unwrap();
            std::fs::write(boot_assets_dir.join("vmlinuz"), b"kernel").unwrap();
            std::fs::write(boot_assets_dir.join("initramfs.img"), b"initramfs").unwrap();
            std::fs::write(boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

            let cas_object = write_cas_object(&objects_dir, b"hello");
            let boot_assets = BootAssetsManifest {
                version: 1,
                generation: 7,
                architecture: "x86_64".to_string(),
                kernel_version: "6.19.8-conary".to_string(),
                kernel: "vmlinuz".to_string(),
                kernel_sha256: digest(b"kernel"),
                initramfs: "initramfs.img".to_string(),
                initramfs_sha256: digest(b"initramfs"),
                efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
                efi_bootloader_sha256: digest(b"efi"),
                created_at: "2026-04-22T00:00:00Z".to_string(),
            };
            let artifact_digest = write_generation_artifact(ArtifactWriteInputs {
                generation_dir: &generation_dir,
                generation: 7,
                architecture: "x86_64",
                erofs_path: &generation_dir.join("root.erofs"),
                cas_base_rel: "../../objects",
                cas_objects: vec![cas_object],
                boot_assets,
            })
            .unwrap();
            GenerationMetadata {
                generation: 7,
                format: GENERATION_FORMAT.to_string(),
                erofs_size: Some(10),
                cas_objects_referenced: Some(1),
                fsverity_enabled: false,
                erofs_verity_digest: None,
                artifact_manifest_sha256: Some(artifact_digest),
                created_at: "2026-04-22T00:00:00Z".to_string(),
                package_count: 1,
                kernel_version: Some("6.19.8-conary".to_string()),
                summary: "fixture".to_string(),
            }
            .write_to(&generation_dir)
            .unwrap();

            Self {
                _tmp: tmp,
                generation_dir,
                objects_dir,
            }
        }

        fn artifact(&self) -> GenerationArtifact {
            crate::generation::artifact::load_generation_artifact(&self.generation_dir).unwrap()
        }
    }

    #[test]
    fn rootfs_projection_creates_runtime_tree() {
        let fixture = Fixture::new();
        let artifact = fixture.artifact();
        let staging = fixture._tmp.path().join("rootfs");

        project_generation_rootfs(&artifact, &staging).unwrap();

        let gen_dir = staging.join("conary/generations/7");
        assert!(gen_dir.join("root.erofs").is_file());
        assert!(gen_dir.join(".conary-gen.json").is_file());
        assert!(gen_dir.join(".conary-artifact.json").is_file());
        assert!(gen_dir.join("cas-manifest.json").is_file());
        assert!(gen_dir.join("boot-assets/manifest.json").is_file());
        assert!(staging.join("conary/etc-state").is_dir());
        assert!(staging.join("usr").is_dir());
        assert!(staging.join("etc").is_dir());
        assert!(staging.join("boot").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn rootfs_projection_creates_current_and_usr_merge_symlinks() {
        let fixture = Fixture::new();
        let artifact = fixture.artifact();
        let staging = fixture._tmp.path().join("rootfs-links");

        project_generation_rootfs(&artifact, &staging).unwrap();

        assert_eq!(
            std::fs::read_link(staging.join("conary/current")).unwrap(),
            PathBuf::from("generations/7")
        );
        for (link, target) in ROOT_SYMLINKS {
            assert_eq!(
                std::fs::read_link(staging.join(link)).unwrap(),
                PathBuf::from(target)
            );
        }
    }

    #[test]
    fn rootfs_projection_copies_only_manifest_listed_cas_objects() {
        let fixture = Fixture::new();
        let extra = write_cas_object(&fixture.objects_dir, b"extra");
        let artifact = fixture.artifact();
        let staging = fixture._tmp.path().join("rootfs-cas");

        project_generation_rootfs(&artifact, &staging).unwrap();

        let objects = staging.join("conary/objects");
        assert!(
            crate::filesystem::object_path(&objects, &artifact.cas_objects[0].sha256)
                .unwrap()
                .is_file()
        );
        assert!(
            !crate::filesystem::object_path(&objects, &extra.sha256)
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn esp_projection_writes_systemd_boot_contract() {
        let fixture = Fixture::new();
        let artifact = fixture.artifact();
        let staging = fixture._tmp.path().join("esp");

        project_generation_esp(&artifact, &staging).unwrap();

        assert!(staging.join("EFI/BOOT/BOOTX64.EFI").is_file());
        assert!(staging.join("vmlinuz").is_file());
        assert!(staging.join("initramfs.img").is_file());

        let loader_conf = std::fs::read_to_string(staging.join("loader/loader.conf")).unwrap();
        assert!(loader_conf.contains("default conary-gen-7"));
        assert!(loader_conf.contains("timeout 3"));
        assert!(loader_conf.contains("console-mode max"));
        assert!(loader_conf.contains("editor no"));

        let bls =
            std::fs::read_to_string(staging.join("loader/entries/conary-gen-7.conf")).unwrap();
        assert!(bls.contains("root=PARTLABEL=CONARY_ROOT"));
        assert!(bls.contains("rootfstype=ext4"));
        assert!(bls.contains(" rw "));
        assert!(bls.contains("conary.generation=7"));
        assert!(bls.contains("console=tty0"));
        assert!(bls.contains("console=ttyS0"));
        assert!(bls.contains("sort-key   conary-7"));
    }

    #[test]
    fn esp_projection_rejects_unsupported_architectures() {
        let fixture = Fixture::new();
        let mut artifact = fixture.artifact();
        artifact.artifact_manifest.architecture = "aarch64".to_string();
        let staging = fixture._tmp.path().join("esp-unsupported");

        let err = project_generation_esp(&artifact, &staging).unwrap_err();

        assert!(err.to_string().contains("only supports x86_64"));
        assert!(!staging.join("EFI").exists());
    }
}
