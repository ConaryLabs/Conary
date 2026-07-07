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
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
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

/// Component manifests can be larger than package metadata but should remain bounded.
const MAX_COMPONENT_MANIFEST_SIZE: u64 = 8 * 1024 * 1024;

/// Total component metadata retained while reconstructing a self-update binary.
const MAX_COMPONENT_MANIFEST_TOTAL_SIZE: u64 = MAX_COMPONENT_MANIFEST_SIZE;

/// Total object payload retained while reconstructing a self-update binary.
const MAX_SELF_UPDATE_OBJECT_TOTAL_SIZE: u64 = MAX_SELF_UPDATE_BINARY_SIZE;

/// A chunked 256 MB binary should not need more than 8192 32 KiB chunks.
const MAX_SELF_UPDATE_OBJECT_COUNT: usize = 8192;

#[derive(Debug, Deserialize)]
struct SelfUpdateComponentManifest {
    files: Vec<SelfUpdateComponentFile>,
}

#[derive(Debug, Deserialize)]
struct SelfUpdateComponentFile {
    path: String,
    hash: String,
    size: u64,
    mode: Option<u32>,
    #[serde(default)]
    chunks: Vec<String>,
    #[serde(rename = "type")]
    file_type: Option<String>,
}

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

fn archive_path_string(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches("./").to_string()
}

fn archive_object_hash(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() == 3 && parts[0] == "objects" && parts[1].len() == 2 && !parts[2].is_empty() {
        return Some(format!("{}{}", parts[1], parts[2]));
    }
    None
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

fn reserve_vec(vec: &mut Vec<u8>, capacity: usize, description: &str) -> Result<()> {
    vec.try_reserve_exact(capacity).map_err(|e| {
        Error::IoError(format!(
            "Failed to reserve {description} buffer ({capacity} bytes): {e}"
        ))
    })
}

fn read_entry_to_vec<R: Read>(
    entry: &mut R,
    declared_size: u64,
    description: &str,
) -> Result<Vec<u8>> {
    let capacity = usize::try_from(declared_size).map_err(|_| {
        Error::IoError(format!(
            "{description} entry too large ({declared_size} bytes)"
        ))
    })?;
    let mut content = Vec::new();
    reserve_vec(&mut content, capacity, description)?;
    entry.read_to_end(&mut content)?;
    Ok(content)
}

fn add_limited_payload(
    total: &mut u64,
    entry_size: u64,
    max_size: u64,
    description: &str,
) -> Result<()> {
    let new_total = total.checked_add(entry_size).ok_or_else(|| {
        Error::IoError(format!(
            "{description} too large (overflow, max {max_size} bytes)"
        ))
    })?;

    if new_total > max_size {
        return Err(Error::IoError(format!(
            "{description} too large ({new_total} bytes, max {max_size})"
        )));
    }

    *total = new_total;
    Ok(())
}

fn verify_object_hash(expected: &str, content: &[u8]) -> Result<()> {
    let actual = crate::hash::sha256(content);
    if actual != expected {
        return Err(Error::ChecksumMismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

fn persist_extracted_binary(content: &[u8], target_dir: &Path, mode: u32) -> Result<PathBuf> {
    if content.len() as u64 > MAX_SELF_UPDATE_BINARY_SIZE {
        return Err(Error::IoError(format!(
            "Binary entry too large ({} bytes, max {MAX_SELF_UPDATE_BINARY_SIZE})",
            content.len()
        )));
    }

    let tmp = tempfile::Builder::new()
        .prefix(".conary-update-")
        .tempfile_in(target_dir)
        .map_err(|e| Error::IoError(format!("Failed to create temp file: {e}")))?;
    {
        use std::io::Write as _;
        tmp.as_file()
            .write_all(content)
            .map_err(|e| Error::IoError(format!("Failed to write binary: {e}")))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable_mode = mode & 0o777;
        let executable_mode = if executable_mode == 0 {
            0o755
        } else {
            executable_mode
        };
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(executable_mode))?;
    }

    let dest = tmp
        .keep()
        .map_err(|e| Error::IoError(format!("Failed to persist temp file: {e}")))?
        .1;
    Ok(dest)
}

fn reconstructed_conary_binary(
    component_manifests: &[Vec<u8>],
    objects: &HashMap<String, Vec<u8>>,
) -> Result<Option<(Vec<u8>, u32)>> {
    for manifest_bytes in component_manifests {
        let manifest: SelfUpdateComponentManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|e| Error::ParseError(format!("Invalid component manifest: {e}")))?;

        for file in manifest.files {
            if file.path != "/usr/bin/conary" && file.path != "usr/bin/conary" {
                continue;
            }
            if file
                .file_type
                .as_deref()
                .is_some_and(|kind| kind != "regular")
            {
                return Err(Error::ParseError(
                    "CCS package conary entry is not a regular file".to_string(),
                ));
            }
            let expected_size = checked_binary_size(file.size)?;

            let content = if file.chunks.is_empty() {
                objects.get(&file.hash).cloned().ok_or_else(|| {
                    Error::ParseError(format!(
                        "CCS package is missing object {} for /usr/bin/conary",
                        file.hash
                    ))
                })?
            } else {
                let mut content = Vec::new();
                reserve_vec(&mut content, expected_size, "/usr/bin/conary")?;
                for chunk_hash in &file.chunks {
                    let chunk = objects.get(chunk_hash).ok_or_else(|| {
                        Error::ParseError(format!(
                            "CCS package is missing chunk object {chunk_hash} for /usr/bin/conary"
                        ))
                    })?;
                    verify_object_hash(chunk_hash, chunk)?;
                    let new_len = content.len().checked_add(chunk.len()).ok_or_else(|| {
                        Error::IoError(format!(
                            "Binary entry too large (overflow, max {MAX_SELF_UPDATE_BINARY_SIZE})"
                        ))
                    })?;
                    if new_len > expected_size {
                        return Err(Error::ParseError(format!(
                            "CCS package conary size mismatch: expected {} bytes, got at least {new_len}",
                            file.size
                        )));
                    }
                    content.extend_from_slice(chunk);
                }
                content
            };

            if content.len() as u64 != file.size {
                return Err(Error::ParseError(format!(
                    "CCS package conary size mismatch: expected {} bytes, got {}",
                    file.size,
                    content.len()
                )));
            }

            let actual_hash = crate::hash::sha256(&content);
            if actual_hash != file.hash {
                return Err(Error::ChecksumMismatch {
                    expected: file.hash,
                    actual: actual_hash,
                });
            }

            return Ok(Some((content, file.mode.unwrap_or(0o100755))));
        }
    }

    Ok(None)
}

/// Extract the conary binary from a CCS package to a temp file
///
/// Returns the path to the extracted binary. The binary is placed in
/// `target_dir` (the same filesystem as the final install path) so that
/// `apply_update`'s atomic `rename()` works. A `NamedTempFile` is used
/// so the partial file is automatically removed on failure.
pub fn extract_binary(ccs_path: &Path, target_dir: &Path) -> Result<PathBuf> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file = fs::File::open(ccs_path)
        .map_err(|e| Error::IoError(format!("Failed to open CCS package: {e}")))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut component_manifests = Vec::new();
    let mut objects = HashMap::new();
    let mut component_manifest_bytes = 0u64;
    let mut object_bytes = 0u64;
    let mut object_count = 0usize;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = archive_path_string(&path);

        // Look for the conary binary in the CCS package
        if path_str.ends_with("usr/bin/conary") || path_str == "conary" {
            let entry_size = entry.header().size()?;
            if entry_size > MAX_SELF_UPDATE_BINARY_SIZE {
                return Err(Error::IoError(format!(
                    "Binary entry too large ({entry_size} bytes, max {MAX_SELF_UPDATE_BINARY_SIZE})"
                )));
            }

            let content = read_entry_to_vec(&mut entry, entry_size, "direct conary binary")?;
            return persist_extracted_binary(&content, target_dir, 0o755);
        }

        if path_str.starts_with("components/") && path_str.ends_with(".json") {
            let entry_size = entry.header().size()?;
            if entry_size > MAX_COMPONENT_MANIFEST_SIZE {
                return Err(Error::ParseError(format!(
                    "Component manifest too large ({entry_size} bytes, max {MAX_COMPONENT_MANIFEST_SIZE})"
                )));
            }
            add_limited_payload(
                &mut component_manifest_bytes,
                entry_size,
                MAX_COMPONENT_MANIFEST_TOTAL_SIZE,
                "Component manifest payload",
            )?;
            let content = read_entry_to_vec(&mut entry, entry_size, "component manifest")?;
            component_manifests.push(content);
            continue;
        }

        if let Some(hash) = archive_object_hash(&path_str) {
            let entry_size = entry.header().size()?;
            if entry_size > MAX_SELF_UPDATE_BINARY_SIZE {
                return Err(Error::IoError(format!(
                    "CCS object too large ({entry_size} bytes, max {MAX_SELF_UPDATE_BINARY_SIZE})"
                )));
            }
            object_count = object_count.checked_add(1).ok_or_else(|| {
                Error::IoError(format!(
                    "Too many CCS objects (overflow, max {MAX_SELF_UPDATE_OBJECT_COUNT})"
                ))
            })?;
            if object_count > MAX_SELF_UPDATE_OBJECT_COUNT {
                return Err(Error::IoError(format!(
                    "Too many CCS objects ({object_count}, max {MAX_SELF_UPDATE_OBJECT_COUNT})"
                )));
            }
            add_limited_payload(
                &mut object_bytes,
                entry_size,
                MAX_SELF_UPDATE_OBJECT_TOTAL_SIZE,
                "CCS object payload",
            )?;
            let content = read_entry_to_vec(&mut entry, entry_size, "CCS object")?;
            objects.insert(hash, content);
        }
    }

    if let Some((content, mode)) = reconstructed_conary_binary(&component_manifests, &objects)? {
        return persist_extracted_binary(&content, target_dir, mode);
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
    use std::io::Write;

    fn create_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        conn
    }

    fn append_test_tar_entry<W: Write>(builder: &mut tar::Builder<W>, path: &str, content: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, content).unwrap();
    }

    fn append_test_object<W: Write>(builder: &mut tar::Builder<W>, hash: &str, content: &[u8]) {
        let path = format!("objects/{}/{}", &hash[..2], &hash[2..]);
        append_test_tar_entry(builder, &path, content);
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
    fn test_extract_binary_reconstructs_conary_from_component_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let ccs_path = dir.path().join("chunked.ccs");

        {
            use flate2::Compression;
            use flate2::write::GzEncoder;
            let file = std::fs::File::create(&ccs_path).unwrap();
            let encoder = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let first = b"#!/bin/sh\n";
            let second = b"echo cas\n";
            let content = [first.as_slice(), second.as_slice()].concat();
            let first_hash = crate::hash::sha256(first);
            let second_hash = crate::hash::sha256(second);
            let file_hash = crate::hash::sha256(&content);

            for (hash, bytes) in [
                (&first_hash, first.as_slice()),
                (&second_hash, second.as_slice()),
            ] {
                let path = format!("objects/{}/{}", &hash[..2], &hash[2..]);
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, path, bytes).unwrap();
            }

            let runtime_manifest = serde_json::json!({
                "name": "runtime",
                "files": [{
                    "path": "/usr/bin/conary",
                    "hash": file_hash,
                    "size": content.len(),
                    "mode": 0o100755,
                    "component": "runtime",
                    "type": "regular",
                    "chunks": [first_hash, second_hash]
                }]
            })
            .to_string();
            let mut header = tar::Header::new_gnu();
            header.set_size(runtime_manifest.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "components/runtime.json",
                    runtime_manifest.as_bytes(),
                )
                .unwrap();
            builder.finish().unwrap();
        }

        let binary_path = extract_binary(&ccs_path, dir.path()).unwrap();
        assert_eq!(
            std::fs::read(&binary_path).unwrap(),
            b"#!/bin/sh\necho cas\n"
        );
    }

    #[test]
    fn test_reconstructed_conary_binary_rejects_oversized_manifest_size_without_panic() {
        let chunk = b"#!/bin/sh\n";
        let chunk_hash = crate::hash::sha256(chunk);
        let file_hash = crate::hash::sha256(chunk);
        let manifest = serde_json::json!({
            "files": [{
                "path": "/usr/bin/conary",
                "hash": file_hash,
                "size": u64::MAX,
                "mode": 0o100755,
                "type": "regular",
                "chunks": [chunk_hash]
            }]
        })
        .to_string()
        .into_bytes();
        let objects = HashMap::from([(chunk_hash, chunk.to_vec())]);

        let result =
            std::panic::catch_unwind(|| reconstructed_conary_binary(&[manifest], &objects))
                .expect("oversized manifest size should return a clean error, not panic");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Binary entry too large"),
            "Error was: {err_msg}"
        );
    }

    #[test]
    fn test_extract_binary_rejects_excessive_component_manifest_payload() {
        let dir = tempfile::tempdir().unwrap();
        let ccs_path = dir.path().join("too-many-components.ccs");

        {
            use flate2::Compression;
            use flate2::write::GzEncoder;
            let file = std::fs::File::create(&ccs_path).unwrap();
            let encoder = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let manifest_payload = vec![b' '; (MAX_COMPONENT_MANIFEST_SIZE / 2 + 1) as usize];
            append_test_tar_entry(&mut builder, "components/runtime-a.json", &manifest_payload);
            append_test_tar_entry(&mut builder, "components/runtime-b.json", &manifest_payload);
            builder.finish().unwrap();
        }

        let result = extract_binary(&ccs_path, dir.path());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Component manifest payload too large"),
            "Error was: {err_msg}"
        );
    }

    #[test]
    fn test_extract_binary_rejects_too_many_object_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ccs_path = dir.path().join("too-many-objects.ccs");

        {
            use flate2::Compression;
            use flate2::write::GzEncoder;
            let file = std::fs::File::create(&ccs_path).unwrap();
            let encoder = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(encoder);

            for index in 0..8193 {
                let hash = format!("{index:064x}");
                append_test_object(&mut builder, &hash, b"x");
            }
            builder.finish().unwrap();
        }

        let result = extract_binary(&ccs_path, dir.path());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Too many CCS objects"),
            "Error was: {err_msg}"
        );
    }

    #[test]
    fn test_extract_binary_rejects_chunk_object_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let ccs_path = dir.path().join("bad-chunk-hash.ccs");

        {
            use flate2::Compression;
            use flate2::write::GzEncoder;
            let file = std::fs::File::create(&ccs_path).unwrap();
            let encoder = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let expected_chunk = b"#!/bin/sh\n";
            let tampered_chunk = b"#!/bin/bash\n";
            let expected_chunk_hash = crate::hash::sha256(expected_chunk);
            let tampered_chunk_hash = crate::hash::sha256(tampered_chunk);

            append_test_object(&mut builder, &expected_chunk_hash, tampered_chunk);

            let runtime_manifest = serde_json::json!({
                "name": "runtime",
                "files": [{
                    "path": "/usr/bin/conary",
                    "hash": tampered_chunk_hash,
                    "size": tampered_chunk.len(),
                    "mode": 0o100755,
                    "component": "runtime",
                    "type": "regular",
                    "chunks": [expected_chunk_hash]
                }]
            })
            .to_string();
            append_test_tar_entry(
                &mut builder,
                "components/runtime.json",
                runtime_manifest.as_bytes(),
            );
            builder.finish().unwrap();
        }

        let result = extract_binary(&ccs_path, dir.path());
        let Err(Error::ChecksumMismatch { expected, actual }) = result else {
            panic!("expected chunk checksum mismatch, got {result:?}");
        };
        assert_eq!(expected, crate::hash::sha256(b"#!/bin/sh\n"));
        assert_eq!(actual, crate::hash::sha256(b"#!/bin/bash\n"));
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
