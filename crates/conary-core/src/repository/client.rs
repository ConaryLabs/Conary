// conary-core/src/repository/client.rs

//! HTTP client for repository operations
//!
//! Provides a wrapper around reqwest with retry support for
//! fetching metadata and downloading files.

use crate::compression::{CompressionFormat, decompress_metadata, decompress_metadata_auto};
use crate::error::{Error, Result};
use crate::repository::catalog::CatalogMetadataStreamAdmission;
use crate::repository::error_helpers::ResultExt;
use crate::repository::error_helpers::http_client_builder_error_message;
use indicatif::ProgressBar;
use reqwest::Client;
use reqwest::header;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

mod stream;
use stream::{ContentRange, ResponseStreamOptions, content_range, stream_response_to_file};

/// Exact identity accumulated while response chunks are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedFileIdentity {
    pub sha256: String,
    pub size: u64,
}

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
    /// Timeout for one file-download request or interval without body progress
    /// (default 300s). This is not a wall-clock limit for the complete file.
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
pub(crate) const MAX_BYTES_RESPONSE_SIZE: u64 = 256 * 1024 * 1024;

fn byte_download_timeout(timeouts: &TimeoutConfig) -> Duration {
    // In-memory downloads cover more than tiny signatures and keys: Remi and
    // distro metadata blobs can legitimately be large enough that they need the
    // same budget as package-file downloads, especially in low-resource QEMU
    // guests.
    timeouts.download
}

fn append_limited_chunk(
    body: &mut Vec<u8>,
    total: &mut u64,
    chunk: &[u8],
    limit: u64,
    url: &str,
) -> Result<()> {
    *total =
        total
            .checked_add(u64::try_from(chunk.len()).map_err(|_| {
                Error::DownloadError("response chunk length exceeds u64".to_string())
            })?)
            .ok_or_else(|| Error::DownloadError("response body size exceeds u64".to_string()))?;
    if *total > limit {
        return Err(Error::DownloadError(format!(
            "Response body too large ({} bytes, max {}): {}",
            *total, limit, url
        )));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

pub(crate) async fn read_response_bytes_with_limit(
    mut response: reqwest::Response,
    limit: u64,
    url: &str,
) -> Result<Vec<u8>> {
    if let Some(content_length) = response.content_length()
        && content_length > limit
    {
        return Err(Error::DownloadError(format!(
            "Response too large ({} bytes, max {}): {}",
            content_length, limit, url
        )));
    }

    let mut body = Vec::new();
    let mut total = 0u64;
    while let Some(chunk) =
        response
            .chunk()
            .await
            .map_err(|error| Error::RepositoryResponseBody {
                url: url.to_string(),
                detail: error.to_string(),
            })?
    {
        append_limited_chunk(&mut body, &mut total, &chunk, limit, url)?;
    }
    Ok(body)
}

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

pub(crate) fn is_file_or_local_reference(url_or_path: &str) -> bool {
    url_or_path.starts_with("file://") || !has_url_scheme(url_or_path)
}

fn has_url_scheme(input: &str) -> bool {
    let Some(colon_index) = input.find(':') else {
        return false;
    };

    let scheme = &input[..colon_index];
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        })
}

fn local_path_from_reference(url_or_path: &str) -> Option<PathBuf> {
    if let Some(path) = url_or_path.strip_prefix("file://") {
        return Some(PathBuf::from(path));
    }

    if has_url_scheme(url_or_path) {
        return None;
    }

    Some(PathBuf::from(url_or_path))
}

fn temp_path_for(dest_path: &Path) -> PathBuf {
    let file_name = dest_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    dest_path.with_file_name(format!(".{file_name}.tmp"))
}

fn finalize_confirmed_staged_download(
    temp_path: &Path,
    dest_path: &Path,
) -> Result<DownloadedFileIdentity> {
    let file = OpenOptions::new().read(true).write(true).open(temp_path)?;
    file.sync_all()?;
    let size = file.metadata()?.len();
    let mut reader = std::io::BufReader::new(&file);
    let sha256 = crate::hash::sha256_reader_hex(&mut reader)?;
    drop(reader);
    drop(file);
    if let Err(error) = fs::rename(temp_path, dest_path) {
        let _ = fs::remove_file(temp_path);
        return Err(Error::IoError(format!(
            "Failed to move {} to {}: {error}",
            temp_path.display(),
            dest_path.display()
        )));
    }
    info!("Successfully downloaded to {}", dest_path.display());
    Ok(DownloadedFileIdentity { sha256, size })
}

fn verify_local_source_metadata(source_path: &Path, expected_size: Option<u64>) -> Result<()> {
    let metadata = fs::metadata(source_path).map_err(|e| {
        Error::DownloadError(format!(
            "Failed to stat local source {}: {e}",
            source_path.display()
        ))
    })?;

    if !metadata.file_type().is_file() {
        return Err(Error::DownloadError(format!(
            "Local source must be a regular file: {}",
            source_path.display()
        )));
    }

    if let Some(expected_size) = expected_size
        && metadata.len() != expected_size
    {
        return Err(Error::DownloadError(format!(
            "Local source file size mismatch for {}: expected {} bytes, got {} bytes",
            source_path.display(),
            expected_size,
            metadata.len()
        )));
    }

    Ok(())
}

fn copy_local_file(source_path: &Path, dest_path: &Path, expected_size: Option<u64>) -> Result<()> {
    verify_local_source_metadata(source_path, expected_size)?;

    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            Error::IoError(format!(
                "Failed to create directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    let temp_path = temp_path_for(dest_path);

    let result = copy_local_file_bounded(source_path, &temp_path, expected_size);
    if let Err(e) = result {
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }

    if let Err(e) = fs::rename(&temp_path, dest_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(Error::IoError(format!(
            "Failed to move {} to {}: {e}",
            temp_path.display(),
            dest_path.display()
        )));
    }

    Ok(())
}

fn copy_local_file_bounded(
    source_path: &Path,
    temp_path: &Path,
    expected_size: Option<u64>,
) -> Result<()> {
    let mut source = File::open(source_path).map_err(|e| {
        Error::DownloadError(format!(
            "Failed to open local source {}: {e}",
            source_path.display()
        ))
    })?;
    let mut dest = File::create(temp_path).map_err(|e| {
        Error::DownloadError(format!(
            "Failed to create temporary copy {}: {e}",
            temp_path.display()
        ))
    })?;

    let mut copied = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        let read = source.read(&mut buffer).map_err(|e| {
            Error::DownloadError(format!(
                "Failed to read local source {}: {e}",
                source_path.display()
            ))
        })?;
        if read == 0 {
            break;
        }

        copied += read as u64;
        if let Some(expected_size) = expected_size
            && copied > expected_size
        {
            return Err(Error::DownloadError(format!(
                "Local source file size mismatch for {}: expected {} bytes, read more than {} bytes",
                source_path.display(),
                expected_size,
                expected_size
            )));
        }

        dest.write_all(&buffer[..read]).map_err(|e| {
            Error::DownloadError(format!(
                "Failed to write temporary copy {}: {e}",
                temp_path.display()
            ))
        })?;
    }

    if let Some(expected_size) = expected_size
        && copied != expected_size
    {
        return Err(Error::DownloadError(format!(
            "Local source file size mismatch for {}: expected {} bytes, copied {} bytes",
            source_path.display(),
            expected_size,
            copied
        )));
    }

    Ok(())
}

pub async fn download_static_or_http_file(url_or_path: &str, dest_path: &Path) -> Result<()> {
    download_static_or_http_file_with_expected_size(url_or_path, dest_path, None).await
}

pub(crate) async fn download_static_or_http_file_with_expected_size(
    url_or_path: &str,
    dest_path: &Path,
    expected_size: Option<u64>,
) -> Result<()> {
    if url_or_path.starts_with("http://") || url_or_path.starts_with("https://") {
        return RepositoryClient::new()?
            .download_file_with_size_limit(url_or_path, dest_path, expected_size)
            .await;
    }

    if let Some(source_path) = local_path_from_reference(url_or_path) {
        return copy_local_file(&source_path, dest_path, expected_size);
    }

    validate_url_scheme(url_or_path)
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

fn http_status_error(status: reqwest::StatusCode, url: &str) -> Error {
    Error::HttpStatus {
        status: status.as_u16(),
        url: url.to_string(),
    }
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
            .map_err(|e| Error::InitError(http_client_builder_error_message(e)))?;

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
                            return Err(http_status_error(status, &metadata_url));
                        }
                        warn!(
                            "Metadata fetch attempt {} got HTTP {}, retrying...",
                            attempt, status
                        );
                        tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                        continue;
                    }

                    if !status.is_success() {
                        return Err(http_status_error(status, &metadata_url));
                    }

                    // Route through bounded streaming download to enforce the
                    // size cap on actual bytes received instead of trusting
                    // Content-Length.
                    let bytes = read_response_bytes_with_limit(
                        response,
                        MAX_BYTES_RESPONSE_SIZE,
                        &metadata_url,
                    )
                    .await?;

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
    /// Transient transport and server failures use the configured bounded retry
    /// policy. Permanent HTTP failures such as 404 return immediately.
    pub async fn download_to_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.download_to_bytes_with_limit(url, MAX_BYTES_RESPONSE_SIZE)
            .await
    }

    /// Download a response body while enforcing the caller's exact byte cap.
    pub(crate) async fn download_to_bytes_with_limit(
        &self,
        url: &str,
        max_size: u64,
    ) -> Result<Vec<u8>> {
        self.download_to_bytes_with_headers_limit(url, max_size)
            .await
            .map(|(_, body)| body)
    }

    async fn download_to_bytes_with_headers_limit(
        &self,
        url: &str,
        max_size: u64,
    ) -> Result<(header::HeaderMap, Vec<u8>)> {
        validate_url_scheme(url)?;

        let max_attempts = self.retry_policy.max_attempts.max(1);
        for attempt in 1..=max_attempts {
            let response = self
                .client
                .get(url)
                .header(header::ACCEPT_ENCODING, "identity")
                .timeout(byte_download_timeout(&self.timeouts))
                .send()
                .await;
            match response {
                Ok(response) if is_transient_error(response.status()) => {
                    if attempt == max_attempts {
                        return Err(http_status_error(response.status(), url));
                    }
                    warn!(
                        "Byte download attempt {} got HTTP {}, retrying...",
                        attempt,
                        response.status()
                    );
                }
                Ok(response) => {
                    if !response.status().is_success() {
                        return Err(http_status_error(response.status(), url));
                    }
                    let headers = response.headers().clone();
                    match read_response_bytes_with_limit(response, max_size, url).await {
                        Ok(body) => return Ok((headers, body)),
                        Err(Error::RepositoryResponseBody { detail, .. })
                            if attempt < max_attempts =>
                        {
                            warn!(
                                "Byte download attempt {attempt} lost its response body: \
                                 {detail}; retrying..."
                            );
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => {
                    if attempt == max_attempts {
                        return Err(error).download_context(url);
                    }
                    warn!("Byte download attempt {attempt} failed: {error}, retrying...");
                }
            }
            tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
        }
        unreachable!("byte download attempt count is clamped to at least one")
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
            decompress_metadata_auto(&bytes, &format!("repository metadata from {url}"))?;

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
                return decompress_metadata_auto(
                    &bytes,
                    &format!("repository metadata from {url}"),
                );
            }
            return Ok(bytes);
        }

        decompress_metadata(&bytes, format, &format!("repository metadata from {url}"))
    }

    /// Download a file to the specified path with retry support
    pub async fn download_file(&self, url: &str, dest_path: &Path) -> Result<()> {
        self.download_file_internal(url, dest_path, "", None, None, None)
            .await
            .map(|_| ())
    }

    async fn download_file_with_size_limit(
        &self,
        url: &str,
        dest_path: &Path,
        max_size: Option<u64>,
    ) -> Result<()> {
        self.download_file_internal(url, dest_path, "", None, max_size, None)
            .await
            .map(|_| ())
    }

    /// Download a file while hashing and sizing the served stream.
    pub async fn download_file_with_identity(
        &self,
        url: &str,
        dest_path: &Path,
    ) -> Result<DownloadedFileIdentity> {
        self.download_file_internal(url, dest_path, "", None, None, None)
            .await
    }

    /// Download metadata while admitting each exact served chunk before its
    /// run-local filesystem write.
    pub(crate) async fn download_file_with_identity_admission(
        &self,
        url: &str,
        dest_path: &Path,
        scratch_admission: &dyn CatalogMetadataStreamAdmission,
    ) -> Result<DownloadedFileIdentity> {
        self.download_file_internal(url, dest_path, "", None, None, Some(scratch_admission))
            .await
    }

    /// Download while hashing and reject a stream larger than its authority.
    ///
    /// Callers that already hold an exact metadata-object size use this before
    /// comparing the returned digest and size with that typed authority.
    pub async fn download_file_with_identity_limit(
        &self,
        url: &str,
        dest_path: &Path,
        max_size: u64,
    ) -> Result<DownloadedFileIdentity> {
        self.download_file_internal(url, dest_path, "", None, Some(max_size), None)
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
        self.download_file_internal(url, dest_path, display_name, progress_bar, None, None)
            .await
            .map(|_| ())
    }

    async fn download_file_internal(
        &self,
        url: &str,
        dest_path: &Path,
        display_name: &str,
        progress_bar: Option<&ProgressBar>,
        max_size: Option<u64>,
        scratch_admission: Option<&dyn CatalogMetadataStreamAdmission>,
    ) -> Result<DownloadedFileIdentity> {
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
            if max_size.is_some_and(|maximum| existing_len > maximum) {
                let _ = fs::remove_file(&temp_path);
                return Err(Error::DownloadError(format!(
                    "partial download exceeds declared size limit of {} bytes",
                    max_size.expect("checked maximum")
                )));
            }

            let mut request = self
                .client
                .get(url)
                .header(header::ACCEPT_ENCODING, "identity");
            if existing_len > 0 {
                debug!(
                    "Found partial download ({} bytes), requesting resume",
                    existing_len
                );
                request = request.header(header::RANGE, format!("bytes={}-", existing_len));
            }

            let response = tokio::time::timeout(self.timeouts.download, request.send()).await;
            match response {
                Err(_) => {
                    if attempt >= self.retry_policy.max_attempts {
                        return Err(Error::DownloadError(format!(
                            "download request made no progress for {:?}",
                            self.timeouts.download
                        )));
                    }
                    warn!(
                        "Download attempt {} made no request progress for {:?}, retrying...",
                        attempt, self.timeouts.download
                    );
                    tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                    continue;
                }
                Ok(Ok(response)) => {
                    let status = response.status();

                    if is_transient_error(status) {
                        if attempt >= self.retry_policy.max_attempts {
                            return Err(http_status_error(status, url));
                        }
                        warn!(
                            "Download attempt {} got HTTP {}, retrying...",
                            attempt, status
                        );
                        tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                        continue;
                    }

                    // A 416 proves completion only when its typed total equals
                    // the exact staged byte count. Otherwise discard the stale
                    // prefix and use a fresh bounded attempt.
                    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                        if existing_len == 0 {
                            return Err(http_status_error(status, url));
                        }
                        let ContentRange::Unsatisfied { total } =
                            content_range(response.headers())?
                        else {
                            return Err(Error::ParseError(
                                "HTTP 416 requires an unsatisfied byte Content-Range".to_string(),
                            ));
                        };
                        if total == existing_len {
                            debug!(
                                "Server confirmed staged download complete at {} bytes",
                                total
                            );
                            return finalize_confirmed_staged_download(&temp_path, dest_path);
                        }
                        fs::remove_file(&temp_path).map_err(|error| {
                            Error::IoError(format!(
                                "Failed to discard stale partial download {}: {error}",
                                temp_path.display()
                            ))
                        })?;
                        if attempt >= self.retry_policy.max_attempts {
                            return Err(Error::DownloadError(format!(
                                "server rejected byte offset {existing_len} for a {total}-byte \
                                 object"
                            )));
                        }
                        warn!(
                            "Server rejected stale byte offset {existing_len} for a {total}-byte \
                             object; restarting..."
                        );
                        tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                        continue;
                    }

                    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
                        return Err(http_status_error(status, url));
                    }

                    // Determine resume vs fresh download
                    let (mut file, offset, total_size) = if status
                        == reqwest::StatusCode::PARTIAL_CONTENT
                        && existing_len > 0
                    {
                        let ContentRange::Partial { start, end, total } =
                            content_range(response.headers())?
                        else {
                            return Err(Error::ParseError(
                                "HTTP 206 requires a concrete byte Content-Range".to_string(),
                            ));
                        };
                        if start != existing_len {
                            return Err(Error::ParseError(format!(
                                "HTTP 206 starts at byte {start}, expected staged offset \
                                     {existing_len}"
                            )));
                        }
                        if max_size.is_some_and(|maximum| total > maximum) {
                            return Err(Error::DownloadError(format!(
                                "byte-range response declares {total} bytes above the \
                                     {maximum}-byte limit",
                                maximum = max_size.expect("checked maximum")
                            )));
                        }
                        let segment_size = end - start + 1;
                        if response
                            .content_length()
                            .is_some_and(|length| length != segment_size)
                        {
                            return Err(Error::ParseError(format!(
                                "HTTP 206 Content-Length disagrees with its {segment_size}-byte \
                                     Content-Range"
                            )));
                        }
                        debug!(
                            "Resuming download from byte {}, total size {}",
                            existing_len, total
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
                        (file, existing_len, total)
                    } else if status == reqwest::StatusCode::PARTIAL_CONTENT {
                        return Err(Error::ParseError(
                            "server returned HTTP 206 when no byte range was requested".to_string(),
                        ));
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

                    let mut hasher = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
                    if offset > 0 {
                        let mut existing = std::io::BufReader::new(File::open(&temp_path)?);
                        let mut buffer = [0_u8; 64 * 1024];
                        loop {
                            let read = existing.read(&mut buffer)?;
                            if read == 0 {
                                break;
                            }
                            hasher.update(&buffer[..read]);
                        }
                    }

                    // Stream response to file, hashing exact served bytes while writing.
                    let downloaded = stream_response_to_file(
                        response,
                        &mut file,
                        &mut hasher,
                        ResponseStreamOptions {
                            total_size,
                            offset,
                            progress_bar,
                            display_name,
                            max_size,
                            scratch_admission,
                            inactivity_timeout: self.timeouts.download,
                        },
                    )
                    .await;
                    let downloaded = match downloaded {
                        Ok(downloaded) => downloaded,
                        Err(error) if attempt < self.retry_policy.max_attempts => {
                            drop(file);
                            warn!("Download attempt {attempt} failed: {error}, retrying...");
                            tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    if total_size > 0 && downloaded != total_size {
                        let error = Error::RepositoryResponseBody {
                            url: url.to_string(),
                            detail: format!(
                                "response ended at {downloaded} bytes, expected {total_size}"
                            ),
                        };
                        if attempt < self.retry_policy.max_attempts {
                            drop(file);
                            warn!(
                                "Download attempt {attempt} was incomplete: {error}; retrying..."
                            );
                            tokio::time::sleep(self.retry_policy.delay_for_attempt(attempt)).await;
                            continue;
                        }
                        return Err(error);
                    }

                    if let Some(pb) = progress_bar {
                        pb.finish_with_message(format!("{} [done]", display_name));
                    }

                    info!("Downloaded {} bytes", downloaded);
                    file.sync_all()?;
                    drop(file);
                    let sha256 = hasher.finalize().value;

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
                    return Ok(DownloadedFileIdentity {
                        sha256,
                        size: downloaded,
                    });
                }
                Ok(Err(e)) => {
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
#[path = "client/tests.rs"]
mod tests;
