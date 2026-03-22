// conary-core/src/self_update.rs

//! Self-update logic for the conary binary
//!
//! Checks Remi for newer versions and handles downloading, verifying,
//! and atomically replacing the running binary.

use crate::db::models::settings;
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::warn;

/// Default update channel URL
pub const DEFAULT_UPDATE_CHANNEL: &str = "https://packages.conary.io/v1/ccs/conary";

/// Settings key for the update channel override
const SETTINGS_KEY_UPDATE_CHANNEL: &str = "update-channel";

/// Response from the /latest endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct LatestVersionInfo {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
}

/// Result of a version check
#[derive(Debug, Clone, PartialEq)]
pub enum VersionCheckResult {
    /// A newer version is available
    UpdateAvailable {
        current: String,
        latest: String,
        download_url: String,
        sha256: String,
        size: u64,
    },
    /// Already at the latest version
    UpToDate { version: String },
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

/// Compare two semver version strings. Returns true if `remote` is newer than `current`.
///
/// Handles pre-release versions per SemVer rules:
/// - A pre-release version (e.g., `1.0.0-alpha.1`) is always older than its
///   release counterpart (`1.0.0`).
/// - Pre-release identifiers are compared left-to-right: numeric identifiers
///   are compared as integers, alphanumeric identifiers are compared
///   lexicographically, and numeric identifiers always sort before
///   alphanumeric ones.
pub fn is_newer(current: &str, remote: &str) -> bool {
    let parse = |v: &str| -> ((u64, u64, u64), Option<Vec<String>>) {
        // Split off pre-release suffix from the patch component
        let parts: Vec<&str> = v.split('.').collect();
        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        let patch_str = parts.get(2).copied().unwrap_or("0");
        let (patch_num, prerelease) = if let Some(dash_pos) = patch_str.find('-') {
            let patch = patch_str[..dash_pos].parse().unwrap_or(0);
            // Collect all pre-release identifiers (patch remainder + any further dot-separated parts)
            let mut pre_parts: Vec<String> = patch_str[dash_pos + 1..]
                .split('.')
                .map(String::from)
                .collect();
            // Include any additional dot-separated parts beyond the third component
            for part in parts.iter().skip(3) {
                pre_parts.push(part.to_string());
            }
            (patch, Some(pre_parts))
        } else {
            let patch = patch_str.parse().unwrap_or(0);
            // Check if there are parts beyond patch that contain a pre-release marker
            if parts.len() > 3 {
                // No dash in patch, but extra components exist; treat as normal version
                (patch, None)
            } else {
                (patch, None)
            }
        };

        ((major, minor, patch_num), prerelease)
    };

    let (remote_ver, remote_pre) = parse(remote);
    let (current_ver, current_pre) = parse(current);

    match remote_ver.cmp(&current_ver) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => {
            // Same version number: compare pre-release
            match (&current_pre, &remote_pre) {
                // Both releases (no pre-release) => not newer
                (None, None) => false,
                // Remote is a release, current is pre-release => remote is newer
                (Some(_), None) => true,
                // Remote is pre-release, current is release => remote is older
                (None, Some(_)) => false,
                // Both pre-release: compare identifiers
                (Some(cur), Some(rem)) => {
                    compare_prerelease(rem, cur) == std::cmp::Ordering::Greater
                }
            }
        }
    }
}

/// Compare pre-release identifier lists per SemVer 2.0 rules.
fn compare_prerelease(a: &[String], b: &[String]) -> std::cmp::Ordering {
    for (ai, bi) in a.iter().zip(b.iter()) {
        let a_num = ai.parse::<u64>();
        let b_num = bi.parse::<u64>();
        let ord = match (a_num, b_num) {
            // Both numeric: compare as integers
            (Ok(an), Ok(bn)) => an.cmp(&bn),
            // Numeric sorts before alphanumeric
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            // Both alphanumeric: lexicographic
            (Err(_), Err(_)) => ai.cmp(bi),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    // Fewer identifiers => lower precedence
    a.len().cmp(&b.len())
}

/// Check for available updates by querying the update channel
pub async fn check_for_update(channel_url: &str, current_version: &str) -> Result<VersionCheckResult> {
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))?;

    let url = format!("{channel_url}/latest");
    let response = client
        .get(&url)
        .header("User-Agent", format!("conary/{current_version}"))
        .send()
        .await
        .map_err(|e| Error::IoError(format!("Failed to check for updates: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::IoError(format!(
            "Update check failed: HTTP {}",
            response.status()
        )));
    }

    let info: LatestVersionInfo = response
        .json()
        .await
        .map_err(|e| Error::ParseError(format!("Invalid update response: {e}")))?;

    if is_newer(current_version, &info.version) {
        Ok(VersionCheckResult::UpdateAvailable {
            current: current_version.to_string(),
            latest: info.version,
            download_url: info.download_url,
            sha256: info.sha256,
            size: info.size,
        })
    } else {
        Ok(VersionCheckResult::UpToDate {
            version: current_version.to_string(),
        })
    }
}

/// Download the CCS package to a temp directory and return the path
///
/// Streams the download through a SHA-256 hasher while writing to disk,
/// avoiding a second full read of the file for verification.
pub async fn download_update(
    download_url: &str,
    expected_sha256: &str,
    dest_dir: &Path,
) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let dest_path = dest_dir.join("conary-update.ccs");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))?;

    let mut response = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| Error::DownloadError(format!("Failed to download update: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::DownloadError(format!(
            "Download failed: HTTP {}",
            response.status()
        )));
    }

    let mut file = fs::File::create(&dest_path)
        .map_err(|e| Error::IoError(format!("Failed to create output file: {e}")))?;
    let mut hasher = Sha256::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::DownloadError(format!("Failed to read download stream: {e}")))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| Error::IoError(format!("Failed to write downloaded data: {e}")))?;
    }
    file.flush()
        .map_err(|e| Error::IoError(format!("Failed to flush download file: {e}")))?;

    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != expected_sha256 {
        fs::remove_file(&dest_path).ok();
        return Err(Error::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual: actual_hash,
        });
    }

    Ok(dest_path)
}

/// Download the CCS package with a visual progress bar
///
/// Like [`download_update`] but displays download progress via `indicatif`.
/// If `content_length` is provided, shows a determinate bar; otherwise a spinner.
pub async fn download_update_with_progress(
    download_url: &str,
    expected_sha256: &str,
    dest_dir: &Path,
    content_length: Option<u64>,
) -> Result<PathBuf> {
    use indicatif::{ProgressBar, ProgressStyle};
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let dest_path = dest_dir.join("conary-update.ccs");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))?;

    let mut response = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| Error::DownloadError(format!("Failed to download update: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::DownloadError(format!(
            "Download failed: HTTP {}",
            response.status()
        )));
    }

    // Use content-length from response header if not provided, fall back to spinner
    let total = content_length.or_else(|| response.content_length());

    let pb = if let Some(size) = total {
        let bar = ProgressBar::new(size);
        bar.set_style(
            ProgressStyle::default_bar()
                .template("  Downloading [{bar:40.green/dim}] {bytes}/{total_bytes}")
                .expect("Invalid progress bar template")
                .progress_chars("##-"),
        );
        bar
    } else {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("  {spinner:.cyan} Downloading... {bytes}")
                .expect("Invalid spinner template"),
        );
        spinner.enable_steady_tick(std::time::Duration::from_millis(100));
        spinner
    };

    let mut file = fs::File::create(&dest_path)
        .map_err(|e| Error::IoError(format!("Failed to create output file: {e}")))?;
    let mut hasher = Sha256::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::DownloadError(format!("Failed to read download stream: {e}")))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| Error::IoError(format!("Failed to write downloaded data: {e}")))?;
        pb.inc(chunk.len() as u64);
    }
    file.flush()
        .map_err(|e| Error::IoError(format!("Failed to flush download file: {e}")))?;

    pb.finish_and_clear();

    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != expected_sha256 {
        fs::remove_file(&dest_path).ok();
        return Err(Error::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual: actual_hash,
        });
    }

    Ok(dest_path)
}

/// Extract the conary binary from a CCS package to a temp file
///
/// Returns the path to the extracted binary. The binary is placed on the
/// same filesystem as `target_dir` to enable atomic rename().
pub fn extract_binary(ccs_path: &Path, target_dir: &Path) -> Result<PathBuf> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tar::Archive;

    let file = fs::File::open(ccs_path)
        .map_err(|e| Error::IoError(format!("Failed to open CCS package: {e}")))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    let dest = target_dir.join(".conary-update.tmp");

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = path.to_string_lossy();

        // Look for the conary binary in the CCS package
        if path_str.ends_with("usr/bin/conary") || path_str == "conary" {
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            fs::write(&dest, &content)
                .map_err(|e| Error::IoError(format!("Failed to write binary: {e}")))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
            }

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

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.0", "1.0.0"));
        assert!(!is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_is_newer_edge_cases() {
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(is_newer("0.99.99", "1.0.0"));
        assert!(is_newer("1", "2"));
        assert!(is_newer("1.0", "1.1"));
    }

    #[test]
    fn test_is_newer_prerelease() {
        // Pre-release is older than release
        assert!(!is_newer("1.0.0", "1.0.0-alpha.1"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0"));
        // Pre-release ordering
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-alpha.2"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-beta.1"));
        assert!(!is_newer("1.0.0-beta.1", "1.0.0-alpha.1"));
        // Same pre-release is not newer
        assert!(!is_newer("1.0.0-alpha.1", "1.0.0-alpha.1"));
    }

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
    fn test_version_check_result_variants() {
        let up_to_date = VersionCheckResult::UpToDate {
            version: "0.1.0".to_string(),
        };
        assert_eq!(
            up_to_date,
            VersionCheckResult::UpToDate {
                version: "0.1.0".to_string()
            }
        );

        let update = VersionCheckResult::UpdateAvailable {
            current: "0.1.0".to_string(),
            latest: "0.2.0".to_string(),
            download_url: "https://example.com/conary-0.2.0.ccs".to_string(),
            sha256: "abc123".to_string(),
            size: 12_000_000,
        };
        match &update {
            VersionCheckResult::UpdateAvailable {
                current, latest, ..
            } => {
                assert_eq!(current, "0.1.0");
                assert_eq!(latest, "0.2.0");
            }
            _ => panic!("Expected UpdateAvailable"),
        }
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
    fn test_is_newer_major_minor_patch() {
        // Major version takes priority
        assert!(is_newer("1.9.9", "2.0.0"));
        assert!(!is_newer("2.0.0", "1.9.9"));
        // Minor version takes priority over patch
        assert!(is_newer("1.0.9", "1.1.0"));
        assert!(!is_newer("1.1.0", "1.0.9"));
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
}
