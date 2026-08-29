// crates/conary-core/src/generation/artifact/cas/tests.rs

use super::*;
use crate::generation::artifact::{
    ARTIFACT_MANIFEST_FILE, ArtifactWriteInputs, BOOT_ASSETS_DIR, BootAssetsManifest,
    CAS_MANIFEST_FILE, CasObjectVerification, GenerationCarrierCapabilities,
    write_generation_artifact,
};
use crate::generation::test_support::write_root_manifests_with_objects;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

struct WriterFixture {
    _tmp: TempDir,
    generation_dir: PathBuf,
    cas_dir: PathBuf,
    erofs_path: PathBuf,
    object: CasObjectRef,
    boot_assets: BootAssetsManifest,
}

impl WriterFixture {
    fn new(payload: &[u8]) -> Self {
        let tmp = TempDir::new().unwrap();
        let artifact_root = tmp.path().join("output");
        let generation_dir = artifact_root.join("generations/1");
        let cas_dir = artifact_root.join("objects");
        let boot_dir = generation_dir.join(BOOT_ASSETS_DIR);
        fs::create_dir_all(boot_dir.join("EFI/BOOT")).unwrap();
        fs::create_dir_all(&cas_dir).unwrap();

        let erofs_path = generation_dir.join("root.erofs");
        fs::write(&erofs_path, b"root-erofs").unwrap();
        let object = write_object(&cas_dir, payload);
        write_root_manifests_with_objects(&generation_dir, std::slice::from_ref(&object));

        let kernel = boot_dir.join("vmlinuz");
        let initramfs = boot_dir.join("initramfs.img");
        let efi = boot_dir.join("EFI/BOOT/BOOTX64.EFI");
        fs::write(&kernel, b"kernel").unwrap();
        fs::write(&initramfs, b"initramfs").unwrap();
        fs::write(&efi, b"efi").unwrap();
        let boot_assets = BootAssetsManifest {
            version: 1,
            generation: 1,
            architecture: "x86_64".to_string(),
            kernel_version: "6.19.8-conary".to_string(),
            kernel: "vmlinuz".to_string(),
            kernel_sha256: digest_file(&kernel),
            initramfs: "initramfs.img".to_string(),
            initramfs_sha256: digest_file(&initramfs),
            efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
            efi_bootloader_sha256: digest_file(&efi),
            created_at: "2026-08-29T00:00:00Z".to_string(),
        };

        Self {
            _tmp: tmp,
            generation_dir,
            cas_dir,
            erofs_path,
            object,
            boot_assets,
        }
    }

    fn presence_proof(&self) -> VerifiedCasObjectPresence<'_> {
        verify_cas_object_presence(&self.cas_dir, std::slice::from_ref(&self.object)).unwrap()
    }

    fn write_with_proof(&self, proof: VerifiedCasObjectPresence<'_>) -> crate::Result<String> {
        write_generation_artifact(ArtifactWriteInputs {
            generation_dir: &self.generation_dir,
            generation: 1,
            architecture: "x86_64",
            erofs_path: &self.erofs_path,
            cas_base_rel: "../../objects",
            cas_verification: CasObjectVerification::VerifiedPresence(&proof),
            boot_assets: self.boot_assets.clone(),
            carrier_capabilities: GenerationCarrierCapabilities::default(),
        })
    }

    fn assert_no_artifact_authority(&self) {
        assert!(!self.generation_dir.join(CAS_MANIFEST_FILE).exists());
        assert!(!self.generation_dir.join(ARTIFACT_MANIFEST_FILE).exists());
    }
}

fn digest_file(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).unwrap()))
}

fn write_object(cas_dir: &Path, bytes: &[u8]) -> CasObjectRef {
    let sha256 = hex::encode(Sha256::digest(bytes));
    let object_path = crate::filesystem::object_path(cas_dir, &sha256).unwrap();
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(object_path, bytes).unwrap();
    CasObjectRef {
        sha256,
        size: bytes.len() as u64,
    }
}

#[test]
fn presence_proof_binds_canonical_root_and_exact_object_set() {
    let tmp = TempDir::new().unwrap();
    let cas_dir = tmp.path().join("objects");
    let other_cas_dir = tmp.path().join("other-objects");
    fs::create_dir_all(&cas_dir).unwrap();
    fs::create_dir_all(&other_cas_dir).unwrap();

    let objects = deduplicate_sort_cas_objects(vec![
        write_object(&cas_dir, b"second"),
        write_object(&cas_dir, b"first"),
    ])
    .unwrap();
    let proof = verify_cas_object_presence(&cas_dir, &objects).unwrap();
    let canonical_cas_dir = fs::canonicalize(&cas_dir).unwrap();

    proof
        .require_exact_match(&canonical_cas_dir, &objects)
        .unwrap();

    let mut changed_objects = objects.clone();
    changed_objects[0].size += 1;
    let err = proof
        .require_exact_match(&canonical_cas_dir, &changed_objects)
        .unwrap_err();
    assert!(err.to_string().contains("object set"));

    let err = proof
        .require_exact_match(&fs::canonicalize(other_cas_dir).unwrap(), &objects)
        .unwrap_err();
    assert!(err.to_string().contains("bound to"));
}

#[test]
fn presence_proof_requires_deduplicated_sorted_inputs() {
    let tmp = TempDir::new().unwrap();
    let cas_dir = tmp.path().join("objects");
    fs::create_dir_all(&cas_dir).unwrap();
    let object_a = write_object(&cas_dir, b"a");
    let object_b = write_object(&cas_dir, b"b");

    let sorted = deduplicate_sort_cas_objects(vec![object_a.clone(), object_b.clone()]).unwrap();
    let mut reversed = sorted.clone();
    reversed.reverse();
    let err = verify_cas_object_presence(&cas_dir, &reversed).unwrap_err();
    assert!(err.to_string().contains("deduplicated, sorted"));

    let err = verify_cas_object_presence(&cas_dir, &[object_a.clone(), object_a]).unwrap_err();
    assert!(err.to_string().contains("deduplicated, sorted"));
}

#[test]
fn presence_proof_rejects_missing_and_wrong_sized_objects() {
    let tmp = TempDir::new().unwrap();
    let cas_dir = tmp.path().join("objects");
    fs::create_dir_all(&cas_dir).unwrap();

    let mut wrong_size = write_object(&cas_dir, b"payload");
    wrong_size.size += 1;
    let err = verify_cas_object_presence(&cas_dir, &[wrong_size]).unwrap_err();
    assert!(err.to_string().contains("size mismatch"));

    let missing = CasObjectRef {
        sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        size: 1,
    };
    let err = verify_cas_object_presence(&cas_dir, &[missing]).unwrap_err();
    assert!(err.to_string().contains("missing CAS object"));
}

#[test]
fn artifact_writer_accepts_only_the_exact_bound_presence_proof() {
    let exact = WriterFixture::new(b"same-payload");
    exact.write_with_proof(exact.presence_proof()).unwrap();
    assert!(exact.generation_dir.join(CAS_MANIFEST_FILE).exists());
    assert!(exact.generation_dir.join(ARTIFACT_MANIFEST_FILE).exists());

    let source = WriterFixture::new(b"same-payload");
    let other_root_proof = source.presence_proof();
    let other_root = WriterFixture::new(b"same-payload");
    let err = other_root.write_with_proof(other_root_proof).unwrap_err();
    assert!(err.to_string().contains("bound to"));
    other_root.assert_no_artifact_authority();

    let changed_set = WriterFixture::new(b"first-payload");
    let changed_set_proof = changed_set.presence_proof();
    let replacement = write_object(&changed_set.cas_dir, b"replacement-payload");
    write_root_manifests_with_objects(
        &changed_set.generation_dir,
        std::slice::from_ref(&replacement),
    );
    let err = changed_set.write_with_proof(changed_set_proof).unwrap_err();
    assert!(err.to_string().contains("object set"));
    changed_set.assert_no_artifact_authority();
}
