// src/repository/download.rs

//! Package and delta download functionality
//!
//! Functions for downloading packages and delta updates from repositories,
//! with checksum and GPG signature verification.
//!
//! # Architecture
//!
//! All public download functions delegate to [`download_package_inner`], which handles:
//! - Filename construction from URL
//! - Download (with optional progress tracking)
//! - Checksum verification with cleanup on failure
//! - Optional GPG signature verification
//!
//! The public functions are thin wrappers providing ergonomic APIs.

use crate::db::models::RepositoryPackage;
use crate::error::{Error, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
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

// =============================================================================
// Core download implementation
// =============================================================================

/// Internal unified download function
///
/// All public download functions delegate to this implementation.
/// Handles filename construction, download, checksum verification, and GPG verification.
fn download_package_inner(
    repo_pkg: &RepositoryPackage,
    dest_dir: &Path,
    options: Option<&DownloadOptions>,
    progress: Option<&ProgressBar>,
) -> Result<PathBuf> {
    let client = RepositoryClient::new()?;

    // Construct destination path from URL filename or generate default
    let dest_path = construct_dest_path(repo_pkg, dest_dir);

    // Download the file (with or without progress)
    if let Some(pb) = progress {
        client.download_file_with_progress(
            &repo_pkg.download_url,
            &dest_path,
            &repo_pkg.name,
            Some(pb),
        )?;
    } else {
        client.download_file(&repo_pkg.download_url, &dest_path)?;
    }

    // Verify checksum - clean up invalid file on failure
    if let Err(e) = verify_checksum(&dest_path, &repo_pkg.checksum) {
        let _ = std::fs::remove_file(&dest_path);
        return Err(e);
    }

    // GPG verification if enabled
    if let Some(opts) = options {
        verify_gpg_signature(repo_pkg, &dest_path, opts)?;
    }

    Ok(dest_path)
}

/// Construct destination path from package info
fn construct_dest_path(repo_pkg: &RepositoryPackage, dest_dir: &Path) -> PathBuf {
    let default_filename = format!("{}-{}.rpm", repo_pkg.name, repo_pkg.version);
    let filename = repo_pkg
        .download_url
        .split('/')
        .next_back()
        .unwrap_or(&default_filename);
    dest_dir.join(filename)
}

/// Verify GPG signature with appropriate error handling
fn verify_gpg_signature(
    repo_pkg: &RepositoryPackage,
    dest_path: &Path,
    opts: &DownloadOptions,
) -> Result<()> {
    if !opts.gpg_check {
        return Ok(());
    }

    match verify_package_signature(dest_path, &repo_pkg.download_url, opts) {
        Ok(()) => {
            info!("GPG signature verified for {}", repo_pkg.name);
            Ok(())
        }
        Err(Error::NotFoundError(msg)) if msg.contains("No signature file") => {
            if opts.gpg_strict {
                Err(Error::GpgVerificationFailed(format!(
                    "GPG signature required but not found for '{}' (strict mode enabled).\n\
                     The repository '{}' requires all packages to have valid GPG signatures.\n\
                     Either provide a signed package or disable strict mode.",
                    repo_pkg.name, opts.repository_name
                )))
            } else {
                warn!("No GPG signature found for {} ({})", repo_pkg.name, msg);
                Ok(())
            }
        }
        Err(Error::NotFoundError(msg)) if msg.contains("GPG key not found") => {
            Err(Error::GpgVerificationFailed(format!(
                "GPG verification failed for '{}': {}.\n\
                 To fix this, run:\n  \
                 conary key-import {} <key-url-or-file>\n\
                 Or disable GPG checking for this repository.",
                repo_pkg.name, msg, opts.repository_name
            )))
        }
        Err(e) => Err(e),
    }
}

// =============================================================================
// Public API (thin wrappers around download_package_inner)
// =============================================================================

/// Download a package from a repository
///
/// Downloads the package and verifies its checksum against the trusted metadata.
/// If verification fails, the corrupted/invalid file is removed before returning
/// the error to prevent cache pollution.
pub fn download_package(repo_pkg: &RepositoryPackage, dest_dir: &Path) -> Result<PathBuf> {
    download_package_inner(repo_pkg, dest_dir, None, None)
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
    download_package_inner(repo_pkg, dest_dir, options, None)
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
///
/// Uses the shared hash module for consistent SHA-256 verification.
pub fn verify_checksum(path: &Path, expected: &str) -> Result<()> {
    debug!("Verifying checksum for {}", path.display());

    crate::hash::verify_file_sha256(path, expected).map_err(|e| Error::ChecksumMismatch {
        expected: e.expected,
        actual: e.actual,
    })?;

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
/// If checksum verification fails, the invalid file is removed before returning
/// the error to prevent cache pollution.
pub fn download_package_with_progress(
    repo_pkg: &RepositoryPackage,
    dest_dir: &Path,
    progress_bar: Option<&ProgressBar>,
) -> Result<PathBuf> {
    download_package_inner(repo_pkg, dest_dir, None, progress_bar)
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
    download_package_inner(repo_pkg, dest_dir, options, progress_bar)
}

/// Multi-progress manager for parallel downloads
///
/// Provides a wrapper around indicatif's MultiProgress for managing
/// multiple concurrent download progress bars, with aggregate statistics.
pub struct DownloadProgress {
    multi: MultiProgress,
    /// Overall progress bar showing total bytes
    overall: Option<ProgressBar>,
    /// Total size in bytes
    total_size: u64,
}

impl DownloadProgress {
    /// Create a new multi-progress manager
    pub fn new() -> Self {
        Self {
            multi: MultiProgress::new(),
            overall: None,
            total_size: 0,
        }
    }

    /// Create a new multi-progress manager with aggregate tracking
    ///
    /// Shows an overall progress bar tracking total bytes downloaded
    /// across all packages.
    pub fn with_aggregate(package_count: usize, total_size: u64) -> Self {
        let multi = MultiProgress::new();

        // Create overall progress bar
        let overall = ProgressBar::new(total_size);
        overall.set_style(
            ProgressStyle::default_bar()
                .template("Total: [{bar:40.green/dim}] {bytes}/{total_bytes} ({bytes_per_sec}) - {msg}")
                .expect("Invalid progress bar template")
                .progress_chars("=>-"),
        );
        overall.set_message(format!("0/{} packages", package_count));

        let overall_bar = multi.add(overall);

        Self {
            multi,
            overall: Some(overall_bar),
            total_size,
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

    /// Update the overall progress with bytes downloaded
    pub fn update_overall(&self, bytes: u64, completed: usize, total: usize) {
        if let Some(ref overall) = self.overall {
            overall.set_position(bytes);
            overall.set_message(format!("{}/{} packages", completed, total));
        }
    }

    /// Mark a download as complete with success message
    pub fn finish_download(pb: &ProgressBar, name: &str) {
        pb.finish_with_message(format!("{} [done]", name));
    }

    /// Mark a download as failed
    pub fn fail_download(pb: &ProgressBar, name: &str, error: &str) {
        pb.abandon_with_message(format!("{} [FAILED: {}]", name, error));
    }

    /// Finish all downloads and show summary
    pub fn finish_all(&self, succeeded: usize, failed: usize, total_bytes: u64) {
        if let Some(ref overall) = self.overall {
            let mb = total_bytes as f64 / 1_048_576.0;
            if failed > 0 {
                overall.finish_with_message(format!(
                    "{} succeeded, {} failed ({:.2} MB)",
                    succeeded, failed, mb
                ));
            } else {
                overall.finish_with_message(format!("{} packages ({:.2} MB)", succeeded, mb));
            }
        }
    }

    /// Get total configured size
    pub fn total_size(&self) -> u64 {
        self.total_size
    }
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self::new()
    }
}
