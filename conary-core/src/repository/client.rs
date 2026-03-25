// conary-core/src/repository/client.rs

//! HTTP client for repository operations
//!
//! Provides a wrapper around reqwest with retry support for
//! fetching metadata and downloading files.

use crate::compression::{CompressionFormat, decompress_auto};
use crate::error::{Error, Result};
use crate::repository::error_helpers::ResultExt;
use indicatif::ProgressBar;
use reqwest::Client;
use reqwest::header;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::metadata::RepositoryMetadata;
use super::retry::RetryConfig;

/// Timeout configuration for different operation types.
///
/// Metadata operations (repo index, package info) use a shorter timeout
/// since they transfer small payloads. File downloads use a longer timeout
/// to accommodate large packages over slow connections.
#[derive(Debug, Clone)]
pub struct TimeoutConfig {
    /// Timeout for metadata requests (default 30s)
    pub metadata: Duration,
    /// Timeout for file/package downloads (default 300s)
    pub download: Duration,
    /// Connection establishment timeout (default 30s)
    pub connect: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            metadata: Duration::from_secs(30),
            download: Duration::from_secs(300),
            connect: Duration::from_secs(30),
        }
    }
}

/// Maximum response size for in-memory downloads (256 MB)
///
/// Fedora metadata can exceed 100 MB once Remi includes enough native
/// package metadata for capability-aware dependency resolution.
const MAX_BYTES_RESPONSE_SIZE: u64 = 256 * 1024 * 1024;

/// Validate that a URL uses an allowed scheme (HTTP or HTTPS only).
///
/// Rejects file://, gopher://, and other non-HTTP schemes to prevent SSRF.
pub fn validate_url_scheme(url: &str) -> Result<()> {
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(())
    } else {
        Err(Error::ConfigError(format!(
            "URL must use http:// or https:// scheme: {}",
            url
        )))
    }
}

/// Stream HTTP response to file with optional progress tracking
///
/// Always streams data in chunks, never buffering the entire response in memory.
/// This is safe for files of any size.
///
/// The `offset` parameter indicates how many bytes were already written (for resumed
/// downloads). The progress bar position starts from `offset` so the user sees
/// correct overall progress.
async fn stream_response_to_file(
    mut response: reqwest::Response,
    file: &mut File,
    total_size: u64,
    offset: u64,
    progress_bar: Option<&ProgressBar>,
    display_name: &str,
) -> Result<u64> {
    // Set up progress bar if provided
    if let Some(pb) = progress_bar {
        if total_size > 0 {
            pb.set_length(total_size);
            pb.set_position(offset);
            pb.set_message(display_name.to_string());
        } else {
            // Unknown size - show bytes downloaded without percentage
            pb.set_message(format!("{} (unknown size)", display_name));
        }
    }

    let mut downloaded: u64 = offset;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::DownloadError(format!("read response stream: {e}")))?
    {
        file.write_all(&chunk).io_context("write download data")?;

        downloaded += chunk.len() as u64;

        if let Some(pb) = progress_bar {
            pb.set_position(downloaded);
        }
    }

    Ok(downloaded)
}

/// Check if an HTTP status code represents a transient server error
/// that should be retried.
fn is_transient_error(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::BAD_GATEWAY
            | reqwest::StatusCode::SERVICE_UNAVAILABLE
            | reqwest::StatusCode::GATEWAY_TIMEOUT
            | reqwest::StatusCode::TOO_MANY_REQUESTS
    )
}

/// HTTP client wrapper with retry support
pub struct RepositoryClient {
    client: Client,
    retry_policy: RetryConfig,
    timeouts: TimeoutConfig,
}

impl RepositoryClient {
    /// Create a new repository client with default timeouts
    pub fn new() -> Result<Self> {
        Self::with_timeouts(TimeoutConfig::default())
    }

    /// Create a new repository client with custom timeouts
    pub fn with_timeouts(timeouts: TimeoutConfig) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(timeouts.connect)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            retry_policy: RetryConfig::default(),
            timeouts,
        })
    }

    /// Set a custom retry config (builder pattern)
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryConfig) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Get a reference to the inner HTTP client
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Fetch repository metadata from URL with retry support
    pub async fn fetch_metadata(&self, url: &str) -> Result<RepositoryMetadata> {
        validate_url_scheme(url)?;
        let metadata_url = if url.ends_with('/') {
            format!("{url}metadata.json")
        } else {
            format!("{url}/metadata.json")
        };

        info!("Fetching repository metadata from {}", metadata_url);

        let mut attempt = 0;
        loop {
            attempt += 1;
            match self
                .client
                .get(&metadata_url)
                .timeout(self.timeouts.metadata)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();

                    if is_transient_error(status) {
                        if attempt >= self.retry_policy.max_attempts {
                            return Err(Error::DownloadError(format!(
                                "HTTP {} from {} after {attempt} attempts",
                                status, metadata_url
                            )));
                        }
                        warn!(
                            "Metadata fetch attempt {} got HTTP {}, retrying...",
                            attempt, status
                        );
                        tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                        continue;
                    }

                    if !status.is_success() {
                        return Err(Error::DownloadError(format!(
                            "HTTP {} from {}",
                            status, metadata_url
                        )));
                    }

                    // Route through bounded download to enforce the 256 MB
                    // size cap, then deserialize from the bounded bytes.
                    // Using response.json() directly would buffer unbounded.
                    if let Some(content_length) = response.content_length()
                        && content_length > MAX_BYTES_RESPONSE_SIZE
                    {
                        return Err(Error::DownloadError(format!(
                            "Metadata response too large ({} bytes, max {}): {}",
                            content_length, MAX_BYTES_RESPONSE_SIZE, metadata_url
                        )));
                    }

                    let bytes = response.bytes().await.map_err(|e| {
                        Error::DownloadError(format!("Failed to read metadata response: {e}"))
                    })?;

                    if bytes.len() as u64 > MAX_BYTES_RESPONSE_SIZE {
                        return Err(Error::DownloadError(format!(
                            "Metadata response body too large ({} bytes, max {}): {}",
                            bytes.len(),
                            MAX_BYTES_RESPONSE_SIZE,
                            metadata_url
                        )));
                    }

                    let metadata: RepositoryMetadata =
                        serde_json::from_slice(&bytes).map_err(|e| {
                            Error::DownloadError(format!("Failed to parse metadata JSON: {e}"))
                        })?;

                    info!(
                        "Successfully fetched metadata for {} packages",
                        metadata.packages.len()
                    );
                    return Ok(metadata);
                }
                Err(e) => {
                    if attempt >= self.retry_policy.max_attempts {
                        return Err(Error::DownloadError(format!(
                            "Failed to fetch metadata after {attempt} attempts: {e}"
                        )));
                    }
                    warn!(
                        "Metadata fetch attempt {} failed: {}, retrying...",
                        attempt, e
                    );
                    tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                }
            }
        }
    }

    /// Download a URL to bytes (for signature files, keys, etc.)
    ///
    /// Returns the response body as bytes, or an error if the download fails.
    /// This method does NOT retry - if the URL returns 404, it returns an error immediately.
    pub async fn download_to_bytes(&self, url: &str) -> Result<Vec<u8>> {
        validate_url_scheme(url)?;

        let response = self
            .client
            .get(url)
            .timeout(self.timeouts.metadata)
            .send()
            .await
            .download_context(url)?;

        if !response.status().is_success() {
            return Err(Error::DownloadError(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        // Check Content-Length if available to reject oversized responses early
        if let Some(content_length) = response.content_length()
            && content_length > MAX_BYTES_RESPONSE_SIZE
        {
            return Err(Error::DownloadError(format!(
                "Response too large ({} bytes, max {}): {}",
                content_length, MAX_BYTES_RESPONSE_SIZE, url
            )));
        }

        let bytes = response.bytes().await.download_context(url)?;

        if bytes.len() as u64 > MAX_BYTES_RESPONSE_SIZE {
            return Err(Error::DownloadError(format!(
                "Response body too large ({} bytes, max {}): {}",
                bytes.len(),
                MAX_BYTES_RESPONSE_SIZE,
                url
            )));
        }

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
    pub async fn fetch_and_decompress(&self, url: &str) -> Result<Vec<u8>> {
        debug!("Fetching and decompressing: {}", url);
        let bytes = self.download_to_bytes(url).await?;

        // Auto-detect and decompress
        let decompressed =
            decompress_auto(&bytes).parse_context(&format!("decompress data from {url}"))?;

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
    pub async fn fetch_and_decompress_string(&self, url: &str) -> Result<String> {
        let bytes = self.fetch_and_decompress(url).await?;
        String::from_utf8(bytes).parse_context(&format!("UTF-8 from {url}"))
    }

    /// Fetch data, optionally decompressing based on URL extension
    ///
    /// Uses the URL extension to determine if decompression is needed.
    /// Use this when the URL clearly indicates the compression format.
    pub async fn fetch_with_extension_hint(&self, url: &str) -> Result<Vec<u8>> {
        let bytes = self.download_to_bytes(url).await?;

        let format = CompressionFormat::from_extension(url);
        if format == CompressionFormat::None {
            // No compression indicated, check magic bytes anyway
            let detected = CompressionFormat::from_magic_bytes(&bytes);
            if detected != CompressionFormat::None {
                debug!(
                    "URL {} has no extension but detected {} compression",
                    url, detected
                );
                return decompress_auto(&bytes)
                    .map_err(|e| Error::ParseError(format!("Failed to decompress: {}", e)));
            }
            return Ok(bytes);
        }

        decompress_auto(&bytes)
            .map_err(|e| Error::ParseError(format!("Failed to decompress {} data: {}", format, e)))
    }

    /// Download a file to the specified path with retry support
    pub async fn download_file(&self, url: &str, dest_path: &Path) -> Result<()> {
        self.download_file_with_progress(url, dest_path, "", None)
            .await
    }

    /// Download a file with optional progress bar display
    ///
    /// Shows a progress bar during download with the package name and download speed.
    /// Falls back to silent download if content-length is unknown or no progress bar is provided.
    ///
    /// Supports resumable downloads: if a `.tmp` file already exists from a previous
    /// interrupted download, sends a `Range` header to resume from where it left off.
    pub async fn download_file_with_progress(
        &self,
        url: &str,
        dest_path: &Path,
        display_name: &str,
        progress_bar: Option<&ProgressBar>,
    ) -> Result<()> {
        validate_url_scheme(url)?;
        info!("Downloading {} to {}", url, dest_path.display());

        // Create parent directory if it doesn't exist
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::IoError(format!(
                    "Failed to create directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let temp_path = dest_path.with_extension("tmp");

        let mut attempt = 0;
        loop {
            attempt += 1;

            // Check for existing partial download
            let existing_len = fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0);

            let mut request = self.client.get(url).timeout(self.timeouts.download);
            if existing_len > 0 {
                debug!(
                    "Found partial download ({} bytes), requesting resume",
                    existing_len
                );
                request = request.header(header::RANGE, format!("bytes={}-", existing_len));
            }

            match request.send().await {
                Ok(response) => {
                    let status = response.status();

                    if is_transient_error(status) {
                        if attempt >= self.retry_policy.max_attempts {
                            return Err(Error::DownloadError(format!(
                                "HTTP {} from {} after {attempt} attempts",
                                status, url
                            )));
                        }
                        warn!(
                            "Download attempt {} got HTTP {}, retrying...",
                            attempt, status
                        );
                        tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                        continue;
                    }

                    // HTTP 416 Range Not Satisfiable - file is already complete
                    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                        if existing_len > 0 {
                            debug!(
                                "Server returned 416, partial file ({} bytes) is already complete",
                                existing_len
                            );
                            if let Err(e) = fs::rename(&temp_path, dest_path) {
                                let _ = fs::remove_file(&temp_path);
                                return Err(Error::IoError(format!(
                                    "Failed to move {} to {}: {e}",
                                    temp_path.display(),
                                    dest_path.display()
                                )));
                            }
                            info!("Successfully downloaded to {}", dest_path.display());
                            return Ok(());
                        }
                        return Err(Error::DownloadError(format!(
                            "HTTP 416 from {} with no partial file",
                            url
                        )));
                    }

                    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
                        return Err(Error::DownloadError(format!(
                            "HTTP {} from {}",
                            status, url
                        )));
                    }

                    // Determine resume vs fresh download
                    let (mut file, offset, total_size) =
                        if status == reqwest::StatusCode::PARTIAL_CONTENT && existing_len > 0 {
                            // Server supports range requests - append to existing file
                            let content_range_total = response
                                .headers()
                                .get(header::CONTENT_RANGE)
                                .and_then(|v| v.to_str().ok())
                                .and_then(|s| {
                                    // Parse "bytes START-END/TOTAL"
                                    s.rsplit('/').next().and_then(|t| t.parse::<u64>().ok())
                                })
                                .unwrap_or(0);
                            debug!(
                                "Resuming download from byte {}, total size {}",
                                existing_len, content_range_total
                            );
                            let file =
                                OpenOptions::new()
                                    .append(true)
                                    .open(&temp_path)
                                    .map_err(|e| {
                                        Error::IoError(format!(
                                            "Failed to open {} for append: {e}",
                                            temp_path.display()
                                        ))
                                    })?;
                            (file, existing_len, content_range_total)
                        } else {
                            // HTTP 200 - server does not support range, or fresh download.
                            // Truncate any existing partial file.
                            let total = response.content_length().unwrap_or(0);
                            let file = File::create(&temp_path).map_err(|e| {
                                Error::IoError(format!(
                                    "Failed to create file {}: {e}",
                                    temp_path.display()
                                ))
                            })?;
                            (file, 0, total)
                        };

                    // Stream response to file, optionally updating progress bar
                    let downloaded = stream_response_to_file(
                        response,
                        &mut file,
                        total_size,
                        offset,
                        progress_bar,
                        display_name,
                    )
                    .await?;

                    if let Some(pb) = progress_bar {
                        pb.finish_with_message(format!("{} [done]", display_name));
                    }

                    info!("Downloaded {} bytes", downloaded);

                    // Atomic rename from temp to final destination
                    if let Err(e) = fs::rename(&temp_path, dest_path) {
                        let _ = fs::remove_file(&temp_path);
                        return Err(Error::IoError(format!(
                            "Failed to move {} to {}: {e}",
                            temp_path.display(),
                            dest_path.display()
                        )));
                    }

                    info!("Successfully downloaded to {}", dest_path.display());
                    return Ok(());
                }
                Err(e) => {
                    if attempt >= self.retry_policy.max_attempts {
                        return Err(Error::DownloadError(format!(
                            "Failed to download after {attempt} attempts: {e}"
                        )));
                    }
                    warn!("Download attempt {} failed: {}, retrying...", attempt, e);
                    tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryConfig::default();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.base_delay, Duration::from_secs(1));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert!((policy.jitter_factor - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn test_retry_policy_exponential_backoff_no_jitter() {
        let policy = RetryConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            jitter_factor: 0.0,
        };

        // attempt 1: 100ms * 2^0 = 100ms
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(100));
        // attempt 2: 100ms * 2^1 = 200ms
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(200));
        // attempt 3: 100ms * 2^2 = 400ms
        assert_eq!(policy.delay_for_attempt(3), Duration::from_millis(400));
        // attempt 4: 100ms * 2^3 = 800ms
        assert_eq!(policy.delay_for_attempt(4), Duration::from_millis(800));
        // attempt 5: 100ms * 2^4 = 1600ms
        assert_eq!(policy.delay_for_attempt(5), Duration::from_millis(1600));
    }

    #[test]
    fn test_retry_policy_max_delay_cap() {
        let policy = RetryConfig {
            max_attempts: 10,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            jitter_factor: 0.0,
        };

        // attempt 1: 1s
        assert_eq!(policy.delay_for_attempt(1), Duration::from_secs(1));
        // attempt 2: 2s
        assert_eq!(policy.delay_for_attempt(2), Duration::from_secs(2));
        // attempt 3: 4s
        assert_eq!(policy.delay_for_attempt(3), Duration::from_secs(4));
        // attempt 4: would be 8s, but capped at 5s
        assert_eq!(policy.delay_for_attempt(4), Duration::from_secs(5));
        // attempt 10: still capped at 5s
        assert_eq!(policy.delay_for_attempt(10), Duration::from_secs(5));
    }

    #[test]
    fn test_retry_policy_jitter_within_bounds() {
        let policy = RetryConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(60),
            jitter_factor: 0.5,
        };

        // Run multiple times to check jitter stays within bounds
        for _ in 0..100 {
            let delay = policy.delay_for_attempt(1);
            // Base is 1000ms, jitter up to 50% = 500ms, so range is [1000, 1500]
            assert!(delay >= Duration::from_millis(1000));
            assert!(delay <= Duration::from_millis(1500));
        }

        for _ in 0..100 {
            let delay = policy.delay_for_attempt(3);
            // Base is 4000ms, jitter up to 50% = 2000ms, so range is [4000, 6000]
            assert!(delay >= Duration::from_millis(4000));
            assert!(delay <= Duration::from_millis(6000));
        }
    }

    #[test]
    fn test_retry_policy_attempt_zero_saturates() {
        let policy = RetryConfig {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            jitter_factor: 0.0,
        };

        // attempt 0 should not panic (saturating_sub handles it)
        let delay = policy.delay_for_attempt(0);
        assert_eq!(delay, Duration::from_millis(100));
    }

    #[test]
    fn test_retry_policy_large_attempt_no_overflow() {
        let policy = RetryConfig {
            max_attempts: 100,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            jitter_factor: 0.0,
        };

        // Very large attempt should not panic, just cap at max_delay
        let delay = policy.delay_for_attempt(64);
        assert_eq!(delay, Duration::from_secs(60));

        let delay = policy.delay_for_attempt(100);
        assert_eq!(delay, Duration::from_secs(60));
    }

    #[test]
    fn test_repository_client_with_retry_policy() {
        let policy = RetryConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(15),
            jitter_factor: 0.1,
        };

        let client = RepositoryClient::new()
            .unwrap()
            .with_retry_policy(policy.clone());

        assert_eq!(client.retry_policy.max_attempts, 5);
        assert_eq!(client.retry_policy.base_delay, Duration::from_millis(500));
    }
}
