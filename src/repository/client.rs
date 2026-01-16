// src/repository/client.rs

//! HTTP client for repository operations
//!
//! Provides a wrapper around reqwest with retry support for
//! fetching metadata and downloading files.

use crate::compression::{decompress_auto, CompressionFormat};
use crate::error::{Error, Result};
use indicatif::ProgressBar;
use reqwest::blocking::Client;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::metadata::RepositoryMetadata;

/// Default timeout for HTTP requests (30 seconds)
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retry attempts for failed downloads
const MAX_RETRIES: u32 = 3;

/// Retry delay in milliseconds
const RETRY_DELAY_MS: u64 = 1000;

/// Buffer size for streaming downloads (8 KB)
const STREAM_BUFFER_SIZE: usize = 8192;

/// Stream HTTP response to file with optional progress tracking
///
/// Always streams data in chunks, never buffering the entire response in memory.
/// This is safe for files of any size.
fn stream_response_to_file(
    mut response: reqwest::blocking::Response,
    file: &mut File,
    total_size: u64,
    progress_bar: Option<&ProgressBar>,
    display_name: &str,
) -> Result<u64> {
    // Set up progress bar if provided
    if let Some(pb) = progress_bar {
        if total_size > 0 {
            pb.set_length(total_size);
            pb.set_message(display_name.to_string());
        } else {
            // Unknown size - show bytes downloaded without percentage
            pb.set_message(format!("{} (unknown size)", display_name));
        }
    }

    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; STREAM_BUFFER_SIZE];

    loop {
        let bytes_read = response.read(&mut buffer).map_err(|e| {
            Error::IoError(format!("Failed to read response: {e}"))
        })?;

        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read]).map_err(|e| {
            Error::IoError(format!("Failed to write data: {e}"))
        })?;

        downloaded += bytes_read as u64;

        if let Some(pb) = progress_bar {
            pb.set_position(downloaded);
        }
    }

    Ok(downloaded)
}

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

    /// Fetch and decompress data from a URL
    ///
    /// Downloads the data and auto-detects the compression format from magic bytes.
    /// Supports gzip, xz, and zstd. Returns decompressed bytes.
    ///
    /// # Example
    /// ```ignore
    /// let client = RepositoryClient::new()?;
    /// let data = client.fetch_and_decompress("https://repo.example.com/Packages.gz")?;
    /// let content = String::from_utf8(data)?;
    /// ```
    pub fn fetch_and_decompress(&self, url: &str) -> Result<Vec<u8>> {
        debug!("Fetching and decompressing: {}", url);
        let bytes = self.download_to_bytes(url)?;

        // Auto-detect and decompress
        let decompressed = decompress_auto(&bytes).map_err(|e| {
            Error::ParseError(format!("Failed to decompress data from {}: {}", url, e))
        })?;

        debug!(
            "Decompressed {} bytes -> {} bytes",
            bytes.len(),
            decompressed.len()
        );
        Ok(decompressed)
    }

    /// Fetch and decompress data as a UTF-8 string
    ///
    /// Convenience method that decompresses and converts to String.
    pub fn fetch_and_decompress_string(&self, url: &str) -> Result<String> {
        let bytes = self.fetch_and_decompress(url)?;
        String::from_utf8(bytes).map_err(|e| {
            Error::ParseError(format!("Invalid UTF-8 in response from {}: {}", url, e))
        })
    }

    /// Fetch data, optionally decompressing based on URL extension
    ///
    /// Uses the URL extension to determine if decompression is needed.
    /// Use this when the URL clearly indicates the compression format.
    pub fn fetch_with_extension_hint(&self, url: &str) -> Result<Vec<u8>> {
        let bytes = self.download_to_bytes(url)?;

        let format = CompressionFormat::from_extension(url);
        if format == CompressionFormat::None {
            // No compression indicated, check magic bytes anyway
            let detected = CompressionFormat::from_magic_bytes(&bytes);
            if detected != CompressionFormat::None {
                debug!("URL {} has no extension but detected {} compression", url, detected);
                return decompress_auto(&bytes).map_err(|e| {
                    Error::ParseError(format!("Failed to decompress: {}", e))
                });
            }
            return Ok(bytes);
        }

        decompress_auto(&bytes).map_err(|e| {
            Error::ParseError(format!("Failed to decompress {} data: {}", format, e))
        })
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

                    // Stream response to file, optionally updating progress bar
                    let downloaded = stream_response_to_file(
                        response,
                        &mut file,
                        total_size,
                        progress_bar,
                        display_name,
                    )?;

                    if let Some(pb) = progress_bar {
                        pb.finish_with_message(format!("{} [done]", display_name));
                    }

                    info!("Downloaded {} bytes", downloaded);

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
