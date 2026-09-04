// conary-core/src/generation/metadata/tests.rs

use super::*;
use crate::ccs::signing::SigningKeyPair;
use tempfile::TempDir;

#[test]
fn test_metadata_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let metadata = GenerationMetadata {
        generation: 42,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(1_048_576),
        cas_objects_referenced: Some(320),
        fsverity_enabled: true,
        erofs_verity_digest: Some("abc123def456".to_string()),
        artifact_manifest_sha256: Some(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        ),
        security_capability_xattr_count: Some(2),
        created_at: "2026-03-04T12:00:00Z".to_string(),
        package_count: 150,
        kernel_version: Some("6.12.1-arch1-1".to_string()),
        summary: "installed vim".to_string(),
    };

    metadata.write_to_with_key_paths(tmp.path(), None).unwrap();
    let loaded = GenerationMetadata::read_from_with_key_paths(tmp.path(), None, None).unwrap();

    assert_eq!(loaded.generation, 42);
    assert_eq!(loaded.format, GENERATION_FORMAT);
    assert_eq!(loaded.erofs_size, Some(1_048_576));
    assert_eq!(loaded.cas_objects_referenced, Some(320));
    assert!(loaded.fsverity_enabled);
    assert_eq!(loaded.erofs_verity_digest.as_deref(), Some("abc123def456"));
    assert_eq!(
        loaded.artifact_manifest_sha256.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(loaded.security_capability_xattr_count, Some(2));
    assert_eq!(loaded.created_at, "2026-03-04T12:00:00Z");
    assert_eq!(loaded.package_count, 150);
    assert_eq!(loaded.kernel_version.as_deref(), Some("6.12.1-arch1-1"));
    assert_eq!(loaded.summary, "installed vim");
}

#[test]
fn test_metadata_roundtrip_no_verity_digest() {
    let tmp = TempDir::new().unwrap();
    let metadata = GenerationMetadata {
        generation: 7,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(512_000),
        cas_objects_referenced: Some(100),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: None,
        created_at: "2026-03-17T10:00:00Z".to_string(),
        package_count: 80,
        kernel_version: None,
        summary: "baseline".to_string(),
    };

    metadata.write_to_with_key_paths(tmp.path(), None).unwrap();

    // Verify the JSON does not contain erofs_verity_digest when None
    let json = std::fs::read_to_string(tmp.path().join(GENERATION_METADATA_FILE)).unwrap();
    assert!(
        !json.contains("erofs_verity_digest"),
        "erofs_verity_digest=None should be skipped in serialization"
    );

    let loaded = GenerationMetadata::read_from_with_key_paths(tmp.path(), None, None).unwrap();
    assert_eq!(loaded.erofs_verity_digest, None);
}

#[test]
fn generation_metadata_write_leaves_no_tmp_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let metadata = GenerationMetadata {
        generation: 1,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(1),
        cas_objects_referenced: Some(0),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: None,
        created_at: "2026-05-26T00:00:00Z".to_string(),
        package_count: 0,
        kernel_version: None,
        summary: "fixture".to_string(),
    };

    metadata.write_to(temp.path()).unwrap();

    assert!(!temp.path().join(".conary-gen.json.tmp").exists());
}

#[test]
fn generation_metadata_rejects_abandoned_shape() {
    let tmp = TempDir::new().unwrap();
    let old_json = r#"{
            "generation": 10,
            "created_at": "2026-01-01T00:00:00Z",
            "package_count": 50,
            "kernel_version": null,
            "summary": "old generation"
        }"#;
    std::fs::write(tmp.path().join(GENERATION_METADATA_FILE), old_json).unwrap();

    let error = GenerationMetadata::read_from_with_key_paths(tmp.path(), None, None).unwrap_err();
    assert!(format!("{error:#}").contains("missing field `format`"));
}

#[test]
fn generation_metadata_rejects_abandoned_reflink_format() {
    let tmp = TempDir::new().unwrap();
    let metadata = GenerationMetadata {
        generation: 10,
        format: "reflink".to_string(),
        erofs_size: None,
        cas_objects_referenced: None,
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        package_count: 50,
        kernel_version: None,
        summary: "abandoned generation".to_string(),
    };

    let error = metadata
        .write_to_with_key_paths(tmp.path(), None)
        .unwrap_err();
    assert!(format!("{error:#}").contains("unsupported format"));
}

#[test]
fn test_pending_marker_roundtrip() {
    let tmp = TempDir::new().unwrap();

    assert!(!is_generation_pending(tmp.path()));

    mark_generation_pending(tmp.path()).unwrap();
    assert!(is_generation_pending(tmp.path()));
    assert!(generation_pending_marker_path(tmp.path()).exists());

    clear_generation_pending(tmp.path()).unwrap();
    assert!(!is_generation_pending(tmp.path()));
}

#[test]
fn test_read_from_rejects_pending_generation() {
    let tmp = TempDir::new().unwrap();
    mark_generation_pending(tmp.path()).unwrap();

    let err = GenerationMetadata::read_from(tmp.path()).unwrap_err();
    assert!(err.to_string().contains("still pending"));
}

fn generate_test_metadata_keys(dir: &TempDir) -> (PathBuf, PathBuf) {
    let private_path = dir.path().join("generation-metadata.private");
    let public_path = dir.path().join("generation-metadata.public");
    let keypair = SigningKeyPair::generate().with_key_id("test-generation-key");
    keypair.save_to_files(&private_path, &public_path).unwrap();
    (private_path, public_path)
}

#[test]
fn test_metadata_signature_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let key_dir = TempDir::new().unwrap();
    let (private_key, public_key) = generate_test_metadata_keys(&key_dir);
    let metadata = GenerationMetadata {
        generation: 12,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(2048),
        cas_objects_referenced: Some(9),
        fsverity_enabled: true,
        erofs_verity_digest: Some("abcd".to_string()),
        artifact_manifest_sha256: None,
        security_capability_xattr_count: Some(1),
        created_at: "2026-03-27T12:00:00Z".to_string(),
        package_count: 3,
        kernel_version: Some("6.13.0".to_string()),
        summary: "signed".to_string(),
    };

    metadata
        .write_to_with_key_paths(tmp.path(), Some(&private_key))
        .unwrap();
    let loaded =
        GenerationMetadata::read_from_with_key_paths(tmp.path(), Some(&public_key), None).unwrap();

    assert_eq!(loaded.summary, "signed");
    assert!(tmp.path().join(GENERATION_METADATA_SIGNATURE_FILE).exists());
}

#[test]
fn test_metadata_signature_rejects_tampering() {
    let tmp = TempDir::new().unwrap();
    let key_dir = TempDir::new().unwrap();
    let (private_key, public_key) = generate_test_metadata_keys(&key_dir);
    let metadata = GenerationMetadata {
        generation: 5,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(10),
        cas_objects_referenced: Some(2),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: Some(4),
        created_at: "2026-03-27T12:00:00Z".to_string(),
        package_count: 1,
        kernel_version: None,
        summary: "original".to_string(),
    };

    metadata
        .write_to_with_key_paths(tmp.path(), Some(&private_key))
        .unwrap();

    let tampered = GenerationMetadata {
        summary: "tampered".to_string(),
        ..metadata.clone()
    };
    std::fs::write(
        tmp.path().join(GENERATION_METADATA_FILE),
        serde_json::to_string_pretty(&tampered).unwrap(),
    )
    .unwrap();

    let err = GenerationMetadata::read_from_with_key_paths(tmp.path(), Some(&public_key), None)
        .unwrap_err();
    assert!(err.to_string().contains("signature verification failed"));
}

#[test]
fn test_metadata_requires_signature_when_verification_key_present() {
    let tmp = TempDir::new().unwrap();
    let key_dir = TempDir::new().unwrap();
    let (_private_key, public_key) = generate_test_metadata_keys(&key_dir);
    let metadata = GenerationMetadata {
        generation: 8,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(128),
        cas_objects_referenced: Some(4),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: None,
        created_at: "2026-03-27T12:00:00Z".to_string(),
        package_count: 2,
        kernel_version: None,
        summary: "unsigned".to_string(),
    };

    metadata.write_to_with_key_paths(tmp.path(), None).unwrap();

    let err = GenerationMetadata::read_from_with_key_paths(tmp.path(), Some(&public_key), None)
        .unwrap_err();
    assert!(err.to_string().contains("unsigned"));
}

#[test]
fn test_metadata_serialization_skips_capability_count_when_absent() {
    let tmp = TempDir::new().unwrap();
    let metadata = GenerationMetadata {
        generation: 99,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(4096),
        cas_objects_referenced: Some(3),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: None,
        created_at: "2026-07-08T00:00:00Z".to_string(),
        package_count: 7,
        kernel_version: None,
        summary: "no capability xattrs".to_string(),
    };

    metadata.write_to_with_key_paths(tmp.path(), None).unwrap();
    let json = std::fs::read_to_string(tmp.path().join(GENERATION_METADATA_FILE)).unwrap();
    assert!(!json.contains("security_capability_xattr_count"));
}

#[test]
fn test_excluded_paths() {
    // These should be excluded (updated EXCLUDED_DIRS includes full "var")
    assert!(is_excluded("home"));
    assert!(is_excluded("/home"));
    assert!(is_excluded("home/peter"));
    assert!(is_excluded("proc"));
    assert!(is_excluded("/proc/cpuinfo"));
    assert!(is_excluded("var"));
    assert!(is_excluded("var/lib"));
    assert!(is_excluded("/var/lib/dpkg"));
    assert!(is_excluded("var/cache"));
    assert!(is_excluded("/var/cache/apt"));
    assert!(is_excluded("sys"));
    assert!(is_excluded("dev"));
    assert!(is_excluded("root"));
    assert!(is_excluded("root/.bashrc"));
    assert!(is_excluded("srv"));
    assert!(is_excluded("srv/http"));
    assert!(is_excluded("opt"));
    assert!(is_excluded("opt/cuda"));
    assert!(is_excluded("tmp"));
    assert!(is_excluded("run"));
    assert!(is_excluded("mnt"));
    assert!(is_excluded("media"));

    // These should NOT be excluded
    assert!(!is_excluded("usr"));
    assert!(!is_excluded("etc"));
    assert!(!is_excluded("/usr/bin"));
    assert!(!is_excluded("boot"));
}

#[test]
fn test_generation_paths() {
    assert_eq!(generations_dir(), PathBuf::from("/conary/generations"));
    assert_eq!(generation_path(1), PathBuf::from("/conary/generations/1"));
    assert_eq!(generation_path(42), PathBuf::from("/conary/generations/42"));
    assert_eq!(current_link(), PathBuf::from("/conary/current"));
    assert_eq!(gc_roots_dir(), PathBuf::from("/conary/gc-roots"));
}
