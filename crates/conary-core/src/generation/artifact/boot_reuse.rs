// crates/conary-core/src/generation/artifact/boot_reuse.rs

//! Narrow verification and staging for unchanged generation boot assets.
//!
//! Ordinary package publication must not deep-open every payload object merely
//! to prove the three files already staged for boot. This module reopens the
//! artifact and boot manifests, verifies their identities and hashes, and
//! hard-links the verified immutable files into the new generation.

use std::path::{Path, PathBuf};

use super::{
    ARTIFACT_MANIFEST_FILE, ARTIFACT_MANIFEST_VERSION, BOOT_ASSETS_DIR, BootAssetsManifest,
    GenerationArtifactManifest, read_required_file, require_manifest_identity,
    require_supported_architecture, require_version, sha256_bytes,
    validate_artifact_manifest_hashes, validate_artifact_manifest_paths,
    validate_boot_asset_relative_path, validate_generation_relative_path, verify_boot_assets,
    verify_bytes_digest,
};
use crate::generation::metadata::{
    GENERATION_METADATA_FILE, GenerationMetadata, is_generation_pending,
};

/// A prior generation's independently reopened boot-asset authority.
#[derive(Debug, Clone)]
pub(crate) struct VerifiedGenerationBootAssets {
    generation_dir: PathBuf,
    generation: i64,
    architecture: String,
    manifest: BootAssetsManifest,
}

impl VerifiedGenerationBootAssets {
    pub(crate) fn generation(&self) -> i64 {
        self.generation
    }

    pub(crate) fn architecture(&self) -> &str {
        &self.architecture
    }

    pub(crate) fn kernel_version(&self) -> &str {
        &self.manifest.kernel_version
    }

    pub(crate) fn kernel_path(&self) -> crate::Result<PathBuf> {
        self.asset_path("kernel", &self.manifest.kernel)
    }

    pub(crate) fn initramfs_path(&self) -> crate::Result<PathBuf> {
        self.asset_path("initramfs", &self.manifest.initramfs)
    }

    pub(crate) fn efi_bootloader_path(&self) -> crate::Result<PathBuf> {
        self.asset_path("efi_bootloader", &self.manifest.efi_bootloader)
    }

    fn asset_path(&self, field: &str, relative: &str) -> crate::Result<PathBuf> {
        Ok(self
            .generation_dir
            .join(BOOT_ASSETS_DIR)
            .join(validate_boot_asset_relative_path(field, relative)?))
    }
}

/// Reopen only the authority required to reuse one generation's boot files.
///
/// This deliberately does not enumerate or hash the generation's complete CAS
/// manifest. The caller has already selected the current generation; this
/// proof is scoped to its artifact identity, boot manifest, and three staged
/// boot files.
pub(crate) fn load_verified_generation_boot_assets(
    generation_dir: &Path,
) -> crate::Result<VerifiedGenerationBootAssets> {
    if is_generation_pending(generation_dir) {
        return Err(crate::Error::NotFound(format!(
            "generation at {} is pending and cannot supply reusable boot assets",
            generation_dir.display()
        )));
    }

    let artifact_path = generation_dir.join(ARTIFACT_MANIFEST_FILE);
    let artifact_bytes = read_required_file("artifact manifest", &artifact_path)?;
    let artifact: GenerationArtifactManifest = serde_json::from_slice(&artifact_bytes)?;
    require_version(
        ".conary-artifact.json",
        artifact.version,
        ARTIFACT_MANIFEST_VERSION,
    )?;
    require_supported_architecture(&artifact.architecture)?;
    validate_artifact_manifest_paths(&artifact)?;
    validate_artifact_manifest_hashes(&artifact)?;
    artifact.carrier_capabilities.validate()?;
    if artifact.metadata != GENERATION_METADATA_FILE {
        return Err(crate::Error::InvalidPath(format!(
            "artifact metadata path must be {GENERATION_METADATA_FILE}, got {}",
            artifact.metadata
        )));
    }

    let metadata = GenerationMetadata::read_from(generation_dir).map_err(|error| {
        crate::Error::InvalidPath(format!(
            "missing or invalid generation metadata for boot reuse: {error}"
        ))
    })?;
    if metadata.generation != artifact.generation {
        return Err(crate::Error::InvalidPath(format!(
            "generation mismatch: metadata has {}, artifact has {}",
            metadata.generation, artifact.generation
        )));
    }
    let artifact_digest = sha256_bytes(&artifact_bytes);
    match metadata.artifact_manifest_sha256.as_deref() {
        Some(expected) if expected == artifact_digest => {}
        Some(expected) => {
            return Err(crate::Error::ChecksumMismatch {
                expected: expected.to_string(),
                actual: artifact_digest,
            });
        }
        None => {
            return Err(crate::Error::InvalidPath(format!(
                "reusable generation metadata must contain artifact_manifest_sha256 for {ARTIFACT_MANIFEST_FILE}"
            )));
        }
    }

    let boot_manifest_rel =
        validate_generation_relative_path("boot_assets", &artifact.boot_assets)?;
    let boot_manifest_bytes = read_required_file(
        "boot-assets manifest",
        &generation_dir.join(boot_manifest_rel),
    )?;
    verify_bytes_digest(
        "boot-assets manifest",
        &boot_manifest_bytes,
        &artifact.boot_assets_sha256,
    )?;
    let boot_assets: BootAssetsManifest = serde_json::from_slice(&boot_manifest_bytes)?;
    require_version("boot-assets manifest", boot_assets.version, 1)?;
    require_manifest_identity(
        "boot-assets manifest",
        artifact.generation,
        &artifact.architecture,
        boot_assets.generation,
        &boot_assets.architecture,
    )?;
    verify_boot_assets(generation_dir, &boot_assets)?;

    Ok(VerifiedGenerationBootAssets {
        generation_dir: generation_dir.to_path_buf(),
        generation: artifact.generation,
        architecture: artifact.architecture,
        manifest: boot_assets,
    })
}

/// Stage verified immutable boot files without copying their payload bytes.
pub(crate) fn stage_reused_boot_assets(
    generation_dir: &Path,
    generation: i64,
    architecture: &str,
    source: &VerifiedGenerationBootAssets,
) -> crate::Result<BootAssetsManifest> {
    require_supported_architecture(architecture)?;
    if source.architecture() != architecture {
        return Err(crate::Error::InvalidPath(format!(
            "reused boot asset architecture mismatch: source has {}, target has {architecture}",
            source.architecture()
        )));
    }

    let boot_assets_dir = generation_dir.join(BOOT_ASSETS_DIR);
    link_boot_asset(
        &source.kernel_path()?,
        &boot_assets_dir.join("vmlinuz"),
        "kernel",
    )?;
    link_boot_asset(
        &source.initramfs_path()?,
        &boot_assets_dir.join("initramfs.img"),
        "initramfs",
    )?;
    link_boot_asset(
        &source.efi_bootloader_path()?,
        &boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"),
        "efi_bootloader",
    )?;

    Ok(BootAssetsManifest {
        version: 1,
        generation,
        architecture: architecture.to_string(),
        kernel_version: source.manifest.kernel_version.clone(),
        kernel: "vmlinuz".to_string(),
        kernel_sha256: source.manifest.kernel_sha256.clone(),
        initramfs: "initramfs.img".to_string(),
        initramfs_sha256: source.manifest.initramfs_sha256.clone(),
        efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
        efi_bootloader_sha256: source.manifest.efi_bootloader_sha256.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn link_boot_asset(source: &Path, destination: &Path, label: &str) -> crate::Result<()> {
    let metadata = std::fs::symlink_metadata(source).map_err(|error| {
        crate::Error::NotFound(format!(
            "missing reusable boot asset {label} at {}: {error}",
            source.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(crate::Error::InvalidPath(format!(
            "reusable boot asset {label} must be a regular non-symlink file: {}",
            source.display()
        )));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::hard_link(source, destination).map_err(|error| {
        crate::Error::IoError(format!(
            "failed to link reusable boot asset {label} from {} to {}: {error}",
            source.display(),
            destination.display()
        ))
    })?;
    Ok(())
}
