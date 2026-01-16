// src/server/mod.rs
//! Conary Refinery Server - On-demand CCS conversion proxy
//!
//! This module provides an HTTP server that:
//! - Serves repository metadata (proxied through Cloudflare)
//! - Serves CCS chunks (direct from origin)
//! - Converts legacy packages (RPM/DEB/Arch) to CCS on-demand
//! - Uses LRU cache eviction to manage disk space

mod cache;
mod conversion;
mod handlers;
mod jobs;
mod routes;

pub use cache::ChunkCache;
pub use conversion::{ConversionService, ServerConversionResult};
pub use jobs::{ConversionJob, JobManager, JobStatus};
pub use routes::create_router;

use anyhow::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
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
        }
    }
}

/// Shared server state
pub struct ServerState {
    pub config: ServerConfig,
    pub job_manager: JobManager,
    pub chunk_cache: ChunkCache,
    pub conversion_service: ConversionService,
}

impl ServerState {
    pub fn new(config: ServerConfig) -> Self {
        let job_manager = JobManager::new(config.max_concurrent_conversions);
        let chunk_cache = ChunkCache::new(
            config.chunk_dir.clone(),
            config.cache_max_bytes,
            config.chunk_ttl_days,
        );
        let conversion_service = ConversionService::new(
            config.chunk_dir.clone(),
            config.cache_dir.clone(),
            config.db_path.clone(),
        );
        Self {
            config,
            job_manager,
            chunk_cache,
            conversion_service,
        }
    }
}

/// Start the Refinery server
pub async fn run_server(config: ServerConfig) -> Result<()> {
    tracing::info!("Starting Conary Refinery server on {}", config.bind_addr);
    tracing::info!("Database: {:?}", config.db_path);
    tracing::info!("Chunk store: {:?}", config.chunk_dir);
    tracing::info!("Max concurrent conversions: {}", config.max_concurrent_conversions);

    let state = Arc::new(RwLock::new(ServerState::new(config.clone())));
    let app = create_router(state.clone());

    // Start background LRU eviction task
    let eviction_state = state.clone();
    tokio::spawn(async move {
        cache::run_eviction_loop(eviction_state).await;
    });

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!("Refinery is ready to serve");

    axum::serve(listener, app).await?;
    Ok(())
}
