// crates/conary-core/src/generation/artifact.rs

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use super::metadata::{GENERATION_METADATA_FILE, GenerationMetadata, generation_path};
use super::root_manifest::{
    GENERATION_ROOT_MANIFEST_FILE, GenerationRootManifest, MUTABLE_STATE_MANIFEST_FILE,
    MutableStateManifest,
};

pub const ARTIFACT_MANIFEST_FILE: &str = ".conary-artifact.json";
pub const ARTIFACT_MANIFEST_VERSION: u32 = 3;
pub const GENERATION_CARRIER_CAPABILITIES_VERSION: u32 = 1;
pub const CAS_MANIFEST_FILE: &str = "cas-manifest.json";
pub const BOOT_ASSETS_DIR: &str = "boot-assets";
pub const BOOT_ASSETS_MANIFEST_REL: &str = "boot-assets/manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationArtifactManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    #[serde(default = "missing_generation_carrier_capabilities")]
    pub carrier_capabilities: GenerationCarrierCapabilities,
    pub metadata: String,
    pub erofs: String,
    pub erofs_sha256: String,
    pub generation_root_manifest: String,
    pub generation_root_manifest_sha256: String,
    pub mutable_state_manifest: String,
    pub mutable_state_manifest_sha256: String,
    pub cas_base: String,
    pub cas_manifest: String,
    pub cas_manifest_sha256: String,
    pub boot_assets: String,
    pub boot_assets_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GenerationCarrierCapabilities {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_backing_security: Option<crate::ccs::ImmutableBackingSecurity>,
}

impl Default for GenerationCarrierCapabilities {
    fn default() -> Self {
        Self {
            version: GENERATION_CARRIER_CAPABILITIES_VERSION,
            immutable_backing_security: None,
        }
    }
}

impl GenerationCarrierCapabilities {
    /// Derive carrier authority from an exact captured offline target root.
    ///
    /// Runtime generations use the initialized host inventory instead. An
    /// offline-assembled target is not running yet, so its signed `/usr` node
    /// is the equivalent target-supplied fact.
    pub fn from_generation_root(generation_root: &GenerationRootManifest) -> crate::Result<Self> {
        let target_usr = generation_root
            .entries
            .iter()
            .find(|entry| entry.path == "/usr")
            .ok_or_else(|| {
                crate::Error::InvalidPath(
                    "generation carrier capability projection requires exact /usr authority"
                        .to_string(),
                )
            })?;
        let immutable_backing_security =
            crate::ccs::ImmutableBackingSecurity::from_target_usr_node(&target_usr.node).map_err(
                |error| {
                    crate::Error::InvalidPath(format!(
                        "generation carrier target /usr security authority is invalid: {error}"
                    ))
                },
            )?;
        let capabilities = Self {
            immutable_backing_security,
            ..Self::default()
        };
        capabilities.validate()?;
        Ok(capabilities)
    }

    pub fn validate(&self) -> crate::Result<()> {
        require_version(
            "generation carrier capabilities",
            self.version,
            GENERATION_CARRIER_CAPABILITIES_VERSION,
        )?;
        if let Some(capability) = &self.immutable_backing_security {
            capability.validate().map_err(|error| {
                crate::Error::InvalidPath(format!(
                    "generation carrier immutable-backing security capability is invalid: {error}"
                ))
            })?;
        }
        Ok(())
    }
}

fn missing_generation_carrier_capabilities() -> GenerationCarrierCapabilities {
    GenerationCarrierCapabilities {
        version: 0,
        immutable_backing_security: None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    pub objects: Vec<CasObjectRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasObjectRef {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootAssetsManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    pub kernel_version: String,
    pub kernel: String,
    pub kernel_sha256: String,
    pub initramfs: String,
    pub initramfs_sha256: String,
    pub efi_bootloader: String,
    pub efi_bootloader_sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct GenerationArtifact {
    pub generation: i64,
    pub generation_dir: PathBuf,
    pub artifact_manifest: GenerationArtifactManifest,
    pub metadata: GenerationMetadata,
    pub erofs_path: PathBuf,
    pub generation_root: GenerationRootManifest,
    pub mutable_state: MutableStateManifest,
    pub cas_dir: PathBuf,
    pub cas_objects: Vec<CasObjectRef>,
    pub boot_assets: BootAssetsManifest,
}

pub struct BootAssetSources<'a> {
    pub generation_dir: &'a Path,
    pub generation: i64,
    pub architecture: &'a str,
    pub kernel_version: &'a str,
    pub kernel: &'a Path,
    pub initramfs: &'a Path,
    pub efi_bootloader: &'a Path,
}

pub struct ArtifactWriteInputs<'a> {
    pub generation_dir: &'a Path,
    pub generation: i64,
    pub architecture: &'a str,
    pub erofs_path: &'a Path,
    pub cas_base_rel: &'a str,
    pub cas_verification: CasObjectVerification,
    pub boot_assets: BootAssetsManifest,
    pub carrier_capabilities: GenerationCarrierCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasObjectVerification {
    Deep,
    AlreadyVerified,
}

pub fn stage_boot_assets(inputs: BootAssetSources<'_>) -> crate::Result<BootAssetsManifest> {
    require_version("boot-assets manifest", 1, 1)?;
    require_supported_architecture(inputs.architecture)?;

    let boot_assets_dir = inputs.generation_dir.join(BOOT_ASSETS_DIR);
    copy_boot_asset(inputs.kernel, &boot_assets_dir.join("vmlinuz"), "kernel")?;
    copy_boot_asset(
        inputs.initramfs,
        &boot_assets_dir.join("initramfs.img"),
        "initramfs",
    )?;
    copy_boot_asset(
        inputs.efi_bootloader,
        &boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"),
        "efi_bootloader",
    )?;

    Ok(BootAssetsManifest {
        version: 1,
        generation: inputs.generation,
        architecture: inputs.architecture.to_string(),
        kernel_version: inputs.kernel_version.to_string(),
        kernel: "vmlinuz".to_string(),
        kernel_sha256: sha256_file(&boot_assets_dir.join("vmlinuz"))?,
        initramfs: "initramfs.img".to_string(),
        initramfs_sha256: sha256_file(&boot_assets_dir.join("initramfs.img"))?,
        efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
        efi_bootloader_sha256: sha256_file(&boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"))?,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

pub fn write_generation_artifact(inputs: ArtifactWriteInputs<'_>) -> crate::Result<String> {
    require_supported_architecture(inputs.architecture)?;
    inputs.carrier_capabilities.validate()?;
    let erofs_rel = path_relative_to_generation(inputs.generation_dir, inputs.erofs_path, "erofs")?;
    let erofs_sha256 = sha256_file(inputs.erofs_path)?;

    let cas_dir = resolve_cas_base(inputs.generation_dir, inputs.cas_base_rel)?;
    require_version("boot-assets manifest", inputs.boot_assets.version, 1)?;
    require_manifest_identity(
        "boot-assets manifest",
        inputs.generation,
        inputs.architecture,
        inputs.boot_assets.generation,
        &inputs.boot_assets.architecture,
    )?;
    verify_boot_assets(inputs.generation_dir, &inputs.boot_assets)?;
    let root_manifest_path = inputs.generation_dir.join(GENERATION_ROOT_MANIFEST_FILE);
    let root_manifest_bytes = read_required_file("generation root manifest", &root_manifest_path)?;
    let root_manifest: GenerationRootManifest = serde_json::from_slice(&root_manifest_bytes)?;
    root_manifest.validate()?;
    let state_manifest_path = inputs.generation_dir.join(MUTABLE_STATE_MANIFEST_FILE);
    let state_manifest_bytes = read_required_file("mutable-state manifest", &state_manifest_path)?;
    let state_manifest: MutableStateManifest = serde_json::from_slice(&state_manifest_bytes)?;
    state_manifest.validate()?;
    let cas_objects = deduplicate_sort_cas_objects(
        root_manifest
            .regular_contents()
            .chain(
                state_manifest
                    .entries
                    .iter()
                    .filter_map(|entry| entry.content.as_ref()),
            )
            .map(|content| CasObjectRef {
                sha256: content.sha256.clone(),
                size: content.size,
            })
            .collect(),
    )?;
    match inputs.cas_verification {
        CasObjectVerification::Deep => verify_cas_objects(&cas_dir, &cas_objects)?,
        CasObjectVerification::AlreadyVerified => {
            verify_cas_object_files_exist_with_expected_sizes(&cas_dir, &cas_objects)?
        }
    }

    let cas_manifest = CasManifest {
        version: 1,
        generation: inputs.generation,
        architecture: inputs.architecture.to_string(),
        objects: cas_objects,
    };
    let cas_manifest_bytes = write_json_pretty(
        &inputs.generation_dir.join(CAS_MANIFEST_FILE),
        &cas_manifest,
    )?;

    let boot_assets_bytes = write_json_pretty(
        &inputs.generation_dir.join(BOOT_ASSETS_MANIFEST_REL),
        &inputs.boot_assets,
    )?;

    let artifact_manifest = GenerationArtifactManifest {
        version: ARTIFACT_MANIFEST_VERSION,
        generation: inputs.generation,
        architecture: inputs.architecture.to_string(),
        carrier_capabilities: inputs.carrier_capabilities,
        metadata: GENERATION_METADATA_FILE.to_string(),
        erofs: erofs_rel,
        erofs_sha256,
        generation_root_manifest: GENERATION_ROOT_MANIFEST_FILE.to_string(),
        generation_root_manifest_sha256: sha256_bytes(&root_manifest_bytes),
        mutable_state_manifest: MUTABLE_STATE_MANIFEST_FILE.to_string(),
        mutable_state_manifest_sha256: sha256_bytes(&state_manifest_bytes),
        cas_base: inputs.cas_base_rel.to_string(),
        cas_manifest: CAS_MANIFEST_FILE.to_string(),
        cas_manifest_sha256: sha256_bytes(&cas_manifest_bytes),
        boot_assets: BOOT_ASSETS_MANIFEST_REL.to_string(),
        boot_assets_sha256: sha256_bytes(&boot_assets_bytes),
    };
    let artifact_bytes = write_json_pretty(
        &inputs.generation_dir.join(ARTIFACT_MANIFEST_FILE),
        &artifact_manifest,
    )?;
    Ok(sha256_bytes(&artifact_bytes))
}

pub fn deduplicate_sort_cas_objects(
    objects: Vec<CasObjectRef>,
) -> crate::Result<Vec<CasObjectRef>> {
    let mut by_hash = BTreeMap::new();
    for object in objects {
        validate_sha256_hex("CAS object sha256", &object.sha256)?;
        match by_hash.insert(object.sha256.clone(), object.size) {
            Some(existing_size) if existing_size != object.size => {
                return Err(crate::Error::ConflictError(format!(
                    "CAS object {} has conflicting sizes: {existing_size} and {}",
                    object.sha256, object.size
                )));
            }
            _ => {}
        }
    }

    Ok(by_hash
        .into_iter()
        .map(|(sha256, size)| CasObjectRef { sha256, size })
        .collect())
}

pub fn load_generation_artifact(generation_dir: &Path) -> crate::Result<GenerationArtifact> {
    load_generation_artifact_with_cas_verification(generation_dir, CasObjectVerification::Deep)
}

pub fn load_generation_artifact_for_activation(
    generation_dir: &Path,
) -> crate::Result<GenerationArtifact> {
    load_generation_artifact_with_cas_verification(
        generation_dir,
        CasObjectVerification::AlreadyVerified,
    )
}

fn load_generation_artifact_with_cas_verification(
    generation_dir: &Path,
    cas_verification: CasObjectVerification,
) -> crate::Result<GenerationArtifact> {
    if super::metadata::is_generation_pending(generation_dir) {
        return Err(crate::Error::NotFound(format!(
            "generation at {} is pending and cannot be exported",
            generation_dir.display()
        )));
    }

    let artifact_path = generation_dir.join(ARTIFACT_MANIFEST_FILE);
    if !artifact_path.exists() {
        return Err(crate::Error::NotFound(format!(
            "pre-export-contract generation: missing {ARTIFACT_MANIFEST_FILE} in {}",
            generation_dir.display()
        )));
    }

    let artifact_bytes = std::fs::read(&artifact_path).map_err(|e| {
        crate::Error::IoError(format!(
            "failed to read artifact manifest {}: {e}",
            artifact_path.display()
        ))
    })?;
    let artifact_manifest: GenerationArtifactManifest = serde_json::from_slice(&artifact_bytes)?;
    require_version(
        ".conary-artifact.json",
        artifact_manifest.version,
        ARTIFACT_MANIFEST_VERSION,
    )?;
    require_supported_architecture(&artifact_manifest.architecture)?;
    validate_artifact_manifest_paths(&artifact_manifest)?;
    validate_artifact_manifest_hashes(&artifact_manifest)?;
    artifact_manifest.carrier_capabilities.validate()?;

    if artifact_manifest.metadata != GENERATION_METADATA_FILE {
        return Err(crate::Error::InvalidPath(format!(
            "artifact metadata path must be {GENERATION_METADATA_FILE}, got {}",
            artifact_manifest.metadata
        )));
    }
    if artifact_manifest.generation_root_manifest != GENERATION_ROOT_MANIFEST_FILE {
        return Err(crate::Error::InvalidPath(format!(
            "artifact generation-root path must be {GENERATION_ROOT_MANIFEST_FILE}, got {}",
            artifact_manifest.generation_root_manifest
        )));
    }
    if artifact_manifest.mutable_state_manifest != MUTABLE_STATE_MANIFEST_FILE {
        return Err(crate::Error::InvalidPath(format!(
            "artifact mutable-state path must be {MUTABLE_STATE_MANIFEST_FILE}, got {}",
            artifact_manifest.mutable_state_manifest
        )));
    }

    let metadata = GenerationMetadata::read_from(generation_dir).map_err(|e| {
        crate::Error::InvalidPath(format!(
            "missing or invalid generation metadata for export artifact: {e}"
        ))
    })?;
    if metadata.generation != artifact_manifest.generation {
        return Err(crate::Error::InvalidPath(format!(
            "generation mismatch: metadata has {}, artifact has {}",
            metadata.generation, artifact_manifest.generation
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
                "exportable generation metadata must contain artifact_manifest_sha256 for {ARTIFACT_MANIFEST_FILE}"
            )));
        }
    }

    let erofs_rel = validate_generation_relative_path("erofs", &artifact_manifest.erofs)?;
    let erofs_path = generation_dir.join(erofs_rel);
    verify_file_digest("root.erofs", &erofs_path, &artifact_manifest.erofs_sha256)?;

    let root_manifest_rel = validate_generation_relative_path(
        "generation_root_manifest",
        &artifact_manifest.generation_root_manifest,
    )?;
    let root_manifest_bytes = read_required_file(
        "generation root manifest",
        &generation_dir.join(root_manifest_rel),
    )?;
    verify_bytes_digest(
        "generation root manifest",
        &root_manifest_bytes,
        &artifact_manifest.generation_root_manifest_sha256,
    )?;
    let generation_root: GenerationRootManifest = serde_json::from_slice(&root_manifest_bytes)?;
    generation_root.validate()?;

    let state_manifest_rel = validate_generation_relative_path(
        "mutable_state_manifest",
        &artifact_manifest.mutable_state_manifest,
    )?;
    let state_manifest_bytes = read_required_file(
        "mutable-state manifest",
        &generation_dir.join(state_manifest_rel),
    )?;
    verify_bytes_digest(
        "mutable-state manifest",
        &state_manifest_bytes,
        &artifact_manifest.mutable_state_manifest_sha256,
    )?;
    let mutable_state: MutableStateManifest = serde_json::from_slice(&state_manifest_bytes)?;
    mutable_state.validate()?;

    let cas_dir = resolve_cas_base(generation_dir, &artifact_manifest.cas_base)?;
    let cas_manifest_rel =
        validate_generation_relative_path("cas_manifest", &artifact_manifest.cas_manifest)?;
    let cas_manifest_path = generation_dir.join(cas_manifest_rel);
    let cas_manifest_bytes = read_required_file("cas-manifest", &cas_manifest_path)?;
    verify_bytes_digest(
        "cas-manifest",
        &cas_manifest_bytes,
        &artifact_manifest.cas_manifest_sha256,
    )?;
    let cas_manifest: CasManifest = serde_json::from_slice(&cas_manifest_bytes)?;
    require_version("cas-manifest", cas_manifest.version, 1)?;
    require_manifest_identity(
        "cas-manifest",
        artifact_manifest.generation,
        &artifact_manifest.architecture,
        cas_manifest.generation,
        &cas_manifest.architecture,
    )?;
    match cas_verification {
        CasObjectVerification::Deep => verify_cas_objects(&cas_dir, &cas_manifest.objects)?,
        CasObjectVerification::AlreadyVerified => {
            verify_cas_object_files_exist_with_expected_sizes(&cas_dir, &cas_manifest.objects)?
        }
    }

    let boot_manifest_rel =
        validate_generation_relative_path("boot_assets", &artifact_manifest.boot_assets)?;
    let boot_manifest_path = generation_dir.join(boot_manifest_rel);
    let boot_manifest_bytes = read_required_file("boot-assets manifest", &boot_manifest_path)?;
    verify_bytes_digest(
        "boot-assets manifest",
        &boot_manifest_bytes,
        &artifact_manifest.boot_assets_sha256,
    )?;
    let boot_assets: BootAssetsManifest = serde_json::from_slice(&boot_manifest_bytes)?;
    require_version("boot-assets manifest", boot_assets.version, 1)?;
    require_manifest_identity(
        "boot-assets manifest",
        artifact_manifest.generation,
        &artifact_manifest.architecture,
        boot_assets.generation,
        &boot_assets.architecture,
    )?;
    verify_boot_assets(generation_dir, &boot_assets)?;

    Ok(GenerationArtifact {
        generation: artifact_manifest.generation,
        generation_dir: generation_dir.to_path_buf(),
        artifact_manifest,
        metadata,
        erofs_path,
        generation_root,
        mutable_state,
        cas_dir,
        cas_objects: cas_manifest.objects,
        boot_assets,
    })
}

pub fn load_installed_generation_artifact(generation: i64) -> crate::Result<GenerationArtifact> {
    load_generation_artifact(&generation_path(generation))
}

fn require_version(name: &str, version: u32, expected: u32) -> crate::Result<()> {
    if version == expected {
        Ok(())
    } else {
        Err(crate::Error::InvalidPath(format!(
            "{name} has unsupported version {version}; expected version {expected}"
        )))
    }
}

fn require_supported_architecture(architecture: &str) -> crate::Result<()> {
    if architecture == "x86_64" {
        Ok(())
    } else {
        Err(crate::Error::NotImplemented(format!(
            "unsupported architecture for generation export: {architecture}"
        )))
    }
}

fn require_manifest_identity(
    name: &str,
    expected_generation: i64,
    expected_architecture: &str,
    actual_generation: i64,
    actual_architecture: &str,
) -> crate::Result<()> {
    if actual_generation != expected_generation {
        return Err(crate::Error::InvalidPath(format!(
            "{name} generation mismatch: expected {expected_generation}, got {actual_generation}"
        )));
    }
    if actual_architecture != expected_architecture {
        return Err(crate::Error::InvalidPath(format!(
            "{name} architecture mismatch: expected {expected_architecture}, got {actual_architecture}"
        )));
    }
    Ok(())
}

fn validate_artifact_manifest_paths(manifest: &GenerationArtifactManifest) -> crate::Result<()> {
    validate_generation_relative_path("metadata", &manifest.metadata)?;
    validate_generation_relative_path("erofs", &manifest.erofs)?;
    validate_generation_relative_path(
        "generation_root_manifest",
        &manifest.generation_root_manifest,
    )?;
    validate_generation_relative_path("mutable_state_manifest", &manifest.mutable_state_manifest)?;
    validate_generation_relative_path("cas_manifest", &manifest.cas_manifest)?;
    validate_generation_relative_path("boot_assets", &manifest.boot_assets)?;
    Ok(())
}

fn validate_artifact_manifest_hashes(manifest: &GenerationArtifactManifest) -> crate::Result<()> {
    validate_sha256_hex("erofs_sha256", &manifest.erofs_sha256)?;
    validate_sha256_hex(
        "generation_root_manifest_sha256",
        &manifest.generation_root_manifest_sha256,
    )?;
    validate_sha256_hex(
        "mutable_state_manifest_sha256",
        &manifest.mutable_state_manifest_sha256,
    )?;
    validate_sha256_hex("cas_manifest_sha256", &manifest.cas_manifest_sha256)?;
    validate_sha256_hex("boot_assets_sha256", &manifest.boot_assets_sha256)?;
    Ok(())
}

fn read_required_file(label: &str, path: &Path) -> crate::Result<Vec<u8>> {
    std::fs::read(path)
        .map_err(|e| crate::Error::NotFound(format!("missing {label} at {}: {e}", path.display())))
}

fn verify_file_digest(label: &str, path: &Path, expected: &str) -> crate::Result<()> {
    validate_sha256_hex(label, expected)?;
    let actual = sha256_file(path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(crate::Error::ChecksumMismatch {
            expected: format!("{label} {expected}"),
            actual,
        })
    }
}

fn verify_bytes_digest(label: &str, bytes: &[u8], expected: &str) -> crate::Result<()> {
    validate_sha256_hex(label, expected)?;
    let actual = sha256_bytes(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(crate::Error::ChecksumMismatch {
            expected: format!("{label} {expected}"),
            actual,
        })
    }
}

fn sha256_file(path: &Path) -> crate::Result<String> {
    let bytes = std::fs::read(path).map_err(|e| {
        crate::Error::NotFound(format!(
            "missing file for SHA-256 verification at {}: {e}",
            path.display()
        ))
    })?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn validate_sha256_hex(field: &str, value: &str) -> crate::Result<()> {
    if value.len() != 64 {
        return Err(crate::Error::InvalidPath(format!(
            "{field} must be a 64-character SHA-256 hex string"
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(crate::Error::InvalidPath(format!(
            "{field} must be lowercase SHA-256 hex"
        )));
    }
    Ok(())
}

pub(crate) fn verify_cas_objects(cas_dir: &Path, objects: &[CasObjectRef]) -> crate::Result<()> {
    let mut seen = HashSet::new();
    for object in objects {
        validate_sha256_hex("CAS object sha256", &object.sha256)?;
        if !seen.insert(object.sha256.clone()) {
            return Err(crate::Error::ConflictError(format!(
                "duplicate CAS manifest entry for {}",
                object.sha256
            )));
        }

        let object_path = crate::filesystem::object_path(cas_dir, &object.sha256)?;
        let metadata = std::fs::metadata(&object_path).map_err(|e| {
            crate::Error::NotFound(format!(
                "missing CAS object {} at {}: {e}",
                object.sha256,
                object_path.display()
            ))
        })?;
        if metadata.len() != object.size {
            return Err(crate::Error::InvalidPath(format!(
                "CAS object {} size mismatch: expected {}, got {}",
                object.sha256,
                object.size,
                metadata.len()
            )));
        }
        let actual = sha256_file(&object_path)?;
        if actual != object.sha256 {
            return Err(crate::Error::ChecksumMismatch {
                expected: format!("CAS object SHA-256 {}", object.sha256),
                actual,
            });
        }
    }
    Ok(())
}

pub(crate) fn verify_cas_object_files_exist_with_expected_sizes(
    cas_dir: &Path,
    objects: &[CasObjectRef],
) -> crate::Result<()> {
    for object in objects {
        let object_path = crate::filesystem::object_path(cas_dir, &object.sha256)?;
        let metadata = std::fs::metadata(&object_path).map_err(|e| {
            crate::Error::NotFound(format!(
                "missing CAS object {} at {}: {e}",
                object.sha256,
                object_path.display()
            ))
        })?;
        if metadata.len() != object.size {
            return Err(crate::Error::InvalidPath(format!(
                "CAS object {} size mismatch: expected {}, got {}",
                object.sha256,
                object.size,
                metadata.len()
            )));
        }
    }
    Ok(())
}

fn verify_boot_assets(generation_dir: &Path, manifest: &BootAssetsManifest) -> crate::Result<()> {
    validate_sha256_hex("kernel_sha256", &manifest.kernel_sha256)?;
    validate_sha256_hex("initramfs_sha256", &manifest.initramfs_sha256)?;
    validate_sha256_hex("efi_bootloader_sha256", &manifest.efi_bootloader_sha256)?;

    verify_boot_asset(
        generation_dir,
        "kernel",
        &manifest.kernel,
        &manifest.kernel_sha256,
    )?;
    verify_boot_asset(
        generation_dir,
        "initramfs",
        &manifest.initramfs,
        &manifest.initramfs_sha256,
    )?;
    verify_boot_asset(
        generation_dir,
        "efi_bootloader",
        &manifest.efi_bootloader,
        &manifest.efi_bootloader_sha256,
    )?;
    Ok(())
}

fn verify_boot_asset(
    generation_dir: &Path,
    field: &str,
    rel: &str,
    expected_sha256: &str,
) -> crate::Result<()> {
    let rel = validate_boot_asset_relative_path(field, rel)?;
    let path = generation_dir.join(BOOT_ASSETS_DIR).join(rel);
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| {
        crate::Error::NotFound(format!(
            "missing boot asset {field} at {}: {e}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(crate::Error::InvalidPath(format!(
            "boot asset {field} must not be a symlink: {}",
            path.display()
        )));
    }
    if !metadata.file_type().is_file() {
        return Err(crate::Error::InvalidPath(format!(
            "boot asset {field} must be a regular file: {}",
            path.display()
        )));
    }
    verify_file_digest(&format!("boot asset {field}"), &path, expected_sha256)
}

fn copy_boot_asset(source: &Path, dest: &Path, label: &str) -> crate::Result<()> {
    let source_metadata = std::fs::metadata(source).map_err(|e| {
        crate::Error::NotFound(format!(
            "missing required boot asset {label} at {}: {e}",
            source.display()
        ))
    })?;
    if !source_metadata.file_type().is_file() {
        return Err(crate::Error::InvalidPath(format!(
            "boot asset source {label} must resolve to a regular file: {}",
            source.display()
        )));
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(existing) = std::fs::symlink_metadata(dest) {
        if !existing.file_type().is_file() && !existing.file_type().is_symlink() {
            return Err(crate::Error::InvalidPath(format!(
                "boot asset destination {label} is not replaceable: {}",
                dest.display()
            )));
        }
        std::fs::remove_file(dest)?;
    }

    std::fs::copy(source, dest).map_err(|e| {
        crate::Error::IoError(format!(
            "failed to copy boot asset {label} from {} to {}: {e}",
            source.display(),
            dest.display()
        ))
    })?;

    let dest_metadata = std::fs::symlink_metadata(dest)?;
    if dest_metadata.file_type().is_symlink() {
        return Err(crate::Error::InvalidPath(format!(
            "staged boot asset {label} must not be a symlink: {}",
            dest.display()
        )));
    }
    if !dest_metadata.file_type().is_file() {
        return Err(crate::Error::InvalidPath(format!(
            "staged boot asset {label} must be a regular file: {}",
            dest.display()
        )));
    }

    Ok(())
}

fn path_relative_to_generation(
    generation_dir: &Path,
    path: &Path,
    field: &str,
) -> crate::Result<String> {
    let rel = if path.is_absolute() {
        path.strip_prefix(generation_dir).map_err(|_| {
            crate::Error::InvalidPath(format!(
                "{field} path must be inside generation directory {}: {}",
                generation_dir.display(),
                path.display()
            ))
        })?
    } else {
        path
    };

    let rel = rel.to_string_lossy().replace('\\', "/");
    validate_generation_relative_path(field, &rel)?;
    Ok(rel)
}

fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> crate::Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &bytes)?;
    Ok(bytes)
}

fn validate_relative_path(field: &str, rel: &str, root_label: &str) -> crate::Result<PathBuf> {
    let path = Path::new(rel);
    if rel.is_empty() {
        return Err(crate::Error::InvalidPath(format!(
            "{field} path must not be empty"
        )));
    }
    if path.is_absolute() {
        return Err(crate::Error::InvalidPath(format!(
            "{field} path must be relative to {root_label}: {rel}"
        )));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::ParentDir => {
                return Err(crate::Error::PathTraversal(format!(
                    "{field} path must not contain '..': {rel}"
                )));
            }
            Component::CurDir => {
                return Err(crate::Error::InvalidPath(format!(
                    "{field} path must be normalized without '.': {rel}"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(crate::Error::InvalidPath(format!(
                    "{field} path must be relative to {root_label}: {rel}"
                )));
            }
        }
    }

    Ok(path.to_path_buf())
}

fn validate_generation_relative_path(field: &str, rel: &str) -> crate::Result<PathBuf> {
    validate_relative_path(field, rel, "the generation directory")
}

fn validate_boot_asset_relative_path(field: &str, rel: &str) -> crate::Result<PathBuf> {
    validate_relative_path(field, rel, BOOT_ASSETS_DIR)
}

fn infer_artifact_root(generation_dir: &Path) -> crate::Result<PathBuf> {
    let generation_dir = std::fs::canonicalize(generation_dir)?;
    let generations_dir = generation_dir.parent().ok_or_else(|| {
        crate::Error::InvalidPath(format!(
            "cannot infer artifact root from {}: expected parent directory named generations",
            generation_dir.display()
        ))
    })?;

    if generations_dir.file_name().and_then(|name| name.to_str()) != Some("generations") {
        return Err(crate::Error::InvalidPath(format!(
            "cannot infer artifact root from {}: expected parent directory named generations",
            generation_dir.display()
        )));
    }

    generations_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "cannot infer artifact root from {}: generations directory has no parent",
                generation_dir.display()
            ))
        })
}

fn resolve_cas_base(generation_dir: &Path, rel: &str) -> crate::Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel.is_empty() {
        return Err(crate::Error::InvalidPath(
            "cas_base path must not be empty".to_string(),
        ));
    }
    if rel_path.is_absolute() {
        return Err(crate::Error::InvalidPath(format!(
            "cas_base must be relative to the generation directory: {rel}"
        )));
    }
    for component in rel_path.components() {
        match component {
            Component::Normal(_) | Component::ParentDir => {}
            Component::CurDir => {
                return Err(crate::Error::InvalidPath(format!(
                    "cas_base path must be normalized without '.': {rel}"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(crate::Error::InvalidPath(format!(
                    "cas_base must be relative to the generation directory: {rel}"
                )));
            }
        }
    }

    let generation_dir = std::fs::canonicalize(generation_dir)?;
    let artifact_root = infer_artifact_root(&generation_dir)?;
    let expected_objects = std::fs::canonicalize(artifact_root.join("objects"))?;
    let resolved = std::fs::canonicalize(generation_dir.join(rel_path))?;

    if resolved != expected_objects {
        return Err(crate::Error::PathTraversal(format!(
            "cas_base must resolve exactly to <artifact-root>/objects; got {}",
            resolved.display()
        )));
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests;
