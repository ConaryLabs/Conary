// conary-core/src/generation/artifact/tests.rs

use super::*;
use crate::generation::metadata::{
    GENERATION_FORMAT, GENERATION_METADATA_FILE, GenerationMetadata, mark_generation_pending,
};
use crate::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_FILE, MUTABLE_STATE_MANIFEST_FILE,
};
use crate::generation::test_support::{write_root_manifests, write_root_manifests_with_objects};
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
    hex::encode(Sha256::digest(bytes))
}

fn digest_file(path: &std::path::Path) -> String {
    digest_bytes(&fs::read(path).unwrap())
}

fn write_json<T: Serialize>(path: &std::path::Path, value: &T) -> Vec<u8> {
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

fn write_cas_object(objects_dir: &std::path::Path, bytes: &[u8]) -> CasObjectRef {
    let sha256 = digest_bytes(bytes);
    let object_path = crate::filesystem::object_path(objects_dir, &sha256).unwrap();
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(object_path, bytes).unwrap();
    CasObjectRef {
        sha256,
        size: bytes.len() as u64,
    }
}

fn metadata_for_fixture(generation: i64, artifact_digest: Option<String>) -> GenerationMetadata {
    GenerationMetadata {
        generation,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(10),
        cas_objects_referenced: Some(1),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: artifact_digest,
        security_capability_xattr_count: None,
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
        let (generation_root_manifest_sha256, mutable_state_manifest_sha256) =
            write_root_manifests(&generation_dir);

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
            version: ARTIFACT_MANIFEST_VERSION,
            generation: 1,
            architecture: "x86_64".to_string(),
            carrier_capabilities: GenerationCarrierCapabilities::default(),
            metadata: GENERATION_METADATA_FILE.to_string(),
            erofs: "root.erofs".to_string(),
            erofs_sha256: digest_file(&root_erofs),
            generation_root_manifest: GENERATION_ROOT_MANIFEST_FILE.to_string(),
            generation_root_manifest_sha256,
            mutable_state_manifest: MUTABLE_STATE_MANIFEST_FILE.to_string(),
            mutable_state_manifest_sha256,
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
        version: ARTIFACT_MANIFEST_VERSION,
        generation: 7,
        architecture: "x86_64".to_string(),
        carrier_capabilities: GenerationCarrierCapabilities {
            immutable_backing_security: Some(crate::ccs::ImmutableBackingSecurity {
                mechanism: crate::ccs::ImmutableBackingSecurityMechanism::Selinux,
                xattr_value: b"system_u:object_r:usr_t:s0\0".to_vec(),
            }),
            ..GenerationCarrierCapabilities::default()
        },
        metadata: ".conary-gen.json".to_string(),
        erofs: "root.erofs".to_string(),
        erofs_sha256: SHA_A.to_string(),
        generation_root_manifest: GENERATION_ROOT_MANIFEST_FILE.to_string(),
        generation_root_manifest_sha256: SHA_B.to_string(),
        mutable_state_manifest: MUTABLE_STATE_MANIFEST_FILE.to_string(),
        mutable_state_manifest_sha256: SHA_C.to_string(),
        cas_base: "../../objects".to_string(),
        cas_manifest: "cas-manifest.json".to_string(),
        cas_manifest_sha256: SHA_D.to_string(),
        boot_assets: "boot-assets/manifest.json".to_string(),
        boot_assets_sha256: SHA_A.to_string(),
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
    assert_eq!(
        artifact.artifact_manifest.carrier_capabilities,
        GenerationCarrierCapabilities::default()
    );
}

#[test]
fn artifact_manifest_rejects_missing_carrier_capability_authority() {
    let fixture = ArtifactFixture::new();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture.artifact_manifest_path).unwrap()).unwrap();
    manifest
        .as_object_mut()
        .unwrap()
        .remove("carrier_capabilities");
    let bytes = write_json(&fixture.artifact_manifest_path, &manifest);
    fixture.write_metadata_digest(Some(digest_bytes(&bytes)));

    let error = load_generation_artifact(&fixture.generation_dir).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("generation carrier capabilities has unsupported version 0")
    );
}

#[test]
fn artifact_manifest_rejects_retired_carrier_capability_authority() {
    let fixture = ArtifactFixture::new();
    fixture.rewrite_artifact_manifest(|manifest| {
        manifest.carrier_capabilities.version = 0;
    });

    let error = load_generation_artifact(&fixture.generation_dir).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("generation carrier capabilities has unsupported version 0")
    );
}

#[test]
fn artifact_v2_is_a_hard_cut_that_requires_generation_rebuild() {
    let fixture = ArtifactFixture::new();
    fixture.rewrite_artifact_manifest(|manifest| {
        manifest.version = 2;
    });

    let error = load_generation_artifact(&fixture.generation_dir).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unsupported version 2; expected version 3")
    );
}

#[test]
fn write_generation_artifact_sorts_cas_manifest_objects() {
    let tmp = TempDir::new().unwrap();
    let artifact_root = tmp.path().join("output");
    let generation_dir = artifact_root.join("generations/1");
    let objects_dir = artifact_root.join("objects");
    fs::create_dir_all(generation_dir.join(BOOT_ASSETS_DIR).join("EFI/BOOT")).unwrap();
    fs::create_dir_all(&objects_dir).unwrap();

    let erofs_path = generation_dir.join("root.erofs");
    fs::write(&erofs_path, b"root-erofs").unwrap();
    fs::write(generation_dir.join("boot-assets/vmlinuz"), b"kernel").unwrap();
    fs::write(
        generation_dir.join("boot-assets/initramfs.img"),
        b"initramfs",
    )
    .unwrap();
    fs::write(
        generation_dir.join("boot-assets/EFI/BOOT/BOOTX64.EFI"),
        b"efi",
    )
    .unwrap();

    let object_a = write_cas_object(&objects_dir, b"a-object");
    let object_b = write_cas_object(&objects_dir, b"b-object");
    write_root_manifests_with_objects(&generation_dir, &[object_b.clone(), object_a.clone()]);
    let boot_assets = BootAssetsManifest {
        version: 1,
        generation: 1,
        architecture: "x86_64".to_string(),
        kernel_version: "6.19.8-conary".to_string(),
        kernel: "vmlinuz".to_string(),
        kernel_sha256: digest_file(&generation_dir.join("boot-assets/vmlinuz")),
        initramfs: "initramfs.img".to_string(),
        initramfs_sha256: digest_file(&generation_dir.join("boot-assets/initramfs.img")),
        efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
        efi_bootloader_sha256: digest_file(
            &generation_dir.join("boot-assets/EFI/BOOT/BOOTX64.EFI"),
        ),
        created_at: "2026-04-22T00:00:00Z".to_string(),
    };

    let artifact_digest = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &generation_dir,
        generation: 1,
        architecture: "x86_64",
        erofs_path: &erofs_path,
        cas_base_rel: "../../objects",
        cas_verification: CasObjectVerification::Deep,
        boot_assets,
        carrier_capabilities: Default::default(),
    })
    .unwrap();
    metadata_for_fixture(1, Some(artifact_digest))
        .write_to(&generation_dir)
        .unwrap();

    let cas_manifest: CasManifest =
        serde_json::from_slice(&fs::read(generation_dir.join(CAS_MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(cas_manifest.objects.len(), 2);
    assert!(cas_manifest.objects[0].sha256 < cas_manifest.objects[1].sha256);

    load_generation_artifact(&generation_dir).unwrap();
}

#[test]
fn preverified_artifact_write_skips_deep_cas_hashing_but_loader_verifies() {
    let tmp = TempDir::new().unwrap();
    let artifact_root = tmp.path().join("output");
    let generation_dir = artifact_root.join("generations/1");
    let objects_dir = artifact_root.join("objects");
    fs::create_dir_all(generation_dir.join(BOOT_ASSETS_DIR).join("EFI/BOOT")).unwrap();
    fs::create_dir_all(&objects_dir).unwrap();

    let erofs_path = generation_dir.join("root.erofs");
    fs::write(&erofs_path, b"root-erofs").unwrap();
    fs::write(generation_dir.join("boot-assets/vmlinuz"), b"kernel").unwrap();
    fs::write(
        generation_dir.join("boot-assets/initramfs.img"),
        b"initramfs",
    )
    .unwrap();
    fs::write(
        generation_dir.join("boot-assets/EFI/BOOT/BOOTX64.EFI"),
        b"efi",
    )
    .unwrap();

    let expected_hash = digest_bytes(b"right");
    let object_path = crate::filesystem::object_path(&objects_dir, &expected_hash).unwrap();
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(object_path, b"wrong").unwrap();
    write_root_manifests_with_objects(
        &generation_dir,
        &[CasObjectRef {
            sha256: expected_hash.clone(),
            size: 5,
        }],
    );
    let boot_assets = BootAssetsManifest {
        version: 1,
        generation: 1,
        architecture: "x86_64".to_string(),
        kernel_version: "6.19.8-conary".to_string(),
        kernel: "vmlinuz".to_string(),
        kernel_sha256: digest_file(&generation_dir.join("boot-assets/vmlinuz")),
        initramfs: "initramfs.img".to_string(),
        initramfs_sha256: digest_file(&generation_dir.join("boot-assets/initramfs.img")),
        efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
        efi_bootloader_sha256: digest_file(
            &generation_dir.join("boot-assets/EFI/BOOT/BOOTX64.EFI"),
        ),
        created_at: "2026-04-22T00:00:00Z".to_string(),
    };

    let artifact_digest = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &generation_dir,
        generation: 1,
        architecture: "x86_64",
        erofs_path: &erofs_path,
        cas_base_rel: "../../objects",
        cas_verification: CasObjectVerification::AlreadyVerified,
        boot_assets,
        carrier_capabilities: Default::default(),
    })
    .unwrap();
    metadata_for_fixture(1, Some(artifact_digest))
        .write_to(&generation_dir)
        .unwrap();

    let artifact = load_generation_artifact_for_activation(&generation_dir).unwrap();
    assert_eq!(artifact.cas_objects.len(), 1);

    let err = load_generation_artifact(&generation_dir).unwrap_err();
    assert!(err.to_string().contains("CAS object SHA-256"));
}

#[cfg(unix)]
#[test]
fn stage_boot_assets_dereferences_symlink_sources() {
    let tmp = TempDir::new().unwrap();
    let generation_dir = tmp.path().join("output/generations/1");
    let source_dir = tmp.path().join("source");
    fs::create_dir_all(&generation_dir).unwrap();
    fs::create_dir_all(source_dir.join("EFI/BOOT")).unwrap();

    fs::write(source_dir.join("vmlinuz-real"), b"kernel").unwrap();
    fs::write(source_dir.join("initramfs-real.img"), b"initramfs").unwrap();
    fs::write(source_dir.join("EFI/BOOT/BOOTX64-real.EFI"), b"efi").unwrap();
    std::os::unix::fs::symlink(source_dir.join("vmlinuz-real"), source_dir.join("vmlinuz"))
        .unwrap();
    std::os::unix::fs::symlink(
        source_dir.join("initramfs-real.img"),
        source_dir.join("initramfs.img"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        source_dir.join("EFI/BOOT/BOOTX64-real.EFI"),
        source_dir.join("EFI/BOOT/BOOTX64.EFI"),
    )
    .unwrap();

    let manifest = stage_boot_assets(BootAssetSources {
        generation_dir: &generation_dir,
        generation: 1,
        architecture: "x86_64",
        kernel_version: "6.19.8-conary",
        kernel: &source_dir.join("vmlinuz"),
        initramfs: &source_dir.join("initramfs.img"),
        efi_bootloader: &source_dir.join("EFI/BOOT/BOOTX64.EFI"),
    })
    .unwrap();

    assert_eq!(manifest.kernel_sha256, digest_bytes(b"kernel"));
    assert!(
        !fs::symlink_metadata(generation_dir.join("boot-assets/vmlinuz"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
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
    fixture.rewrite_artifact_manifest(|manifest| {
        manifest.version = ARTIFACT_MANIFEST_VERSION + 1;
    });
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
