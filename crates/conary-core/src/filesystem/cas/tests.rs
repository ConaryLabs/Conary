// conary-core/src/filesystem/cas/tests.rs

use super::*;
use tempfile::TempDir;

#[test]
fn test_compute_hash() {
    let content = b"Hello, World!";
    let hash = CasStore::compute_sha256(content);
    assert_eq!(
        hash,
        "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
    );
}

#[test]
fn test_compute_symlink_hash() {
    let target = "/usr/lib/libfoo.so.1";
    let hash = CasStore::compute_symlink_hash(target);

    // Should be SHA-256 of raw target bytes (matching CcsBuilder convention)
    let expected = CasStore::compute_sha256(b"/usr/lib/libfoo.so.1");
    assert_eq!(hash, expected);

    // Hash should be 64 chars (256 bits hex)
    assert_eq!(hash.len(), 64);
}

#[test]
fn test_symlink_hash_consistency() {
    // Verify that compute_symlink_hash and store_symlink produce the same hash
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    let target = "/usr/bin/python3";
    let computed_hash = CasStore::compute_symlink_hash(target);
    let stored_hash = cas.store_symlink(target).unwrap();

    assert_eq!(
        computed_hash, stored_hash,
        "compute_symlink_hash and store_symlink must produce identical hashes"
    );
}

#[test]
fn test_symlink_hash_consistency_with_xxh128() {
    // Verify symlink hashes are consistent even when CAS uses Xxh128
    // This tests that store_symlink always uses SHA-256 for symlinks,
    // regardless of the CAS's configured algorithm.
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::with_algorithm(temp_dir.path(), HashAlgorithm::Xxh128).unwrap();

    // CAS is configured for Xxh128
    assert_eq!(cas.algorithm(), HashAlgorithm::Xxh128);

    let target = "/usr/lib64/libssl.so.3";
    let computed_hash = CasStore::compute_symlink_hash(target);
    let stored_hash = cas.store_symlink(target).unwrap();

    // Symlink hashes must match compute_symlink_hash (always SHA-256)
    assert_eq!(
        computed_hash, stored_hash,
        "Symlink hash must use SHA-256 even when CAS uses Xxh128"
    );

    // Hash should be 64 chars (SHA-256), not 32 (XXH128)
    assert_eq!(
        stored_hash.len(),
        64,
        "Symlink hash must be SHA-256 (64 chars)"
    );

    // Verify the symlink can be retrieved
    let retrieved = cas.retrieve_symlink(&stored_hash).unwrap();
    assert_eq!(retrieved, target);
}

#[test]
fn test_compute_hash_with_algorithm() {
    let content = b"Hello, World!";

    // SHA-256
    let sha_hash = CasStore::compute_hash_with(HashAlgorithm::Sha256, content);
    assert_eq!(sha_hash.len(), 64);

    // XXH128
    let xxh_hash = CasStore::compute_hash_with(HashAlgorithm::Xxh128, content);
    assert_eq!(xxh_hash.len(), 32);
}

#[test]
fn test_store_and_retrieve() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    let content = b"Test content for CAS";
    let hash = cas.store(content).unwrap();

    // Verify stored content
    let retrieved = cas.retrieve(&hash).unwrap();
    assert_eq!(content, retrieved.as_slice());
}

#[test]
fn test_store_and_retrieve_xxh128() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::with_algorithm(temp_dir.path(), HashAlgorithm::Xxh128).unwrap();

    assert_eq!(cas.algorithm(), HashAlgorithm::Xxh128);

    let content = b"Test content for fast CAS";
    let hash = cas.store(content).unwrap();

    // XXH128 produces 32-char hex (128 bits)
    assert_eq!(hash.len(), 32);

    // Verify stored content
    let retrieved = cas.retrieve(&hash).unwrap();
    assert_eq!(content, retrieved.as_slice());
}

#[test]
fn test_deduplication() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    let content = b"Duplicate content";
    let hash1 = cas.store(content).unwrap();
    let hash2 = cas.store(content).unwrap();

    // Same content should give same hash
    assert_eq!(hash1, hash2);

    // Should exist in CAS
    assert!(cas.exists(&hash1));
}

#[test]
fn test_hash_to_path() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    let hash = "abc123def456";
    let path = cas.hash_to_path(hash).unwrap();

    let expected = temp_dir.path().join("ab").join("c123def456");
    assert_eq!(path, expected);
}

#[test]
fn test_retrieve_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    // Use a valid-format hex hash that simply doesn't exist in the store
    let result = cas.retrieve("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert!(result.is_err());
}

#[test]
fn test_hardlink_from_immutable_root() {
    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();

    // Create a file to "adopt"
    let existing_file = temp_dir.path().join("existing_file.txt");
    let content = b"Content to be hardlinked into CAS";
    fs::write(&existing_file, content).unwrap();

    // Hardlink into CAS
    let hash = cas.hardlink_from_immutable_root(&existing_file).unwrap();

    // Verify content is in CAS
    assert!(cas.exists(&hash));
    let retrieved = cas.retrieve(&hash).unwrap();
    assert_eq!(content, retrieved.as_slice());
}

#[test]
#[cfg(unix)]
fn test_hardlink_from_immutable_root_preserves_adopted_inode() {
    use std::os::unix::fs::MetadataExt;

    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();

    // Create a file to "adopt"
    let existing_file = temp_dir.path().join("shared_inode.txt");
    let content = b"This file will share an inode with CAS";
    fs::write(&existing_file, content).unwrap();

    // Get original inode
    let original_inode = fs::metadata(&existing_file).unwrap().ino();

    // Hardlink into CAS
    let hash = cas.hardlink_from_immutable_root(&existing_file).unwrap();

    // Get CAS file inode
    let cas_path = cas.hash_to_path(&hash).unwrap();
    let cas_inode = fs::metadata(&cas_path).unwrap().ino();

    // Should be the same inode (hardlink)
    assert_eq!(
        original_inode, cas_inode,
        "Hardlinked file should share inode with original"
    );

    // nlink should be 2
    let nlink = fs::metadata(&existing_file).unwrap().nlink();
    assert_eq!(nlink, 2, "Hardlinked file should have nlink=2");
}

#[test]
#[cfg(unix)]
fn test_hardlink_survives_original_deletion() {
    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();

    // Create a file to "adopt"
    let existing_file = temp_dir.path().join("will_be_deleted.txt");
    let content = b"This file will be deleted but CAS keeps it";
    fs::write(&existing_file, content).unwrap();

    // Hardlink into CAS
    let hash = cas.hardlink_from_immutable_root(&existing_file).unwrap();

    // Delete the original file (simulating RPM removal)
    fs::remove_file(&existing_file).unwrap();
    assert!(!existing_file.exists());

    // CAS should still have the content
    assert!(cas.exists(&hash));
    let retrieved = cas.retrieve(&hash).unwrap();
    assert_eq!(content, retrieved.as_slice());
}

#[test]
fn test_hardlink_with_known_hash() {
    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();

    // Create a file
    let existing_file = temp_dir.path().join("known_hash.txt");
    let content = b"Content with pre-computed hash";
    fs::write(&existing_file, content).unwrap();

    // Pre-compute hash
    let expected_hash = CasStore::compute_sha256(content);

    // Hardlink with known hash (no verification)
    let hash = cas
        .hardlink_from_immutable_root_with_hash(&existing_file, &expected_hash, false)
        .unwrap();

    assert_eq!(hash, expected_hash);
    assert!(cas.exists(&hash));
}

#[test]
fn test_hardlink_deduplication() {
    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();

    // Create two files with same content
    let file1 = temp_dir.path().join("file1.txt");
    let file2 = temp_dir.path().join("file2.txt");
    let content = b"Identical content in two files";
    fs::write(&file1, content).unwrap();
    fs::write(&file2, content).unwrap();

    // Hardlink first file
    let hash1 = cas.hardlink_from_immutable_root(&file1).unwrap();

    // Hardlink second file - should detect duplicate
    let hash2 = cas.hardlink_from_immutable_root(&file2).unwrap();

    // Same hash
    assert_eq!(hash1, hash2);

    // Content retrievable
    let retrieved = cas.retrieve(&hash1).unwrap();
    assert_eq!(content, retrieved.as_slice());
}

#[test]
#[cfg(unix)]
fn test_hardlink_from_immutable_root_shares_inode() {
    use std::os::unix::fs::MetadataExt;

    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();
    let existing_file = temp_dir.path().join("sealed_inode.txt");
    let content = b"This sealed-source helper intentionally shares an inode";
    fs::write(&existing_file, content).unwrap();
    let original_inode = fs::metadata(&existing_file).unwrap().ino();

    let hash = cas.hardlink_from_immutable_root(&existing_file).unwrap();
    let cas_path = cas.hash_to_path(&hash).unwrap();

    assert_eq!(original_inode, fs::metadata(&cas_path).unwrap().ino());
}

#[test]
#[cfg(unix)]
fn test_store_file_copy_repairs_existing_shared_cas_object() {
    use std::os::unix::fs::MetadataExt;

    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();
    let sealed_file = temp_dir.path().join("sealed.txt");
    let mutable_file = temp_dir.path().join("mutable.txt");
    let content = b"same content through two capture paths";
    fs::write(&sealed_file, content).unwrap();
    fs::write(&mutable_file, content).unwrap();

    let hash = cas.hardlink_from_immutable_root(&sealed_file).unwrap();
    let shared_path = cas.hash_to_path(&hash).unwrap();
    assert_eq!(
        fs::metadata(&sealed_file).unwrap().ino(),
        fs::metadata(&shared_path).unwrap().ino()
    );

    let copied_hash = cas.store_file_copy_from_existing(&mutable_file).unwrap();
    assert_eq!(copied_hash, hash);
    assert_ne!(
        fs::metadata(&sealed_file).unwrap().ino(),
        fs::metadata(&shared_path).unwrap().ino(),
        "mutable-source copy must break a touched shared CAS object"
    );
    assert_eq!(cas.retrieve(&hash).unwrap(), content);
}

#[test]
fn test_atomic_store_unique_temp_names() {
    // Verify that successive stores use different temp name counters
    // by checking that concurrent stores to the same hash do not corrupt data
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    let content1 = b"Content A for uniqueness test";
    let content2 = b"Content B for uniqueness test";

    let hash1 = cas.store(content1).unwrap();
    let hash2 = cas.store(content2).unwrap();

    // Different content should produce different hashes
    assert_ne!(hash1, hash2);

    // Both should be retrievable without corruption
    let retrieved1 = cas.retrieve(&hash1).unwrap();
    let retrieved2 = cas.retrieve(&hash2).unwrap();
    assert_eq!(content1, retrieved1.as_slice());
    assert_eq!(content2, retrieved2.as_slice());
}

#[test]
fn test_cleanup_orphaned_temps() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    // Create a fake orphaned temp file inside a CAS subdirectory
    let sub_dir = temp_dir.path().join("ab");
    fs::create_dir_all(&sub_dir).unwrap();
    let orphan = sub_dir.join("c123def456.tmp.99999.0");
    fs::write(&orphan, "orphaned data").unwrap();

    // With a very large max_age, nothing should be removed (file is too new)
    let removed = cas
        .cleanup_orphaned_temps(std::time::Duration::from_secs(999_999))
        .unwrap();
    assert_eq!(removed, 0);
    assert!(orphan.exists());

    // With zero max_age, the file should be removed (it is older than 0 seconds)
    let removed = cas
        .cleanup_orphaned_temps(std::time::Duration::from_secs(0))
        .unwrap();
    assert_eq!(removed, 1);
    assert!(!orphan.exists());
}

#[test]
fn test_cleanup_ignores_non_temp_files() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    // Store real content so there is a real CAS file
    let content = b"Real CAS content that should survive cleanup";
    let hash = cas.store(content).unwrap();

    // Cleanup with zero threshold should not touch real CAS files
    let removed = cas
        .cleanup_orphaned_temps(std::time::Duration::from_secs(0))
        .unwrap();
    assert_eq!(removed, 0);

    // Real content should still be retrievable
    let retrieved = cas.retrieve(&hash).unwrap();
    assert_eq!(content, retrieved.as_slice());
}

#[test]
fn test_iter_objects_basic() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    // Store two distinct objects
    let hash1 = cas.store(b"alpha").unwrap();
    let hash2 = cas.store(b"bravo").unwrap();

    // Also create temp files that should be skipped
    let prefix_dir = temp_dir.path().join(&hash1[..2]);
    fs::write(prefix_dir.join(".tmp_in_progress"), b"temp").unwrap();
    fs::write(prefix_dir.join("something.tmp"), b"temp2").unwrap();
    // Temp file matching atomic_store() naming: {hash}.tmp.{pid}.{counter}
    fs::write(prefix_dir.join("abcdef1234.tmp.12345.0"), b"temp3").unwrap();

    let mut results: Vec<(String, PathBuf)> =
        cas.iter_objects().collect::<Result<Vec<_>>>().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let hashes: Vec<&str> = results.iter().map(|(h, _)| h.as_str()).collect();
    assert!(hashes.contains(&hash1.as_str()));
    assert!(hashes.contains(&hash2.as_str()));
    assert_eq!(
        hashes.len(),
        2,
        "Temp files should be excluded, got: {:?}",
        hashes
    );
}

#[test]
fn test_iter_objects_empty() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();

    let results: Vec<_> = cas.iter_objects().collect::<Result<Vec<_>>>().unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_object_path_valid_hex() {
    let root = std::path::Path::new("/cas");
    // 64-char SHA-256 hex hash
    let hash = "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f";
    let path = object_path(root, hash).unwrap();
    assert_eq!(
        path,
        std::path::PathBuf::from(
            "/cas/df/fd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        )
    );
}

#[test]
fn test_object_path_rejects_path_traversal() {
    let root = std::path::Path::new("/cas");
    let bad_hash = "../../../etc/passwd";
    let err = object_path(root, bad_hash).unwrap_err();
    assert!(matches!(err, crate::Error::InvalidPath(_)));
}

#[test]
fn test_object_path_rejects_non_hex() {
    let root = std::path::Path::new("/cas");
    let bad_hash = "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg";
    let err = object_path(root, bad_hash).unwrap_err();
    assert!(matches!(err, crate::Error::InvalidPath(_)));
}

#[test]
fn test_object_path_rejects_too_short() {
    let root = std::path::Path::new("/cas");
    let err = object_path(root, "abc").unwrap_err();
    assert!(matches!(err, crate::Error::InvalidPath(_)));
}

#[test]
fn test_hash_to_path_rejects_non_hex_hashes() {
    let temp_dir = TempDir::new().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();
    let err = cas.hash_to_path("zzzz").unwrap_err();
    assert!(matches!(err, crate::Error::InvalidPath(_)));
}
