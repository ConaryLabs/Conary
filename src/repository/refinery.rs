// src/repository/refinery.rs

//! Refinery client for fetching CCS packages from conversion proxies
//!
//! The Refinery is a server that converts legacy packages (RPM/DEB/Arch) to CCS
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
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

#[cfg(feature = "server")]
use crate::repository::chunk_fetcher::{ChunkFetcher, ChunkFetcherBuilder, CompositeChunkFetcher};
#[cfg(feature = "server")]
use std::sync::Arc;

/// Default timeout for initial request (30 seconds)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for polling (5 minutes max wait)
const POLL_TIMEOUT: Duration = Duration::from_secs(300);

/// Poll interval (2 seconds)
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Chunk download timeout (60 seconds per chunk)
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

/// Response when package needs conversion (202 Accepted)
#[derive(Debug, Deserialize)]
pub struct ConversionAccepted {
    pub status: String,
    pub job_id: String,
    pub poll_url: String,
    pub eta_seconds: Option<u32>,
}

/// Job status response from polling endpoint
#[derive(Debug, Deserialize)]
pub struct JobStatus {
    pub job_id: String,
    pub status: String,
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub progress: Option<u8>,
    pub error: Option<String>,
    pub manifest: Option<PackageManifest>,
}

/// Package manifest with chunk list
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub distro: String,
    pub chunks: Vec<ChunkRef>,
    pub total_size: u64,
    pub content_hash: String,
}

/// Reference to a chunk in the CAS
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkRef {
    pub hash: String,
    pub size: u64,
    pub offset: u64,
}

/// Client for interacting with a Refinery server
pub struct RefineryClient {
    client: Client,
    base_url: String,
}

impl RefineryClient {
    /// Create a new Refinery client
    pub fn new(base_url: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {e}")))?;

        // Normalize base URL (remove trailing slash)
        let base_url = base_url.trim_end_matches('/').to_string();

        Ok(Self { client, base_url })
    }

    /// Request a package from the Refinery
    ///
    /// Returns the manifest when the package is ready. If conversion is needed,
    /// this will poll automatically until complete or timeout.
    pub fn get_package(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
    ) -> Result<PackageManifest> {
        let url = if let Some(v) = version {
            format!("{}/v1/{}/packages/{}?version={}", self.base_url, distro, name, v)
        } else {
            format!("{}/v1/{}/packages/{}", self.base_url, distro, name)
        };

        info!("Requesting package from Refinery: {}", url);

        let response = self.client.get(&url).send().map_err(|e| {
            Error::DownloadError(format!("Failed to connect to Refinery: {e}"))
        })?;

        match response.status().as_u16() {
            200 => {
                // Package ready - parse manifest
                let manifest: PackageManifest = response.json().map_err(|e| {
                    Error::DownloadError(format!("Failed to parse package manifest: {e}"))
                })?;
                info!("Package ready: {} chunks, {} bytes", manifest.chunks.len(), manifest.total_size);
                Ok(manifest)
            }
            202 => {
                // Conversion in progress - need to poll
                let accepted: ConversionAccepted = response.json().map_err(|e| {
                    Error::DownloadError(format!("Failed to parse 202 response: {e}"))
                })?;
                info!(
                    "Package conversion queued (job {}), ETA: {:?}s",
                    accepted.job_id, accepted.eta_seconds
                );
                self.poll_for_completion(&accepted.job_id)
            }
            404 => {
                Err(Error::NotFoundError(format!(
                    "Package '{}' not found in {} repositories",
                    name, distro
                )))
            }
            503 => {
                Err(Error::DownloadError(
                    "Refinery conversion queue is full, try again later".to_string(),
                ))
            }
            status => {
                let body = response.text().unwrap_or_default();
                Err(Error::DownloadError(format!(
                    "Refinery returned HTTP {}: {}",
                    status, body
                )))
            }
        }
    }

    /// Poll for job completion
    fn poll_for_completion(&self, job_id: &str) -> Result<PackageManifest> {
        let url = format!("{}/v1/jobs/{}", self.base_url, job_id);
        let start = std::time::Instant::now();

        // Create a spinner for visual feedback
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Invalid spinner template"),
        );
        spinner.set_message(format!("Converting package (job {})...", job_id));
        spinner.enable_steady_tick(Duration::from_millis(100));

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
            let response = self.client.get(&url).send().map_err(|e| {
                Error::DownloadError(format!("Failed to poll job status: {e}"))
            })?;

            if !response.status().is_success() {
                spinner.finish_with_message("Poll failed");
                return Err(Error::DownloadError(format!(
                    "Job poll returned HTTP {}",
                    response.status()
                )));
            }

            let status: JobStatus = response.json().map_err(|e| {
                Error::DownloadError(format!("Failed to parse job status: {e}"))
            })?;

            match status.status.as_str() {
                "ready" => {
                    spinner.finish_with_message("Conversion complete");
                    info!("Conversion complete for job {}", job_id);

                    // The manifest should be in the response, but if not we need to
                    // re-request the package endpoint
                    if let Some(manifest) = status.manifest {
                        return Ok(manifest);
                    }

                    // Re-request to get manifest
                    let version = status.version.as_deref();
                    return self.get_package(&status.distro, &status.package, version);
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
                    std::thread::sleep(POLL_INTERVAL);
                }
                other => {
                    warn!("Unknown job status: {}", other);
                    std::thread::sleep(POLL_INTERVAL);
                }
            }
        }
    }

    /// Download all chunks for a package
    ///
    /// Downloads chunks in parallel (up to 4 concurrent) and returns a map
    /// of hash -> data for assembly.
    pub fn download_chunks(
        &self,
        manifest: &PackageManifest,
        progress: Option<&ProgressBar>,
    ) -> Result<HashMap<String, Vec<u8>>> {
        let mut chunks = HashMap::new();

        info!("Downloading {} chunks for {}", manifest.chunks.len(), manifest.name);

        // Create a client with longer timeout for chunk downloads
        let chunk_client = Client::builder()
            .timeout(CHUNK_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create chunk client: {e}")))?;

        let total_size: u64 = manifest.chunks.iter().map(|c| c.size).sum();
        let mut downloaded: u64 = 0;

        if let Some(pb) = progress {
            pb.set_length(total_size);
            pb.set_message(format!("Downloading {} chunks", manifest.chunks.len()));
        }

        for chunk in &manifest.chunks {
            let url = format!("{}/v1/chunks/{}", self.base_url, chunk.hash);
            debug!("Downloading chunk: {} ({} bytes)", chunk.hash, chunk.size);

            let response = chunk_client.get(&url).send().map_err(|e| {
                Error::DownloadError(format!("Failed to download chunk {}: {e}", chunk.hash))
            })?;

            if !response.status().is_success() {
                return Err(Error::DownloadError(format!(
                    "Chunk {} returned HTTP {}",
                    chunk.hash, response.status()
                )));
            }

            let data = response.bytes().map_err(|e| {
                Error::DownloadError(format!("Failed to read chunk {}: {e}", chunk.hash))
            })?;

            // Verify chunk hash
            use sha2::{Digest, Sha256};
            let actual_hash = format!("{:x}", Sha256::digest(&data));
            if actual_hash != chunk.hash {
                return Err(Error::ChecksumMismatch {
                    expected: chunk.hash.clone(),
                    actual: actual_hash,
                });
            }

            downloaded += data.len() as u64;
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
        let mut file = std::fs::File::create(output_path).map_err(|e| {
            Error::IoError(format!("Failed to create output file: {e}"))
        })?;

        // Write chunks in order
        for chunk_ref in sorted_chunks {
            let data = chunks.get(&chunk_ref.hash).ok_or_else(|| {
                Error::DownloadError(format!("Missing chunk: {}", chunk_ref.hash))
            })?;

            file.write_all(data).map_err(|e| {
                Error::IoError(format!("Failed to write chunk: {e}"))
            })?;
        }

        // Verify total size
        let metadata = std::fs::metadata(output_path).map_err(|e| {
            Error::IoError(format!("Failed to read output file metadata: {e}"))
        })?;

        if metadata.len() != manifest.total_size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", manifest.total_size),
                actual: format!("{} bytes", metadata.len()),
            });
        }

        // Verify content hash
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let file_data = std::fs::read(output_path).map_err(|e| {
            Error::IoError(format!("Failed to read output file for verification: {e}"))
        })?;
        hasher.update(&file_data);
        let actual_hash = format!("{:x}", hasher.finalize());

        if actual_hash != manifest.content_hash {
            // Clean up invalid file
            let _ = std::fs::remove_file(output_path);
            return Err(Error::ChecksumMismatch {
                expected: manifest.content_hash.clone(),
                actual: actual_hash,
            });
        }

        info!("CCS package assembled and verified: {}", output_path.display());
        Ok(())
    }

    /// High-level: Fetch a package from Refinery and save to disk
    ///
    /// This is the main entry point for downloading CCS packages.
    /// Uses the direct download endpoint to get the pre-built CCS package.
    ///
    /// If conversion is needed, the download endpoint triggers it and returns
    /// 202 Accepted with a job ID for polling.
    pub fn fetch_package(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        // Use the direct download endpoint
        let url = if let Some(v) = version {
            format!("{}/v1/{}/packages/{}/download?version={}", self.base_url, distro, name, v)
        } else {
            format!("{}/v1/{}/packages/{}/download", self.base_url, distro, name)
        };

        info!("Downloading CCS package from Refinery: {}", url);

        let response = self.client.get(&url).send().map_err(|e| {
            Error::DownloadError(format!("Failed to connect to Refinery: {e}"))
        })?;

        match response.status().as_u16() {
            200 => {
                // Package ready - download it
                self.download_ccs_response(response, name, output_dir)
            }
            202 => {
                // Conversion in progress - poll then retry download
                let accepted: ConversionAccepted = response.json().map_err(|e| {
                    Error::DownloadError(format!("Failed to parse 202 response: {e}"))
                })?;
                info!(
                    "Package conversion queued (job {}), ETA: {:?}s",
                    accepted.job_id, accepted.eta_seconds
                );
                let _manifest = self.poll_for_completion(&accepted.job_id)?;

                // Retry download after conversion completes
                info!("Conversion complete, downloading CCS package");
                let retry_response = self.client.get(&url).send().map_err(|e| {
                    Error::DownloadError(format!("Failed to retry download: {e}"))
                })?;

                if retry_response.status().as_u16() != 200 {
                    return Err(Error::DownloadError(format!(
                        "Download after conversion returned HTTP {}",
                        retry_response.status()
                    )));
                }

                self.download_ccs_response(retry_response, name, output_dir)
            }
            404 => {
                Err(Error::NotFoundError(format!(
                    "Package '{}' not found in {} repositories",
                    name, distro
                )))
            }
            503 => {
                Err(Error::DownloadError(
                    "Refinery conversion queue is full, try again later".to_string(),
                ))
            }
            status => {
                let body = response.text().unwrap_or_default();
                Err(Error::DownloadError(format!(
                    "Refinery returned HTTP {}: {}",
                    status, body
                )))
            }
        }
    }

    /// Download the CCS file from a successful response
    fn download_ccs_response(
        &self,
        response: reqwest::blocking::Response,
        name: &str,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        // Get content length for progress bar
        let content_length = response.content_length().unwrap_or(0);

        // Extract filename from Content-Disposition header or generate one
        let filename = response
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                // Parse filename="something.ccs"
                v.split("filename=").nth(1).map(|s| s.trim_matches('"').to_string())
            })
            .unwrap_or_else(|| format!("{}.ccs", name));

        let output_path = output_dir.join(&filename);

        // Create progress bar
        let pb = ProgressBar::new(content_length);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) {msg}")
                .expect("Invalid progress bar template")
                .progress_chars("#>-"),
        );
        pb.set_message(format!("Downloading {}", filename));

        // Download to file with progress
        let mut file = std::fs::File::create(&output_path).map_err(|e| {
            Error::IoError(format!("Failed to create output file: {e}"))
        })?;

        let mut downloaded: u64 = 0;
        let mut reader = response;
        let mut buffer = [0u8; 8192];

        loop {
            let bytes_read = std::io::Read::read(&mut reader, &mut buffer).map_err(|e| {
                Error::DownloadError(format!("Failed to read response: {e}"))
            })?;

            if bytes_read == 0 {
                break;
            }

            file.write_all(&buffer[..bytes_read]).map_err(|e| {
                Error::IoError(format!("Failed to write to output file: {e}"))
            })?;

            downloaded += bytes_read as u64;
            pb.set_position(downloaded);
        }

        pb.finish_with_message(format!("Downloaded {} ({} bytes)", filename, downloaded));
        info!("CCS package downloaded: {}", output_path.display());

        // Verify the file is a valid CCS package (gzip-compressed tar)
        // CCS packages are gzipped tar archives, so check for gzip magic bytes
        let mut magic = [0u8; 2];
        {
            use std::io::Read;
            let mut file = std::fs::File::open(&output_path).map_err(|e| {
                Error::IoError(format!("Failed to read downloaded file: {e}"))
            })?;
            file.read_exact(&mut magic).map_err(|e| {
                Error::IoError(format!("Failed to read magic bytes: {e}"))
            })?;
        }

        // Gzip magic: 0x1f 0x8b
        if magic != [0x1f, 0x8b] {
            // Clean up invalid file
            let _ = std::fs::remove_file(&output_path);
            return Err(Error::DownloadError(
                "Downloaded file is not a valid CCS package (expected gzip)".to_string()
            ));
        }

        Ok(output_path)
    }

    /// Fetch package by downloading chunks and assembling (legacy method)
    ///
    /// This method is kept for compatibility but the direct download endpoint
    /// is preferred as it's simpler and more reliable.
    #[allow(dead_code)]
    pub fn fetch_package_via_chunks(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        // Get manifest (may poll if conversion needed)
        let manifest = self.get_package(distro, name, version)?;

        // Create progress bar for chunk download
        let pb = ProgressBar::new(manifest.total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) {msg}")
                .expect("Invalid progress bar template")
                .progress_chars("#>-"),
        );

        // Download chunks
        let chunks = self.download_chunks(&manifest, Some(&pb))?;

        // Assemble package
        let output_path = output_dir.join(format!("{}-{}.ccs", manifest.name, manifest.version));
        Self::assemble_package(&manifest, &chunks, &output_path)?;

        Ok(output_path)
    }

    /// Check if Refinery is healthy
    pub fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);
        match self.client.get(&url).send() {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

/// Async Refinery client with HTTP/2 multiplexed chunk fetching
///
/// This client uses the ChunkFetcher trait for high-performance parallel
/// downloads with automatic caching and fallback support.
///
/// # Example
/// ```ignore
/// let client = AsyncRefineryClient::new("http://localhost:8080", "/var/cache/conary")?;
/// let manifest = client.get_package("arch", "nginx", None).await?;
/// let chunks = client.download_chunks(&manifest).await?;
/// client.assemble_package(&manifest, &chunks, Path::new("nginx.ccs"))?;
/// ```
#[cfg(feature = "server")]
pub struct AsyncRefineryClient {
    http_client: reqwest::Client,
    base_url: String,
    chunk_fetcher: Arc<CompositeChunkFetcher>,
}

#[cfg(feature = "server")]
impl AsyncRefineryClient {
    /// Create a new async Refinery client
    ///
    /// # Arguments
    /// * `base_url` - Base URL of the Refinery server
    /// * `cache_dir` - Directory for local chunk cache
    pub fn new(base_url: &str, cache_dir: impl AsRef<Path>) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();

        let http_client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {e}")))?;

        // Build chunk fetcher: local cache -> HTTP
        let chunk_fetcher = ChunkFetcherBuilder::new()
            .with_local_cache(&cache_dir)
            .with_http_concurrent(&base_url, 16)? // 16 concurrent HTTP/2 streams
            .build();

        Ok(Self {
            http_client,
            base_url,
            chunk_fetcher: Arc::new(chunk_fetcher),
        })
    }

    /// Create with a custom chunk fetcher
    ///
    /// Allows injecting custom fetcher chains for testing or special configurations.
    pub fn with_fetcher(base_url: &str, fetcher: CompositeChunkFetcher) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();

        let http_client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            http_client,
            base_url,
            chunk_fetcher: Arc::new(fetcher),
        })
    }

    /// Request a package manifest from the Refinery
    ///
    /// Returns the manifest when the package is ready. If conversion is needed,
    /// this will poll automatically until complete or timeout.
    pub async fn get_package(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
    ) -> Result<PackageManifest> {
        let url = if let Some(v) = version {
            format!("{}/v1/{}/packages/{}?version={}", self.base_url, distro, name, v)
        } else {
            format!("{}/v1/{}/packages/{}", self.base_url, distro, name)
        };

        info!("Requesting package from Refinery: {}", url);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            Error::DownloadError(format!("Failed to connect to Refinery: {e}"))
        })?;

        match response.status().as_u16() {
            200 => {
                let manifest: PackageManifest = response.json().await.map_err(|e| {
                    Error::DownloadError(format!("Failed to parse package manifest: {e}"))
                })?;
                info!(
                    "Package ready: {} chunks, {} bytes",
                    manifest.chunks.len(),
                    manifest.total_size
                );
                Ok(manifest)
            }
            202 => {
                let accepted: ConversionAccepted = response.json().await.map_err(|e| {
                    Error::DownloadError(format!("Failed to parse 202 response: {e}"))
                })?;
                info!(
                    "Package conversion queued (job {}), ETA: {:?}s",
                    accepted.job_id, accepted.eta_seconds
                );
                self.poll_for_completion_async(&accepted.job_id).await
            }
            404 => Err(Error::NotFoundError(format!(
                "Package '{}' not found in {} repositories",
                name, distro
            ))),
            503 => Err(Error::DownloadError(
                "Refinery conversion queue is full, try again later".to_string(),
            )),
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(Error::DownloadError(format!(
                    "Refinery returned HTTP {}: {}",
                    status, body
                )))
            }
        }
    }

    /// Poll for job completion (async version)
    async fn poll_for_completion_async(&self, job_id: &str) -> Result<PackageManifest> {
        let url = format!("{}/v1/jobs/{}", self.base_url, job_id);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > POLL_TIMEOUT {
                return Err(Error::TimeoutError(format!(
                    "Conversion job {} timed out after {:?}",
                    job_id, POLL_TIMEOUT
                )));
            }

            let response = self.http_client.get(&url).send().await.map_err(|e| {
                Error::DownloadError(format!("Failed to poll job status: {e}"))
            })?;

            if !response.status().is_success() {
                return Err(Error::DownloadError(format!(
                    "Job poll returned HTTP {}",
                    response.status()
                )));
            }

            let status: JobStatus = response.json().await.map_err(|e| {
                Error::DownloadError(format!("Failed to parse job status: {e}"))
            })?;

            match status.status.as_str() {
                "ready" => {
                    info!("Conversion complete for job {}", job_id);
                    if let Some(manifest) = status.manifest {
                        return Ok(manifest);
                    }
                    let version = status.version.as_deref();
                    return Box::pin(self.get_package(&status.distro, &status.package, version))
                        .await;
                }
                "failed" => {
                    let error_msg = status.error.unwrap_or_else(|| "Unknown error".to_string());
                    return Err(Error::DownloadError(format!(
                        "Conversion failed: {}",
                        error_msg
                    )));
                }
                "converting" | "queued" => {
                    if let Some(progress) = status.progress {
                        debug!("Converting {} ({}%)...", status.package, progress);
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
                other => {
                    warn!("Unknown job status: {}", other);
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }

    /// Download all chunks for a package using HTTP/2 multiplexing
    ///
    /// Uses the ChunkFetcher for parallel downloads with automatic local caching.
    /// This is significantly faster than sequential downloads for packages with
    /// many small chunks.
    pub async fn download_chunks(
        &self,
        manifest: &PackageManifest,
    ) -> Result<HashMap<String, Vec<u8>>> {
        let hashes: Vec<String> = manifest.chunks.iter().map(|c| c.hash.clone()).collect();

        info!(
            "Downloading {} chunks for {} via HTTP/2 ({} bytes total)",
            hashes.len(),
            manifest.name,
            manifest.total_size
        );

        let start = std::time::Instant::now();
        let chunks = self.chunk_fetcher.fetch_many(&hashes).await?;
        let elapsed = start.elapsed();

        let total_bytes: usize = chunks.values().map(|v| v.len()).sum();
        let throughput = total_bytes as f64 / elapsed.as_secs_f64() / 1024.0 / 1024.0;

        info!(
            "Downloaded {} chunks ({} bytes) in {:.2}s ({:.2} MB/s)",
            chunks.len(),
            total_bytes,
            elapsed.as_secs_f64(),
            throughput
        );

        Ok(chunks)
    }

    /// Assemble a CCS package from downloaded chunks
    ///
    /// Writes chunks to the output file in order according to manifest offsets.
    /// This is synchronous as it's I/O bound and doesn't benefit from async.
    pub fn assemble_package(
        manifest: &PackageManifest,
        chunks: &HashMap<String, Vec<u8>>,
        output_path: &Path,
    ) -> Result<()> {
        // Delegate to the sync implementation
        RefineryClient::assemble_package(manifest, chunks, output_path)
    }

    /// High-level: Fetch and assemble a package
    ///
    /// Gets the manifest, downloads chunks in parallel, and assembles the package.
    pub async fn fetch_package(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        // Get manifest
        let manifest = self.get_package(distro, name, version).await?;

        // Download chunks in parallel
        let chunks = self.download_chunks(&manifest).await?;

        // Assemble package
        let output_path = output_dir.join(format!("{}-{}.ccs", manifest.name, manifest.version));
        Self::assemble_package(&manifest, &chunks, &output_path)?;

        Ok(output_path)
    }

    /// Download chunks with progress callback
    ///
    /// For UI integration that needs progress updates.
    pub async fn download_chunks_with_progress<F>(
        &self,
        manifest: &PackageManifest,
        mut on_progress: F,
    ) -> Result<HashMap<String, Vec<u8>>>
    where
        F: FnMut(usize, usize) + Send, // (completed, total)
    {
        let hashes: Vec<String> = manifest.chunks.iter().map(|c| c.hash.clone()).collect();
        let total = hashes.len();

        info!("Downloading {} chunks with progress tracking", total);

        // For now, download all at once and report completion
        // A more sophisticated implementation would use streaming
        let chunks = self.chunk_fetcher.fetch_many(&hashes).await?;
        on_progress(total, total);

        Ok(chunks)
    }

    /// Check if Refinery is healthy
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);
        match self.http_client.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    /// Get the underlying chunk fetcher for advanced use cases
    pub fn chunk_fetcher(&self) -> Arc<CompositeChunkFetcher> {
        self.chunk_fetcher.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_url_normalization() {
        // With trailing slash
        let client = RefineryClient::new("http://localhost:8080/").unwrap();
        assert_eq!(client.base_url, "http://localhost:8080");

        // Without trailing slash
        let client = RefineryClient::new("http://localhost:8080").unwrap();
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_conversion_accepted_parsing() {
        let json = r#"{"status":"queued","job_id":"123","poll_url":"/v1/jobs/123","eta_seconds":30}"#;
        let accepted: ConversionAccepted = serde_json::from_str(json).unwrap();
        assert_eq!(accepted.status, "queued");
        assert_eq!(accepted.job_id, "123");
        assert_eq!(accepted.eta_seconds, Some(30));
    }

    #[test]
    fn test_job_status_parsing() {
        let json = r#"{"job_id":"1","status":"ready","distro":"arch","package":"gzip","version":null,"progress":null,"error":null,"manifest":null}"#;
        let status: JobStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status.status, "ready");
        assert_eq!(status.package, "gzip");
    }

    #[cfg(feature = "server")]
    mod async_tests {
        use super::*;

        #[test]
        fn test_async_client_base_url_normalization() {
            let temp_dir = tempfile::tempdir().unwrap();

            // With trailing slash
            let client = AsyncRefineryClient::new("http://localhost:8080/", temp_dir.path()).unwrap();
            assert_eq!(client.base_url, "http://localhost:8080");

            // Without trailing slash
            let client = AsyncRefineryClient::new("http://localhost:8080", temp_dir.path()).unwrap();
            assert_eq!(client.base_url, "http://localhost:8080");
        }

        #[test]
        fn test_async_client_with_custom_fetcher() {
            use crate::repository::chunk_fetcher::{CompositeChunkFetcher, LocalCacheFetcher};

            let temp_dir = tempfile::tempdir().unwrap();
            let cache = LocalCacheFetcher::new(temp_dir.path());
            let fetcher = CompositeChunkFetcher::new(vec![Arc::new(cache)]);

            let client = AsyncRefineryClient::with_fetcher("http://localhost:8080", fetcher).unwrap();
            assert_eq!(client.base_url, "http://localhost:8080");
        }

        #[tokio::test]
        async fn test_async_client_health_check_unreachable() {
            let temp_dir = tempfile::tempdir().unwrap();
            let client = AsyncRefineryClient::new("http://localhost:59999", temp_dir.path()).unwrap();

            // Should return false for unreachable server
            let result = client.health_check().await.unwrap();
            assert!(!result);
        }

        #[test]
        fn test_manifest_parsing() {
            let json = r#"{
                "name": "nginx",
                "version": "1.24.0",
                "distro": "arch",
                "chunks": [
                    {"hash": "abc123", "size": 1024, "offset": 0},
                    {"hash": "def456", "size": 2048, "offset": 1024}
                ],
                "total_size": 3072,
                "content_hash": "xyz789"
            }"#;

            let manifest: PackageManifest = serde_json::from_str(json).unwrap();
            assert_eq!(manifest.name, "nginx");
            assert_eq!(manifest.chunks.len(), 2);
            assert_eq!(manifest.total_size, 3072);
        }
    }
}
