// conary-core/src/repository/remi/async_client.rs

use super::*;

/// Async Remi client with HTTP/2 multiplexed chunk fetching.
///
/// This client uses the `ChunkFetcher` trait for high-performance parallel
/// downloads with automatic caching and fallback support.
///
/// # Example
/// ```ignore
/// let client = AsyncRemiClient::new("http://localhost:8080", "/var/cache/conary")?;
/// let manifest = client.get_package("arch", "nginx", None, None).await?;
/// let chunks = client.download_chunks(&manifest).await?;
/// client.assemble_package(&manifest, &chunks, Path::new("nginx.ccs"))?;
/// ```
pub struct AsyncRemiClient {
    http_client: reqwest::Client,
    pub(super) core: RemiClientCore,
    chunk_fetcher: Arc<CompositeChunkFetcher>,
}

impl AsyncRemiClient {
    /// Create a new async Remi client
    ///
    /// # Arguments
    /// * `base_url` - Base URL of the Remi server
    /// * `cache_dir` - Directory for local chunk cache
    pub fn new(base_url: &str, cache_dir: impl AsRef<Path>) -> Result<Self> {
        let core = RemiClientCore::new(base_url)?;

        let http_client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(http_client_builder_error_message(e)))?;

        // Build chunk fetcher: local cache -> HTTP
        let chunk_fetcher = ChunkFetcherBuilder::new()
            .with_local_cache(&cache_dir)
            .with_http_concurrent(&core.base_url, 16)? // 16 concurrent HTTP/2 streams
            .build();

        Ok(Self {
            http_client,
            core,
            chunk_fetcher: Arc::new(chunk_fetcher),
        })
    }

    /// Create with a custom chunk fetcher
    ///
    /// Allows injecting custom fetcher chains for testing or special configurations.
    pub fn with_fetcher(base_url: &str, fetcher: CompositeChunkFetcher) -> Result<Self> {
        let core = RemiClientCore::new(base_url)?;

        let http_client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(http_client_builder_error_message(e)))?;

        Ok(Self {
            http_client,
            core,
            chunk_fetcher: Arc::new(fetcher),
        })
    }

    /// Request a package manifest from the Remi
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

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .download_context(&url)?;

        match response.status().as_u16() {
            200 => {
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
                self.poll_for_completion_async(&accepted.job_id).await
            }
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(self.core.map_http_error(status, body, name, distro))
            }
        }
    }

    /// Poll for job completion (async version)
    async fn poll_for_completion_async(&self, job_id: &str) -> Result<PackageManifest> {
        let url = self.core.job_url(job_id);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > POLL_TIMEOUT {
                return Err(Error::TimeoutError(format!(
                    "Conversion job {} timed out after {:?}",
                    job_id, POLL_TIMEOUT
                )));
            }

            let response = self
                .http_client
                .get(&url)
                .send()
                .await
                .download_context(&url)?;

            if !response.status().is_success() {
                return Err(Error::DownloadError(format!(
                    "Job poll returned HTTP {}",
                    response.status()
                )));
            }

            let status: JobStatus = response.json().await.parse_context("job status")?;

            match status.status.as_str() {
                "ready" => {
                    info!("Conversion complete for job {}", job_id);
                    if let Some(manifest) = status.manifest {
                        return Ok(manifest);
                    }
                    let version = status.version.as_deref();
                    return Box::pin(self.get_package(
                        &status.distro,
                        &status.package,
                        version,
                        status.architecture.as_deref(),
                    ))
                    .await;
                }
                "failed" => {
                    let error_msg = status.error.unwrap_or_else(|| "Unknown error".to_string());
                    return Err(Error::DownloadError(format!(
                        "Conversion failed: {}",
                        error_msg
                    )));
                }
                "review-required" | "blocked" => {
                    return Err(
                        terminal_publication_status_error(&status).unwrap_or_else(|| {
                            Error::DownloadError(format!(
                                "Remi returned terminal conversion status {} for {}/{}",
                                status.status, status.distro, status.package
                            ))
                        }),
                    );
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
        RemiClient::assemble_package(manifest, chunks, output_path)
    }

    /// High-level: Fetch and assemble a package
    ///
    /// Gets the manifest, downloads chunks in parallel, and assembles the package.
    pub async fn fetch_package(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        output_dir: &Path,
    ) -> Result<PathBuf> {
        // Get manifest
        let manifest = self
            .get_package(distro, name, version, architecture)
            .await?;

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

    /// Check if Remi is healthy
    pub async fn health_check(&self) -> Result<bool> {
        let url = self.core.health_url();
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
