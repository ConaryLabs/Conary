// conary-server/src/server/mod.rs
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

pub mod analytics;
pub mod audit;
pub mod auth;
mod bloom;
mod cache;
pub mod config;
mod conversion;
pub mod delta_manifests;
pub mod federated_index;
pub mod forgejo;
mod handlers;
mod index_gen;
mod jobs;
pub mod lite;
pub mod mcp;
pub mod metrics;
mod negative_cache;
pub mod popularity;
mod prewarm;
pub mod r2;
pub mod rate_limit;
mod routes;
pub mod search;
pub mod security;

pub use analytics::AnalyticsRecorder;
pub use bloom::{BloomStats, ChunkBloomFilter};
pub use cache::ChunkCache;
pub use config::RemiConfig;
pub use conversion::{ConversionService, ServerConversionResult};
pub use index_gen::{IndexGenConfig, IndexGenResult, generate_indices};
pub use jobs::{ConversionJob, JobManager, JobStatus};
pub use lite::{ProxyConfig, run_proxy};
pub use metrics::{MetricsSnapshot, ServerMetrics};
pub use negative_cache::NegativeCache;
pub use prewarm::{PrewarmConfig, PrewarmResult, run_prewarm};
pub use r2::R2Store;
pub use routes::{create_admin_router, create_external_admin_router, create_router};
pub use search::SearchEngine;
pub use security::BanList;

use anyhow::Result;
use dashmap::DashMap;
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

    // === Web frontend ===
    /// Path to SvelteKit static build directory (None = disabled)
    pub web_root: Option<PathBuf>,
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
            web_root: None,
        }
    }
}

/// Event broadcast from admin operations (e.g., CI triggers, token changes)
#[derive(Clone, Debug, serde::Serialize)]
pub struct AdminEvent {
    /// Event type identifier (e.g., "token.created", "ci.triggered")
    pub event_type: String,
    /// Event payload
    pub data: serde_json::Value,
    /// ISO 8601 timestamp
    pub timestamp: String,
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
    /// Negative cache for "not found" responses
    pub negative_cache: Arc<NegativeCache>,
    /// Trusted proxy header for real IP extraction (e.g., "CF-Connecting-IP")
    pub trusted_proxy_header: Option<String>,
    /// R2 object storage for CDN-backed chunk distribution
    pub r2_store: Option<Arc<R2Store>>,
    /// Redirect chunk GET requests to R2 presigned URLs instead of streaming locally
    pub r2_redirect: bool,
    /// Full-text search engine (Tantivy)
    pub search_engine: Option<Arc<SearchEngine>>,
    /// Download analytics recorder (buffered writes)
    pub analytics: Option<Arc<AnalyticsRecorder>>,
    /// Federated sparse index configuration (from federation peers)
    pub federated_config: Option<federated_index::FederatedIndexConfig>,
    /// Federated sparse index cache (TTL-based in-memory cache)
    pub federated_cache: Option<Arc<federated_index::FederatedIndexCache>>,
    /// In-flight upstream fetches for request coalescing (thundering herd prevention).
    /// Key is chunk hash; value is a broadcast sender that waiters subscribe to.
    /// When the first fetch completes, all waiters are notified.
    pub inflight_fetches: Arc<DashMap<String, tokio::sync::broadcast::Sender<()>>>,
    /// Forgejo instance URL for CI proxy (from config)
    pub forgejo_url: Option<String>,
    /// Forgejo API token for CI proxy (from config)
    pub forgejo_token: Option<String>,
    /// Broadcast channel for admin events (SSE stream)
    pub admin_events: tokio::sync::broadcast::Sender<AdminEvent>,
    /// Rate limiters for the external admin API
    pub rate_limiters: Option<Arc<crate::server::rate_limit::AdminRateLimiters>>,
}

impl ServerState {
    pub fn new(config: ServerConfig) -> Self {
        Self::with_options(config, None, Duration::from_secs(15 * 60))
    }

    /// Publish an admin event to SSE subscribers.
    ///
    /// The send error is intentionally ignored — it only occurs when no
    /// subscribers are connected, which is perfectly normal.
    pub fn publish_event(&self, event_type: &str, data: serde_json::Value) {
        let event = AdminEvent {
            event_type: event_type.to_string(),
            data,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        let _ = self.admin_events.send(event);
    }

    pub fn with_options(
        config: ServerConfig,
        trusted_proxy_header: Option<String>,
        negative_cache_ttl: Duration,
    ) -> Self {
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
            None, // R2 store set later after state initialization
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
        let negative_cache = Arc::new(NegativeCache::new(negative_cache_ttl));
        let (admin_events, _) = tokio::sync::broadcast::channel(1024);

        Self {
            config,
            job_manager,
            chunk_cache,
            conversion_service,
            bloom_filter,
            http_client,
            metrics,
            ban_list,
            negative_cache,
            trusted_proxy_header,
            r2_store: None,
            r2_redirect: false,
            search_engine: None,
            analytics: None,
            federated_config: None,
            federated_cache: None,
            inflight_fetches: Arc::new(DashMap::new()),
            forgejo_url: None,
            forgejo_token: None,
            admin_events,
            rate_limiters: None,
        }
    }
}

/// Start the Remi server from a configuration file
pub async fn run_server_from_config(remi_config: &RemiConfig) -> Result<()> {
    let server_config = remi_config.to_server_config()?;
    let admin_bind = remi_config.admin_bind_addr()?;
    let negative_cache_ttl = remi_config.negative_cache_duration()?;
    let trusted_proxy_header = remi_config.trusted_proxy_header().map(String::from);

    tracing::info!("Starting Conary Remi server");
    tracing::info!("  Public API: {}", server_config.bind_addr);
    tracing::info!("  Admin API:  {} (localhost only)", admin_bind);
    tracing::info!("  Storage root: {:?}", remi_config.storage_root());
    tracing::info!("  Database: {:?}", server_config.db_path);
    tracing::info!(
        "  Max concurrent conversions: {}",
        server_config.max_concurrent_conversions
    );

    if server_config.enable_bloom_filter {
        tracing::info!(
            "  Bloom filter: enabled ({} expected chunks)",
            server_config.bloom_expected_chunks
        );
    }
    if let Some(ref upstream) = server_config.upstream_url {
        tracing::info!("  Pull-through caching: enabled (upstream: {})", upstream);
    }
    if server_config.enable_rate_limit {
        tracing::info!(
            "  Rate limiting: {} rps, {} burst",
            server_config.rate_limit_rps,
            server_config.rate_limit_burst
        );
    }
    if let Some(ref header) = trusted_proxy_header {
        tracing::info!("  Trusted proxy header: {}", header);
    }

    // Ensure storage directories exist
    for dir in remi_config.storage_dirs() {
        if !dir.exists() {
            tracing::info!("Creating directory: {:?}", dir);
            std::fs::create_dir_all(&dir)?;
        }
    }

    // Initialize the database if it doesn't exist
    if !server_config.db_path.exists() {
        tracing::info!("Initializing database at {:?}", server_config.db_path);
        conary_core::db::init(&server_config.db_path)?;
    }

    let state = Arc::new(RwLock::new(ServerState::with_options(
        server_config.clone(),
        trusted_proxy_header,
        negative_cache_ttl,
    )));

    // Initialize R2 storage if enabled
    if remi_config.r2.enabled {
        if let Some(ref endpoint) = remi_config.r2.endpoint {
            let r2_config = r2::R2Config {
                endpoint: endpoint.clone(),
                bucket: remi_config.r2.bucket.clone(),
                prefix: remi_config.r2.prefix.clone(),
                region: "auto".to_string(),
            };
            match R2Store::new(&r2_config) {
                Ok(store) => {
                    tracing::info!(
                        "  R2 storage: enabled (bucket: {}, write-through: {}, redirect: {})",
                        remi_config.r2.bucket,
                        remi_config.r2.write_through,
                        remi_config.r2.r2_redirect
                    );
                    let mut state_w = state.write().await;
                    state_w.r2_store = Some(Arc::new(store));
                    state_w.r2_redirect = remi_config.r2.r2_redirect;
                }
                Err(e) => {
                    tracing::error!("  R2 storage: failed to initialize: {}", e);
                }
            }
        } else {
            tracing::warn!("  R2 storage: enabled but no endpoint configured");
        }
    }

    // Initialize search engine if enabled
    if remi_config.search.enabled {
        let index_dir = remi_config.search_index_dir();
        tracing::info!("  Search engine: enabled (index: {:?})", index_dir);
        match SearchEngine::new(&index_dir) {
            Ok(engine) => {
                let engine = Arc::new(engine);
                // Rebuild index from DB in background
                let rebuild_engine = Arc::clone(&engine);
                let rebuild_db = server_config.db_path.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = rebuild_engine.rebuild_from_db(&rebuild_db) {
                        tracing::error!("Failed to rebuild search index: {}", e);
                    }
                });
                state.write().await.search_engine = Some(engine);
            }
            Err(e) => {
                tracing::error!("Failed to initialize search engine: {}", e);
            }
        }
    }

    // Initialize download analytics
    {
        let analytics = Arc::new(AnalyticsRecorder::new(server_config.db_path.clone()));
        tokio::spawn(analytics::run_analytics_loop(Arc::clone(&analytics)));
        state.write().await.analytics = Some(analytics);
        tracing::info!("  Download analytics: enabled");
    }

    // Initialize Bloom filter from existing chunks
    if server_config.enable_bloom_filter {
        let state_clone = state.clone();
        tokio::spawn(async move {
            if let Err(e) = initialize_bloom_filter(state_clone).await {
                tracing::error!("Failed to initialize Bloom filter: {}", e);
            }
        });
    }

    // Initialize federated sparse index if federation peers are configured
    if remi_config.federation.enabled && !remi_config.federation.peers.is_empty() {
        let fed_config = federated_index::FederatedIndexConfig {
            upstream_urls: remi_config.federation.peers.clone(),
            timeout: Duration::from_secs(10),
            cache_ttl: Duration::from_secs(300),
        };
        let fed_cache = Arc::new(federated_index::FederatedIndexCache::new());

        tracing::info!(
            "  Federated index: enabled ({} upstream peers)",
            fed_config.upstream_urls.len()
        );

        let mut state_w = state.write().await;
        state_w.federated_config = Some(fed_config);
        state_w.federated_cache = Some(fed_cache);
    }

    // Create routers
    let public_app = create_router(state.clone()).await;
    let admin_app = create_admin_router(state.clone());

    // Start background LRU eviction task
    tokio::spawn(cache::run_eviction_loop(state.clone()));

    // Start negative cache cleanup task
    tokio::spawn(negative_cache::run_cleanup_loop(state.clone()));

    // Start rate limiter and ban list cleanup task to prevent unbounded memory growth
    {
        let cleanup_state = state.clone();
        tokio::spawn(async move {
            let cleanup_interval = std::time::Duration::from_secs(300);
            loop {
                tokio::time::sleep(cleanup_interval).await;
                let ban_list = cleanup_state.read().await.ban_list.clone();
                ban_list.cleanup().await;
            }
        });
    }

    // Start background pre-warming if enabled
    if remi_config.prewarm.enabled && !remi_config.prewarm.distros.is_empty() {
        let prewarm_interval =
            crate::server::config::parse_duration(&remi_config.prewarm.metadata_sync_interval)
                .map(|d| d.as_secs() / 3600)
                .unwrap_or(6);
        let max_per_run = remi_config.prewarm.convert_top_n;

        for distro in &remi_config.prewarm.distros {
            let db = server_config.db_path.display().to_string();
            let chunks = server_config.chunk_dir.display().to_string();
            let cache = server_config.cache_dir.display().to_string();
            let d = distro.clone();

            tracing::info!(
                "  Pre-warm: enabled for {} (every {}h, top {} packages)",
                d,
                prewarm_interval,
                max_per_run
            );

            tokio::spawn(async move {
                prewarm::run_prewarm_background(
                    db,
                    chunks,
                    cache,
                    d,
                    prewarm_interval,
                    max_per_run,
                    None,
                )
                .await;
            });
        }
    }

    // Conditionally bind the external admin listener
    let external_admin_listener = if remi_config.admin.enabled {
        let bind = remi_config.external_admin_bind_addr()?;

        // Set forgejo config on state
        {
            let mut state_w = state.write().await;
            state_w.forgejo_url = remi_config.admin.forgejo_url.clone();
            state_w.forgejo_token = remi_config.admin.forgejo_token.clone();
        }

        // Initialize admin rate limiters
        {
            let limiters = Arc::new(crate::server::rate_limit::AdminRateLimiters::new(
                remi_config.admin.rate_limit_read_rpm,
                remi_config.admin.rate_limit_write_rpm,
                remi_config.admin.rate_limit_auth_fail_rpm,
            ));
            state.write().await.rate_limiters = Some(limiters);
        }

        // Bootstrap token from REMI_ADMIN_TOKEN env var
        if let Ok(env_token) = std::env::var("REMI_ADMIN_TOKEN") {
            let db_path = server_config.db_path.clone();
            let hash = crate::server::auth::hash_token(&env_token);
            tokio::task::spawn_blocking(move || {
                if let Ok(conn) = conary_core::db::open(&db_path)
                    && conary_core::db::models::admin_token::find_by_hash(&conn, &hash)
                        .unwrap_or(None)
                        .is_none()
                {
                    let _ = conary_core::db::models::admin_token::create(
                        &conn,
                        "env-bootstrap",
                        &hash,
                        "admin",
                    );
                    tracing::info!("  Admin token created from REMI_ADMIN_TOKEN env var");
                }
            })
            .await?;
        }

        let listener = tokio::net::TcpListener::bind(bind).await?;
        tracing::info!("  External admin API: {}", bind);
        Some(listener)
    } else {
        None
    };

    // Bind listeners
    let public_listener = tokio::net::TcpListener::bind(server_config.bind_addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_bind).await?;

    tracing::info!("Remi is ready to serve");

    // Create the external admin router only if enabled
    let external_admin_future = if let Some(listener) = external_admin_listener {
        let app = create_external_admin_router(state.clone());
        let fut = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        );
        Some(fut)
    } else {
        None
    };

    // Run all servers concurrently
    // Use into_make_service_with_connect_info to provide ConnectInfo to handlers
    tokio::select! {
        result = axum::serve(public_listener, public_app.into_make_service_with_connect_info::<std::net::SocketAddr>()) => {
            result?;
        }
        result = axum::serve(admin_listener, admin_app.into_make_service_with_connect_info::<std::net::SocketAddr>()) => {
            result?;
        }
        result = async {
            if let Some(fut) = external_admin_future {
                fut.await
            } else {
                std::future::pending().await
            }
        } => {
            result?;
        }
    }

    Ok(())
}

/// Start the Remi server (legacy single-port mode)
pub async fn run_server(config: ServerConfig) -> Result<()> {
    tracing::info!("Starting Conary Remi server on {}", config.bind_addr);
    tracing::info!("Database: {:?}", config.db_path);
    tracing::info!("Chunk store: {:?}", config.chunk_dir);
    tracing::info!(
        "Max concurrent conversions: {}",
        config.max_concurrent_conversions
    );

    if config.enable_bloom_filter {
        tracing::info!(
            "Bloom filter: enabled ({} expected chunks)",
            config.bloom_expected_chunks
        );
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

    // Initialize the database if it doesn't exist
    if !config.db_path.exists() {
        tracing::info!("Initializing database at {:?}", config.db_path);
        conary_core::db::init(&config.db_path)?;
    }

    let state = Arc::new(RwLock::new(ServerState::new(config.clone())));

    // Initialize search engine if a search index dir is available
    {
        let index_dir = config
            .db_path
            .parent()
            .unwrap_or(std::path::Path::new("/tmp"))
            .join("search-index");
        match SearchEngine::new(&index_dir) {
            Ok(engine) => {
                let engine = Arc::new(engine);
                let rebuild_engine = Arc::clone(&engine);
                let rebuild_db = config.db_path.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = rebuild_engine.rebuild_from_db(&rebuild_db) {
                        tracing::error!("Failed to rebuild search index: {}", e);
                    }
                });
                state.write().await.search_engine = Some(engine);
                tracing::info!("Search engine: enabled");
            }
            Err(e) => {
                tracing::warn!("Search engine unavailable: {}", e);
            }
        }
    }

    // Initialize download analytics
    {
        let analytics_recorder = Arc::new(AnalyticsRecorder::new(config.db_path.clone()));
        tokio::spawn(analytics::run_analytics_loop(Arc::clone(
            &analytics_recorder,
        )));
        state.write().await.analytics = Some(analytics_recorder);
    }

    // Initialize Bloom filter from existing chunks
    if config.enable_bloom_filter {
        let state_clone = state.clone();
        tokio::spawn(async move {
            if let Err(e) = initialize_bloom_filter(state_clone).await {
                tracing::error!("Failed to initialize Bloom filter: {}", e);
            }
        });
    }

    let app = create_router(state.clone()).await;

    // Start background LRU eviction task
    tokio::spawn(cache::run_eviction_loop(state.clone()));

    // Start ban list cleanup task to prevent unbounded memory growth
    {
        let cleanup_state = state.clone();
        tokio::spawn(async move {
            let cleanup_interval = std::time::Duration::from_secs(300);
            loop {
                tokio::time::sleep(cleanup_interval).await;
                let ban_list = cleanup_state.read().await.ban_list.clone();
                ban_list.cleanup().await;
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!("Remi is ready to serve");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
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

    let hashes = handlers::chunks::scan_chunk_hashes(&objects_dir).await?;
    for hash in &hashes {
        bloom.add(hash);
    }

    tracing::info!("Bloom filter initialized with {} chunks", hashes.len());
    Ok(())
}
