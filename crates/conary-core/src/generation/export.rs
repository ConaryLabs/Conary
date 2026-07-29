// conary-core/src/generation/export.rs

use std::ffi::OsString;
use std::fs::File;
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
use crate::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_FILE, MUTABLE_STATE_MANIFEST_FILE, apply_resolved_payload_metadata,
    materialize_state_root,
};
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};

const RUNTIME_ROOT_DIRS: &[&str] = &["usr", "etc", "boot"];
const ESP_SIZE_MB: u64 = 512;
const GPT_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;
const IMAGE_SIZE_MARGIN_BYTES: u64 = 256 * 1024 * 1024;
const EXT4_MINIMIZE_HEADROOM_DIVISOR: u64 = 2;
const ISO_VOLUME_ID: &str = "CONARY_ISO";
const ISO_EFI_IMAGE_REL: &str = "EFI/efiboot.img";
const ISO_EFI_IMAGE_SIZE_BYTES: u64 = 64 * 1024 * 1024;
const DISABLE_SOURCE_FSTAB_OPTION: &str = "fstab=no";
const WRITABLE_ESP_MOUNT_OPTION: &str =
    "systemd.mount-extra=PARTLABEL=CONARY_ESP:/boot:vfat:defaults,noatime";

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
    pub provenance_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct GenerationExportTools {
    pub systemd_repart: PathBuf,
    pub qemu_img: PathBuf,
    pub xorriso: PathBuf,
    pub mkfs_vfat: PathBuf,
    pub mmd: PathBuf,
    pub mcopy: PathBuf,
}

impl Default for GenerationExportTools {
    fn default() -> Self {
        Self {
            systemd_repart: PathBuf::from("systemd-repart"),
            qemu_img: PathBuf::from("qemu-img"),
            xorriso: PathBuf::from("xorriso"),
            mkfs_vfat: PathBuf::from("mkfs.vfat"),
            mmd: PathBuf::from("mmd"),
            mcopy: PathBuf::from("mcopy"),
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
    let artifact = load_artifact_for_options(&options)?;
    ensure_export_architecture(&artifact)?;

    match options.format {
        GenerationExportFormat::Raw => export_raw(&artifact, &options, tools),
        GenerationExportFormat::Qcow2 => export_qcow2(&artifact, &options, tools),
        GenerationExportFormat::Iso => export_iso(&artifact, &options, tools),
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
        architecture: crate::image::arch::TargetArch::X86_64,
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
    let provenance_path =
        write_output_provenance(artifact, GenerationExportFormat::Raw, &options.output, size)?;

    Ok(GenerationExportResult {
        path: options.output.clone(),
        format: GenerationExportFormat::Raw,
        size,
        raw_path: None,
        provenance_path: Some(provenance_path),
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
            let _ = std::fs::remove_file(output_provenance_path(
                &raw_tmp,
                GenerationExportFormat::Raw,
            ));
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
    let remove_provenance_result = std::fs::remove_file(output_provenance_path(
        &raw_tmp,
        GenerationExportFormat::Raw,
    ));
    if !output.status.success() {
        return Err(crate::Error::IoError(format!(
            "qemu-img convert failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    remove_result?;
    remove_provenance_result?;
    let size = std::fs::metadata(&options.output)?.len();
    let provenance_path = write_output_provenance(
        artifact,
        GenerationExportFormat::Qcow2,
        &options.output,
        size,
    )?;
    Ok(GenerationExportResult {
        path: options.output.clone(),
        format: GenerationExportFormat::Qcow2,
        size,
        raw_path: Some(raw_result.path),
        provenance_path: Some(provenance_path),
    })
}

fn export_iso(
    artifact: &GenerationArtifact,
    options: &GenerationExportOptions,
    tools: &GenerationExportTools,
) -> crate::Result<GenerationExportResult> {
    let parent = options.output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".conary-generation-iso-")
        .tempdir_in(parent)?;
    let iso_root = staging.path().join("iso-root");
    project_generation_rootfs(artifact, &iso_root)?;

    let efi_staging = staging.path().join("efi-staging");
    project_generation_iso_esp(artifact, &efi_staging)?;
    let efi_image = iso_root.join(ISO_EFI_IMAGE_REL);
    if let Some(parent) = efi_image.parent() {
        std::fs::create_dir_all(parent)?;
    }
    File::create(&efi_image)?.set_len(ISO_EFI_IMAGE_SIZE_BYTES)?;

    run_mkfs_vfat(tools, &efi_image)?;
    run_mmd(tools, &efi_image, "::/EFI")?;
    run_mmd(tools, &efi_image, "::/EFI/BOOT")?;
    run_mmd(tools, &efi_image, "::/loader")?;
    run_mmd(tools, &efi_image, "::/loader/entries")?;
    run_mcopy(
        tools,
        &efi_image,
        &efi_staging.join("EFI/BOOT/BOOTX64.EFI"),
        "::/EFI/BOOT/BOOTX64.EFI",
    )?;
    run_mcopy(
        tools,
        &efi_image,
        &efi_staging.join("vmlinuz"),
        "::/vmlinuz",
    )?;
    run_mcopy(
        tools,
        &efi_image,
        &efi_staging.join("initramfs.img"),
        "::/initramfs.img",
    )?;
    run_mcopy(
        tools,
        &efi_image,
        &efi_staging.join("loader/loader.conf"),
        "::/loader/loader.conf",
    )?;
    run_mcopy(
        tools,
        &efi_image,
        &efi_staging.join(format!(
            "loader/entries/conary-gen-{}.conf",
            artifact.generation
        )),
        &format!("::/loader/entries/conary-gen-{}.conf", artifact.generation),
    )?;
    run_xorriso(tools, &iso_root, &options.output)?;

    let size = std::fs::metadata(&options.output)?.len();
    let provenance_path =
        write_output_provenance(artifact, GenerationExportFormat::Iso, &options.output, size)?;
    Ok(GenerationExportResult {
        path: options.output.clone(),
        format: GenerationExportFormat::Iso,
        size,
        raw_path: None,
        provenance_path: Some(provenance_path),
    })
}

fn raw_temp_path(output: &Path) -> PathBuf {
    let mut raw = OsString::from(output.as_os_str());
    raw.push(".raw.tmp");
    PathBuf::from(raw)
}

fn output_provenance_path(output: &Path, format: GenerationExportFormat) -> PathBuf {
    output.with_extension(format!("{format}.conary-provenance.json"))
}

fn write_output_provenance(
    artifact: &GenerationArtifact,
    format: GenerationExportFormat,
    output: &Path,
    size: u64,
) -> crate::Result<PathBuf> {
    let provenance_path = output_provenance_path(output, format);
    let artifact_manifest_sha256 = artifact
        .metadata
        .artifact_manifest_sha256
        .as_deref()
        .ok_or_else(|| {
            crate::Error::InvalidPath(
                "exported generation metadata is missing artifact_manifest_sha256".to_string(),
            )
        })?;
    let output_sha256 = sha256_file(output)?;
    let manifest = serde_json::json!({
        "version": 1,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "generation": artifact.generation,
        "architecture": artifact.artifact_manifest.architecture,
        "format": format.to_string(),
        "source": {
            "generation_metadata": GENERATION_METADATA_FILE,
            "artifact_manifest": ARTIFACT_MANIFEST_FILE,
            "artifact_manifest_sha256": artifact_manifest_sha256,
            "cas_manifest": CAS_MANIFEST_FILE,
            "cas_manifest_sha256": artifact.artifact_manifest.cas_manifest_sha256,
            "boot_assets_manifest": artifact.artifact_manifest.boot_assets,
            "boot_assets_sha256": artifact.artifact_manifest.boot_assets_sha256,
        },
        "output": {
            "path": output.display().to_string(),
            "size": size,
            "sha256": output_sha256,
        },
    });
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(&provenance_path, bytes)?;
    Ok(provenance_path)
}

fn sha256_file(path: &Path) -> crate::Result<String> {
    let mut file = File::open(path).map_err(|e| {
        crate::Error::IoError(format!(
            "failed to open {} for SHA-256: {e}",
            path.display()
        ))
    })?;
    crate::hash::sha256_reader_hex(&mut file).map_err(|e| {
        crate::Error::IoError(format!(
            "failed to hash {} with SHA-256: {e}",
            path.display()
        ))
    })
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
    project_generation_rootfs_with_security_apply(
        artifact,
        staging_dir,
        |capability, objects_dest| {
            capability.apply_to_tree(objects_dest).map_err(|error| {
                crate::Error::IoError(format!(
                    "failed to apply generation carrier immutable-backing security authority: {error}"
                ))
            })
        },
    )
}

fn project_generation_rootfs_with_security_apply(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
    apply_security: impl FnOnce(&crate::ccs::ImmutableBackingSecurity, &Path) -> crate::Result<()>,
) -> crate::Result<PathBuf> {
    validate_carrier_root_metadata(artifact)?;
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
    copy_file(
        &artifact.generation_dir.join(GENERATION_ROOT_MANIFEST_FILE),
        &generation_dest.join(GENERATION_ROOT_MANIFEST_FILE),
    )?;
    copy_file(
        &artifact.generation_dir.join(MUTABLE_STATE_MANIFEST_FILE),
        &generation_dest.join(MUTABLE_STATE_MANIFEST_FILE),
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
    let cas = crate::filesystem::CasStore::new(&objects_dest)?;
    materialize_state_root(&artifact.mutable_state, &cas, staging_dir)?;
    let etc_state = staging_dir
        .join("conary/etc-state")
        .join(artifact.generation.to_string());
    std::fs::create_dir_all(
        etc_state
            .parent()
            .expect("generation etc-state path always has a parent"),
    )?;
    let materialized_etc = staging_dir.join("etc");
    if materialized_etc.is_dir() {
        std::fs::rename(&materialized_etc, &etc_state)?;
    } else {
        std::fs::create_dir(&etc_state)?;
    }
    std::fs::create_dir_all(staging_dir.join("conary/etc-lower"))?;
    std::fs::create_dir_all(staging_dir.join("conary/mnt"))?;

    for dir in RUNTIME_ROOT_DIRS.iter().chain(EXCLUDED_DIRS.iter()) {
        std::fs::create_dir_all(staging_dir.join(dir))?;
    }
    if let Some(capability) = &artifact
        .artifact_manifest
        .carrier_capabilities
        .immutable_backing_security
    {
        apply_security(capability, &objects_dest)?;
    }
    create_root_symlinks(artifact, staging_dir)?;
    restore_carrier_root_metadata(artifact, staging_dir)?;

    Ok(staging_dir.to_path_buf())
}

fn validate_carrier_root_metadata(artifact: &GenerationArtifact) -> crate::Result<()> {
    for relative in RUNTIME_ROOT_DIRS {
        carrier_mountpoint_node(artifact, &format!("/{relative}"))?;
    }
    for (relative, target) in ROOT_SYMLINKS {
        carrier_root_symlink_node(artifact, &format!("/{relative}"), target)?;
    }
    Ok(())
}

fn restore_carrier_root_metadata(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
) -> crate::Result<()> {
    for relative in RUNTIME_ROOT_DIRS {
        let manifest_path = format!("/{relative}");
        let node = carrier_mountpoint_node(artifact, &manifest_path)?;
        apply_resolved_payload_metadata(&staging_dir.join(relative), node)?;
    }
    for (relative, target) in ROOT_SYMLINKS {
        let node = carrier_root_symlink_node(artifact, &format!("/{relative}"), target)?;
        apply_resolved_payload_metadata(&staging_dir.join(relative), node)?;
    }

    // Root metadata is restored last because every projected file, directory,
    // and symlink changes the root directory timestamp.
    apply_resolved_payload_metadata(staging_dir, &artifact.generation_root.root)
}

fn carrier_root_symlink_node<'a>(
    artifact: &'a GenerationArtifact,
    manifest_path: &str,
    expected_target: &str,
) -> crate::Result<&'a ResolvedPayloadNode> {
    let entry = artifact
        .generation_root
        .entries
        .iter()
        .find(|entry| entry.path == manifest_path)
        .ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "exportable generation is missing exact carrier root-symlink metadata for {manifest_path}"
            ))
        })?;
    match &entry.node.source.kind {
        PayloadNodeKind::Symlink { target } if target == expected_target => Ok(&entry.node),
        PayloadNodeKind::Symlink { target } => Err(crate::Error::InvalidPath(format!(
            "carrier root symlink {manifest_path} targets {target:?}; expected {expected_target:?}"
        ))),
        _ => Err(crate::Error::InvalidPath(format!(
            "carrier root symlink {manifest_path} must be a symlink in the generation artifact"
        ))),
    }
}

fn carrier_mountpoint_node<'a>(
    artifact: &'a GenerationArtifact,
    manifest_path: &str,
) -> crate::Result<&'a ResolvedPayloadNode> {
    let entry = artifact
        .generation_root
        .entries
        .iter()
        .chain(artifact.mutable_state.entries.iter())
        .find(|entry| entry.path == manifest_path)
        .ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "exportable generation is missing exact carrier mountpoint metadata for {manifest_path}"
            ))
        })?;
    if !matches!(&entry.node.source.kind, PayloadNodeKind::Directory) {
        return Err(crate::Error::InvalidPath(format!(
            "carrier mountpoint {manifest_path} must be a directory in the generation artifact"
        )));
    }
    Ok(&entry.node)
}

pub fn project_generation_esp(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
) -> crate::Result<PathBuf> {
    project_generation_esp_for_carrier(artifact, staging_dir, BootCarrier::WritableDisk)
}

fn project_generation_iso_esp(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
) -> crate::Result<PathBuf> {
    project_generation_esp_for_carrier(artifact, staging_dir, BootCarrier::ReadonlyIso)
}

#[derive(Debug, Clone, Copy)]
enum BootCarrier {
    WritableDisk,
    ReadonlyIso,
}

fn project_generation_esp_for_carrier(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
    carrier: BootCarrier,
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
    let boot_options = match carrier {
        BootCarrier::WritableDisk => format!(
            "root=PARTLABEL=CONARY_ROOT rootfstype={} rw {DISABLE_SOURCE_FSTAB_OPTION} {WRITABLE_ESP_MOUNT_OPTION} conary.generation={} console=tty0 console=ttyS0",
            crate::image::repart::BLS_ROOTFSTYPE,
            artifact.generation
        ),
        BootCarrier::ReadonlyIso => format!(
            "root=LABEL={ISO_VOLUME_ID} rootfstype=iso9660 ro {DISABLE_SOURCE_FSTAB_OPTION} conary.generation={} conary.carrier=readonly systemd.mask=boot.mount console=tty0 console=ttyS0",
            artifact.generation
        ),
    };
    std::fs::write(
        entries_dir.join(format!("conary-gen-{}.conf", artifact.generation)),
        format!(
            "title      Conary Generation {0}\n\
             linux      /vmlinuz\n\
             initrd     /initramfs.img\n\
             options    {1}\n\
             sort-key   conary-{0}\n",
            artifact.generation, boot_options
        ),
    )?;

    Ok(staging_dir.to_path_buf())
}

fn run_mkfs_vfat(tools: &GenerationExportTools, efi_image: &Path) -> crate::Result<()> {
    let output = Command::new(&tools.mkfs_vfat)
        .args(["-n", "CONARYEFI"])
        .arg(efi_image)
        .output()
        .map_err(|e| crate::Error::IoError(format!("failed to run mkfs.vfat: {e}")))?;
    ensure_command_success("mkfs.vfat", output)
}

fn run_mmd(tools: &GenerationExportTools, efi_image: &Path, dir: &str) -> crate::Result<()> {
    let output = Command::new(&tools.mmd)
        .arg("-i")
        .arg(efi_image)
        .arg(dir)
        .output()
        .map_err(|e| crate::Error::IoError(format!("failed to run mmd: {e}")))?;
    ensure_command_success("mmd", output)
}

fn run_mcopy(
    tools: &GenerationExportTools,
    efi_image: &Path,
    source: &Path,
    dest: &str,
) -> crate::Result<()> {
    let output = Command::new(&tools.mcopy)
        .arg("-i")
        .arg(efi_image)
        .arg(source)
        .arg(dest)
        .output()
        .map_err(|e| crate::Error::IoError(format!("failed to run mcopy: {e}")))?;
    ensure_command_success("mcopy", output)
}

fn run_xorriso(
    tools: &GenerationExportTools,
    iso_root: &Path,
    output_iso: &Path,
) -> crate::Result<()> {
    let output = Command::new(&tools.xorriso)
        .args([
            "-as",
            "mkisofs",
            "-iso-level",
            "3",
            "-full-iso9660-filenames",
            "-R",
            "-J",
            "-V",
            ISO_VOLUME_ID,
            "-o",
        ])
        .arg(output_iso)
        .args([
            "-e",
            ISO_EFI_IMAGE_REL,
            "-no-emul-boot",
            "-isohybrid-gpt-basdat",
        ])
        .arg(iso_root)
        .output()
        .map_err(|e| crate::Error::IoError(format!("failed to run xorriso: {e}")))?;
    ensure_command_success("xorriso", output)
}

fn ensure_command_success(tool: &str, output: std::process::Output) -> crate::Result<()> {
    if output.status.success() {
        return Ok(());
    }
    Err(crate::Error::IoError(format!(
        "{tool} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    )))
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
fn create_root_symlinks(artifact: &GenerationArtifact, staging_dir: &Path) -> crate::Result<()> {
    for (link, expected_target) in ROOT_SYMLINKS {
        let node = carrier_root_symlink_node(artifact, &format!("/{link}"), expected_target)?;
        let PayloadNodeKind::Symlink { target } = &node.source.kind else {
            unreachable!("carrier_root_symlink_node validated the node kind");
        };
        let link_path = staging_dir.join(link);
        let _ = std::fs::remove_file(&link_path);
        std::os::unix::fs::symlink(target, link_path)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_root_symlinks(artifact: &GenerationArtifact, staging_dir: &Path) -> crate::Result<()> {
    for (link, expected_target) in ROOT_SYMLINKS {
        let node = carrier_root_symlink_node(artifact, &format!("/{link}"), expected_target)?;
        let PayloadNodeKind::Symlink { target } = &node.source.kind else {
            unreachable!("carrier_root_symlink_node validated the node kind");
        };
        std::fs::write(staging_dir.join(link), format!("{target}\n"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
