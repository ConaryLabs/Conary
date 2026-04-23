// crates/conary-core/src/generation/artifact.rs

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use super::metadata::{GENERATION_METADATA_FILE, GenerationMetadata, generation_path};

pub const ARTIFACT_MANIFEST_FILE: &str = ".conary-artifact.json";
pub const CAS_MANIFEST_FILE: &str = "cas-manifest.json";
pub const BOOT_ASSETS_DIR: &str = "boot-assets";
pub const BOOT_ASSETS_MANIFEST_REL: &str = "boot-assets/manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationArtifactManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    pub metadata: String,
    pub erofs: String,
    pub erofs_sha256: String,
    pub cas_base: String,
    pub cas_manifest: String,
    pub cas_manifest_sha256: String,
    pub boot_assets: String,
    pub boot_assets_sha256: String,
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
    pub cas_dir: PathBuf,
    pub cas_objects: Vec<CasObjectRef>,
    pub boot_assets: BootAssetsManifest,
}

pub fn load_generation_artifact(generation_dir: &Path) -> crate::Result<GenerationArtifact> {
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
    require_version(".conary-artifact.json", artifact_manifest.version)?;
    require_supported_architecture(&artifact_manifest.architecture)?;
    validate_artifact_manifest_paths(&artifact_manifest)?;
    validate_artifact_manifest_hashes(&artifact_manifest)?;

    if artifact_manifest.metadata != GENERATION_METADATA_FILE {
        return Err(crate::Error::InvalidPath(format!(
            "artifact metadata path must be {GENERATION_METADATA_FILE}, got {}",
            artifact_manifest.metadata
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
    require_version("cas-manifest", cas_manifest.version)?;
    require_manifest_identity(
        "cas-manifest",
        artifact_manifest.generation,
        &artifact_manifest.architecture,
        cas_manifest.generation,
        &cas_manifest.architecture,
    )?;
    verify_cas_objects(&cas_dir, &cas_manifest.objects)?;

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
    require_version("boot-assets manifest", boot_assets.version)?;
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
        cas_dir,
        cas_objects: cas_manifest.objects,
        boot_assets,
    })
}

pub fn load_installed_generation_artifact(generation: i64) -> crate::Result<GenerationArtifact> {
    load_generation_artifact(&generation_path(generation))
}

fn require_version(name: &str, version: u32) -> crate::Result<()> {
    if version == 1 {
        Ok(())
    } else {
        Err(crate::Error::InvalidPath(format!(
            "{name} has unsupported version {version}; expected version 1"
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
    validate_generation_relative_path("cas_manifest", &manifest.cas_manifest)?;
    validate_generation_relative_path("boot_assets", &manifest.boot_assets)?;
    Ok(())
}

fn validate_artifact_manifest_hashes(manifest: &GenerationArtifactManifest) -> crate::Result<()> {
    validate_sha256_hex("erofs_sha256", &manifest.erofs_sha256)?;
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
    format!("{:x}", Sha256::digest(bytes))
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

fn verify_cas_objects(cas_dir: &Path, objects: &[CasObjectRef]) -> crate::Result<()> {
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
mod tests {
    use super::*;
    use crate::generation::metadata::{
        GENERATION_FORMAT, GENERATION_METADATA_FILE, GenerationMetadata, mark_generation_pending,
    };
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SHA_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const SHA_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    struct ArtifactFixture {
        _tmp: TempDir,
        generation_dir: PathBuf,
        root_erofs: PathBuf,
        cas_manifest_path: PathBuf,
        boot_manifest_path: PathBuf,
        artifact_manifest_path: PathBuf,
        cas_object_hash: String,
        kernel_path: PathBuf,
    }

    fn digest_bytes(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn digest_file(path: &std::path::Path) -> String {
        digest_bytes(&fs::read(path).unwrap())
    }

    fn write_json<T: Serialize>(path: &std::path::Path, value: &T) -> Vec<u8> {
        let bytes = serde_json::to_vec_pretty(value).unwrap();
        fs::write(path, &bytes).unwrap();
        bytes
    }

    fn metadata_for_fixture(
        generation: i64,
        artifact_digest: Option<String>,
    ) -> GenerationMetadata {
        GenerationMetadata {
            generation,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(10),
            cas_objects_referenced: Some(1),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            artifact_manifest_sha256: artifact_digest,
            created_at: "2026-04-22T00:00:00Z".to_string(),
            package_count: 1,
            kernel_version: Some("6.19.8-conary".to_string()),
            summary: "fixture generation".to_string(),
        }
    }

    impl ArtifactFixture {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let artifact_root = tmp.path().join("output");
            let generation_dir = artifact_root.join("generations/1");
            let objects_dir = artifact_root.join("objects");
            let boot_assets_dir = generation_dir.join(BOOT_ASSETS_DIR);
            fs::create_dir_all(&generation_dir).unwrap();
            fs::create_dir_all(&objects_dir).unwrap();
            fs::create_dir_all(boot_assets_dir.join("EFI/BOOT")).unwrap();

            let root_erofs = generation_dir.join("root.erofs");
            fs::write(&root_erofs, b"root-erofs").unwrap();

            let cas_object_bytes = b"cas-object";
            let cas_object_hash = digest_bytes(cas_object_bytes);
            let cas_object_path =
                crate::filesystem::object_path(&objects_dir, &cas_object_hash).unwrap();
            fs::create_dir_all(cas_object_path.parent().unwrap()).unwrap();
            fs::write(&cas_object_path, cas_object_bytes).unwrap();

            let kernel_path = boot_assets_dir.join("vmlinuz");
            let initramfs_path = boot_assets_dir.join("initramfs.img");
            let efi_path = boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI");
            fs::write(&kernel_path, b"kernel").unwrap();
            fs::write(&initramfs_path, b"initramfs").unwrap();
            fs::write(&efi_path, b"efi").unwrap();

            let cas_manifest_path = generation_dir.join(CAS_MANIFEST_FILE);
            let cas_manifest = CasManifest {
                version: 1,
                generation: 1,
                architecture: "x86_64".to_string(),
                objects: vec![CasObjectRef {
                    sha256: cas_object_hash.clone(),
                    size: cas_object_bytes.len() as u64,
                }],
            };
            let cas_manifest_bytes = write_json(&cas_manifest_path, &cas_manifest);

            let boot_manifest_path = generation_dir.join(BOOT_ASSETS_MANIFEST_REL);
            let boot_manifest = BootAssetsManifest {
                version: 1,
                generation: 1,
                architecture: "x86_64".to_string(),
                kernel_version: "6.19.8-conary".to_string(),
                kernel: "vmlinuz".to_string(),
                kernel_sha256: digest_file(&kernel_path),
                initramfs: "initramfs.img".to_string(),
                initramfs_sha256: digest_file(&initramfs_path),
                efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
                efi_bootloader_sha256: digest_file(&efi_path),
                created_at: "2026-04-22T00:00:00Z".to_string(),
            };
            let boot_manifest_bytes = write_json(&boot_manifest_path, &boot_manifest);

            let artifact_manifest_path = generation_dir.join(ARTIFACT_MANIFEST_FILE);
            let artifact_manifest = GenerationArtifactManifest {
                version: 1,
                generation: 1,
                architecture: "x86_64".to_string(),
                metadata: GENERATION_METADATA_FILE.to_string(),
                erofs: "root.erofs".to_string(),
                erofs_sha256: digest_file(&root_erofs),
                cas_base: "../../objects".to_string(),
                cas_manifest: CAS_MANIFEST_FILE.to_string(),
                cas_manifest_sha256: digest_bytes(&cas_manifest_bytes),
                boot_assets: BOOT_ASSETS_MANIFEST_REL.to_string(),
                boot_assets_sha256: digest_bytes(&boot_manifest_bytes),
            };
            let artifact_bytes = write_json(&artifact_manifest_path, &artifact_manifest);
            metadata_for_fixture(1, Some(digest_bytes(&artifact_bytes)))
                .write_to(&generation_dir)
                .unwrap();

            Self {
                _tmp: tmp,
                generation_dir,
                root_erofs,
                cas_manifest_path,
                boot_manifest_path,
                artifact_manifest_path,
                cas_object_hash,
                kernel_path,
            }
        }

        fn artifact_manifest(&self) -> GenerationArtifactManifest {
            serde_json::from_slice(&fs::read(&self.artifact_manifest_path).unwrap()).unwrap()
        }

        fn cas_manifest(&self) -> CasManifest {
            serde_json::from_slice(&fs::read(&self.cas_manifest_path).unwrap()).unwrap()
        }

        fn boot_manifest(&self) -> BootAssetsManifest {
            serde_json::from_slice(&fs::read(&self.boot_manifest_path).unwrap()).unwrap()
        }

        fn write_metadata_digest(&self, digest: Option<String>) {
            metadata_for_fixture(1, digest)
                .write_to(&self.generation_dir)
                .unwrap();
        }

        fn rewrite_artifact_manifest(&self, mutate: impl FnOnce(&mut GenerationArtifactManifest)) {
            let mut manifest = self.artifact_manifest();
            mutate(&mut manifest);
            let bytes = write_json(&self.artifact_manifest_path, &manifest);
            self.write_metadata_digest(Some(digest_bytes(&bytes)));
        }

        fn rewrite_cas_manifest(&self, mutate: impl FnOnce(&mut CasManifest), update_parent: bool) {
            let mut manifest = self.cas_manifest();
            mutate(&mut manifest);
            let bytes = write_json(&self.cas_manifest_path, &manifest);
            if update_parent {
                self.rewrite_artifact_manifest(|artifact| {
                    artifact.cas_manifest_sha256 = digest_bytes(&bytes);
                });
            }
        }

        fn rewrite_boot_manifest(
            &self,
            mutate: impl FnOnce(&mut BootAssetsManifest),
            update_parent: bool,
        ) {
            let mut manifest = self.boot_manifest();
            mutate(&mut manifest);
            let bytes = write_json(&self.boot_manifest_path, &manifest);
            if update_parent {
                self.rewrite_artifact_manifest(|artifact| {
                    artifact.boot_assets_sha256 = digest_bytes(&bytes);
                });
            }
        }
    }

    #[test]
    fn artifact_manifest_json_roundtrips() {
        let manifest = GenerationArtifactManifest {
            version: 1,
            generation: 7,
            architecture: "x86_64".to_string(),
            metadata: ".conary-gen.json".to_string(),
            erofs: "root.erofs".to_string(),
            erofs_sha256: SHA_A.to_string(),
            cas_base: "../../objects".to_string(),
            cas_manifest: "cas-manifest.json".to_string(),
            cas_manifest_sha256: SHA_B.to_string(),
            boot_assets: "boot-assets/manifest.json".to_string(),
            boot_assets_sha256: SHA_C.to_string(),
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let loaded: GenerationArtifactManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, manifest);
    }

    #[test]
    fn cas_manifest_json_roundtrips() {
        let manifest = CasManifest {
            version: 1,
            generation: 7,
            architecture: "x86_64".to_string(),
            objects: vec![CasObjectRef {
                sha256: SHA_A.to_string(),
                size: 4096,
            }],
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let loaded: CasManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, manifest);
    }

    #[test]
    fn boot_assets_manifest_json_roundtrips() {
        let manifest = BootAssetsManifest {
            version: 1,
            generation: 7,
            architecture: "x86_64".to_string(),
            kernel_version: "6.19.8-conary".to_string(),
            kernel: "vmlinuz".to_string(),
            kernel_sha256: SHA_A.to_string(),
            initramfs: "initramfs.img".to_string(),
            initramfs_sha256: SHA_B.to_string(),
            efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
            efi_bootloader_sha256: SHA_D.to_string(),
            created_at: "2026-04-22T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let loaded: BootAssetsManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, manifest);
    }

    #[test]
    fn generation_relative_paths_reject_absolute_and_parent_traversal() {
        for field in ["metadata", "erofs", "cas_manifest", "boot_assets"] {
            assert!(validate_generation_relative_path(field, "/absolute").is_err());
            assert!(validate_generation_relative_path(field, "../escape").is_err());
            assert!(validate_generation_relative_path(field, "safe/../escape").is_err());
        }
    }

    #[test]
    fn boot_asset_relative_paths_reject_absolute_and_parent_traversal() {
        assert!(validate_boot_asset_relative_path("kernel", "/vmlinuz").is_err());
        assert!(validate_boot_asset_relative_path("kernel", "../vmlinuz").is_err());
        assert!(validate_boot_asset_relative_path("kernel", "EFI/../vmlinuz").is_err());
        assert_eq!(
            validate_boot_asset_relative_path("efi_bootloader", "EFI/BOOT/BOOTX64.EFI").unwrap(),
            PathBuf::from("EFI/BOOT/BOOTX64.EFI")
        );
    }

    #[test]
    fn cas_base_resolves_to_artifact_root_objects() {
        let tmp = TempDir::new().unwrap();
        let generation_dir = tmp.path().join("output/generations/1");
        let objects_dir = tmp.path().join("output/objects");
        std::fs::create_dir_all(&generation_dir).unwrap();
        std::fs::create_dir_all(&objects_dir).unwrap();

        let resolved = resolve_cas_base(&generation_dir, "../../objects").unwrap();

        assert_eq!(resolved, std::fs::canonicalize(objects_dir).unwrap());
    }

    #[test]
    fn cas_base_rejects_absolute_and_outside_artifact_root() {
        let tmp = TempDir::new().unwrap();
        let generation_dir = tmp.path().join("output/generations/1");
        std::fs::create_dir_all(&generation_dir).unwrap();
        std::fs::create_dir_all(tmp.path().join("output/objects")).unwrap();
        std::fs::create_dir_all(tmp.path().join("objects")).unwrap();

        assert!(resolve_cas_base(&generation_dir, "/objects").is_err());
        assert!(resolve_cas_base(&generation_dir, "../../../objects").is_err());
    }

    #[test]
    fn artifact_root_requires_parent_named_generations() {
        let tmp = TempDir::new().unwrap();
        let generation_dir = tmp.path().join("output/not-generations/1");
        std::fs::create_dir_all(&generation_dir).unwrap();

        let err = infer_artifact_root(&generation_dir).unwrap_err();

        assert!(
            err.to_string()
                .contains("parent directory named generations")
        );
    }

    #[test]
    fn complete_artifact_loads_successfully() {
        let fixture = ArtifactFixture::new();

        let artifact = load_generation_artifact(&fixture.generation_dir).unwrap();

        assert_eq!(artifact.generation, 1);
        assert_eq!(
            artifact
                .metadata
                .artifact_manifest_sha256
                .as_deref()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(artifact.erofs_path, fixture.root_erofs);
        assert_eq!(artifact.cas_objects.len(), 1);
        assert_eq!(artifact.boot_assets.kernel, "vmlinuz");
    }

    #[test]
    fn pending_generations_are_rejected() {
        let fixture = ArtifactFixture::new();
        mark_generation_pending(&fixture.generation_dir).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("pending"));
    }

    #[test]
    fn missing_artifact_manifest_reports_pre_export_contract() {
        let fixture = ArtifactFixture::new();
        fs::remove_file(&fixture.artifact_manifest_path).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("pre-export-contract"));
    }

    #[test]
    fn missing_metadata_reports_incomplete_artifact() {
        let fixture = ArtifactFixture::new();
        fs::remove_file(fixture.generation_dir.join(GENERATION_METADATA_FILE)).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("metadata"));
    }

    #[test]
    fn artifact_manifest_requires_matching_metadata_digest() {
        let fixture = ArtifactFixture::new();
        fixture.write_metadata_digest(None);

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("artifact_manifest_sha256"));
    }

    #[test]
    fn mismatched_generation_across_manifests_fails() {
        let fixture = ArtifactFixture::new();
        fixture.rewrite_cas_manifest(|manifest| manifest.generation = 2, true);

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("generation"));
    }

    #[test]
    fn mismatched_architecture_across_manifests_fails() {
        let fixture = ArtifactFixture::new();
        fixture.rewrite_boot_manifest(
            |manifest| manifest.architecture = "aarch64".to_string(),
            true,
        );

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("architecture"));
    }

    #[test]
    fn bad_erofs_digest_fails() {
        let fixture = ArtifactFixture::new();
        fs::write(&fixture.root_erofs, b"tampered-root").unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("root.erofs"));
    }

    #[test]
    fn bad_child_manifest_digest_fails() {
        let fixture = ArtifactFixture::new();
        fixture.rewrite_cas_manifest(|manifest| manifest.objects.clear(), false);

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("cas-manifest"));
    }

    #[test]
    fn missing_cas_manifest_fails() {
        let fixture = ArtifactFixture::new();
        fs::remove_file(&fixture.cas_manifest_path).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("cas-manifest"));
    }

    #[test]
    fn missing_cas_object_fails() {
        let fixture = ArtifactFixture::new();
        let object_path = crate::filesystem::object_path(
            &fixture.generation_dir.join("../../objects"),
            &fixture.cas_object_hash,
        )
        .unwrap();
        fs::remove_file(object_path).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("CAS object"));
    }

    #[test]
    fn cas_object_size_mismatch_fails() {
        let fixture = ArtifactFixture::new();
        fixture.rewrite_cas_manifest(|manifest| manifest.objects[0].size += 1, true);

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("size"));
    }

    #[test]
    fn cas_object_sha_mismatch_fails() {
        let fixture = ArtifactFixture::new();
        let object_path = crate::filesystem::object_path(
            &fixture.generation_dir.join("../../objects"),
            &fixture.cas_object_hash,
        )
        .unwrap();
        fs::write(object_path, b"same-len!!").unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("SHA-256"));
    }

    #[test]
    fn duplicate_cas_manifest_entries_are_rejected() {
        let fixture = ArtifactFixture::new();
        fixture.rewrite_cas_manifest(
            |manifest| manifest.objects.push(manifest.objects[0].clone()),
            true,
        );

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn unsorted_cas_manifest_entries_load_successfully() {
        let fixture = ArtifactFixture::new();
        let second_bytes = b"second-object";
        let second_hash = digest_bytes(second_bytes);
        let objects_dir = fixture.generation_dir.join("../../objects");
        let second_path = crate::filesystem::object_path(&objects_dir, &second_hash).unwrap();
        fs::create_dir_all(second_path.parent().unwrap()).unwrap();
        fs::write(second_path, second_bytes).unwrap();
        fixture.rewrite_cas_manifest(
            |manifest| {
                manifest.objects.insert(
                    0,
                    CasObjectRef {
                        sha256: second_hash,
                        size: second_bytes.len() as u64,
                    },
                );
            },
            true,
        );

        let artifact = load_generation_artifact(&fixture.generation_dir).unwrap();

        assert_eq!(artifact.cas_objects.len(), 2);
    }

    #[test]
    fn missing_boot_assets_manifest_fails() {
        let fixture = ArtifactFixture::new();
        fs::remove_file(&fixture.boot_manifest_path).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("boot-assets"));
    }

    #[test]
    fn missing_boot_asset_fails() {
        let fixture = ArtifactFixture::new();
        fs::remove_file(&fixture.kernel_path).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("boot asset"));
    }

    #[cfg(unix)]
    #[test]
    fn boot_asset_symlink_fails() {
        let fixture = ArtifactFixture::new();
        fs::remove_file(&fixture.kernel_path).unwrap();
        std::os::unix::fs::symlink("/boot/vmlinuz", &fixture.kernel_path).unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("symlink"));
    }

    #[test]
    fn boot_asset_sha_mismatch_fails() {
        let fixture = ArtifactFixture::new();
        fs::write(&fixture.kernel_path, b"tampered-kernel").unwrap();

        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

        assert!(err.to_string().contains("boot asset"));
    }

    #[test]
    fn invalid_sha256_strings_are_rejected() {
        let fixture = ArtifactFixture::new();
        fixture.rewrite_artifact_manifest(|manifest| {
            manifest.erofs_sha256 = SHA_A.to_ascii_uppercase();
        });
        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();
        assert!(err.to_string().contains("lowercase"));

        let fixture = ArtifactFixture::new();
        fixture.rewrite_artifact_manifest(|manifest| {
            manifest.erofs_sha256 = "abc123".to_string();
        });
        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();
        assert!(err.to_string().contains("64"));
    }

    #[test]
    fn unknown_manifest_versions_are_rejected() {
        let fixture = ArtifactFixture::new();
        fixture.rewrite_artifact_manifest(|manifest| manifest.version = 2);
        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();
        assert!(err.to_string().contains("version"));

        let fixture = ArtifactFixture::new();
        fixture.rewrite_cas_manifest(|manifest| manifest.version = 2, true);
        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();
        assert!(err.to_string().contains("version"));

        let fixture = ArtifactFixture::new();
        fixture.rewrite_boot_manifest(|manifest| manifest.version = 2, true);
        let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn unsupported_architectures_are_rejected() {
        for architecture in ["aarch64", "riscv64"] {
            let fixture = ArtifactFixture::new();
            fixture.rewrite_artifact_manifest(|manifest| {
                manifest.architecture = architecture.to_string();
            });

            let err = load_generation_artifact(&fixture.generation_dir).unwrap_err();

            assert!(err.to_string().contains("unsupported"));
        }
    }
}
