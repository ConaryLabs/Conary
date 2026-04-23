// crates/conary-core/src/generation/artifact.rs

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

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

    generations_dir.parent().map(Path::to_path_buf).ok_or_else(|| {
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
    use std::path::PathBuf;
    use tempfile::TempDir;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SHA_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const SHA_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

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

        assert!(err.to_string().contains("parent directory named generations"));
    }
}
