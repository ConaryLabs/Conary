// crates/conary-core/src/generation/builder/verity/tests.rs

use super::*;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let image = tmp.path().join("root.erofs");
    std::fs::write(&image, b"erofs-image").unwrap();
    GenerationMetadata {
        generation: 7,
        format: crate::generation::metadata::GENERATION_FORMAT.to_string(),
        erofs_size: Some(4096),
        cas_objects_referenced: Some(3),
        fsverity_enabled: false,
        erofs_verity_digest: Some("ab".repeat(32)),
        artifact_manifest_sha256: None,
        security_capability_xattr_count: None,
        created_at: "2026-09-05T00:00:00Z".into(),
        package_count: 2,
        kernel_version: Some("6.16.1".into()),
        summary: "verity fixture".into(),
    }
    .write_to(tmp.path())
    .unwrap();
    (tmp, image)
}

#[test]
fn enablement_persists_flag_and_preserves_digest_only_after_success() {
    for newly_enabled in [true, false] {
        let (tmp, image) = fixture();
        enable_generation_rootfs_verity_with(tmp.path(), &image, |path| {
            assert_eq!(path, image);
            assert!(
                !GenerationMetadata::read_from(tmp.path())
                    .unwrap()
                    .fsverity_enabled
            );
            Ok(newly_enabled)
        })
        .unwrap();
        let metadata = GenerationMetadata::read_from(tmp.path()).unwrap();
        assert!(metadata.fsverity_enabled);
        assert_eq!(metadata.erofs_verity_digest, Some("ab".repeat(32)));
    }
}

#[test]
fn failed_enablement_does_not_advertise_verified_metadata() {
    let (tmp, image) = fixture();
    let error = enable_generation_rootfs_verity_with(tmp.path(), &image, |_| {
        Err(FsVerityError::NotSupported(image.clone()))
    })
    .unwrap_err();
    assert!(error.to_string().contains("fs-verity support"));
    assert!(
        !GenerationMetadata::read_from(tmp.path())
            .unwrap()
            .fsverity_enabled
    );
}

#[test]
fn missing_digest_fails_before_enablement() {
    let (tmp, image) = fixture();
    let mut metadata = GenerationMetadata::read_from(tmp.path()).unwrap();
    metadata.erofs_verity_digest = None;
    metadata.write_to(tmp.path()).unwrap();
    let error = enable_generation_rootfs_verity_with(tmp.path(), &image, |_| {
        panic!("missing digest must not reach the ioctl")
    })
    .unwrap_err();
    assert!(matches!(error, Error::BootVerity(_)));
    assert!(
        !GenerationMetadata::read_from(tmp.path())
            .unwrap()
            .fsverity_enabled
    );
}
