// src/server/mod.rs
//! Conary Remi Server - On-demand CCS conversion proxy
//!
//! This module provides an HTTP server that:
//! - Serves repository metadata (proxied through Cloudflare)
//! - Serves CCS chunks (direct from origin)
//! - Converts legacy packages (RPM/DEB/Arch) to CCS on-demand
//! - Uses LRU cache eviction to manage disk space
//!
//! Phase 0 hardening features:
//! - Bloom filter for fast negative lookups (DoS protection)
//! - Pull-through caching (fetch from upstream on miss)
//! - Batch endpoints for efficient multi-chunk operations
//! - Metrics tracking for observability
//! - Rate limiting per IP/peer

mod bloom;
mod cache;
mod conversion;
mod handlers;
mod index_gen;
mod jobs;
pub mod metrics;
mod prewarm;
mod routes;
pub mod security;

pub use bloom::{BloomStats, ChunkBloomFilter};
pub use cache::ChunkCache;
pub use conversion::{ConversionService, ServerConversionResult};
pub use index_gen::{generate_indices, IndexGenConfig, IndexGenResult};
pub use jobs::{ConversionJob, JobManager, JobStatus};
pub use metrics::{MetricsSnapshot, ServerMetrics};
pub use prewarm::{run_prewarm, PrewarmConfig, PrewarmResult};
pub use routes::create_router;
pub use security::BanList;

use anyhow::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind to
    pub bind_addr: SocketAddr,
    /// Path to the Conary database
    pub db_path: PathBuf,
    /// Path to the chunk store
    pub chunk_dir: PathBuf,
    /// Path to the cache/scratch directory
    pub cache_dir: PathBuf,
    /// Maximum concurrent conversions
    pub max_concurrent_conversions: usize,
    /// LRU eviction threshold in bytes (default 700GB)
    pub cache_max_bytes: u64,
    /// Chunk TTL in days before LRU eviction
    pub chunk_ttl_days: u32,

    // === Phase 0 additions ===
    /// Enable Bloom filter for fast negative lookups
    pub enable_bloom_filter: bool,
    /// Expected number of chunks (for Bloom filter sizing)
    pub bloom_expected_chunks: usize,
    /// Upstream URL for pull-through caching (None = disabled)
    pub upstream_url: Option<String>,
    /// Request timeout for upstream fetches
    pub upstream_timeout: Duration,
    /// Enable rate limiting
    pub enable_rate_limit: bool,
    /// Rate limit: requests per second per IP
    pub rate_limit_rps: u32,
    /// Rate limit: burst size
    pub rate_limit_burst: u32,

    // === Security (Phase 4) ===
    /// CORS allowed origins for chunk endpoints (empty = deny all external)
    pub cors_allowed_origins: Vec<String>,
    /// Enable audit logging for requests
    pub enable_audit_log: bool,
    /// Ban threshold: consecutive failures before temporary ban
    pub ban_threshold: u32,
    /// Ban duration in seconds
    pub ban_duration_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8080".parse().unwrap(),
            db_path: PathBuf::from("/var/lib/conary/conary.db"),
            chunk_dir: PathBuf::from("/var/lib/conary/data/chunks"),
            cache_dir: PathBuf::from("/var/lib/conary/data/cache"),
            max_concurrent_conversions: 4,
            cache_max_bytes: 700 * 1024 * 1024 * 1024, // 700GB
            chunk_ttl_days: 30,
            // Phase 0 defaults
            enable_bloom_filter: true,
            bloom_expected_chunks: 1_000_000,
            upstream_url: None,
            upstream_timeout: Duration::from_secs(30),
            enable_rate_limit: true,
            rate_limit_rps: 100,
            rate_limit_burst: 200,
            // Security defaults
            cors_allowed_origins: Vec::new(), // Empty = same-origin only for chunks
            enable_audit_log: true,
            ban_threshold: 10,
            ban_duration_secs: 300, // 5 minutes
        }
    }
}

/// Shared server state
pub struct ServerState {
    pub config: ServerConfig,
    pub job_manager: JobManager,
    pub chunk_cache: ChunkCache,
    pub conversion_service: ConversionService,
    /// Bloom filter for fast negative chunk lookups
    pub bloom_filter: Option<Arc<ChunkBloomFilter>>,
    /// HTTP client for upstream fetches
    pub http_client: reqwest::Client,
    /// Metrics collector
    pub metrics: Arc<ServerMetrics>,
    /// Ban list for misbehaving IPs
    pub ban_list: Arc<BanList>,
}

impl ServerState {
    pub fn new(config: ServerConfig) -> Self {
        let job_manager = JobManager::new(config.max_concurrent_conversions);
        let chunk_cache = ChunkCache::new(
            config.chunk_dir.clone(),
            config.cache_max_bytes,
            config.chunk_ttl_days,
            config.db_path.clone(),
        );
        let conversion_service = ConversionService::new(
            config.chunk_dir.clone(),
            config.cache_dir.clone(),
            config.db_path.clone(),
        );

        // Initialize Bloom filter if enabled
        let bloom_filter = if config.enable_bloom_filter {
            tracing::info!(
                "Initializing Bloom filter for {} expected chunks",
                config.bloom_expected_chunks
            );
            Some(Arc::new(ChunkBloomFilter::new(
                config.bloom_expected_chunks,
                0.01, // 1% false positive rate
            )))
        } else {
            None
        };

        // Create HTTP client for upstream fetches
        let http_client = reqwest::Client::builder()
            .timeout(config.upstream_timeout)
            .user_agent("conary-remi/0.1")
            .build()
            .expect("Failed to create HTTP client");

        let metrics = Arc::new(ServerMetrics::new());
        let ban_list = Arc::new(BanList::new(config.ban_duration_secs, config.ban_threshold));

        Self {
            config,
            job_manager,
            chunk_cache,
            conversion_service,
            bloom_filter,
            http_client,
            metrics,
            ban_list,
        }
    }
}

/// Start the Remi server
pub async fn run_server(config: ServerConfig) -> Result<()> {
    tracing::info!("Starting Conary Remi server on {}", config.bind_addr);
    tracing::info!("Database: {:?}", config.db_path);
    tracing::info!("Chunk store: {:?}", config.chunk_dir);
    tracing::info!("Max concurrent conversions: {}", config.max_concurrent_conversions);

    if config.enable_bloom_filter {
        tracing::info!("Bloom filter: enabled ({} expected chunks)", config.bloom_expected_chunks);
    }
    if let Some(ref upstream) = config.upstream_url {
        tracing::info!("Pull-through caching: enabled (upstream: {})", upstream);
    }
    if config.enable_rate_limit {
        tracing::info!(
            "Rate limiting: {} rps, {} burst",
            config.rate_limit_rps,
            config.rate_limit_burst
        );
    }

    let state = Arc::new(RwLock::new(ServerState::new(config.clone())));

    // Initialize Bloom filter from existing chunks
    if config.enable_bloom_filter {
        let state_clone = state.clone();
        tokio::spawn(async move {
            if let Err(e) = initialize_bloom_filter(state_clone).await {
                tracing::error!("Failed to initialize Bloom filter: {}", e);
            }
        });
    }

    let app = create_router(state.clone());

    // Start background LRU eviction task
    let eviction_state = state.clone();
    tokio::spawn(async move {
        cache::run_eviction_loop(eviction_state).await;
    });

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!("Remi is ready to serve");

    axum::serve(listener, app).await?;
    Ok(())
}

/// Initialize Bloom filter by scanning existing chunks
async fn initialize_bloom_filter(state: Arc<RwLock<ServerState>>) -> Result<()> {
    let state_guard = state.read().await;

    let bloom = match &state_guard.bloom_filter {
        Some(b) => Arc::clone(b),
        None => return Ok(()),
    };

    let objects_dir = state_guard.config.chunk_dir.join("objects");
    drop(state_guard);

    if !objects_dir.exists() {
        tracing::info!("No existing chunks to index in Bloom filter");
        return Ok(());
    }

    tracing::info!("Scanning existing chunks for Bloom filter...");

    let mut count = 0;
    let mut stack = vec![objects_dir];

    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = entry.metadata().await?;

            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                // Skip temp files
                if path.extension().is_some_and(|ext| ext == "tmp") {
                    continue;
                }

                // Extract hash from path
                if let Some(hash) = extract_hash_from_path(&path) {
                    bloom.add(&hash);
                    count += 1;
                }
            }
        }
    }

    tracing::info!("Bloom filter initialized with {} chunks", count);
    Ok(())
}

/// Extract hash from chunk path (e.g., /chunks/objects/ab/cdef... -> abcdef...)
fn extract_hash_from_path(path: &std::path::Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let prefix = parent.file_name()?.to_str()?;
    Some(format!("{}{}", prefix, file_name))
}
