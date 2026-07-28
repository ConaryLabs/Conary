// conary-core/src/self_update.rs

//! Self-update logic for the conary binary
//!
//! Checks Remi for newer versions and handles downloading, verifying,
//! and atomically replacing the running binary.

mod download;
mod versioning;

use crate::db::models::settings;
use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::warn;

pub use download::{download_update, download_update_with_progress};
pub use versioning::{
    LatestVersionInfo, VersionCheckResult, check_for_update, fetch_latest_version_info,
    fetch_version_info, is_newer, validate_download_origin,
};

/// Trusted Ed25519 public keys for verifying self-update signatures (hex-encoded).
pub const TRUSTED_UPDATE_KEYS: &[&str] =
    &["08eaacd1fa08389d38dc3f00d20be9df306da5367e65ad3be1f36e9d801e8003"];

/// Build the CCS package trust policy from the same pinned release keys used
/// for the detached update signature.
pub fn trusted_update_ccs_policy() -> Result<crate::ccs::verify::TrustPolicy> {
    if TRUSTED_UPDATE_KEYS.is_empty() {
        return Err(Error::ParseError(
            "No trusted self-update package keys are configured".to_string(),
        ));
    }
    let keys = TRUSTED_UPDATE_KEYS
        .iter()
        .map(|key| {
            let bytes = hex::decode(key).map_err(|error| {
                Error::ParseError(format!("Invalid trusted self-update key: {error}"))
            })?;
            if bytes.len() != 32 {
                return Err(Error::ParseError(format!(
                    "Invalid trusted self-update key length {}; expected 32",
                    bytes.len()
                )));
            }
            Ok(BASE64.encode(bytes))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(crate::ccs::verify::TrustPolicy::strict(keys))
}

/// Errors from self-update signature verification
#[derive(Debug, thiserror::Error)]
pub enum UpdateSignatureError {
    #[error("invalid signature: no trusted key verified the signature")]
    Untrusted,
    #[error("malformed signature data: {0}")]
    Malformed(String),
}

/// Verify an Ed25519 signature over the SHA-256 hash of a CCS package using the
/// provided list of trusted public keys.
///
/// Returns `Ok(())` if any trusted key successfully verifies the signature.
pub fn verify_update_signature_with_keys(
    sha256_hex: &str,
    signature_base64: &str,
    trusted_keys: &[&str],
) -> std::result::Result<(), UpdateSignatureError> {
    let sig_bytes = BASE64
        .decode(signature_base64)
        .map_err(|e| UpdateSignatureError::Malformed(format!("base64 decode: {e}")))?;

    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| UpdateSignatureError::Malformed(format!("signature bytes: {e}")))?;

    let message = sha256_hex.as_bytes();

    for key_hex in trusted_keys {
        let key_bytes = match hex::decode(key_hex) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let key_array: [u8; 32] = match key_bytes.try_into() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let verifying_key = match VerifyingKey::from_bytes(&key_array) {
            Ok(k) => k,
            Err(_) => continue,
        };
        if verifying_key.verify_strict(message, &signature).is_ok() {
            return Ok(());
        }
    }

    Err(UpdateSignatureError::Untrusted)
}

/// Verify an Ed25519 signature over the SHA-256 hash of a CCS update package.
///
/// Checks against [`TRUSTED_UPDATE_KEYS`].
pub fn verify_update_signature(
    sha256_hex: &str,
    signature_base64: &str,
) -> std::result::Result<(), UpdateSignatureError> {
    if TRUSTED_UPDATE_KEYS.is_empty() {
        warn!(
            "No trusted update keys configured (TRUSTED_UPDATE_KEYS is empty). \
             Signature verification cannot be performed."
        );
        return Err(UpdateSignatureError::Untrusted);
    }
    verify_update_signature_with_keys(sha256_hex, signature_base64, TRUSTED_UPDATE_KEYS)
}

/// Default update channel URL
pub const DEFAULT_UPDATE_CHANNEL: &str = "https://remi.conary.io/v1/ccs/conary";

/// Settings key for the update channel override
const SETTINGS_KEY_UPDATE_CHANNEL: &str = "update-channel";

/// A conary binary should never exceed 256 MB.
const MAX_SELF_UPDATE_BINARY_SIZE: u64 = 256 * 1024 * 1024;

/// Read the exact update channel override, or use the built-in channel.
pub fn get_update_channel(conn: &Connection) -> Result<String> {
    match settings::get(conn, SETTINGS_KEY_UPDATE_CHANNEL)? {
        Some(url) => Ok(url),
        None => Ok(DEFAULT_UPDATE_CHANNEL.to_string()),
    }
}

/// Persist an update channel after the command layer validates its URL policy.
pub fn set_update_channel(conn: &Connection, url: &str) -> Result<()> {
    settings::set(conn, SETTINGS_KEY_UPDATE_CHANNEL, url)
}

fn checked_binary_size(size: u64) -> Result<usize> {
    if size > MAX_SELF_UPDATE_BINARY_SIZE {
        return Err(Error::IoError(format!(
            "Binary entry too large ({size} bytes, max {MAX_SELF_UPDATE_BINARY_SIZE})"
        )));
    }

    usize::try_from(size).map_err(|_| {
        Error::IoError(format!(
            "Binary entry too large ({size} bytes, max {MAX_SELF_UPDATE_BINARY_SIZE})"
        ))
    })
}

fn canonical_conary_entry(
    verified: &crate::ccs::verify::VerifiedCcsArchive,
) -> Result<&crate::ccs::builder::FileEntry> {
    if verified.authority().identity.name != "conary" {
        return Err(Error::ParseError(format!(
            "Self-update package identity is {:?}, expected \"conary\"",
            verified.authority().identity.name
        )));
    }

    let mut conary_entry = None;
    for component in verified.components().values() {
        for file in &component.files {
            if file.path != "/usr/bin/conary" {
                continue;
            }
            if file.component != component.name {
                return Err(Error::ParseError(format!(
                    "Self-update payload component mismatch: file declares {:?}, component is {:?}",
                    file.component, component.name
                )));
            }
            if conary_entry.replace(file).is_some() {
                return Err(Error::ParseError(
                    "Self-update package contains multiple /usr/bin/conary entries".to_string(),
                ));
            }
        }
    }

    let file = conary_entry.ok_or_else(|| {
        Error::ParseError(
            "CCS package does not contain canonical /usr/bin/conary payload authority".to_string(),
        )
    })?;
    file.node
        .validate()
        .and_then(|()| file.node.validate_content(file.content.as_ref()))
        .map_err(|error| {
            Error::ParseError(format!(
                "Invalid /usr/bin/conary payload authority: {error}"
            ))
        })?;
    if !file.node.kind.is_regular() {
        return Err(Error::ParseError(
            "CCS package /usr/bin/conary entry is not a regular file".to_string(),
        ));
    }
    if file.node.mode & 0o111 == 0 {
        return Err(Error::ParseError(
            "CCS package /usr/bin/conary entry is not executable".to_string(),
        ));
    }

    let authority = verified.authority();
    let crate::ccs::v2::schema::PackageKindV2::Package(package) = &authority.kind else {
        return Err(Error::ParseError(
            "CCS v2 self-update authority is not a package".to_string(),
        ));
    };
    let mut signed_entries = package
        .files
        .iter()
        .filter(|candidate| candidate.path == "/usr/bin/conary");
    let signed = signed_entries.next().ok_or_else(|| {
        Error::ParseError("CCS v2 authority does not declare /usr/bin/conary".to_string())
    })?;
    if signed_entries.next().is_some() {
        return Err(Error::ParseError(
            "CCS v2 authority declares /usr/bin/conary more than once".to_string(),
        ));
    }
    if signed.node != file.node
        || signed.content != file.content
        || signed.component != file.component
    {
        return Err(Error::ParseError(
            "CCS v2 /usr/bin/conary authority disagrees with component payload".to_string(),
        ));
    }

    Ok(file)
}

fn canonical_conary_payload<'a>(
    verified: &'a crate::ccs::verify::VerifiedCcsArchive,
    file: &crate::ccs::builder::FileEntry,
) -> Result<&'a crate::packages::payload::PackagePayloadFile> {
    let authority = file.content.as_ref().ok_or_else(|| {
        Error::ParseError("CCS package /usr/bin/conary is missing content authority".to_string())
    })?;
    checked_binary_size(authority.size)?;
    let mut matches = verified
        .payload()
        .files()
        .iter()
        .filter(|candidate| candidate.path == file.path);
    let payload = matches.next().ok_or_else(|| {
        Error::ParseError(format!(
            "CCS package is missing authenticated payload source {} for /usr/bin/conary",
            authority.sha256
        ))
    })?;
    if matches.next().is_some() {
        return Err(Error::ParseError(
            "CCS package repeats authenticated /usr/bin/conary payload source".to_string(),
        ));
    }
    if payload.node != file.node || payload.content_authority.as_ref() != Some(authority) {
        return Err(Error::ParseError(
            "authenticated /usr/bin/conary payload source disagrees with signed authority"
                .to_string(),
        ));
    }
    Ok(payload)
}

fn persist_extracted_binary(
    payload: &crate::packages::payload::PackagePayloadFile,
    target_dir: &Path,
) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    use std::io::{Read as _, Write as _};

    let authority = payload.content_authority.as_ref().ok_or_else(|| {
        Error::ParseError("CCS package /usr/bin/conary is missing content authority".to_string())
    })?;
    checked_binary_size(authority.size)?;
    let mut source = payload.open_content()?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".conary-update-")
        .tempfile_in(target_dir)
        .map_err(|e| Error::IoError(format!("Failed to create temp file: {e}")))?;
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        copied = copied.checked_add(read as u64).ok_or_else(|| {
            Error::IoError("self-update payload size arithmetic overflow".to_string())
        })?;
        if copied > authority.size {
            return Err(Error::ParseError(format!(
                "CCS package /usr/bin/conary size mismatch: expected {} bytes, got at least {copied}",
                authority.size
            )));
        }
        digest.update(&buffer[..read]);
        tmp.as_file_mut().write_all(&buffer[..read])?;
    }
    if copied != authority.size {
        return Err(Error::ParseError(format!(
            "CCS package /usr/bin/conary size mismatch: expected {} bytes, got {copied}",
            authority.size
        )));
    }
    let actual = hex::encode(digest.finalize());
    if actual != authority.sha256 {
        return Err(Error::ChecksumMismatch {
            expected: authority.sha256.clone(),
            actual,
        });
    }
    tmp.as_file_mut().sync_all()?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            tmp.path(),
            fs::Permissions::from_mode(payload.node.mode & 0o7777),
        )?;
    }

    let dest = tmp
        .keep()
        .map_err(|e| Error::IoError(format!("Failed to persist temp file: {e}")))?
        .1;
    Ok(dest)
}

/// Extract the exact `/usr/bin/conary` payload from a current CCS package.
///
/// The shared CCS archive reader owns schema parsing and component authority.
/// Self-update accepts no direct-binary or flattened legacy component fallback.
pub fn extract_binary(
    verified: &crate::ccs::verify::VerifiedCcsArchive,
    target_dir: &Path,
) -> Result<PathBuf> {
    let entry = canonical_conary_entry(verified)?;
    let payload = canonical_conary_payload(verified, entry)?;
    persist_extracted_binary(payload, target_dir)
}

/// Atomically replace the running conary binary and register in CAS
///
/// 1. Read and register the verified update in CAS.
/// 2. rename() temp binary -> target path (atomic on same filesystem).
///
/// CAS registration is a precondition: the running binary is not replaced
/// when its rollback object cannot be persisted.
pub fn apply_update(new_binary_path: &Path, target_path: &Path, objects_dir: &str) -> Result<()> {
    use crate::filesystem::CasStore;

    let content = fs::read(new_binary_path)?;
    let cas = CasStore::new(objects_dir)?;
    cas.store(&content)?;

    // Atomic rename (source and target must be on same filesystem)
    fs::rename(new_binary_path, target_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Error::IoError(format!(
                "Permission denied: cannot replace {}. Try running with sudo.",
                target_path.display()
            ))
        } else {
            Error::IoError(format!(
                "Failed to replace binary at {}: {e}",
                target_path.display()
            ))
        }
    })?;

    Ok(())
}

/// Verify the extracted binary runs and reports the expected version
pub fn verify_binary(binary_path: &Path, expected_version: &str) -> Result<()> {
    let output = Command::new(binary_path)
        .arg("--version")
        .output()
        .map_err(|e| Error::IoError(format!("Failed to execute new binary: {e}")))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "New binary exited with status {}",
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected_version) {
        return Err(Error::IoError(format!(
            "Version mismatch: expected '{}' in output, got '{}'",
            expected_version,
            stdout.trim()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        chunks: &[&[u8]],
    ) -> crate::ccs::builder::BuildResult {
        let content = chunks.concat();
        let mut build = current_self_update_build(package_name, &content);
        let chunk_hashes = chunks
            .iter()
            .map(|chunk| crate::hash::sha256(chunk))
            .collect::<Vec<_>>();
        build.files[0].chunks = Some(chunk_hashes.clone());
        build.components.get_mut("runtime").unwrap().files[0].chunks = Some(chunk_hashes);
        build.chunked = true;
        build
    }

    fn replace_current_object_bytes(path: &Path, hash: &str, replacement: &[u8]) {
        let target = format!("objects/{}/{}", &hash[..2], &hash[2..]);
        let file = std::fs::File::open(path).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
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
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (entry_path, bytes) in entries {
            append_test_tar_entry(&mut builder, entry_path.to_str().unwrap(), &bytes);
        }
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
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let builder = tar::Builder::new(encoder);
        builder.into_inner().unwrap().finish().unwrap();

        let key = crate::ccs::signing::SigningKeyPair::generate();
        let error = crate::ccs::verify::verify_package(
            &ccs_path,
            &crate::ccs::verify::TrustPolicy::strict(vec![key.public_key_base64()]),
        )
        .unwrap_err();
        assert_error_chain_contains(&error, "missing required current v2 MANIFEST authority");
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
        let first = b"#!/bin/sh\n";
        let second = b"echo chunks\n";
        let content = [first.as_slice(), second.as_slice()].concat();
        let build = chunked_self_update_build("conary", &[first, second]);
        let key = write_current_self_update_ccs(&ccs_path, &build);

        let verified = verify_test_self_update_ccs(&ccs_path, &key);
        let content_hash = crate::hash::sha256(&content);
        let crate::ccs::v2::schema::PackageKindV2::Package(package) = &verified.authority().kind
        else {
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
        assert!(verified.components()["runtime"].files[0].chunks.is_none());
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
        let result =
            verify_update_signature_with_keys("tampered_hash", &sig_b64, &[key_hex.as_str()]);
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

        let result =
            verify_update_signature_with_keys(sha256_hex, &sig_b64, &[wrong_key_hex.as_str()]);
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
}
