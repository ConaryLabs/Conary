// crates/conary-core/src/self_update/tests.rs

use super::*;
use gzp::ZWriter;
use std::io::{Read, Write};

fn create_test_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    conn
}

fn append_test_tar_entry<W: Write>(builder: &mut tar::Builder<W>, path: &str, content: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, path, content).unwrap();
}

fn current_self_update_build(
    package_name: &str,
    content: &[u8],
) -> crate::ccs::builder::BuildResult {
    crate::ccs::builder::test_support::single_file_build_result_at(
        package_name,
        "1.0.0",
        "/usr/bin/conary",
        content,
    )
}

fn write_current_self_update_ccs(
    path: &Path,
    build: &crate::ccs::builder::BuildResult,
) -> crate::ccs::signing::SigningKeyPair {
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("self-update-test");
    crate::ccs::builder::write_signed_current_ccs_package(build, path, &key, false).unwrap();
    key
}

fn chunked_self_update_build(
    package_name: &str,
    content: &[u8],
) -> crate::ccs::builder::BuildResult {
    let mut build = current_self_update_build(package_name, content);
    let chunk_references = crate::ccs::chunking::Chunker::new()
        .chunk_bytes(content)
        .iter()
        .map(crate::ccs::chunking::Chunk::reference)
        .collect::<Vec<_>>();
    build.files[0].chunks = Some(chunk_references.clone());
    build.components.get_mut("runtime").unwrap().files[0].chunks = Some(chunk_references);
    build.chunked = true;
    build
}

fn replace_current_object_bytes(path: &Path, hash: &str, replacement: &[u8]) {
    let target = format!("objects/{}/{}", &hash[..2], &hash[2..]);
    let file = std::fs::File::open(path).unwrap();
    let decoder = crate::ccs::archive_framing::MgzipDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut replaced = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let entry_path = entry.path().unwrap().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if entry_path == Path::new(&target) {
            bytes = replacement.to_vec();
            replaced = true;
        }
        entries.push((entry_path, bytes));
    }
    drop(archive);
    assert!(replaced, "current CCS fixture object {hash} was not found");

    let file = std::fs::File::create(path).unwrap();
    let encoder = gzp::par::compress::ParCompressBuilder::<gzp::deflate::Mgzip>::new()
        .buffer_size(crate::ccs::CCS_BUDGET.archive_compression_block_bytes)
        .unwrap()
        .num_threads(1)
        .unwrap()
        .compression_level(flate2::Compression::default())
        .from_writer(file);
    let mut builder = tar::Builder::new(encoder);
    for (entry_path, bytes) in entries {
        append_test_tar_entry(&mut builder, entry_path.to_str().unwrap(), &bytes);
    }
    builder.finish().unwrap();
    builder.into_inner().unwrap().finish().unwrap();
}

fn verify_test_self_update_ccs(
    path: &Path,
    key: &crate::ccs::signing::SigningKeyPair,
) -> crate::ccs::verify::VerifiedCcsArchive {
    crate::ccs::verify::verify_package(
        path,
        &crate::ccs::verify::TrustPolicy::strict(vec![key.public_key_base64()]),
    )
    .unwrap()
}

fn assert_error_chain_contains(error: &anyhow::Error, expected: &str) {
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string().contains(expected)),
        "expected {expected:?} in error chain: {error:#}"
    );
}

#[test]
fn test_get_update_channel_default() {
    let conn = create_test_db();
    let channel = get_update_channel(&conn).unwrap();
    assert_eq!(channel, DEFAULT_UPDATE_CHANNEL);
}

#[test]
fn test_set_update_channel() {
    let conn = create_test_db();
    set_update_channel(&conn, "https://internal.example.com/conary").unwrap();
    let channel = get_update_channel(&conn).unwrap();
    assert_eq!(channel, "https://internal.example.com/conary");
}

#[test]
fn test_apply_update_atomic_rename() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("conary-new");
    let target = dir.path().join("conary");

    // Create source binary
    fs::write(&source, b"new-binary-content").unwrap();
    // Create existing target
    fs::write(&target, b"old-binary-content").unwrap();

    let objects_dir = dir.path().join("objects");
    fs::create_dir_all(&objects_dir).unwrap();

    apply_update(&source, &target, objects_dir.to_str().unwrap()).unwrap();

    // Source should be gone (renamed)
    assert!(!source.exists());
    // Target should have new content
    assert_eq!(fs::read(&target).unwrap(), b"new-binary-content");
}

#[test]
fn test_verify_binary_nonexistent() {
    let result = verify_binary(Path::new("/nonexistent/binary"), "1.0.0");
    assert!(result.is_err());
}

#[test]
fn test_update_channel_persistence() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    // Default channel
    let default = get_update_channel(&conn).unwrap();
    assert_eq!(default, DEFAULT_UPDATE_CHANNEL);

    // Set custom
    let custom = "https://mirror.internal/v1/ccs/conary";
    set_update_channel(&conn, custom).unwrap();
    assert_eq!(get_update_channel(&conn).unwrap(), custom);

    // Override again
    let custom2 = "https://other.mirror/v1/ccs/conary";
    set_update_channel(&conn, custom2).unwrap();
    assert_eq!(get_update_channel(&conn).unwrap(), custom2);
}

#[test]
fn extract_binary_rejects_archive_without_current_manifest_authority() {
    let dir = tempfile::tempdir().unwrap();
    let ccs_path = dir.path().join("legacy-direct-binary.ccs");

    let file = std::fs::File::create(&ccs_path).unwrap();
    let encoder = gzp::par::compress::ParCompressBuilder::<gzp::deflate::Mgzip>::new()
        .buffer_size(crate::ccs::CCS_BUDGET.archive_compression_block_bytes)
        .unwrap()
        .num_threads(1)
        .unwrap()
        .compression_level(flate2::Compression::default())
        .from_writer(file);
    let mut builder = tar::Builder::new(encoder);
    builder.finish().unwrap();
    builder.into_inner().unwrap().finish().unwrap();

    let key = crate::ccs::signing::SigningKeyPair::generate();
    let error = crate::ccs::verify::verify_package(
        &ccs_path,
        &crate::ccs::verify::TrustPolicy::strict(vec![key.public_key_base64()]),
    )
    .unwrap_err();
    assert_error_chain_contains(&error, "missing required current v3 MANIFEST authority");
}

#[test]
fn extract_binary_reads_current_unchunked_payload_authority() {
    let dir = tempfile::tempdir().unwrap();
    let ccs_path = dir.path().join("current.ccs");
    let content = b"#!/bin/sh\necho current\n";
    let build = current_self_update_build("conary", content);
    let key = write_current_self_update_ccs(&ccs_path, &build);

    let verified = verify_test_self_update_ccs(&ccs_path, &key);
    let binary = extract_binary(&verified, dir.path()).unwrap();
    assert_eq!(std::fs::read(binary).unwrap(), content);
}

#[test]
fn extract_binary_reads_current_authority_from_chunked_builder_state() {
    let dir = tempfile::tempdir().unwrap();
    let ccs_path = dir.path().join("chunked.ccs");
    let mut content = b"#!/bin/sh\necho chunks\n".to_vec();
    content.resize(crate::ccs::chunking::MIN_CHUNK_SIZE as usize + 4096, b'#');
    let build = chunked_self_update_build("conary", &content);
    let key = write_current_self_update_ccs(&ccs_path, &build);

    let verified = verify_test_self_update_ccs(&ccs_path, &key);
    let content_hash = crate::hash::sha256(&content);
    let crate::ccs::v3::schema::PackageKindV3::Package(package) = &verified.authority().kind else {
        panic!("self-update fixture must carry package authority");
    };
    assert_eq!(
        package.files[0].content.as_ref().unwrap().sha256,
        content_hash
    );
    assert_eq!(
        verified
            .payload()
            .files()
            .iter()
            .filter_map(|file| file.content_authority.as_ref())
            .map(|authority| authority.sha256.as_str())
            .collect::<Vec<_>>(),
        vec![content_hash]
    );
    assert!(
        verified.components()["runtime"].files[0]
            .chunks
            .as_ref()
            .is_some_and(|chunks| !chunks.is_empty())
    );
    let binary = extract_binary(&verified, dir.path()).unwrap();
    assert_eq!(std::fs::read(binary).unwrap(), content);
}

#[test]
fn extract_binary_rejects_wrong_package_identity() {
    let dir = tempfile::tempdir().unwrap();
    let ccs_path = dir.path().join("wrong-package.ccs");
    let content = b"#!/bin/sh\n";
    let build = current_self_update_build("not-conary", content);
    let key = write_current_self_update_ccs(&ccs_path, &build);

    let verified = verify_test_self_update_ccs(&ccs_path, &key);
    let error = extract_binary(&verified, dir.path()).unwrap_err();
    assert!(error.to_string().contains("expected \"conary\""));
}

#[test]
fn checked_binary_size_rejects_overflow_without_allocating() {
    let error = checked_binary_size(u64::MAX).unwrap_err();
    assert!(error.to_string().contains("Binary entry too large"));
}

#[test]
fn extract_binary_rejects_current_object_path_hash_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let ccs_path = dir.path().join("bad-object-hash.ccs");
    let expected_content = b"#!/bin/sh\n";
    let tampered_content = b"#!/bin/zz\n";
    let expected_hash = crate::hash::sha256(expected_content);
    let build = current_self_update_build("conary", expected_content);
    let key = write_current_self_update_ccs(&ccs_path, &build);
    replace_current_object_bytes(&ccs_path, &expected_hash, tampered_content);

    let error = crate::ccs::verify::verify_package(
        &ccs_path,
        &crate::ccs::verify::TrustPolicy::strict(vec![key.public_key_base64()]),
    )
    .unwrap_err();
    assert_error_chain_contains(&error, "CCS object path hash mismatch");
}

#[test]
fn test_apply_update_source_missing() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("nonexistent");
    let target = dir.path().join("conary");
    std::fs::write(&target, b"old").unwrap();

    let result = apply_update(
        &source,
        &target,
        dir.path().join("objects").to_str().unwrap(),
    );
    assert!(result.is_err());
    // Original target should be unchanged
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
}

#[test]
fn test_apply_update_cas_failure_preserves_running_binary() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("conary-new");
    let target = dir.path().join("conary");
    let objects_path = dir.path().join("objects");
    std::fs::write(&source, b"new").unwrap();
    std::fs::write(&target, b"old").unwrap();
    std::fs::write(&objects_path, b"not a directory").unwrap();

    let result = apply_update(&source, &target, objects_path.to_str().unwrap());

    assert!(result.is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
    assert_eq!(std::fs::read(&source).unwrap(), b"new");
}

/// Helper: create a deterministic Ed25519 keypair from a fixed seed for tests.
fn test_keypair() -> (ed25519_dalek::SigningKey, ed25519_dalek::VerifyingKey) {
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
    let verifying_key = signing_key.verifying_key();
    (signing_key, verifying_key)
}

#[test]
fn test_verify_update_signature_valid() {
    use ed25519_dalek::Signer;

    let (signing_key, verifying_key) = test_keypair();
    let sha256_hex = "abc123def456";
    let signature = signing_key.sign(sha256_hex.as_bytes());
    let sig_b64 = BASE64.encode(signature.to_bytes());
    let key_hex = hex::encode(verifying_key.as_bytes());

    let result = verify_update_signature_with_keys(sha256_hex, &sig_b64, &[key_hex.as_str()]);
    assert!(result.is_ok());
}

#[test]
fn test_verify_update_signature_tampered_hash() {
    use ed25519_dalek::Signer;

    let (signing_key, verifying_key) = test_keypair();
    let sha256_hex = "abc123def456";
    let signature = signing_key.sign(sha256_hex.as_bytes());
    let sig_b64 = BASE64.encode(signature.to_bytes());
    let key_hex = hex::encode(verifying_key.as_bytes());

    // Verify against a different hash -> should fail
    let result = verify_update_signature_with_keys("tampered_hash", &sig_b64, &[key_hex.as_str()]);
    assert!(matches!(result, Err(UpdateSignatureError::Untrusted)));
}

#[test]
fn test_verify_update_signature_wrong_key() {
    use ed25519_dalek::Signer;

    let (signing_key, _) = test_keypair();
    let sha256_hex = "abc123def456";
    let signature = signing_key.sign(sha256_hex.as_bytes());
    let sig_b64 = BASE64.encode(signature.to_bytes());

    // Use a different key
    let wrong_key = ed25519_dalek::SigningKey::from_bytes(&[99u8; 32]);
    let wrong_key_hex = hex::encode(wrong_key.verifying_key().as_bytes());

    let result = verify_update_signature_with_keys(sha256_hex, &sig_b64, &[wrong_key_hex.as_str()]);
    assert!(matches!(result, Err(UpdateSignatureError::Untrusted)));
}

#[test]
fn test_verify_update_signature_malformed_base64() {
    let result =
        verify_update_signature_with_keys("somehash", "not-valid-base64!!!", &["aabbccdd"]);
    assert!(matches!(result, Err(UpdateSignatureError::Malformed(_))));
}

#[test]
fn test_verify_update_signature_empty_key_list() {
    use ed25519_dalek::Signer;

    let (signing_key, _) = test_keypair();
    let sha256_hex = "abc123def456";
    let signature = signing_key.sign(sha256_hex.as_bytes());
    let sig_b64 = BASE64.encode(signature.to_bytes());

    let result = verify_update_signature_with_keys(sha256_hex, &sig_b64, &[]);
    assert!(matches!(result, Err(UpdateSignatureError::Untrusted)));
}

#[test]
fn test_verify_update_signature_does_not_bypass_in_tests() {
    use ed25519_dalek::Signer;

    let (signing_key, _) = test_keypair();
    let sha256_hex = "abc123def456";
    let signature = signing_key.sign(sha256_hex.as_bytes());
    let sig_b64 = BASE64.encode(signature.to_bytes());

    let result = verify_update_signature(sha256_hex, &sig_b64);
    assert!(matches!(result, Err(UpdateSignatureError::Untrusted)));
}
