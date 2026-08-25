// apps/remi/src/server/mod.rs
//! Conary Remi Server - On-demand CCS conversion proxy
//!
//! This module provides an HTTP server that:
//! - Serves repository metadata (proxied through Cloudflare)
//! - Serves CCS chunks from R2 authority or a local-only store
//! - Converts native package formats (RPM/DEB/Arch/eopkg) to CCS on demand
//! - Uses R2-verified bounded LRU caching to manage disk space
//!
//! Phase 0 hardening features:
//! - Bloom filter for fast negative lookups (DoS protection)
//! - Pull-through caching (fetch from upstream on miss)
//! - Batched missing-chunk discovery
//! - Metrics tracking for observability
//! - Rate limiting per IP/peer

pub mod admin_service;
mod analytics;
pub mod artifact_paths;
pub mod audit;
pub mod auth;
mod bloom;
mod bounded_cache;
mod cache;
mod canonical_fetch;
mod canonical_job;
pub mod catalog_authority;
pub mod catalog_gc;
pub mod catalog_refresh;
pub mod chunk_gc;
pub mod config;
mod conversion;
mod conversion_crawl;
pub mod conversion_timing;
mod database_writer;
pub mod delta_manifests;
pub mod federated_index;
mod handlers;
mod index_gen;
mod jobs;
pub mod lite;
pub mod mcp;
pub mod metrics;
pub mod native_publish;
mod negative_cache;
pub mod popularity;
mod prewarm;
pub mod profile_catalog;
mod promotion_evidence;
pub mod publication;
mod publication_scheduler;
pub mod r2;
pub mod r2_durability;
pub mod rate_limit;
mod readiness;
pub mod release_publish;
pub mod repository_manifest;
mod routes;
pub(crate) mod runtime_lock;
mod runtime_session;
pub mod search;
pub mod security;
pub(crate) mod signing_authority;
pub mod test_db;
mod universe_publish;
mod universe_validation;

pub(crate) use analytics::AnalyticsRecorder;
pub use bloom::{BloomStats, ChunkBloomFilter};
pub use bounded_cache::{BoundedCache, BoundedCacheReport};
pub use cache::ChunkCache;
pub use config::RemiConfig;
pub use conversion::{
    ConversionBenchmarkEnvironment, ConversionBenchmarkEvidence, ConversionBenchmarkSample,
    ConversionBenchmarkSampleClass, ConversionService, ServerConversionResult,
};
pub use conversion_crawl::{
    CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
    CONVERSION_PROOF_KEY_SCHEMA_V1, CONVERSION_PROOF_SCHEMA_V1, CcsArtifactReopenProofV1,
    CcsTargetCompatibilityProofV1, ConversionCrawlConfig, ConversionCrawlFailureV4,
    ConversionCrawlOutcomeStateV4, ConversionCrawlPackageOutcomeV4, ConversionCrawlProfileV4,
    ConversionProofDispositionV1, ConversionProofKeyV1, ConversionProofTargetContractV1,
    ConversionProofV1, REMI_CONVERSION_CRAWL_SCHEMA_V4, RemiConversionCrawlV4,
    run_conversion_crawl, write_and_reopen_conversion_crawl,
};
pub use index_gen::{IndexGenConfig, IndexGenResult, generate_indices};
pub use jobs::{ConversionJob, JobManager, JobStatus};
pub use lite::{ProxyConfig, run_proxy};
pub use metrics::{MetricsSnapshot, ServerMetrics};
pub use negative_cache::NegativeCache;
pub use prewarm::{PrewarmConfig, PrewarmFailure, PrewarmResult, run_prewarm};
pub use promotion_evidence::{
    REMI_PROMOTION_EVIDENCE_SCHEMA_V1, RemiPromotionCanonicalMapV1, RemiPromotionEvidenceV1,
    RemiPromotionProfileEvidenceV1,
};
pub use r2::R2Store;
pub use routes::{create_admin_router, create_external_admin_router, create_router};
pub use search::SearchEngine;
pub use security::BanList;

use anyhow::{Context, Result};
use dashmap::DashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

async fn ensure_admin_bootstrap_token(
    db_path: PathBuf,
    database_writer: database_writer::DatabaseWriter,
    token: &str,
    source_name: &str,
    source_description: &str,
) -> Result<()> {
    let hash = crate::server::auth::hash_token(token);
    let source_name = source_name.to_string();
    let source_description = source_description.to_string();
    tokio::task::spawn_blocking(move || -> Result<()> {
        database_writer.execute(|| {
            let conn = open_runtime_db(&db_path)?;
            if conary_core::db::models::admin_token::find_by_hash(&conn, &hash)?.is_none() {
                conary_core::db::models::admin_token::create(&conn, &source_name, &hash, "admin")?;
                tracing::info!("  Admin token created from {}", source_description);
            }
            Ok(())
        })
    })
    .await??;
    Ok(())
}

/// Server configuration
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    /// Address to bind to
    pub bind_addr: SocketAddr,
    /// Path to the Conary database
    pub db_path: PathBuf,
    /// Path to the chunk store
    pub chunk_dir: PathBuf,
    /// Path to the cache/scratch directory
    pub cache_dir: PathBuf,
    /// Root containing immutable activated source and profile catalogs.
    pub catalog_dir: PathBuf,
    /// Private disposable catalog construction root.
    pub catalog_candidate_dir: PathBuf,
    /// Maximum concurrent conversions
    pub max_concurrent_conversions: usize,
    /// LRU eviction threshold in bytes (default 700GB)
    pub cache_max_bytes: u64,
    /// Free bytes the serving root must have before readiness reports ready.
    pub readiness_min_free_bytes: u64,

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

    // === Release publication ===
    /// Trusted release signer and repository signing-key configuration.
    pub release_publish: crate::server::config::ReleasePublishSection,
}

impl Default for ServerConfig {
    fn default() -> Self {
        crate::server::config::RemiConfig::default()
            .to_server_config()
            .expect("default RemiConfig should always produce ServerConfig")
    }
}

/// Event broadcast from admin operations (e.g., CI triggers, token changes)
#[derive(Clone, Debug, serde::Serialize)]
pub struct AdminEvent {
    /// Event type identifier (e.g., "token.created")
    pub event_type: String,
    /// Event payload
    pub data: serde_json::Value,
    /// ISO 8601 timestamp
    pub timestamp: String,
}

/// Shared server state
///
/// NOTE: This struct has grown large. Consider decomposing into sub-structs:
/// - `CacheState` (chunk_cache, bloom_filter, negative_cache)
/// - `ConversionState` (conversion_service, job_manager)
/// - `FederationState` (federated_config, federated_cache)
/// - `AdminState` (admin_events, test_db_path)
// TODO: Decompose ServerState into focused sub-structs to improve readability.
pub struct ServerState {
    pub config: ServerConfig,
    pub(crate) database_writer: database_writer::DatabaseWriter,
    pub(crate) catalog_authority: catalog_authority::CatalogAuthority,
    pub(crate) publication_coordinator: Arc<Mutex<()>>,
    pub(crate) catalog_gc_coordinator: Arc<Mutex<()>>,
    pub(crate) publication_readiness: readiness::PublicationReadiness,
    pub(crate) required_source_profiles: Vec<String>,
    pub job_manager: JobManager,
    pub chunk_cache: ChunkCache,
    pub bounded_cache: BoundedCache,
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
    /// Full-text search engine (Tantivy)
    pub search_engine: Option<Arc<SearchEngine>>,
    /// Download analytics recorder (buffered writes)
    pub(crate) analytics: Option<Arc<AnalyticsRecorder>>,
    /// Federated sparse index configuration (from federation peers)
    pub federated_config: Option<federated_index::FederatedIndexConfig>,
    /// Federated sparse index cache (TTL-based in-memory cache)
    pub federated_cache: Option<Arc<federated_index::FederatedIndexCache>>,
    /// In-flight upstream fetches for request coalescing (thundering herd prevention).
    /// Key is chunk hash; value is a broadcast sender that waiters subscribe to.
    /// When the first fetch completes, all waiters are notified.
    pub inflight_fetches: Arc<DashMap<String, tokio::sync::broadcast::Sender<()>>>,
    /// Broadcast channel for admin events (SSE stream)
    pub admin_events: tokio::sync::broadcast::Sender<AdminEvent>,
    /// Path to the separate test data database (test_db module)
    pub test_db_path: Option<String>,
    /// Canonical registry configuration (rebuild cooldown, rules dir, etc.)
    pub canonical_config: crate::server::config::CanonicalSection,
}

impl ServerState {
    pub fn new(config: ServerConfig) -> Result<Self> {
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
    ) -> Result<Self> {
        let job_manager = JobManager::new(config.max_concurrent_conversions);
        let database_writer = database_writer::DatabaseWriter::default();
        let catalog_authority = catalog_authority::CatalogAuthority::from_paths(
            config.db_path.clone(),
            config.catalog_dir.clone(),
            database_writer.clone(),
        );
        let publication_coordinator = Arc::new(Mutex::new(()));
        let catalog_gc_coordinator = Arc::new(Mutex::new(()));
        let chunk_cache = ChunkCache::new(
            config.chunk_dir.clone(),
            config.cache_max_bytes,
            config.db_path.clone(),
        );
        let bounded_cache = BoundedCache::new(chunk_cache.clone());
        let conversion_service = ConversionService::new(
            config.chunk_dir.clone(),
            config.cache_dir.clone(),
            config.db_path.clone(),
            None, // R2 store set later after state initialization
        )
        .with_catalog_authority(catalog_authority.clone())
        .with_database_writer(database_writer.clone())
        .with_publication_coordinator(Arc::clone(&publication_coordinator))
        .with_bounded_cache(bounded_cache.clone())
        .with_repository_keys_dir(config.release_publish.repository_keys_dir.clone());

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
        let http_client = build_http_client(config.upstream_timeout, "conary-remi/0.1")?;

        let metrics = Arc::new(ServerMetrics::new());
        let ban_list = Arc::new(BanList::new(config.ban_duration_secs, config.ban_threshold));
        let negative_cache = Arc::new(NegativeCache::new(negative_cache_ttl));
        let (admin_events, _) = tokio::sync::broadcast::channel(1024);

        Ok(Self {
            config,
            database_writer,
            catalog_authority,
            publication_coordinator,
            catalog_gc_coordinator,
            publication_readiness: readiness::PublicationReadiness::default(),
            required_source_profiles: Vec::new(),
            job_manager,
            chunk_cache,
            bounded_cache,
            conversion_service,
            bloom_filter,
            http_client,
            metrics,
            ban_list,
            negative_cache,
            trusted_proxy_header,
            r2_store: None,
            search_engine: None,
            analytics: None,
            federated_config: None,
            federated_cache: None,
            inflight_fetches: Arc::new(DashMap::new()),
            admin_events,
            test_db_path: Some(
                std::env::var("CONARY_TEST_DB_PATH")
                    .unwrap_or_else(|_| "/conary/test-data.db".to_string()),
            ),
            canonical_config: crate::server::config::CanonicalSection::default(),
        })
    }
}

fn build_http_client(timeout: Duration, user_agent: &str) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(user_agent)
        .build()
        .context("Failed to create HTTP client")
}

/// Open a database connection for the already-initialized server runtime.
///
/// Server startup calls [`ensure_database_ready`] before background tasks or
/// hot request paths begin using SQLite, so those paths can skip repeating the
/// current-schema validation on every connection open.
pub(crate) fn open_runtime_db(
    path: impl AsRef<std::path::Path>,
) -> conary_core::Result<rusqlite::Connection> {
    conary_core::db::open_fast(path)
}

fn ensure_database_ready(db_path: &std::path::Path) -> Result<()> {
    if !db_path.exists() {
        tracing::info!("Initializing database at {:?}", db_path);
        conary_core::db::init(db_path)?;
        return Ok(());
    }

    tracing::info!("Checking database schema at {:?}", db_path);
    let _conn = conary_core::db::open(db_path)?;
    Ok(())
}

fn prepare_runtime_storage(
    remi_config: &RemiConfig,
    server_config: &ServerConfig,
) -> Result<runtime_lock::RuntimeRootLock> {
    let locked_db_path = remi_config.storage_root().join("metadata/conary.db");
    if server_config.db_path != locked_db_path {
        anyhow::bail!(
            "Remi runtime database {} is outside the locked storage-root authority {}",
            server_config.db_path.display(),
            locked_db_path.display()
        );
    }
    let runtime_lock = runtime_lock::RuntimeRootLock::acquire(remi_config.storage_root())?;
    tracing::info!("  Runtime root lock: {:?}", runtime_lock.lock_path());
    create_runtime_storage_directories(remi_config)?;
    ensure_database_ready(&server_config.db_path)?;
    runtime_session::begin(&server_config.db_path)?;
    tracing::info!("  Runtime reader session: installed with stale-reader recovery");
    Ok(runtime_lock)
}

fn create_runtime_storage_directories(remi_config: &RemiConfig) -> Result<()> {
    for dir in remi_config.storage_dirs() {
        if !dir.exists() {
            tracing::info!("Creating directory: {:?}", dir);
            std::fs::create_dir_all(&dir)?;
        }
    }
    Ok(())
}

/// Initialize storage directories without starting listeners.
///
/// The one-shot CLI path uses the same process-wide ownership boundary as the
/// long-running server, then releases it when initialization completes.
pub fn initialize_storage_directories(remi_config: &RemiConfig) -> Result<()> {
    let _runtime_lock = runtime_lock::RuntimeRootLock::acquire(remi_config.storage_root())?;
    create_runtime_storage_directories(remi_config)
}

/// Start the Remi server from a configuration file.
///
/// This boundary owns both the Tokio runtime and the runtime-root lock so the
/// kernel lock remains held while Tokio finishes any blocking shutdown work.
pub fn run_server_from_config(remi_config: &RemiConfig) -> Result<()> {
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

    // Acquire before any runtime database or storage-tree mutation, then keep
    // the guard outside Tokio so runtime shutdown cannot outlive ownership.
    let runtime_lock = prepare_runtime_storage(remi_config, &server_config)?;
    run_with_runtime_lock(runtime_lock, move || {
        run_server_on_runtime(
            remi_config,
            server_config,
            admin_bind,
            negative_cache_ttl,
            trusted_proxy_header,
        )
    })
}

fn run_with_runtime_lock<F, Fut>(
    runtime_lock: runtime_lock::RuntimeRootLock,
    entry: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let _runtime_lock = runtime_lock;
    conary_bootstrap::run_with_runtime(entry)
}

async fn run_server_on_runtime(
    remi_config: &RemiConfig,
    server_config: ServerConfig,
    admin_bind: SocketAddr,
    negative_cache_ttl: Duration,
    trusted_proxy_header: Option<String>,
) -> Result<()> {
    let required_source_profiles = if let Some(manifest_path) =
        remi_config.repository_manifest.as_deref()
    {
        let manifest = repository_manifest::RepositoryManifest::load(manifest_path)?;
        let keys_root = server_config
            .release_publish
            .repository_keys_dir
            .as_deref()
            .context("release_publish.repository_keys_dir is required with repository_manifest")?;
        signing_authority::ensure_repository_authority(&manifest, keys_root)?;
        signing_authority::ensure_universe_authority(keys_root)?;
        let mut conn = open_runtime_db(&server_config.db_path)?;
        let reconciled = manifest.reconcile(&mut conn)?;
        tracing::info!(
            "  Repository manifest: {} inserted, {} updated, {} removed, {} unchanged",
            reconciled.inserted,
            reconciled.updated,
            reconciled.removed,
            reconciled.unchanged
        );
        let mut profiles = manifest
            .repositories
            .iter()
            .filter(|repository| repository.enabled)
            .filter(|repository| {
                conary_core::repository::supported_profiles::profile_by_public_id(
                    &repository.profile,
                )
                .is_some()
            })
            .map(|repository| repository.profile.clone())
            .collect::<Vec<_>>();
        profiles.sort();
        profiles.dedup();
        profiles
    } else {
        Vec::new()
    };

    let state = Arc::new(RwLock::new(ServerState::with_options(
        server_config.clone(),
        trusted_proxy_header,
        negative_cache_ttl,
    )?));

    // Retain the validated configuration policy separately from persisted
    // runtime state so readiness can prove that every required profile exists.
    {
        let mut state_w = state.write().await;
        state_w.canonical_config = remi_config.canonical.clone();
        state_w.required_source_profiles = required_source_profiles;
    }

    let recovered_catalog_runs = catalog_gc::recover_catalog_refresh_runs(&state)
        .await
        .context("recover expired immutable catalog refresh runs during startup")?;
    tracing::info!(
        recovered_runs = recovered_catalog_runs,
        "  Catalog recovery: exact terminal candidates removed"
    );

    let catalog_gc = catalog_gc::collect_catalog_garbage(&state)
        .await
        .context("collect unreachable immutable catalogs during startup")?;
    tracing::info!(
        deleted_profiles = catalog_gc.deleted_profile_resources,
        deleted_sources = catalog_gc.deleted_source_resources,
        removed_bundles = catalog_gc.removed_bundles,
        "  Catalog GC: exact reachability collection complete"
    );

    // Initialize R2 storage if enabled
    if remi_config.r2.enabled {
        let endpoint = remi_config
            .r2
            .endpoint
            .as_ref()
            .context("r2.endpoint is required when R2 authority is enabled")?;
        let r2_config = r2::R2Config {
            endpoint: endpoint.clone(),
            bucket: remi_config.r2.bucket.clone(),
            prefix: remi_config.r2.prefix.clone(),
            region: "auto".to_string(),
        };
        let store =
            Arc::new(R2Store::new(&r2_config).context("initialize mandatory R2 chunk authority")?);
        store
            .head_chunk("0000000000000000000000000000000000000000000000000000000000000000")
            .await
            .context("probe mandatory R2 chunk authority")?;
        tracing::info!(
            "  R2 storage: durable authority enabled (bucket: {})",
            remi_config.r2.bucket
        );
        let mut state_w = state.write().await;
        state_w.r2_store = Some(Arc::clone(&store));
        state_w.conversion_service = state_w
            .conversion_service
            .clone()
            .with_r2_store(Some(store));
    }

    // Initialize search engine if enabled
    if remi_config.search.enabled {
        let index_dir = remi_config.search_index_dir();
        tracing::info!("  Search engine: enabled (index: {:?})", index_dir);
        match SearchEngine::new(&index_dir) {
            Ok(engine) => {
                let engine = Arc::new(engine);
                // Rebuild from exact immutable profile readers in background.
                let rebuild_engine = Arc::clone(&engine);
                let rebuild_db = server_config.db_path.clone();
                let (rebuild_catalog_authority, rebuild_source_profiles) = {
                    let state_guard = state.read().await;
                    (
                        state_guard.catalog_authority.clone(),
                        state_guard.required_source_profiles.clone(),
                    )
                };
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = rebuild_engine.rebuild_from_catalogs(
                        &rebuild_db,
                        &rebuild_catalog_authority,
                        &rebuild_source_profiles,
                    ) {
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
        let database_writer = state.read().await.database_writer.clone();
        let analytics = Arc::new(AnalyticsRecorder::new(
            server_config.db_path.clone(),
            database_writer,
        ));
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

    // One scheduler owns initial and periodic repository/canonical publication.
    // Its startup cycle is repository refresh -> canonical fetch/rebuild ->
    // eligible prewarm, and its two periodic clocks cannot overlap.
    {
        let refresh_interval =
            crate::server::config::parse_duration(&remi_config.prewarm.metadata_sync_interval)?;
        let canonical_config = remi_config.canonical.clone();
        let canonical_interval = Duration::from_secs(
            canonical_config
                .fetch_interval_hours
                .checked_mul(3600)
                .context("canonical.fetch_interval_hours is too large")?,
        );
        let prewarm_jobs = if remi_config.prewarm.enabled {
            remi_config
                .prewarm
                .distros
                .iter()
                .map(|distro| PrewarmConfig {
                    db_path: server_config.db_path.display().to_string(),
                    chunk_dir: server_config.chunk_dir.display().to_string(),
                    cache_dir: server_config.cache_dir.display().to_string(),
                    repository_keys_dir: server_config.release_publish.repository_keys_dir.clone(),
                    distro: distro.clone(),
                    max_packages: remi_config.prewarm.convert_top_n,
                    popularity_file: None,
                    pattern: None,
                    dry_run: false,
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let prewarm_conversion_permits = {
            let state = state.read().await;
            state.job_manager.semaphore()
        };
        let prewarm_conversion_service = {
            let state = state.read().await;
            state.conversion_service.clone()
        };
        tracing::info!(
            "  Metadata refresh: enabled every {}s",
            refresh_interval.as_secs()
        );
        tracing::info!(
            "  Canonical fetch: sequenced after startup refresh, then every {}h",
            canonical_config.fetch_interval_hours
        );
        for job in &prewarm_jobs {
            tracing::info!(
                "  Pre-warm: enabled for {} after each metadata refresh (top {})",
                job.distro,
                job.max_packages
            );
        }
        publication_scheduler::spawn(publication_scheduler::PublicationSchedule {
            state: state.clone(),
            refresh_interval,
            canonical_interval,
            canonical_config,
            db_path: server_config.db_path.clone(),
            prewarm_jobs,
            prewarm_conversion_permits,
            prewarm_conversion_service,
        });
    }

    // Admin rate limiters live outside ServerState to avoid per-request RwLock
    // acquisition. Set once at startup, shared via axum Extension layer.
    let mut admin_rate_limiters: Option<Arc<crate::server::rate_limit::AdminRateLimiters>> = None;

    // Conditionally bind the external admin listener
    let external_admin_listener = if remi_config.admin.enabled {
        let bind = remi_config.external_admin_bind_addr()?;

        // Initialize admin rate limiters. Read the trusted proxy header back
        // from state (the original was moved into ServerState earlier).
        let proxy_header_for_limiters = state.read().await.trusted_proxy_header.clone();
        let limiters = Arc::new(crate::server::rate_limit::AdminRateLimiters::new(
            remi_config.admin.rate_limit_read_rpm,
            remi_config.admin.rate_limit_write_rpm,
            remi_config.admin.rate_limit_auth_fail_rpm,
            proxy_header_for_limiters,
        ));

        // Spawn periodic cleanup for governor DashMap entries
        tokio::spawn(crate::server::rate_limit::run_limiter_cleanup(Arc::clone(
            &limiters,
        )));

        admin_rate_limiters = Some(limiters);
        let bootstrap_writer = state.read().await.database_writer.clone();

        if let Some(config_token) = remi_config.admin.bootstrap_token.as_deref() {
            ensure_admin_bootstrap_token(
                server_config.db_path.clone(),
                bootstrap_writer.clone(),
                config_token,
                "config-bootstrap",
                "admin.bootstrap_token config",
            )
            .await?;
        }

        // REMI_ADMIN_TOKEN remains supported as an environment override/bootstrap path.
        if let Ok(env_token) = std::env::var("REMI_ADMIN_TOKEN") {
            ensure_admin_bootstrap_token(
                server_config.db_path.clone(),
                bootstrap_writer,
                &env_token,
                "env-bootstrap",
                "REMI_ADMIN_TOKEN env var",
            )
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

    tracing::info!("Remi listeners are active; publication readiness is reported at /health/ready");

    // Create the external admin router only if enabled
    let external_admin_future = if let Some(listener) = external_admin_listener {
        let app = create_external_admin_router(state.clone(), admin_rate_limiters);
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

#[cfg(test)]
mod tests {
    use super::{
        build_http_client, ensure_admin_bootstrap_token, ensure_database_ready, open_runtime_db,
        prepare_runtime_storage, run_with_runtime_lock,
        runtime_lock::{RuntimeRootLock, RuntimeRootLockError},
    };

    fn test_db_path() -> std::path::PathBuf {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let db_path = tmp.path().join("conary.db");
        {
            let conn = rusqlite::Connection::open(&db_path).expect("open sqlite");
            conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
            conary_core::db::schema::ensure_current(&conn).expect("ensure current schema");
        }
        std::mem::forget(tmp);
        db_path
    }

    #[tokio::test]
    async fn ensure_admin_bootstrap_token_inserts_once() {
        let db_path = test_db_path();
        let database_writer = crate::server::database_writer::DatabaseWriter::default();
        ensure_admin_bootstrap_token(
            db_path.clone(),
            database_writer.clone(),
            "bootstrap-token",
            "config-bootstrap",
            "admin.bootstrap_token config",
        )
        .await
        .expect("seed bootstrap token");
        ensure_admin_bootstrap_token(
            db_path.clone(),
            database_writer,
            "bootstrap-token",
            "config-bootstrap",
            "admin.bootstrap_token config",
        )
        .await
        .expect("seed bootstrap token idempotently");

        let conn = conary_core::db::open(&db_path).expect("open db");
        let found = conary_core::db::models::admin_token::find_by_hash(
            &conn,
            &crate::server::auth::hash_token("bootstrap-token"),
        )
        .expect("query token")
        .expect("token exists");
        assert_eq!(found.name, "config-bootstrap");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM admin_tokens", [], |row| row.get(0))
            .expect("count tokens");
        assert_eq!(count, 1);
    }

    #[test]
    fn open_runtime_db_requires_existing_ready_database() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let db_path = tmp.path().join("missing.db");

        let err = open_runtime_db(&db_path).expect_err("missing db should not auto-initialize");
        assert!(
            err.to_string().contains("Database not found"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ensure_database_ready_allows_runtime_db_fast_path() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let db_path = tmp.path().join("conary.db");

        ensure_database_ready(&db_path).expect("prepare db");
        let conn = open_runtime_db(&db_path).expect("open prepared db");

        let version = conary_core::db::schema::get_schema_version(&conn).expect("schema version");
        assert_eq!(version, conary_core::db::schema::SCHEMA_VERSION);
    }

    #[test]
    fn rejected_runtime_owner_cannot_initialize_the_database() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let root = temp.path().join("runtime");
        let first = RuntimeRootLock::acquire(&root).expect("acquire first owner");
        let mut remi_config = super::RemiConfig::default();
        remi_config.storage.root = root;
        let server_config = remi_config.to_server_config().expect("server config");

        let error = prepare_runtime_storage(&remi_config, &server_config)
            .expect_err("second owner must fail before storage initialization");

        assert!(matches!(
            error.downcast_ref::<RuntimeRootLockError>(),
            Some(RuntimeRootLockError::AlreadyOwned { .. })
        ));
        assert!(!server_config.db_path.exists());
        assert!(!server_config.chunk_dir.exists());

        drop(first);
        let second = prepare_runtime_storage(&remi_config, &server_config)
            .expect("released root should be reusable");
        assert_eq!(
            second.lock_path().parent(),
            Some(
                std::fs::canonicalize(remi_config.storage_root())
                    .unwrap()
                    .as_path()
            )
        );
        assert!(server_config.db_path.exists());
    }

    #[test]
    fn runtime_shutdown_finishes_before_the_root_lock_is_released() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let root = temp.path().join("runtime");
        let owner = RuntimeRootLock::acquire(&root).expect("acquire runtime owner");
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);

        let runtime_thread = std::thread::spawn(move || {
            run_with_runtime_lock(owner, move || async move {
                tokio::task::spawn_blocking(move || {
                    started_tx.send(()).expect("report blocking work start");
                    release_rx.recv().expect("wait for blocking work release");
                });
                Ok(())
            })
        });

        started_rx.recv().expect("observe blocking shutdown work");
        let error = RuntimeRootLock::acquire(&root)
            .expect_err("runtime shutdown must retain root ownership");
        assert!(matches!(error, RuntimeRootLockError::AlreadyOwned { .. }));

        release_tx.send(()).expect("release blocking shutdown work");
        runtime_thread
            .join()
            .expect("join runtime owner")
            .expect("runtime owner result");
        RuntimeRootLock::acquire(&root).expect("runtime shutdown should release root ownership");
    }

    #[test]
    fn build_http_client_rejects_invalid_user_agent() {
        let err = build_http_client(std::time::Duration::from_secs(30), "bad\0agent")
            .expect_err("invalid user agent should be surfaced as an error");
        assert!(err.to_string().contains("HTTP client"));
    }

    #[test]
    fn test_server_state_drops_forgejo_config_fields() {
        let source = include_str!("mod.rs");
        let forgejo_url_field = ["pub forgejo_", "url:"].concat();
        let forgejo_token_field = ["pub forgejo_", "token:"].concat();
        assert!(
            !source.contains(&forgejo_url_field),
            "server state should not retain forgejo_url"
        );
        assert!(
            !source.contains(&forgejo_token_field),
            "server state should not retain forgejo_token"
        );
    }
}
