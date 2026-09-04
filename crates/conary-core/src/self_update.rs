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
    let crate::ccs::v3::schema::PackageKindV3::Package(package) = &authority.kind else {
        return Err(Error::ParseError(
            "CCS v3 self-update authority is not a package".to_string(),
        ));
    };
    let mut signed_entries = package
        .files
        .iter()
        .filter(|candidate| candidate.path == "/usr/bin/conary");
    let signed = signed_entries.next().ok_or_else(|| {
        Error::ParseError("CCS v3 authority does not declare /usr/bin/conary".to_string())
    })?;
    if signed_entries.next().is_some() {
        return Err(Error::ParseError(
            "CCS v3 authority declares /usr/bin/conary more than once".to_string(),
        ));
    }
    if signed.node != file.node
        || signed.content != file.content
        || signed.component != file.component
    {
        return Err(Error::ParseError(
            "CCS v3 /usr/bin/conary authority disagrees with component payload".to_string(),
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
mod tests;
