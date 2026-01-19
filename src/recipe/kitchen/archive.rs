// src/recipe/kitchen/archive.rs

//! Archive and source file utilities for the Kitchen

use crate::error::{Error, Result};
use crate::hash::{hash_bytes, HashAlgorithm};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Download a file from a URL
pub fn download_file(url: &str, dest: &Path) -> Result<()> {
    // Use curl for now (could use reqwest later)
    let output = Command::new("curl")
        .args(["-fsSL", "-o", dest.to_str().expect("path must be valid utf-8"), url])
        .output()
        .map_err(|e| Error::DownloadError(format!("curl failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::DownloadError(format!(
            "Failed to download {}: {}",
            url,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

/// Verify file checksum
///
/// The expected checksum should be in the format "algorithm:hash"
/// (e.g., "sha256:abc123..." or "xxh128:def456...")
pub fn verify_file_checksum(path: &Path, expected: &str) -> Result<bool> {
    let content = fs::read(path)?;

    let (algorithm, expected_hash) = expected
        .split_once(':')
        .ok_or_else(|| Error::ParseError("Invalid checksum format".to_string()))?;

    let algo = match algorithm {
        "sha256" => HashAlgorithm::Sha256,
        "xxh128" => HashAlgorithm::Xxh128,
        _ => {
            return Err(Error::ParseError(format!(
                "Unsupported checksum algorithm: {} (supported: sha256, xxh128)",
                algorithm
            )))
        }
    };

    let actual = hash_bytes(algo, &content);
    Ok(actual.as_str() == expected_hash)
}

/// Extract an archive to a destination directory
///
/// Supports: .tar.gz, .tgz, .tar.xz, .txz, .tar.bz2, .tbz2, .tar.zst, .tar
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let filename = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let args: Vec<&str> = if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        vec!["-xzf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")]
    } else if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
        vec!["-xJf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")]
    } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
        vec!["-xjf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")]
    } else if filename.ends_with(".tar.zst") {
        vec!["--zstd", "-xf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")]
    } else if filename.ends_with(".tar") {
        vec!["-xf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")]
    } else {
        return Err(Error::ParseError(format!(
            "Unknown archive format: {}",
            filename
        )));
    };

    let output = Command::new("tar")
        .args(&args)
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
        .args(["-p", &strip.to_string(), "-i", patch_path.to_str().expect("patch path must be valid utf-8")])
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
        let result = verify_file_checksum(Path::new("/nonexistent"), "invalid");
        assert!(result.is_err());

        let result = verify_file_checksum(Path::new("/nonexistent"), "unknown:abc");
        assert!(result.is_err()); // unsupported algorithm
    }

    #[test]
    fn test_extract_archive_unknown_format() {
        let result = extract_archive(Path::new("file.unknown"), Path::new("/tmp"));
        assert!(result.is_err());
    }
}
