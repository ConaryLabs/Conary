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
/// Real key will be added when release signing is enabled.
pub const TRUSTED_UPDATE_KEYS: &[&str] = &[];

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

/// Get the update channel URL from settings or fall back to default
pub fn get_update_channel(conn: &Connection) -> Result<String> {
    match settings::get(conn, SETTINGS_KEY_UPDATE_CHANNEL)? {
        Some(url) => Ok(url),
        None => Ok(DEFAULT_UPDATE_CHANNEL.to_string()),
    }
}

/// Set a custom update channel URL
pub fn set_update_channel(conn: &Connection, url: &str) -> Result<()> {
    settings::set(conn, SETTINGS_KEY_UPDATE_CHANNEL, url)
}

/// Extract the conary binary from a CCS package to a temp file
///
/// Returns the path to the extracted binary. The binary is placed in
/// `target_dir` (the same filesystem as the final install path) so that
/// `apply_update`'s atomic `rename()` works. A `NamedTempFile` is used
/// so the partial file is automatically removed on failure.
pub fn extract_binary(ccs_path: &Path, target_dir: &Path) -> Result<PathBuf> {
    use flate2::read::GzDecoder;
    use std::io::{Read, Write as _};
    use tar::Archive;

    let file = fs::File::open(ccs_path)
        .map_err(|e| Error::IoError(format!("Failed to open CCS package: {e}")))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = path.to_string_lossy();

        // Look for the conary binary in the CCS package
        if path_str.ends_with("usr/bin/conary") || path_str == "conary" {
            // Guard against malicious/corrupted packages with oversized entries.
            // A conary binary should never exceed 256 MB.
            const MAX_BINARY_SIZE: u64 = 256 * 1024 * 1024;
            let entry_size = entry.header().size()?;
            if entry_size > MAX_BINARY_SIZE {
                return Err(Error::IoError(format!(
                    "Binary entry too large ({entry_size} bytes, max {MAX_BINARY_SIZE})"
                )));
            }

            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;

            // Use a NamedTempFile in target_dir (same filesystem as the install
            // target) so the file is cleaned up automatically on failure. On
            // success the caller will rename() it into place atomically.
            let tmp = tempfile::Builder::new()
                .prefix(".conary-update-")
                .tempfile_in(target_dir)
                .map_err(|e| Error::IoError(format!("Failed to create temp file: {e}")))?;
            tmp.as_file()
                .write_all(&content)
                .map_err(|e| Error::IoError(format!("Failed to write binary: {e}")))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755))?;
            }

            // Persist the temp file so the caller can rename() it.
            let dest = tmp
                .keep()
                .map_err(|e| Error::IoError(format!("Failed to persist temp file: {e}")))?
                .1;
            return Ok(dest);
        }
    }

    Err(Error::ParseError(
        "CCS package does not contain a conary binary".to_string(),
    ))
}

/// Atomically replace the running conary binary and register in CAS
///
/// 1. rename() temp binary -> target path (atomic on same filesystem)
/// 2. Store new binary hash in CAS (best-effort)
pub fn apply_update(new_binary_path: &Path, target_path: &Path, objects_dir: &str) -> Result<()> {
    use crate::filesystem::CasStore;

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

    // Register new binary in CAS (best-effort: if this fails, binary still works)
    let content = match fs::read(target_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(
                path = %target_path.display(),
                error = %e,
                "failed to read updated binary for CAS registration"
            );
            Vec::new()
        }
    };
    if !content.is_empty()
        && let Ok(cas) = CasStore::new(objects_dir)
        && let Err(e) = cas.store(&content)
    {
        warn!(error = %e, "failed to register updated binary in CAS");
    }

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

    fn create_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        conn
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
        crate::db::schema::migrate(&conn).unwrap();

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
    fn test_extract_binary_empty_archive() {
        let dir = tempfile::tempdir().unwrap();
        let ccs_path = dir.path().join("empty.ccs");

        // Create a valid but empty gzipped tar
        {
            use flate2::Compression;
            use flate2::write::GzEncoder;
            let file = std::fs::File::create(&ccs_path).unwrap();
            let encoder = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(encoder);
            builder.finish().unwrap();
        }

        let result = extract_binary(&ccs_path, dir.path());
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("does not contain"), "Error was: {err_msg}");
    }

    #[test]
    fn test_extract_binary_finds_conary() {
        let dir = tempfile::tempdir().unwrap();
        let ccs_path = dir.path().join("test.ccs");

        // Create a gzipped tar with a usr/bin/conary entry
        {
            use flate2::Compression;
            use flate2::write::GzEncoder;
            let file = std::fs::File::create(&ccs_path).unwrap();
            let encoder = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let content = b"#!/bin/sh\necho test";
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "usr/bin/conary", &content[..])
                .unwrap();
            builder.finish().unwrap();
        }

        let result = extract_binary(&ccs_path, dir.path());
        assert!(result.is_ok(), "extract_binary failed: {:?}", result.err());

        let binary_path = result.unwrap();
        assert!(binary_path.exists());
        assert_eq!(
            std::fs::read(&binary_path).unwrap(),
            b"#!/bin/sh\necho test"
        );
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
