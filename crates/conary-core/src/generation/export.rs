// conary-core/src/generation/export.rs

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use crate::generation::artifact::{
    ARTIFACT_MANIFEST_FILE, BOOT_ASSETS_DIR, CAS_MANIFEST_FILE, GenerationArtifact,
    load_generation_artifact, load_installed_generation_artifact,
};
use crate::generation::metadata::{
    EXCLUDED_DIRS, GENERATION_METADATA_FILE, GENERATION_METADATA_SIGNATURE_FILE, ROOT_SYMLINKS,
};

const RUNTIME_ROOT_DIRS: &[&str] = &["usr", "etc", "boot"];
const ESP_SIZE_MB: u64 = 512;
const GPT_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;
const IMAGE_SIZE_MARGIN_BYTES: u64 = 256 * 1024 * 1024;
const EXT4_MINIMIZE_HEADROOM_DIVISOR: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationExportFormat {
    Raw,
    Qcow2,
    Iso,
}

impl FromStr for GenerationExportFormat {
    type Err = crate::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "raw" => Ok(Self::Raw),
            "qcow2" => Ok(Self::Qcow2),
            "iso" => Ok(Self::Iso),
            other => Err(crate::Error::InvalidPath(format!(
                "invalid generation export format {other}; expected raw, qcow2, or iso"
            ))),
        }
    }
}

impl std::fmt::Display for GenerationExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raw => write!(f, "raw"),
            Self::Qcow2 => write!(f, "qcow2"),
            Self::Iso => write!(f, "iso"),
        }
    }
}

pub struct GenerationExportOptions {
    pub generation: Option<i64>,
    pub generation_path: Option<PathBuf>,
    pub format: GenerationExportFormat,
    pub output: PathBuf,
    pub size_bytes: Option<u64>,
}

#[derive(Debug)]
pub struct GenerationExportResult {
    pub path: PathBuf,
    pub format: GenerationExportFormat,
    pub size: u64,
    pub raw_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct GenerationExportTools {
    pub systemd_repart: PathBuf,
    pub qemu_img: PathBuf,
}

impl Default for GenerationExportTools {
    fn default() -> Self {
        Self {
            systemd_repart: PathBuf::from("systemd-repart"),
            qemu_img: PathBuf::from("qemu-img"),
        }
    }
}

pub fn export_generation_image(
    options: GenerationExportOptions,
) -> crate::Result<GenerationExportResult> {
    export_generation_image_with_tools(options, &GenerationExportTools::default())
}

pub fn export_generation_image_with_tools(
    options: GenerationExportOptions,
    tools: &GenerationExportTools,
) -> crate::Result<GenerationExportResult> {
    if options.format == GenerationExportFormat::Iso {
        return Err(crate::Error::NotImplemented(
            "ISO export is reserved on the generation artifact contract but not implemented yet"
                .to_string(),
        ));
    }

    let artifact = load_artifact_for_options(&options)?;
    ensure_export_architecture(&artifact)?;

    match options.format {
        GenerationExportFormat::Raw => export_raw(&artifact, &options, tools),
        GenerationExportFormat::Qcow2 => export_qcow2(&artifact, &options, tools),
        GenerationExportFormat::Iso => unreachable!("ISO returned above"),
    }
}

fn load_artifact_for_options(
    options: &GenerationExportOptions,
) -> crate::Result<GenerationArtifact> {
    match (options.generation, options.generation_path.as_deref()) {
        (Some(_), Some(_)) => Err(crate::Error::InvalidPath(
            "generation number and generation path are mutually exclusive".to_string(),
        )),
        (Some(generation), None) => load_installed_generation_artifact(generation),
        (None, Some(path)) => load_generation_artifact(path),
        (None, None) => load_generation_artifact(Path::new("/conary/current")),
    }
}

fn export_raw(
    artifact: &GenerationArtifact,
    options: &GenerationExportOptions,
    tools: &GenerationExportTools,
) -> crate::Result<GenerationExportResult> {
    let parent = options.output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".conary-generation-export-")
        .tempdir_in(parent)?;
    let rootfs = staging.path().join("rootfs");
    let esp = staging.path().join("esp");
    project_generation_rootfs(artifact, &rootfs)?;
    project_generation_esp(artifact, &esp)?;

    let minimum_size = minimum_image_size_bytes(&rootfs)?;
    let size_bytes = options.size_bytes.unwrap_or(minimum_size);
    if size_bytes < minimum_size {
        return Err(crate::Error::InvalidPath(format!(
            "requested image size {size_bytes} bytes is below minimum {minimum_size} bytes"
        )));
    }

    let definitions = staging.path().join("repart.d");
    let plan = crate::image::repart::DiskImagePlan {
        architecture: crate::bootstrap::TargetArch::X86_64,
        esp_staging_dir: esp,
        root_staging_dir: rootfs,
        output_raw: options.output.clone(),
        size_bytes,
    };
    let size = crate::image::repart::create_raw_image(
        &plan,
        &definitions,
        &tools.systemd_repart,
        ESP_SIZE_MB,
    )
    .map_err(|e| crate::Error::IoError(e.to_string()))?;

    Ok(GenerationExportResult {
        path: options.output.clone(),
        format: GenerationExportFormat::Raw,
        size,
        raw_path: None,
    })
}

fn export_qcow2(
    artifact: &GenerationArtifact,
    options: &GenerationExportOptions,
    tools: &GenerationExportTools,
) -> crate::Result<GenerationExportResult> {
    let raw_tmp = raw_temp_path(&options.output);
    let raw_options = GenerationExportOptions {
        generation: None,
        generation_path: None,
        format: GenerationExportFormat::Raw,
        output: raw_tmp.clone(),
        size_bytes: options.size_bytes,
    };
    let raw_result = match export_raw(artifact, &raw_options, tools) {
        Ok(result) => result,
        Err(error) => {
            let _ = std::fs::remove_file(&raw_tmp);
            return Err(error);
        }
    };

    let output = Command::new(&tools.qemu_img)
        .args(["convert", "-f", "raw", "-O", "qcow2", "-c"])
        .arg(&raw_tmp)
        .arg(&options.output)
        .output()
        .map_err(|e| crate::Error::IoError(format!("failed to run qemu-img: {e}")))?;
    let remove_result = std::fs::remove_file(&raw_tmp);
    if !output.status.success() {
        return Err(crate::Error::IoError(format!(
            "qemu-img convert failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    remove_result?;
    let size = std::fs::metadata(&options.output)?.len();
    Ok(GenerationExportResult {
        path: options.output.clone(),
        format: GenerationExportFormat::Qcow2,
        size,
        raw_path: Some(raw_result.path),
    })
}

fn raw_temp_path(output: &Path) -> PathBuf {
    let mut raw = OsString::from(output.as_os_str());
    raw.push(".raw.tmp");
    PathBuf::from(raw)
}

fn ensure_export_architecture(artifact: &GenerationArtifact) -> crate::Result<()> {
    if artifact.artifact_manifest.architecture == "x86_64" {
        Ok(())
    } else {
        Err(crate::Error::NotImplemented(format!(
            "generation export only supports x86_64, got {}",
            artifact.artifact_manifest.architecture
        )))
    }
}

fn minimum_image_size_bytes(rootfs_staging_dir: &Path) -> crate::Result<u64> {
    let rootfs_size = dir_size(rootfs_staging_dir)?;
    let ext4_headroom = rootfs_size.div_ceil(EXT4_MINIMIZE_HEADROOM_DIVISOR);
    rootfs_size
        .checked_add(ext4_headroom)
        .and_then(|size| size.checked_add(ESP_SIZE_MB * 1024 * 1024))
        .and_then(|size| size.checked_add(GPT_OVERHEAD_BYTES))
        .and_then(|size| size.checked_add(IMAGE_SIZE_MARGIN_BYTES))
        .ok_or_else(|| {
            crate::Error::InternalError(format!(
                "generation export image size overflow for rootfs size {rootfs_size}"
            ))
        })
}

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

fn dir_size(path: &Path) -> crate::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            total += dir_size(&path)?;
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    Ok(total)
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

    #[cfg(unix)]
    fn write_script(path: &Path, content: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, content).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn fake_tools(dir: &Path) -> GenerationExportTools {
        let repart = dir.join("systemd-repart");
        let qemu_img = dir.join("qemu-img");
        let repart_log = dir.join("repart.log");
        let qemu_log = dir.join("qemu.log");
        write_script(
            &repart,
            &format!(
                "#!/bin/sh\nlast=''\nfor arg in \"$@\"; do echo \"$arg\" >> '{}'; last=\"$arg\"; done\nprintf raw > \"$last\"\n",
                repart_log.display()
            ),
        );
        write_script(
            &qemu_img,
            &format!(
                "#!/bin/sh\nprev=''\nlast=''\nfor arg in \"$@\"; do echo \"$arg\" >> '{}'; prev=\"$last\"; last=\"$arg\"; done\nprintf qcow2 > \"$last\"\n",
                qemu_log.display()
            ),
        );
        GenerationExportTools {
            systemd_repart: repart,
            qemu_img,
        }
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

    #[test]
    fn export_format_parsing_reports_allowed_values() {
        let err = GenerationExportFormat::from_str("vmdk").unwrap_err();
        assert!(err.to_string().contains("raw, qcow2, or iso"));
        assert_eq!(
            GenerationExportFormat::from_str("raw").unwrap(),
            GenerationExportFormat::Raw
        );
        assert_eq!(
            GenerationExportFormat::from_str("qcow2").unwrap(),
            GenerationExportFormat::Qcow2
        );
        assert_eq!(
            GenerationExportFormat::from_str("iso").unwrap(),
            GenerationExportFormat::Iso
        );
    }

    #[test]
    fn iso_export_returns_reserved_error_without_loading_artifact() {
        let err = export_generation_image(GenerationExportOptions {
            generation: None,
            generation_path: Some(PathBuf::from("/does/not/exist")),
            format: GenerationExportFormat::Iso,
            output: PathBuf::from("out.iso"),
            size_bytes: None,
        })
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("ISO export is reserved on the generation artifact contract")
        );
    }

    #[test]
    fn minimum_size_includes_fixed_overhead_and_margin() {
        let fixture = Fixture::new();
        let artifact = fixture.artifact();
        let staging = fixture._tmp.path().join("minimum-rootfs");
        project_generation_rootfs(&artifact, &staging).unwrap();

        let minimum = minimum_image_size_bytes(&staging).unwrap();

        assert!(
            minimum >= (ESP_SIZE_MB * 1024 * 1024) + GPT_OVERHEAD_BYTES + IMAGE_SIZE_MARGIN_BYTES
        );
        assert!(minimum > dir_size(&staging).unwrap());
    }

    #[test]
    fn minimum_size_scales_for_ext4_minimize_headroom() {
        let tmp = TempDir::new().unwrap();
        let rootfs = tmp.path().join("large-rootfs");
        std::fs::create_dir_all(&rootfs).unwrap();
        let large_file = std::fs::File::create(rootfs.join("large-cas-object")).unwrap();
        large_file.set_len(7 * 1024 * 1024 * 1024).unwrap();

        let minimum = minimum_image_size_bytes(&rootfs).unwrap();

        assert!(
            minimum >= 11 * 1024 * 1024 * 1024,
            "7GiB rootfs should default to an image large enough for ext4 metadata; got {minimum}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn undersized_export_reports_requested_and_minimum_sizes() {
        let fixture = Fixture::new();
        let tools = fake_tools(fixture._tmp.path());
        let output = fixture._tmp.path().join("undersized.raw");

        let err = export_generation_image_with_tools(
            GenerationExportOptions {
                generation: None,
                generation_path: Some(fixture.generation_dir.clone()),
                format: GenerationExportFormat::Raw,
                output,
                size_bytes: Some(1),
            },
            &tools,
        )
        .unwrap_err();

        assert!(err.to_string().contains("requested image size 1 bytes"));
        assert!(err.to_string().contains("minimum"));
    }

    #[cfg(unix)]
    #[test]
    fn raw_export_calls_shared_repart_backend_and_cleans_staging() {
        let fixture = Fixture::new();
        let tools = fake_tools(fixture._tmp.path());
        let output = fixture._tmp.path().join("gen.raw");

        let result = export_generation_image_with_tools(
            GenerationExportOptions {
                generation: None,
                generation_path: Some(fixture.generation_dir.clone()),
                format: GenerationExportFormat::Raw,
                output: output.clone(),
                size_bytes: Some(1024 * 1024 * 1024),
            },
            &tools,
        )
        .unwrap();

        assert_eq!(result.path, output);
        assert_eq!(result.format, GenerationExportFormat::Raw);
        assert!(result.size > 0);
        assert!(output.is_file());
        let repart_log = std::fs::read_to_string(fixture._tmp.path().join("repart.log")).unwrap();
        assert!(repart_log.contains("--root=/"));
        let output_path = output.to_string_lossy().into_owned();
        assert!(repart_log.contains(&output_path));
        assert!(
            !std::fs::read_dir(fixture._tmp.path())
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".conary-generation-export-"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn raw_export_passes_4k_aligned_size_to_repart() {
        let fixture = Fixture::new();
        let tools = fake_tools(fixture._tmp.path());
        let output = fixture._tmp.path().join("aligned.raw");

        export_generation_image_with_tools(
            GenerationExportOptions {
                generation: None,
                generation_path: Some(fixture.generation_dir.clone()),
                format: GenerationExportFormat::Raw,
                output,
                size_bytes: Some(1024 * 1024 * 1024 + 1),
            },
            &tools,
        )
        .unwrap();

        let repart_log = std::fs::read_to_string(fixture._tmp.path().join("repart.log")).unwrap();
        assert!(repart_log.lines().any(|line| line == "--size=1073745920"));
    }

    #[cfg(unix)]
    #[test]
    fn qcow2_export_converts_raw_and_removes_temp_raw() {
        let fixture = Fixture::new();
        let tools = fake_tools(fixture._tmp.path());
        let output = fixture._tmp.path().join("gen.qcow2");
        let raw_tmp = raw_temp_path(&output);

        let result = export_generation_image_with_tools(
            GenerationExportOptions {
                generation: None,
                generation_path: Some(fixture.generation_dir.clone()),
                format: GenerationExportFormat::Qcow2,
                output: output.clone(),
                size_bytes: Some(1024 * 1024 * 1024),
            },
            &tools,
        )
        .unwrap();

        assert_eq!(result.path, output);
        assert_eq!(result.format, GenerationExportFormat::Qcow2);
        assert!(output.is_file());
        assert!(!raw_tmp.exists());
        let qemu_log = std::fs::read_to_string(fixture._tmp.path().join("qemu.log")).unwrap();
        assert!(qemu_log.contains("convert"));
        assert!(qemu_log.contains("-O"));
        assert!(qemu_log.contains("qcow2"));
    }
}
