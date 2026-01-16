// src/repository/chunk_fetcher.rs
//! Chunk fetcher trait and implementations
//!
//! Provides a transport abstraction for fetching chunks from various backends:
//! - HTTP/HTTPS (CDN, S3, nginx)
//! - Local filesystem cache
//! - Future: IPFS, BitTorrent DHT
//!
//! Fetchers can be composed into chains with fallback behavior.

use crate::error::{Error, Result};
use crate::hash::verify_sha256;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Result of a chunk fetch operation
#[derive(Debug)]
pub struct ChunkData {
    /// The chunk hash (SHA-256)
    pub hash: String,
    /// The chunk data
    pub data: Vec<u8>,
    /// Which fetcher retrieved this chunk
    pub source: String,
}

/// Trait for fetching chunks from various backends
#[async_trait]
pub trait ChunkFetcher: Send + Sync {
    /// Fetch a single chunk by its SHA-256 hash
    ///
    /// Returns the chunk data if found, or an error if not available.
    async fn fetch(&self, hash: &str) -> Result<Vec<u8>>;

    /// Check if a chunk exists without downloading it
    ///
    /// Default implementation tries to fetch and discards the result.
    async fn exists(&self, hash: &str) -> bool {
        self.fetch(hash).await.is_ok()
    }

    /// Fetch multiple chunks in parallel
    ///
    /// Default implementation fetches sequentially. Implementations should
    /// override this for better performance (e.g., HTTP/2 multiplexing).
    async fn fetch_many(&self, hashes: &[String]) -> Result<HashMap<String, Vec<u8>>> {
        let mut results = HashMap::new();
        for hash in hashes {
            let data = self.fetch(hash).await?;
            results.insert(hash.clone(), data);
        }
        Ok(results)
    }

    /// Get a human-readable name for this fetcher (for logging/metrics)
    fn name(&self) -> &str;
}

/// HTTP chunk fetcher using reqwest
///
/// Supports HTTP/2 multiplexing for parallel chunk fetching.
pub struct HttpChunkFetcher {
    client: reqwest::Client,
    base_url: String,
    /// Maximum concurrent requests
    max_concurrent: usize,
    /// Whether to verify chunk hashes
    verify_hashes: bool,
}

impl HttpChunkFetcher {
    /// Create a new HTTP chunk fetcher
    pub fn new(base_url: &str) -> Result<Self> {
        Self::with_options(base_url, 8, true)
    }

    /// Create with custom options
    pub fn with_options(base_url: &str, max_concurrent: usize, verify_hashes: bool) -> Result<Self> {
        let client = reqwest::Client::builder()
            .http2_prior_knowledge() // Use HTTP/2 for multiplexing
            .pool_max_idle_per_host(max_concurrent)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {e}")))?;

        let base_url = base_url.trim_end_matches('/').to_string();

        Ok(Self {
            client,
            base_url,
            max_concurrent,
            verify_hashes,
        })
    }

    /// Verify a chunk's hash matches its content
    ///
    /// Uses the shared hash module for consistent SHA-256 verification.
    fn verify_hash(hash: &str, data: &[u8]) -> Result<()> {
        verify_sha256(data, hash).map_err(|e| Error::ChecksumMismatch {
            expected: e.expected,
            actual: e.actual,
        })
    }
}

#[async_trait]
impl ChunkFetcher for HttpChunkFetcher {
    async fn fetch(&self, hash: &str) -> Result<Vec<u8>> {
        let url = format!("{}/v1/chunks/{}", self.base_url, hash);
        debug!("Fetching chunk via HTTP: {}", hash);

        let response = self.client.get(&url).send().await.map_err(|e| {
            Error::DownloadError(format!("Failed to fetch chunk {}: {e}", hash))
        })?;

        if !response.status().is_success() {
            return Err(Error::DownloadError(format!(
                "Chunk {} returned HTTP {}",
                hash,
                response.status()
            )));
        }

        let data = response.bytes().await.map_err(|e| {
            Error::DownloadError(format!("Failed to read chunk {}: {e}", hash))
        })?;

        if self.verify_hashes {
            Self::verify_hash(hash, &data)?;
        }

        Ok(data.to_vec())
    }

    async fn exists(&self, hash: &str) -> bool {
        let url = format!("{}/v1/chunks/{}", self.base_url, hash);
        match self.client.head(&url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    async fn fetch_many(&self, hashes: &[String]) -> Result<HashMap<String, Vec<u8>>> {
        use futures::stream::{self, StreamExt};

        let semaphore = Arc::new(Semaphore::new(self.max_concurrent));
        let client = &self.client;
        let base_url = &self.base_url;
        let verify = self.verify_hashes;

        info!("Fetching {} chunks via HTTP/2 (max {} concurrent)", hashes.len(), self.max_concurrent);

        let fetches = stream::iter(hashes.iter().cloned())
            .map(|hash| {
                let permit = semaphore.clone();
                let client = client.clone();
                let base_url = base_url.clone();
                async move {
                    let _permit = permit.acquire().await.unwrap();
                    let url = format!("{}/v1/chunks/{}", base_url, hash);

                    let response = client.get(&url).send().await.map_err(|e| {
                        Error::DownloadError(format!("Failed to fetch chunk {}: {e}", hash))
                    })?;

                    if !response.status().is_success() {
                        return Err(Error::DownloadError(format!(
                            "Chunk {} returned HTTP {}",
                            hash,
                            response.status()
                        )));
                    }

                    let data = response.bytes().await.map_err(|e| {
                        Error::DownloadError(format!("Failed to read chunk {}: {e}", hash))
                    })?;

                    if verify {
                        Self::verify_hash(&hash, &data)?;
                    }

                    Ok::<_, Error>((hash, data.to_vec()))
                }
            })
            .buffer_unordered(self.max_concurrent);

        let results: Vec<_> = fetches.collect().await;

        let mut chunks = HashMap::new();
        for result in results {
            let (hash, data) = result?;
            chunks.insert(hash, data);
        }

        Ok(chunks)
    }

    fn name(&self) -> &str {
        "http"
    }
}

/// Local filesystem cache fetcher
///
/// Checks a local directory for cached chunks before falling back to network.
pub struct LocalCacheFetcher {
    cache_dir: PathBuf,
}

impl LocalCacheFetcher {
    /// Create a new local cache fetcher
    pub fn new(cache_dir: impl AsRef<Path>) -> Self {
        Self {
            cache_dir: cache_dir.as_ref().to_path_buf(),
        }
    }

    /// Get the path for a chunk hash
    fn chunk_path(&self, hash: &str) -> PathBuf {
        // Use two-level directory structure: {hash[0:2]}/{hash[2:]}
        let (prefix, rest) = hash.split_at(2.min(hash.len()));
        self.cache_dir.join("objects").join(prefix).join(rest)
    }

    /// Store a chunk in the cache
    pub async fn store(&self, hash: &str, data: &[u8]) -> Result<()> {
        let path = self.chunk_path(hash);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                Error::IoError(format!("Failed to create cache directory: {e}"))
            })?;
        }

        // Write atomically via temp file
        let temp_path = path.with_extension("tmp");
        tokio::fs::write(&temp_path, data).await.map_err(|e| {
            Error::IoError(format!("Failed to write chunk to cache: {e}"))
        })?;
        tokio::fs::rename(&temp_path, &path).await.map_err(|e| {
            Error::IoError(format!("Failed to rename temp file: {e}"))
        })?;

        debug!("Cached chunk: {}", hash);
        Ok(())
    }
}

#[async_trait]
impl ChunkFetcher for LocalCacheFetcher {
    async fn fetch(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.chunk_path(hash);

        if !path.exists() {
            return Err(Error::NotFoundError(format!("Chunk {} not in cache", hash)));
        }

        let data = tokio::fs::read(&path).await.map_err(|e| {
            Error::IoError(format!("Failed to read cached chunk {}: {e}", hash))
        })?;

        debug!("Cache hit: {}", hash);
        Ok(data)
    }

    async fn exists(&self, hash: &str) -> bool {
        self.chunk_path(hash).exists()
    }

    fn name(&self) -> &str {
        "local-cache"
    }
}

/// Composite fetcher that tries multiple backends in order
///
/// Provides fallback behavior: try local cache first, then CDN, then origin.
pub struct CompositeChunkFetcher {
    fetchers: Vec<Arc<dyn ChunkFetcher>>,
    /// Optional local cache to store fetched chunks
    cache: Option<LocalCacheFetcher>,
}

impl CompositeChunkFetcher {
    /// Create a new composite fetcher
    pub fn new(fetchers: Vec<Arc<dyn ChunkFetcher>>) -> Self {
        Self {
            fetchers,
            cache: None,
        }
    }

    /// Create with automatic caching of fetched chunks
    pub fn with_cache(fetchers: Vec<Arc<dyn ChunkFetcher>>, cache_dir: impl AsRef<Path>) -> Self {
        Self {
            fetchers,
            cache: Some(LocalCacheFetcher::new(cache_dir)),
        }
    }

    /// Add a fetcher to the chain
    pub fn add_fetcher(&mut self, fetcher: Arc<dyn ChunkFetcher>) {
        self.fetchers.push(fetcher);
    }
}

#[async_trait]
impl ChunkFetcher for CompositeChunkFetcher {
    async fn fetch(&self, hash: &str) -> Result<Vec<u8>> {
        let mut last_error = None;

        for fetcher in &self.fetchers {
            match fetcher.fetch(hash).await {
                Ok(data) => {
                    // Cache the result if we have a cache and this wasn't from cache
                    if let Some(cache) = &self.cache
                        && fetcher.name() != "local-cache"
                        && let Err(e) = cache.store(hash, &data).await
                    {
                        warn!("Failed to cache chunk {}: {}", hash, e);
                    }
                    return Ok(data);
                }
                Err(e) => {
                    debug!("Fetcher '{}' failed for chunk {}: {}", fetcher.name(), hash, e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            Error::NotFoundError(format!("No fetchers available for chunk {}", hash))
        }))
    }

    async fn exists(&self, hash: &str) -> bool {
        for fetcher in &self.fetchers {
            if fetcher.exists(hash).await {
                return true;
            }
        }
        false
    }

    async fn fetch_many(&self, hashes: &[String]) -> Result<HashMap<String, Vec<u8>>> {
        // Try to fetch all from cache first
        let mut results = HashMap::new();
        let mut remaining: Vec<String> = Vec::new();

        // Check cache for each hash
        if let Some(cache_fetcher) = self.fetchers.iter().find(|f| f.name() == "local-cache") {
            for hash in hashes {
                match cache_fetcher.fetch(hash).await {
                    Ok(data) => {
                        results.insert(hash.clone(), data);
                    }
                    Err(_) => {
                        remaining.push(hash.clone());
                    }
                }
            }
        } else {
            remaining = hashes.to_vec();
        }

        if remaining.is_empty() {
            return Ok(results);
        }

        info!(
            "Cache hit: {}/{}, fetching {} from network",
            results.len(),
            hashes.len(),
            remaining.len()
        );

        // Fetch remaining from network fetchers
        for fetcher in &self.fetchers {
            if fetcher.name() == "local-cache" {
                continue;
            }

            match fetcher.fetch_many(&remaining).await {
                Ok(fetched) => {
                    // Cache the results
                    if let Some(cache) = &self.cache {
                        for (hash, data) in &fetched {
                            if let Err(e) = cache.store(hash, data).await {
                                warn!("Failed to cache chunk {}: {}", hash, e);
                            }
                        }
                    }

                    results.extend(fetched);
                    return Ok(results);
                }
                Err(e) => {
                    warn!("Fetcher '{}' failed for batch: {}", fetcher.name(), e);
                }
            }
        }

        Err(Error::DownloadError(format!(
            "Failed to fetch {} chunks from all fetchers",
            remaining.len()
        )))
    }

    fn name(&self) -> &str {
        "composite"
    }
}

/// Builder for creating chunk fetcher chains
pub struct ChunkFetcherBuilder {
    fetchers: Vec<Arc<dyn ChunkFetcher>>,
    cache_dir: Option<PathBuf>,
}

impl ChunkFetcherBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            fetchers: Vec::new(),
            cache_dir: None,
        }
    }

    /// Add a local cache as the first fetcher
    pub fn with_local_cache(mut self, cache_dir: impl AsRef<Path>) -> Self {
        let cache = LocalCacheFetcher::new(&cache_dir);
        self.fetchers.insert(0, Arc::new(cache));
        self.cache_dir = Some(cache_dir.as_ref().to_path_buf());
        self
    }

    /// Add an HTTP fetcher
    pub fn with_http(mut self, base_url: &str) -> Result<Self> {
        let fetcher = HttpChunkFetcher::new(base_url)?;
        self.fetchers.push(Arc::new(fetcher));
        Ok(self)
    }

    /// Add an HTTP fetcher with custom concurrency
    pub fn with_http_concurrent(mut self, base_url: &str, max_concurrent: usize) -> Result<Self> {
        let fetcher = HttpChunkFetcher::with_options(base_url, max_concurrent, true)?;
        self.fetchers.push(Arc::new(fetcher));
        Ok(self)
    }

    /// Add a custom fetcher
    pub fn with_fetcher(mut self, fetcher: Arc<dyn ChunkFetcher>) -> Self {
        self.fetchers.push(fetcher);
        self
    }

    /// Build the composite fetcher
    pub fn build(self) -> CompositeChunkFetcher {
        if let Some(cache_dir) = self.cache_dir {
            CompositeChunkFetcher::with_cache(self.fetchers, cache_dir)
        } else {
            CompositeChunkFetcher::new(self.fetchers)
        }
    }
}

impl Default for ChunkFetcherBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256;

    #[test]
    fn test_local_cache_path() {
        let cache = LocalCacheFetcher::new("/var/cache/conary");
        let path = cache.chunk_path("abcdef1234567890");
        assert!(path.to_string_lossy().contains("objects/ab/cdef1234567890"));
    }

    #[test]
    fn test_hash_verification() {
        let data = b"hello world";
        let hash = sha256(data);

        // Valid hash should pass
        assert!(HttpChunkFetcher::verify_hash(&hash, data).is_ok());

        // Invalid hash should fail
        assert!(HttpChunkFetcher::verify_hash("wrong", data).is_err());
    }

    #[test]
    fn test_builder() {
        let builder = ChunkFetcherBuilder::new()
            .with_local_cache("/tmp/test-cache");

        let composite = builder.build();
        assert_eq!(composite.fetchers.len(), 1);
        assert_eq!(composite.fetchers[0].name(), "local-cache");
    }

    #[tokio::test]
    async fn test_local_cache_store_and_fetch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = LocalCacheFetcher::new(temp_dir.path());

        let data = b"test chunk data";
        let hash = sha256(data);

        // Store
        cache.store(&hash, data).await.unwrap();

        // Fetch
        let fetched = cache.fetch(&hash).await.unwrap();
        assert_eq!(fetched, data);

        // Exists
        assert!(cache.exists(&hash).await);
        assert!(!cache.exists("nonexistent").await);
    }

    #[tokio::test]
    async fn test_composite_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Create cache with one chunk
        let cache = LocalCacheFetcher::new(temp_dir.path());
        let data = b"cached chunk";
        let hash = sha256(data);
        cache.store(&hash, data).await.unwrap();

        // Create composite with just cache (no network)
        let composite = CompositeChunkFetcher::new(vec![Arc::new(cache)]);

        // Should find cached chunk
        let result = composite.fetch(&hash).await.unwrap();
        assert_eq!(result, data);

        // Should fail for non-existent chunk
        assert!(composite.fetch("nonexistent").await.is_err());
    }
}
