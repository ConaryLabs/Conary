// crates/conary-core/src/repository/remi.rs

//! Remi client for fetching CCS packages from conversion proxies
//!
//! Remi converts native package formats (RPM/DEB/Arch) to CCS
//! format on-demand. When a package isn't cached, the server returns 202 Accepted
//! with a job ID that the client polls until conversion completes.
//!
//! # Flow
//! 1. Request package: GET /v1/{distro}/packages/{name}
//! 2. If 200: Package ready, parse manifest
//! 3. If 202: Conversion in progress, poll /v1/jobs/{id}
//! 4. Once ready: Download chunks listed in manifest
//! 5. Assemble CCS package from chunks

use crate::error::{Error, Result};
use crate::filesystem::path::sanitize_filename;
use crate::repository::error_helpers::{ResultExt, http_client_builder_error_message};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::repository::chunk_fetcher::{ChunkFetcher, ChunkFetcherBuilder, CompositeChunkFetcher};
use std::sync::Arc;

mod acquisition_error;
use acquisition_error::ReadyPackageAcquisitionError;
mod async_client;
pub use async_client::AsyncRemiClient;
mod protocol;
use protocol::RemiClientCore;
pub use protocol::{ChunkRef, ConversionAccepted, JobStatus, PackageManifest};

/// Default timeout for initial request (30 seconds)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for polling (5 minutes max wait)
const POLL_TIMEOUT: Duration = Duration::from_secs(300);

/// Poll interval (2 seconds)
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Number of times to retry when Remi's conversion queue is temporarily full.
const QUEUE_FULL_MAX_RETRIES: u32 = 5;

/// Chunk download timeout (60 seconds per chunk)
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum total chunk bytes accepted from a single Remi package download.
const MAX_TOTAL_CHUNK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug)]
enum ReadyPackageDownload {
    Downloaded(PathBuf),
    Accepted(ConversionAccepted),
}

fn check_total_chunk_bytes(current: u64, next: u64) -> Result<u64> {
    let total = current
        .checked_add(next)
        .ok_or_else(|| Error::DownloadError("Total chunk bytes overflowed".to_string()))?;
    if total > MAX_TOTAL_CHUNK_BYTES {
        return Err(Error::DownloadError(format!(
            "Remi package exceeds maximum total chunk bytes ({total} > {MAX_TOTAL_CHUNK_BYTES})"
        )));
    }
    Ok(total)
}

fn identity_get(client: &Client, url: &str) -> reqwest::RequestBuilder {
    client.get(url).header(header::ACCEPT_ENCODING, "identity")
}

#[cfg(not(test))]
fn ready_download_retry_config() -> crate::repository::retry::RetryConfig {
    crate::repository::retry::RetryConfig::quick()
}

#[cfg(test)]
fn ready_download_retry_config() -> crate::repository::retry::RetryConfig {
    crate::repository::retry::RetryConfig {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        jitter_factor: 0.0,
    }
}

#[cfg(not(test))]
fn queue_full_retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(1_u64 << attempt.saturating_sub(1).min(4))
}

#[cfg(test)]
fn queue_full_retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(attempt as u64)
}

/// Client for interacting with a Remi server
pub struct RemiClient {
    client: Client,
    core: RemiClientCore,
}

impl RemiClient {
    /// Create a new Remi client
    pub fn new(base_url: &str) -> Result<Self> {
        let core = RemiClientCore::new(base_url)?;

        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(http_client_builder_error_message(e)))?;

        Ok(Self { client, core })
    }

    /// Request a package from the Remi
    ///
    /// Returns the manifest when the package is ready. If conversion is needed,
    /// this will poll automatically until complete or timeout.
    pub async fn get_package(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<PackageManifest> {
        let url = self
            .core
            .package_url(distro, name, version, None, architecture);

        info!("Requesting package from Remi: {}", url);

        let response = self.client.get(&url).send().await.download_context(&url)?;

        match response.status().as_u16() {
            200 => {
                // Package ready - parse manifest
                let manifest: PackageManifest =
                    response.json().await.parse_context("package manifest")?;
                info!(
                    "Package ready: {} chunks, {} bytes",
                    manifest.chunks.len(),
                    manifest.total_size
                );
                Ok(manifest)
            }
            202 => {
                let accepted: ConversionAccepted =
                    response.json().await.parse_context("202 response")?;
                info!(
                    "Package conversion queued (job {}), ETA: {:?}s",
                    accepted.job_id, accepted.eta_seconds
                );
                self.poll_for_completion(&accepted.job_id).await
            }
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(self.core.map_http_error(status, body, name, distro))
            }
        }
    }

    async fn send_download_request_with_queue_retry(
        &self,
        url: &str,
        name: &str,
        distro: &str,
    ) -> std::result::Result<reqwest::Response, ReadyPackageAcquisitionError> {
        for attempt in 0..=QUEUE_FULL_MAX_RETRIES {
            let response = identity_get(&self.client, url)
                .send()
                .await
                .map_err(|source| ReadyPackageAcquisitionError::transport(url, source))?;
            let status = response.status().as_u16();
            if status != 503 {
                return Ok(response);
            }

            let body = response.text().await.unwrap_or_default();
            if attempt == QUEUE_FULL_MAX_RETRIES {
                return Err(self.core.map_http_error(status, body, name, distro).into());
            }

            let retry_after = queue_full_retry_delay(attempt + 1);
            warn!(
                "Remi conversion queue full for {} on {}; retrying in {:?} ({}/{})",
                name,
                distro,
                retry_after,
                attempt + 1,
                QUEUE_FULL_MAX_RETRIES
            );
            tokio::time::sleep(retry_after).await;
        }

        unreachable!("queue-full retry loop always returns")
    }

    /// Poll for job completion
    ///
    /// Retries up to 3 times on transient errors (5xx, timeout, connection
    /// refused) with exponential backoff. 4xx errors fail immediately.
    async fn poll_for_completion(&self, job_id: &str) -> Result<PackageManifest> {
        let url = self.core.job_url(job_id);
        let start = std::time::Instant::now();
        let max_transient_retries: u32 = 3;

        // Create a spinner for visual feedback
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap_or(ProgressStyle::default_spinner()),
        );
        spinner.set_message(format!("Converting package (job {})...", job_id));
        spinner.enable_steady_tick(Duration::from_millis(100));

        let mut consecutive_transient_failures: u32 = 0;

        loop {
            // Check timeout
            if start.elapsed() > POLL_TIMEOUT {
                spinner.finish_with_message("Conversion timed out");
                return Err(Error::TimeoutError(format!(
                    "Conversion job {} timed out after {:?}",
                    job_id, POLL_TIMEOUT
                )));
            }

            // Poll job status
            let response = match self.client.get(&url).send().await {
                Ok(resp) => resp,
                Err(e) => {
                    // Connection error or timeout -- transient, retry
                    consecutive_transient_failures += 1;
                    if consecutive_transient_failures > max_transient_retries {
                        spinner.finish_with_message("Poll failed");
                        return Err(Error::DownloadError(format!(
                            "Failed to poll job status after {} retries: {e}",
                            max_transient_retries
                        )));
                    }
                    let backoff = Duration::from_millis(
                        500 * u64::from(2_u32.saturating_pow(consecutive_transient_failures - 1)),
                    );
                    warn!(
                        "Transient error polling job {} (attempt {}/{}): {e}, retrying in {:?}",
                        job_id, consecutive_transient_failures, max_transient_retries, backoff
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            };

            let status_code = response.status().as_u16();
            if !response.status().is_success() {
                if status_code >= 500 {
                    // Server error -- transient, retry
                    consecutive_transient_failures += 1;
                    if consecutive_transient_failures > max_transient_retries {
                        spinner.finish_with_message("Poll failed");
                        return Err(Error::DownloadError(format!(
                            "Job poll returned HTTP {} after {} retries",
                            status_code, max_transient_retries
                        )));
                    }
                    let backoff = Duration::from_millis(
                        500 * u64::from(2_u32.saturating_pow(consecutive_transient_failures - 1)),
                    );
                    warn!(
                        "Server error polling job {} (HTTP {}, attempt {}/{}), retrying in {:?}",
                        job_id,
                        status_code,
                        consecutive_transient_failures,
                        max_transient_retries,
                        backoff
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                // 4xx -- not transient, fail immediately
                spinner.finish_with_message("Poll failed");
                return Err(Error::DownloadError(format!(
                    "Job poll returned HTTP {}",
                    status_code
                )));
            }

            // Successful response -- reset transient failure counter
            consecutive_transient_failures = 0;

            let status: JobStatus = response.json().await.parse_context("job status")?;

            match status.status.as_str() {
                "ready" => {
                    spinner.finish_with_message("Conversion complete");
                    info!("Conversion complete for job {}", job_id);

                    // The manifest should be in the response, but if not we need to
                    // re-request the package endpoint
                    if let Some(manifest) = status.manifest {
                        return Ok(manifest);
                    }

                    // Re-request to get manifest (direct request, not recursive poll)
                    let url = self.core.package_url(
                        &status.distro,
                        &status.package,
                        status.version.as_deref(),
                        None,
                        status.architecture.as_deref(),
                    );
                    let response = self.client.get(&url).send().await.download_context(&url)?;
                    if !response.status().is_success() {
                        return Err(Error::DownloadError(format!(
                            "Re-request for manifest failed: HTTP {}",
                            response.status()
                        )));
                    }
                    let manifest = response.json().await.parse_context("manifest")?;
                    return Ok(manifest);
                }
                "failed" => {
                    spinner.finish_with_message("Conversion failed");
                    let error_msg = status.error.unwrap_or_else(|| "Unknown error".to_string());
                    return Err(Error::DownloadError(format!(
                        "Conversion failed: {}",
                        error_msg
                    )));
                }
                "converting" | "queued" => {
                    // Still in progress - update spinner and continue polling
                    if let Some(progress) = status.progress {
                        spinner.set_message(format!(
                            "Converting {} ({}%)...",
                            status.package, progress
                        ));
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
                other => {
                    spinner.finish_with_message("Conversion protocol error");
                    return Err(Error::DownloadError(format!(
                        "Remi returned unsupported conversion status {other} for {}/{}",
                        status.distro, status.package
                    )));
                }
            }
        }
    }

    /// Download all chunks for a package
    ///
    /// Downloads chunks sequentially and returns a map of hash -> data
    /// for assembly.
    pub async fn download_chunks(
        &self,
        manifest: &PackageManifest,
        progress: Option<&ProgressBar>,
    ) -> Result<HashMap<String, Vec<u8>>> {
        let mut chunks = HashMap::new();

        info!(
            "Downloading {} chunks for {}",
            manifest.chunks.len(),
            manifest.name
        );

        let total_size = manifest
            .chunks
            .iter()
            .try_fold(0u64, |acc, chunk| check_total_chunk_bytes(acc, chunk.size))?;
        let mut downloaded: u64 = 0;

        if let Some(pb) = progress {
            pb.set_length(total_size);
            pb.set_message(format!("Downloading {} chunks", manifest.chunks.len()));
        }

        let retry_config = crate::repository::retry::RetryConfig::quick();

        for chunk in &manifest.chunks {
            let url = self.core.chunk_url(&chunk.hash);
            debug!("Downloading chunk: {} ({} bytes)", chunk.hash, chunk.size);

            let data = crate::repository::retry::with_retry_async(&retry_config, || {
                let url = &url;
                let chunk_hash = &chunk.hash;
                async move {
                    let response = self
                        .client
                        .get(url.as_str())
                        .header(header::ACCEPT_ENCODING, "identity")
                        .timeout(CHUNK_TIMEOUT)
                        .send()
                        .await
                        .download_context(url)?;

                    let status_code = response.status().as_u16();
                    if status_code >= 500 || status_code == 408 || status_code == 429 {
                        return Err(Error::DownloadError(format!(
                            "Chunk {} returned HTTP {}",
                            chunk_hash, status_code
                        )));
                    }

                    if !response.status().is_success() {
                        // 4xx (other than 408/429) -- not transient, fail immediately
                        return Err(Error::DownloadError(format!(
                            "Chunk {} returned HTTP {}",
                            chunk_hash,
                            response.status()
                        )));
                    }

                    let bytes = response.bytes().await.download_context(url)?;
                    Ok(bytes)
                }
            })
            .await?;

            // Verify chunk hash using shared hash module
            crate::hash::verify_sha256(&data, &chunk.hash).map_err(|e| {
                Error::ChecksumMismatch {
                    expected: e.expected,
                    actual: e.actual,
                }
            })?;

            downloaded = check_total_chunk_bytes(downloaded, data.len() as u64)?;
            if let Some(pb) = progress {
                pb.set_position(downloaded);
            }

            chunks.insert(chunk.hash.clone(), data.to_vec());
        }

        if let Some(pb) = progress {
            pb.finish_with_message(format!(
                "Downloaded {} chunks ({} bytes)",
                chunks.len(),
                downloaded
            ));
        }

        info!("Downloaded {} chunks ({} bytes)", chunks.len(), downloaded);
        Ok(chunks)
    }

    /// Assemble a CCS package from downloaded chunks
    ///
    /// Writes chunks to the output file in order according to manifest offsets.
    pub fn assemble_package(
        manifest: &PackageManifest,
        chunks: &HashMap<String, Vec<u8>>,
        output_path: &Path,
    ) -> Result<()> {
        info!("Assembling CCS package: {}", output_path.display());

        // Sort chunks by offset
        let mut sorted_chunks: Vec<_> = manifest.chunks.iter().collect();
        sorted_chunks.sort_by_key(|c| c.offset);

        // Create output file
        let mut file = std::fs::File::create(output_path).io_context("create output file")?;

        // Write chunks in order
        for chunk_ref in sorted_chunks {
            let data = chunks.get(&chunk_ref.hash).ok_or_else(|| {
                Error::DownloadError(format!("Missing chunk: {}", chunk_ref.hash))
            })?;

            file.write_all(data).io_context("write chunk")?;
        }

        // Verify total size
        let metadata = std::fs::metadata(output_path).io_context("read output file metadata")?;

        if metadata.len() != manifest.total_size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", manifest.total_size),
                actual: format!("{} bytes", metadata.len()),
            });
        }

        // Verify content hash using shared hash module
        if let Err(e) = crate::hash::verify_file_sha256(output_path, &manifest.content_hash) {
            // Clean up invalid file
            let _ = std::fs::remove_file(output_path);
            return Err(Error::ChecksumMismatch {
                expected: e.expected,
                actual: e.actual,
            });
        }

        info!(
            "CCS package assembled and verified: {}",
            output_path.display()
        );
        Ok(())
    }

    async fn download_ready_package_once(
        &self,
        url: &str,
        name: &str,
        distro: &str,
        output_dir: &Path,
    ) -> std::result::Result<ReadyPackageDownload, ReadyPackageAcquisitionError> {
        let response = self
            .send_download_request_with_queue_retry(url, name, distro)
            .await?;

        match response.status().as_u16() {
            200 => self
                .download_ccs_response(response, name, output_dir)
                .await
                .map(ReadyPackageDownload::Downloaded),
            202 => {
                let accepted: ConversionAccepted =
                    response.json().await.parse_context("202 response")?;
                Ok(ReadyPackageDownload::Accepted(accepted))
            }
            status => {
                let body = response.text().await.unwrap_or_default();
                let error = self.core.map_http_error(status, body, name, distro);
                Err(ReadyPackageAcquisitionError::http_response(status, error))
            }
        }
    }

    async fn download_ready_package_with_retry(
        &self,
        url: &str,
        name: &str,
        distro: &str,
        output_dir: &Path,
    ) -> Result<ReadyPackageDownload> {
        let retry_config = ready_download_retry_config();
        let max_attempts = retry_config.max_attempts.max(1);
        let mut last_error = None;

        for attempt in 1..=max_attempts {
            match self
                .download_ready_package_once(url, name, distro, output_dir)
                .await
            {
                Ok(download) => return Ok(download),
                Err(error) if attempt < max_attempts && error.is_retryable() => {
                    let delay = retry_config.delay_for_attempt(attempt);
                    warn!(
                        "Remi download attempt {}/{} for {} failed: {}; retrying in {:?}",
                        attempt, max_attempts, name, error, delay
                    );
                    last_error = Some(error);
                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error.into_repository_error()),
            }
        }

        Err(last_error
            .expect("max_attempts is clamped to >= 1, so at least one iteration ran")
            .into_repository_error())
    }

    /// High-level: Fetch a package from Remi and save to disk
    ///
    /// This is the main entry point for downloading CCS packages.
    /// Uses the direct download endpoint to get the pre-built CCS package.
    ///
    /// If conversion is needed, the download endpoint triggers it and returns
    /// 202 Accepted with a job ID for polling.
    pub async fn fetch_package(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        // Use the direct download endpoint
        let url = self
            .core
            .download_url(distro, name, version, None, architecture);

        info!("Downloading CCS package from Remi: {}", url);

        match self
            .download_ready_package_with_retry(&url, name, distro, output_dir)
            .await?
        {
            ReadyPackageDownload::Downloaded(path) => Ok(path),
            ReadyPackageDownload::Accepted(accepted) => {
                // Conversion in progress - poll then retry download
                info!(
                    "Package conversion queued (job {}), ETA: {:?}s",
                    accepted.job_id, accepted.eta_seconds
                );
                let _manifest = self.poll_for_completion(&accepted.job_id).await?;

                // Retry download after conversion completes
                // Small delay + retry loop to handle server propagation timing
                info!("Conversion complete, downloading CCS package");

                let max_retries = 5;
                let last_status = 202;

                for attempt in 1..=max_retries {
                    // Brief delay before retry (increases with each attempt)
                    tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;

                    match self
                        .download_ready_package_with_retry(&url, name, distro, output_dir)
                        .await?
                    {
                        ReadyPackageDownload::Downloaded(path) => return Ok(path),
                        ReadyPackageDownload::Accepted(_) => {
                            debug!(
                                "Download returned 202, retrying ({}/{})",
                                attempt, max_retries
                            );
                        }
                    }
                }

                Err(Error::DownloadError(format!(
                    "Download after conversion still returned HTTP {} after {} retries",
                    last_status, max_retries
                )))
            }
        }
    }

    /// Download the CCS file from a successful response
    async fn download_ccs_response(
        &self,
        response: reqwest::Response,
        name: &str,
        output_dir: &Path,
    ) -> std::result::Result<PathBuf, ReadyPackageAcquisitionError> {
        // Get content length for progress bar
        let content_length = response.content_length().unwrap_or(0);

        // Extract filename from Content-Disposition header or generate one
        let filename = response
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                // Parse filename="something.ccs"
                v.split("filename=")
                    .nth(1)
                    .map(|s| s.trim_matches('"').to_string())
            })
            .unwrap_or_else(|| format!("{}.ccs", name));

        // Sanitize filename to prevent path traversal from malicious servers
        let filename = sanitize_filename(&filename).unwrap_or_else(|_| format!("{}.ccs", name));

        let output_path = output_dir.join(&filename);

        // Create progress bar
        let pb = ProgressBar::new(content_length);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) {msg}")
                .unwrap_or(ProgressStyle::default_bar())
                .progress_chars("#>-"),
        );
        pb.set_message(format!("Downloading {}", filename));

        // Keep every attempt private until its complete body has been flushed and
        // validated. The same-directory stage makes the final rename atomic and
        // lets NamedTempFile clean up stream, local I/O, and validation failures.
        let mut staged = tempfile::Builder::new()
            .prefix(".conary-remi-download-")
            .tempfile_in(output_dir)
            .io_context("create staged output file")?;

        let mut downloaded: u64 = 0;
        let mut response = response;

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(ReadyPackageAcquisitionError::response_stream)?
        {
            staged
                .as_file_mut()
                .write_all(&chunk)
                .io_context("write staged output file")?;

            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        staged
            .as_file_mut()
            .flush()
            .io_context("flush staged output file")?;

        // Verify the file is a valid CCS package (gzip-compressed tar)
        // CCS packages are gzipped tar archives, so check for gzip magic bytes
        let mut magic = [0u8; 2];
        {
            use std::io::Read;
            let mut file =
                std::fs::File::open(staged.path()).io_context("read staged output file")?;
            file.read_exact(&mut magic).io_context("read magic bytes")?;
        }

        // Gzip magic: 0x1f 0x8b
        if magic != [0x1f, 0x8b] {
            return Err(Error::DownloadError(
                "Downloaded file is not a valid CCS package (expected gzip)".to_string(),
            )
            .into());
        }

        staged.persist(&output_path).map_err(|error| {
            Error::IoError(format!(
                "Failed to publish downloaded package at {}: {}",
                output_path.display(),
                error.error
            ))
        })?;

        pb.finish_with_message(format!("Downloaded {} ({} bytes)", filename, downloaded));
        info!("CCS package downloaded: {}", output_path.display());

        Ok(output_path)
    }

    /// Check if Remi is healthy
    pub async fn health_check(&self) -> Result<bool> {
        let url = self.core.health_url();
        match self.client.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests;
