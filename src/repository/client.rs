// src/repository/client.rs

//! HTTP client for repository operations
//!
//! Provides a wrapper around reqwest with retry support for
//! fetching metadata and downloading files.

use crate::error::{Error, Result};
use reqwest::blocking::Client;
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

use super::metadata::RepositoryMetadata;

/// Default timeout for HTTP requests (30 seconds)
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retry attempts for failed downloads
const MAX_RETRIES: u32 = 3;

/// Retry delay in milliseconds
const RETRY_DELAY_MS: u64 = 1000;

/// HTTP client wrapper with retry support
pub struct RepositoryClient {
    client: Client,
    max_retries: u32,
}

impl RepositoryClient {
    /// Create a new repository client
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            max_retries: MAX_RETRIES,
        })
    }

    /// Get a reference to the inner HTTP client
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Fetch repository metadata from URL with retry support
    pub fn fetch_metadata(&self, url: &str) -> Result<RepositoryMetadata> {
        let metadata_url = if url.ends_with('/') {
            format!("{url}metadata.json")
        } else {
            format!("{url}/metadata.json")
        };

        info!("Fetching repository metadata from {}", metadata_url);

        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.client.get(&metadata_url).send() {
                Ok(response) => {
                    if !response.status().is_success() {
                        return Err(Error::DownloadError(format!(
                            "HTTP {} from {}",
                            response.status(),
                            metadata_url
                        )));
                    }

                    let metadata: RepositoryMetadata = response.json().map_err(|e| {
                        Error::DownloadError(format!("Failed to parse metadata JSON: {e}"))
                    })?;

                    info!("Successfully fetched metadata for {} packages", metadata.packages.len());
                    return Ok(metadata);
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(Error::DownloadError(format!(
                            "Failed to fetch metadata after {attempt} attempts: {e}"
                        )));
                    }
                    warn!("Metadata fetch attempt {} failed: {}, retrying...", attempt, e);
                    std::thread::sleep(Duration::from_millis(RETRY_DELAY_MS * attempt as u64));
                }
            }
        }
    }

    /// Download a file to the specified path with retry support
    pub fn download_file(&self, url: &str, dest_path: &Path) -> Result<()> {
        info!("Downloading {} to {}", url, dest_path.display());

        // Create parent directory if it doesn't exist
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::IoError(format!("Failed to create directory {}: {e}", parent.display()))
            })?;
        }

        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.client.get(url).send() {
                Ok(mut response) => {
                    if !response.status().is_success() {
                        return Err(Error::DownloadError(format!(
                            "HTTP {} from {}",
                            response.status(),
                            url
                        )));
                    }

                    // Write to temporary file first
                    let temp_path = dest_path.with_extension("tmp");
                    let mut file = File::create(&temp_path).map_err(|e| {
                        Error::IoError(format!("Failed to create file {}: {e}", temp_path.display()))
                    })?;

                    // Copy response body to file
                    io::copy(&mut response, &mut file).map_err(|e| {
                        Error::IoError(format!("Failed to write downloaded data: {e}"))
                    })?;

                    // Atomic rename from temp to final destination
                    fs::rename(&temp_path, dest_path).map_err(|e| {
                        Error::IoError(format!(
                            "Failed to move {} to {}: {e}",
                            temp_path.display(),
                            dest_path.display()
                        ))
                    })?;

                    info!("Successfully downloaded to {}", dest_path.display());
                    return Ok(());
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(Error::DownloadError(format!(
                            "Failed to download after {attempt} attempts: {e}"
                        )));
                    }
                    warn!("Download attempt {} failed: {}, retrying...", attempt, e);
                    std::thread::sleep(Duration::from_millis(RETRY_DELAY_MS * attempt as u64));
                }
            }
        }
    }
}
