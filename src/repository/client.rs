// src/repository/client.rs

//! HTTP client for repository operations
//!
//! Provides a wrapper around reqwest with retry support for
//! fetching metadata and downloading files.

use crate::error::{Error, Result};
use indicatif::ProgressBar;
use reqwest::blocking::Client;
use std::fs::{self, File};
use std::io::{self, Read, Write};
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

    /// Download a URL to bytes (for signature files, keys, etc.)
    ///
    /// Returns the response body as bytes, or an error if the download fails.
    /// This method does NOT retry - if the URL returns 404, it returns an error immediately.
    pub fn download_to_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| Error::DownloadError(format!("Failed to fetch {}: {}", url, e)))?;

        if !response.status().is_success() {
            return Err(Error::DownloadError(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        let bytes = response
            .bytes()
            .map_err(|e| Error::DownloadError(format!("Failed to read response: {}", e)))?;

        Ok(bytes.to_vec())
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

    /// Download a file with progress bar display
    ///
    /// Shows a progress bar during download with the package name and download speed.
    /// Falls back to silent download if content-length is unknown.
    pub fn download_file_with_progress(
        &self,
        url: &str,
        dest_path: &Path,
        display_name: &str,
        progress_bar: Option<&ProgressBar>,
    ) -> Result<()> {
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
                Ok(response) => {
                    if !response.status().is_success() {
                        return Err(Error::DownloadError(format!(
                            "HTTP {} from {}",
                            response.status(),
                            url
                        )));
                    }

                    // Get content length for progress tracking
                    let total_size = response.content_length().unwrap_or(0);

                    // Write to temporary file first
                    let temp_path = dest_path.with_extension("tmp");
                    let mut file = File::create(&temp_path).map_err(|e| {
                        Error::IoError(format!("Failed to create file {}: {e}", temp_path.display()))
                    })?;

                    // If we have a progress bar and know the size, use chunked download
                    if let Some(pb) = progress_bar {
                        if total_size > 0 {
                            pb.set_length(total_size);
                            pb.set_message(display_name.to_string());

                            let mut downloaded: u64 = 0;
                            let mut reader = response;
                            let mut buffer = [0u8; 8192];

                            loop {
                                let bytes_read = reader.read(&mut buffer).map_err(|e| {
                                    Error::IoError(format!("Failed to read response: {e}"))
                                })?;

                                if bytes_read == 0 {
                                    break;
                                }

                                file.write_all(&buffer[..bytes_read]).map_err(|e| {
                                    Error::IoError(format!("Failed to write data: {e}"))
                                })?;

                                downloaded += bytes_read as u64;
                                pb.set_position(downloaded);
                            }

                            pb.finish_with_message(format!("{} [done]", display_name));
                        } else {
                            // Unknown size - use spinner mode
                            pb.set_message(format!("{} (unknown size)", display_name));
                            io::copy(&mut response.bytes().map_err(|e| {
                                Error::IoError(format!("Failed to read response: {e}"))
                            })?.as_ref(), &mut file).map_err(|e| {
                                Error::IoError(format!("Failed to write data: {e}"))
                            })?;
                            pb.finish_with_message(format!("{} [done]", display_name));
                        }
                    } else {
                        // No progress bar - use simple copy
                        io::copy(&mut response.bytes().map_err(|e| {
                            Error::IoError(format!("Failed to read response: {e}"))
                        })?.as_ref(), &mut file).map_err(|e| {
                            Error::IoError(format!("Failed to write data: {e}"))
                        })?;
                    }

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
