// conary-core/src/recipe/kitchen/archive.rs

//! Archive and source file utilities for the Kitchen

use crate::error::{Error, Result};
use crate::hash::{HashAlgorithm, hash_bytes};
use crate::recipe::kitchen::config::SourceChecksumPolicy;
use std::fs;
use std::path::Path;
use std::process::Command;
use tracing::warn;

fn gnu_fetch_candidates(url: &str) -> Vec<String> {
    let mut candidates = vec![url.to_string()];

    for prefix in ["https://ftpmirror.gnu.org/", "http://ftpmirror.gnu.org/"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            candidates.push(format!("https://ftp.gnu.org/gnu/{rest}"));
            break;
        }
    }

    candidates
}

/// Download a file from a URL
pub fn download_file(url: &str, dest: &Path) -> Result<()> {
    // Reject non-HTTP(S) URL schemes to prevent file:// and other injections
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(Error::DownloadError(format!(
            "Only http:// and https:// URLs are supported for source downloads, got: {}",
            &url[..url.len().min(50)]
        )));
    }

    // Use curl for now (could use reqwest later)
    let dest_str = dest.to_str().ok_or_else(|| {
        Error::DownloadError(format!("Non-UTF-8 download path: {}", dest.display()))
    })?;
    let mut last_error = String::new();
    for candidate in gnu_fetch_candidates(url) {
        let output = Command::new("curl")
            .args([
                "-fsSL",
                "--connect-timeout",
                "30",
                "--max-time",
                "600",
                "--retry",
                "3",
                "-o",
                dest_str,
                &candidate,
            ])
            .output()
            .map_err(|e| Error::DownloadError(format!("curl failed: {}", e)))?;

        if output.status.success() {
            last_error.clear();
            break;
        }

        last_error = format!(
            "Failed to download {}: {}",
            candidate,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    if !last_error.is_empty() {
        return Err(Error::DownloadError(last_error));
    }

    Ok(())
}

/// Verify file checksum.
///
/// The expected checksum should be in the format "algorithm:hash"
/// (e.g., "sha256:abc123..." or "xxh128:def456...")
///
/// Returns `Ok(None)` when the checksum matches, or `Ok(Some(actual_hash))`
/// when it does not, allowing callers to include the actual hash in error
/// messages. Returns `Err` on I/O failure or unsupported algorithm.
pub fn verify_file_checksum(
    path: &Path,
    expected: &str,
    policy: SourceChecksumPolicy,
) -> Result<Option<String>> {
    let content = fs::read(path)?;

    let (algorithm, expected_hash) = expected
        .split_once(':')
        .ok_or_else(|| Error::ParseError("Invalid checksum format".to_string()))?;

    let algo = match algorithm {
        "sha256" => HashAlgorithm::Sha256,
        "xxh128" => HashAlgorithm::Xxh128,
        _ if policy == SourceChecksumPolicy::BootstrapLegacy => {
            warn!(
                "Skipping unsupported checksum algorithm {} in bootstrap legacy mode",
                algorithm
            );
            return Ok(None);
        }
        _ => {
            return Err(Error::ParseError(format!(
                "Unsupported checksum algorithm: {} (supported: sha256, xxh128)",
                algorithm
            )));
        }
    };

    let actual = hash_bytes(algo, &content);
    if actual.as_str() == expected_hash {
        Ok(None)
    } else {
        Ok(Some(format!("{}:{}", algorithm, actual.as_str())))
    }
}

/// Extract an archive to a destination directory
///
/// Supports: .tar.gz, .tgz, .tar.xz, .txz, .tar.bz2, .tbz2, .tar.zst, .tar
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let filename = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let archive_str = archive
        .to_str()
        .ok_or_else(|| Error::IoError(format!("Non-UTF-8 archive path: {}", archive.display())))?;
    let dest_str = dest
        .to_str()
        .ok_or_else(|| Error::IoError(format!("Non-UTF-8 destination path: {}", dest.display())))?;

    let flags: &[&str] = if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        &["-xzf"]
    } else if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
        &["-xJf"]
    } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
        &["-xjf"]
    } else if filename.ends_with(".tar.zst") {
        &["--zstd", "-xf"]
    } else if filename.ends_with(".tar") {
        &["-xf"]
    } else {
        return Err(Error::ParseError(format!(
            "Unknown archive format: {}",
            filename
        )));
    };

    let output = Command::new("tar")
        .args(flags)
        .args([archive_str, "-C", dest_str])
        .arg("--no-same-owner")
        .arg("--no-same-permissions")
        .output()
        .map_err(|e| Error::IoError(format!("tar failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "Failed to extract archive: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

/// Apply a patch to the source directory
pub fn apply_patch(source_dir: &Path, patch_path: &Path, strip: u32) -> Result<()> {
    let output = Command::new("patch")
        .args([
            "-p",
            &strip.to_string(),
            "-i",
            patch_path.to_str().ok_or_else(|| {
                Error::IoError(format!("Non-UTF-8 patch path: {}", patch_path.display()))
            })?,
        ])
        .current_dir(source_dir)
        .output()
        .map_err(|e| Error::IoError(format!("patch failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "Failed to apply patch: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_checksum_format() {
        // Just testing the format parsing (not actual file content)
        let result = verify_file_checksum(
            Path::new("/nonexistent"),
            "invalid",
            SourceChecksumPolicy::Supported,
        );
        assert!(result.is_err());

        let result = verify_file_checksum(
            Path::new("/nonexistent"),
            "unknown:abc",
            SourceChecksumPolicy::Supported,
        );
        assert!(result.is_err()); // unsupported algorithm
    }

    #[test]
    fn test_extract_archive_unknown_format() {
        let result = extract_archive(Path::new("file.unknown"), Path::new("/tmp"));
        assert!(result.is_err());
    }

    #[test]
    fn test_gnu_fetch_candidates_adds_canonical_fallback_for_ftpmirror() {
        let candidates = gnu_fetch_candidates("https://ftpmirror.gnu.org/bash/bash-5.3.tar.gz");
        assert_eq!(
            candidates,
            vec![
                "https://ftpmirror.gnu.org/bash/bash-5.3.tar.gz".to_string(),
                "https://ftp.gnu.org/gnu/bash/bash-5.3.tar.gz".to_string()
            ]
        );
    }
}
