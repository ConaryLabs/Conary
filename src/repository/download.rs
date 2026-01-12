// src/repository/download.rs

//! Package and delta download functionality
//!
//! Functions for downloading packages and delta updates from repositories,
//! with checksum verification.

use crate::db::models::RepositoryPackage;
use crate::error::{Error, Result};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::client::RepositoryClient;
use super::metadata::DeltaInfo;

/// Download a package from a repository
pub fn download_package(repo_pkg: &RepositoryPackage, dest_dir: &Path) -> Result<PathBuf> {
    let client = RepositoryClient::new()?;

    // Construct destination path
    let default_filename = format!("{}-{}.rpm", repo_pkg.name, repo_pkg.version);
    let filename = repo_pkg
        .download_url
        .split('/')
        .next_back()
        .unwrap_or(&default_filename);

    let dest_path = dest_dir.join(filename);

    // Download the file
    client.download_file(&repo_pkg.download_url, &dest_path)?;

    // Verify checksum
    verify_checksum(&dest_path, &repo_pkg.checksum)?;

    Ok(dest_path)
}

/// Download a delta update file
///
/// # Arguments
/// * `delta_info` - Delta metadata from repository
/// * `package_name` - Name of the package (for filename construction)
/// * `to_version` - Target version (for filename construction)
/// * `dest_dir` - Destination directory for the delta file
///
/// # Returns
/// Path to the downloaded and verified delta file
pub fn download_delta(
    delta_info: &DeltaInfo,
    package_name: &str,
    to_version: &str,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let client = RepositoryClient::new()?;

    // Construct destination path
    let default_filename = format!(
        "{}-{}-to-{}.delta",
        package_name, delta_info.from_version, to_version
    );
    let filename = delta_info
        .delta_url
        .split('/')
        .next_back()
        .unwrap_or(&default_filename);

    let dest_path = dest_dir.join(filename);

    info!(
        "Downloading delta for {} ({} -> {})",
        package_name, delta_info.from_version, to_version
    );

    // Download the delta file
    client.download_file(&delta_info.delta_url, &dest_path)?;

    // Verify checksum
    verify_checksum(&dest_path, &delta_info.delta_checksum)?;

    info!(
        "Delta downloaded successfully: {} bytes (compression ratio: {:.1}%)",
        delta_info.delta_size,
        delta_info.compression_ratio * 100.0
    );

    Ok(dest_path)
}

/// Verify file checksum matches expected value
pub fn verify_checksum(path: &Path, expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};

    debug!("Verifying checksum for {}", path.display());

    let mut file = File::open(path)
        .map_err(|e| Error::IoError(format!("Failed to open file for checksum: {e}")))?;

    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)
        .map_err(|e| Error::IoError(format!("Failed to read file for checksum: {e}")))?;

    let actual = format!("{:x}", hasher.finalize());

    if actual != expected {
        return Err(Error::ChecksumMismatch {
            expected: expected.to_string(),
            actual,
        });
    }

    debug!("Checksum verified: {}", expected);
    Ok(())
}
