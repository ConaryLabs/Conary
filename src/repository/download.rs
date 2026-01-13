// src/repository/download.rs

//! Package and delta download functionality
//!
//! Functions for downloading packages and delta updates from repositories,
//! with checksum and GPG signature verification.

use crate::db::models::RepositoryPackage;
use crate::error::{Error, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use super::client::RepositoryClient;
use super::gpg::GpgVerifier;
use super::metadata::DeltaInfo;

/// Options for package download with GPG verification
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Whether to verify GPG signatures
    pub gpg_check: bool,
    /// When true, packages MUST have valid GPG signatures - missing signatures are errors
    pub gpg_strict: bool,
    /// Directory where GPG keys are stored
    pub keyring_dir: PathBuf,
    /// Name of the repository (for key lookup)
    pub repository_name: String,
}

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

/// Download a package with optional GPG signature verification
///
/// This function extends `download_package` with GPG signature checking.
/// When `options` is provided and `gpg_check` is true, it will:
/// 1. Download the package and verify its checksum
/// 2. Attempt to download and verify a detached signature (.sig or .asc)
///
/// # Failure Modes
/// - Invalid signature: Always fails (security boundary)
/// - Missing signature: Fails in strict mode, warns otherwise
/// - Missing key: Fails with actionable error message
///
/// # Strict Mode
/// When `gpg_strict` is true, missing signatures are treated as errors.
/// This ensures all packages from the repository have valid signatures.
pub fn download_package_verified(
    repo_pkg: &RepositoryPackage,
    dest_dir: &Path,
    options: Option<&DownloadOptions>,
) -> Result<PathBuf> {
    // First, download and verify checksum
    let dest_path = download_package(repo_pkg, dest_dir)?;

    // If GPG options provided and gpg_check enabled, verify signature
    if let Some(opts) = options
        && opts.gpg_check
    {
        match verify_package_signature(&dest_path, &repo_pkg.download_url, opts) {
            Ok(()) => {
                info!("GPG signature verified for {}", repo_pkg.name);
            }
            Err(Error::NotFoundError(msg)) if msg.contains("No signature file") => {
                // Signature not found - strict mode fails, otherwise warn
                if opts.gpg_strict {
                    return Err(Error::GpgVerificationFailed(format!(
                        "GPG signature required but not found for '{}' (strict mode enabled).\n\
                         The repository '{}' requires all packages to have valid GPG signatures.\n\
                         Either provide a signed package or disable strict mode.",
                        repo_pkg.name, opts.repository_name
                    )));
                }
                warn!("No GPG signature found for {} ({})", repo_pkg.name, msg);
            }
            Err(Error::NotFoundError(msg)) if msg.contains("GPG key not found") => {
                // Key not imported - fail with helpful message
                return Err(Error::GpgVerificationFailed(format!(
                    "GPG verification failed for '{}': {}.\n\
                     To fix this, run:\n  \
                     conary key-import {} <key-url-or-file>\n\
                     Or disable GPG checking for this repository.",
                    repo_pkg.name, msg, opts.repository_name
                )));
            }
            Err(e) => {
                // Other errors (invalid signature, etc.) - fail
                return Err(e);
            }
        }
    }

    Ok(dest_path)
}

/// Verify GPG signature for a downloaded package
///
/// Attempts to download detached signature files (.sig, .asc) and verify
/// them against the imported GPG key for the repository.
fn verify_package_signature(
    package_path: &Path,
    download_url: &str,
    options: &DownloadOptions,
) -> Result<()> {
    debug!(
        "Verifying GPG signature for {:?} from repository '{}'",
        package_path, options.repository_name
    );

    // Create verifier
    let verifier = GpgVerifier::new(options.keyring_dir.clone())?;

    // Check if we have a key for this repository
    if !verifier.has_key(&options.repository_name) {
        return Err(Error::NotFoundError(format!(
            "GPG key not found for repository '{}'",
            options.repository_name
        )));
    }

    // Try to download signature file (try .sig first, then .asc)
    let client = RepositoryClient::new()?;
    let signature_extensions = [".sig", ".asc"];

    for ext in &signature_extensions {
        let sig_url = format!("{}{}", download_url, ext);
        debug!("Trying to download signature from: {}", sig_url);

        // Try to download signature
        match client.download_to_bytes(&sig_url) {
            Ok(sig_data) => {
                // Save signature to temp file
                let sig_path = package_path.with_extension(
                    package_path
                        .extension()
                        .map(|e| format!("{}{}", e.to_string_lossy(), ext))
                        .unwrap_or_else(|| ext[1..].to_string()),
                );

                std::fs::write(&sig_path, &sig_data).map_err(|e| {
                    Error::IoError(format!("Failed to write signature file: {}", e))
                })?;

                // Verify signature
                let result =
                    verifier.verify_signature(package_path, &sig_path, &options.repository_name);

                // Clean up signature file
                let _ = std::fs::remove_file(&sig_path);

                return result;
            }
            Err(_) => {
                // Try next extension
                debug!("Signature not found at {}", sig_url);
                continue;
            }
        }
    }

    // No signature file found
    Err(Error::NotFoundError(
        "No signature file found (.sig or .asc)".to_string(),
    ))
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

// =============================================================================
// Progress-aware download functions
// =============================================================================

/// Create a styled progress bar for package downloads
fn create_progress_bar(size: u64, name: &str) -> ProgressBar {
    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) {msg}")
            .expect("Invalid progress bar template")
            .progress_chars("#>-"),
    );
    pb.set_message(name.to_string());
    pb
}

/// Download a package with progress bar display
///
/// Shows download progress including bytes downloaded, speed, and package name.
pub fn download_package_with_progress(
    repo_pkg: &RepositoryPackage,
    dest_dir: &Path,
    progress_bar: Option<&ProgressBar>,
) -> Result<PathBuf> {
    let client = RepositoryClient::new()?;

    // Construct destination path
    let default_filename = format!("{}-{}.rpm", repo_pkg.name, repo_pkg.version);
    let filename = repo_pkg
        .download_url
        .split('/')
        .next_back()
        .unwrap_or(&default_filename);

    let dest_path = dest_dir.join(filename);

    // Download the file with progress
    client.download_file_with_progress(
        &repo_pkg.download_url,
        &dest_path,
        &repo_pkg.name,
        progress_bar,
    )?;

    // Verify checksum
    verify_checksum(&dest_path, &repo_pkg.checksum)?;

    Ok(dest_path)
}

/// Download a package with progress and optional GPG verification
///
/// Combines progress display with GPG signature verification.
/// See [`download_package_verified`] for details on strict mode behavior.
pub fn download_package_verified_with_progress(
    repo_pkg: &RepositoryPackage,
    dest_dir: &Path,
    options: Option<&DownloadOptions>,
    progress_bar: Option<&ProgressBar>,
) -> Result<PathBuf> {
    // Download with progress
    let dest_path = download_package_with_progress(repo_pkg, dest_dir, progress_bar)?;

    // If GPG options provided and gpg_check enabled, verify signature
    if let Some(opts) = options
        && opts.gpg_check
    {
        match verify_package_signature(&dest_path, &repo_pkg.download_url, opts) {
            Ok(()) => {
                info!("GPG signature verified for {}", repo_pkg.name);
            }
            Err(Error::NotFoundError(msg)) if msg.contains("No signature file") => {
                // Signature not found - strict mode fails, otherwise warn
                if opts.gpg_strict {
                    return Err(Error::GpgVerificationFailed(format!(
                        "GPG signature required but not found for '{}' (strict mode enabled).\n\
                         The repository '{}' requires all packages to have valid GPG signatures.\n\
                         Either provide a signed package or disable strict mode.",
                        repo_pkg.name, opts.repository_name
                    )));
                }
                warn!("No GPG signature found for {} ({})", repo_pkg.name, msg);
            }
            Err(Error::NotFoundError(msg)) if msg.contains("GPG key not found") => {
                return Err(Error::GpgVerificationFailed(format!(
                    "GPG verification failed for '{}': {}.\n\
                     To fix this, run:\n  \
                     conary key-import {} <key-url-or-file>\n\
                     Or disable GPG checking for this repository.",
                    repo_pkg.name, msg, opts.repository_name
                )));
            }
            Err(e) => {
                return Err(e);
            }
        }
    }

    Ok(dest_path)
}

/// Multi-progress manager for parallel downloads
///
/// Provides a wrapper around indicatif's MultiProgress for managing
/// multiple concurrent download progress bars.
pub struct DownloadProgress {
    multi: MultiProgress,
}

impl DownloadProgress {
    /// Create a new multi-progress manager
    pub fn new() -> Self {
        Self {
            multi: MultiProgress::new(),
        }
    }

    /// Create a progress bar for a package download
    ///
    /// The progress bar is automatically added to the multi-progress display.
    pub fn add_download(&self, name: &str, size: u64) -> ProgressBar {
        let pb = create_progress_bar(size, name);
        self.multi.add(pb)
    }

    /// Create a spinner for downloads with unknown size
    pub fn add_spinner(&self, name: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {bytes} ({bytes_per_sec}) {msg}")
                .expect("Invalid spinner template"),
        );
        pb.set_message(name.to_string());
        self.multi.add(pb)
    }

    /// Mark a download as complete with success message
    pub fn finish_download(pb: &ProgressBar, name: &str) {
        pb.finish_with_message(format!("{} [done]", name));
    }

    /// Mark a download as failed
    pub fn fail_download(pb: &ProgressBar, name: &str, error: &str) {
        pb.abandon_with_message(format!("{} [FAILED: {}]", name, error));
    }
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self::new()
    }
}
