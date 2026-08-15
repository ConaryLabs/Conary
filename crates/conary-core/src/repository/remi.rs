// crates/conary-core/src/repository/remi.rs

//! Remi client for fetching CCS packages from conversion proxies
//!
//! Remi converts native package formats (RPM/DEB/Arch/eopkg) to CCS
//! format on-demand. When a package isn't cached, the server returns 202 Accepted
//! with a job ID that the client polls until conversion completes.
//!
//! # Flow
//! 1. Request package: GET /v1/{distro}/packages/{name}
//! 2. If 200: Package ready, parse manifest
//! 3. If 202: Conversion in progress, poll /v1/jobs/{id}
//! 4. Authenticate the signed transport descriptor
//! 5. Reuse verified CAS objects, fetch only misses, and reconstruct the CCS package

use crate::error::{Error, Result};
use crate::filesystem::path::sanitize_filename;
use crate::repository::error_helpers::{ResultExt, http_client_builder_error_message};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

mod protocol;
use protocol::RemiClientCore;
pub use protocol::{ConversionAccepted, ConversionJobState, JobStatus, PackageManifest};

/// Default timeout for initial request (30 seconds)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll interval (2 seconds)
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Chunk download timeout (60 seconds per chunk)
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

fn identity_get(client: &Client, url: &str) -> reqwest::RequestBuilder {
    client.get(url).header(header::ACCEPT_ENCODING, "identity")
}

/// Client for interacting with a Remi server
pub struct RemiClient {
    client: Client,
    core: RemiClientCore,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemiFetchMetrics {
    pub objects_total: u64,
    pub cas_hits: u64,
    pub objects_downloaded: u64,
    pub bytes_downloaded: u64,
}

#[derive(Debug)]
pub struct RemiFetchedPackage {
    pub path: PathBuf,
    pub metrics: RemiFetchMetrics,
}

pub struct RemiFetchRequest<'a> {
    pub distro: &'a str,
    pub name: &'a str,
    pub version: Option<&'a str>,
    pub architecture: Option<&'a str>,
    pub output_dir: &'a Path,
    pub cas: &'a crate::filesystem::CasStore,
    pub trust_policy: &'a crate::ccs::TrustPolicy,
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
    /// this polls until Remi reports a terminal job state or the caller cancels.
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
                    "Package ready: {} signed objects, {} archive bytes",
                    manifest.transport.objects.len(),
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

    /// Poll for job completion
    ///
    /// Retries up to 3 times on transient errors (5xx, timeout, connection
    /// refused) with exponential backoff. 4xx errors fail immediately.
    async fn poll_for_completion(&self, job_id: &str) -> Result<PackageManifest> {
        let url = self.core.job_url(job_id);
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

            let body = response.text().await.download_context(&url)?;
            let status: JobStatus = serde_json::from_str(&body).parse_context("job status")?;

            match status.poll_decision() {
                protocol::JobPollDecision::Ready => {
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
                protocol::JobPollDecision::Failed(error_msg) => {
                    spinner.finish_with_message("Conversion failed");
                    return Err(Error::DownloadError(format!(
                        "Conversion failed: {}",
                        error_msg
                    )));
                }
                protocol::JobPollDecision::Wait => {
                    // Still in progress - update spinner and continue polling
                    if let Some(progress) = status.progress {
                        spinner.set_message(format!(
                            "Converting {} ({}%)...",
                            status.package, progress
                        ));
                    }
                    self.core.wait_for_next_poll().await;
                }
            }
        }
    }

    /// High-level: Fetch a package from Remi and save to disk
    ///
    /// This is the main entry point for acquiring CCS packages.
    /// It authenticates the repository descriptor before fetching its exact
    /// signed objects and reuses any objects already present in the local CAS.
    ///
    /// If conversion is needed, the package endpoint triggers it and returns
    /// 202 Accepted with a job ID for polling.
    pub async fn fetch_package(&self, request: RemiFetchRequest<'_>) -> Result<PathBuf> {
        self.fetch_package_with_metrics(request)
            .await
            .map(|result| result.path)
    }

    pub async fn fetch_package_with_metrics(
        &self,
        request: RemiFetchRequest<'_>,
    ) -> Result<RemiFetchedPackage> {
        let RemiFetchRequest {
            distro,
            name,
            version,
            architecture,
            output_dir,
            cas,
            trust_policy,
        } = request;
        let manifest = self
            .get_package(distro, name, version, architecture)
            .await?;
        if manifest.distro != distro || manifest.name != name {
            return Err(Error::DownloadError(format!(
                "Remi transport identity {}/{} disagrees with request {distro}/{name}",
                manifest.distro, manifest.name
            )));
        }
        let prepared = manifest
            .transport
            .authenticate(trust_policy)
            .map_err(|error| {
                Error::DownloadError(format!("authenticate Remi transport: {error}"))
            })?;
        if prepared.package_name() != manifest.name
            || prepared.package_version() != manifest.version
        {
            return Err(Error::DownloadError(
                "Remi transport identity disagrees with signed CCS authority".to_string(),
            ));
        }
        let expected = prepared
            .expected_objects()
            .map(|(sha256, size)| (sha256.to_string(), size))
            .collect::<Vec<_>>();
        let mut batch = cas.verified_object_batch(expected.clone())?;
        let mut metrics = RemiFetchMetrics {
            objects_total: expected.len() as u64,
            ..Default::default()
        };

        for (sha256, size) in &expected {
            if batch.reuse_trusted(sha256)? {
                metrics.cas_hits += 1;
                continue;
            }
            let url = self.core.chunk_url(sha256);
            let mut response = identity_get(&self.client, &url)
                .timeout(CHUNK_TIMEOUT)
                .send()
                .await
                .download_context(&url)?;
            if !response.status().is_success() {
                return Err(Error::DownloadError(format!(
                    "signed CCS object {sha256} returned HTTP {}",
                    response.status()
                )));
            }
            if let Some(length) = response.content_length()
                && length != *size
            {
                return Err(Error::DownloadError(format!(
                    "signed CCS object {sha256} declared HTTP length {length}, expected {size}"
                )));
            }
            let capacity = usize::try_from(*size).map_err(|_| {
                Error::DownloadError(format!("signed CCS object {sha256} exceeds host size"))
            })?;
            let mut bytes = Vec::with_capacity(capacity);
            while let Some(chunk) = response.chunk().await.download_context(&url)? {
                let next = bytes.len().checked_add(chunk.len()).ok_or_else(|| {
                    Error::DownloadError(format!("signed CCS object {sha256} size overflow"))
                })?;
                if next as u64 > *size {
                    return Err(Error::DownloadError(format!(
                        "signed CCS object {sha256} exceeds expected size {size}"
                    )));
                }
                bytes.extend_from_slice(&chunk);
            }
            batch.ingest(sha256, &mut std::io::Cursor::new(&bytes))?;
            metrics.objects_downloaded += 1;
            metrics.bytes_downloaded =
                metrics.bytes_downloaded.checked_add(*size).ok_or_else(|| {
                    Error::DownloadError("Remi transfer counter overflow".to_string())
                })?;
        }

        let verified = prepared
            .verify_object_set(batch.commit()?)
            .map_err(|error| Error::DownloadError(format!("verify Remi object set: {error}")))?;
        std::fs::create_dir_all(output_dir).io_context("create Remi output directory")?;
        let filename = sanitize_filename(&format!("{}-{}.ccs", manifest.name, manifest.version))?;
        let output_path = output_dir.join(filename);
        crate::ccs::transport::write_verified_transport_archive(&verified, &output_path)
            .map_err(|error| Error::IoError(format!("write reconstructed CCS package: {error}")))?;
        Ok(RemiFetchedPackage {
            path: output_path,
            metrics,
        })
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
