// apps/remi/src/server/mod.rs
//! Conary Remi Server - On-demand CCS conversion proxy
//!
//! This module provides an HTTP server that:
//! - Serves repository metadata (proxied through Cloudflare)
//! - Serves CCS chunks from R2 authority or a local-only store
//! - Converts native package formats (RPM/DEB/Arch/eopkg) to CCS on demand
//! - Uses R2-verified bounded LRU caching to manage disk space
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
mod catalog_capacity;
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
mod native_oracle_input;
pub mod native_publish;
mod negative_cache;
mod operator;
pub mod popularity;
mod prewarm;
mod private_output;
pub mod profile_catalog;
mod promotion;
mod promotion_evidence;
mod promotion_proof;
pub(crate) mod public_universe;
pub mod publication;
mod publication_coordinator;
mod publication_scheduler;
pub mod r2;
pub mod r2_durability;
pub mod rate_limit;
mod readiness;
pub mod release_publish;
pub mod repository_manifest;
mod resolution_survey;
mod routes;
pub(crate) mod runtime_lock;
mod runtime_session;
pub mod search;
pub mod security;
pub(crate) mod signing_authority;
mod startup;
pub mod test_db;
mod universe_publish;
pub(crate) mod universe_revision_inspection;
mod universe_validation;

pub(crate) use analytics::AnalyticsRecorder;
pub use bloom::{BloomStats, ChunkBloomFilter};
pub use bounded_cache::{BoundedCache, BoundedCacheReport};
pub use cache::ChunkCache;
pub use catalog_authority::ProfileRevisionSelection;
pub use config::RemiConfig;
pub use conversion::{
    CONVERSION_BENCHMARK_SCHEMA_V8, ConversionBenchmarkAuthority,
    ConversionBenchmarkCatalogAuthority, ConversionBenchmarkCatalogQuery,
    ConversionBenchmarkCatalogReopen, ConversionBenchmarkCatalogSetup, ConversionBenchmarkConfig,
    ConversionBenchmarkEnvironment, ConversionBenchmarkEvidence, ConversionBenchmarkOutcome,
    ConversionBenchmarkOutputProof, ConversionBenchmarkProcessUsage, ConversionBenchmarkReportV8,
    ConversionBenchmarkRootIdentity, ConversionBenchmarkSelectionKind, ConversionBenchmarkSetup,
    ConversionBenchmarkSubject, ConversionBenchmarkView, ConversionBenchmarkViews,
    ConversionService, ServerConversionResult, run_conversion_benchmark_from_config,
};
pub use conversion_crawl::{
    CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
    CONVERSION_PROOF_KEY_SCHEMA_V1, CONVERSION_PROOF_SCHEMA_V1, CcsArtifactReopenProofV1,
    CcsTargetCompatibilityProofV1, ConversionCrawlFailureV4, ConversionCrawlOutcomeStateV4,
    ConversionCrawlPackageOutcomeV4, ConversionCrawlProfileV4, ConversionProofDispositionV1,
    ConversionProofKeyV1, ConversionProofTargetContractV1, ConversionProofV1,
    REMI_CONVERSION_CRAWL_SCHEMA_V4, RemiConversionCrawlV4, run_conversion_crawl_from_config,
    write_and_reopen_conversion_crawl,
};
pub use index_gen::{IndexGenConfig, IndexGenResult, generate_indices};
pub use jobs::{ConversionJob, JobManager, JobStatus};
pub use lite::{ProxyConfig, run_proxy};
pub use metrics::{MetricsSnapshot, ServerMetrics};
pub use native_oracle_input::{
    NATIVE_ORACLE_INPUT_MANIFEST_FILE, NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY,
    NATIVE_ORACLE_INPUT_SCHEMA_V1, NativeOracleInputConfig, NativeOracleInputObjectV1,
    NativeOracleInputOutcome, NativeOracleInputProfileV1, NativeOracleInputSetV1,
    materialize_native_oracle_inputs, reopen_native_oracle_input_bundle,
};
pub use negative_cache::NegativeCache;
pub(crate) use operator::acquire_existing_runtime_storage;
pub use operator::{
    initialize_storage_directories, run_promotion_activation_from_config,
    run_promotion_proof_from_config, run_resolution_surveys_from_config,
};
pub use prewarm::{PrewarmConfig, PrewarmFailure, PrewarmResult, run_prewarm};
pub use promotion::{RemiPromotionActivationConfig, RemiPromotionActivationOutcome};
pub use promotion_evidence::{
    REMI_PROMOTION_EVIDENCE_SCHEMA_V1, RemiPromotionCanonicalMapV1, RemiPromotionEvidenceConfig,
    RemiPromotionEvidenceV1, RemiPromotionProfileEvidenceInput, RemiPromotionProfileEvidenceV1,
    produce_remi_promotion_evidence, reopen_remi_promotion_evidence,
};
pub use promotion_proof::{
    RemiPromotionProofConfig, RemiPromotionProofOutcome, RemiPromotionProofProfileInput,
};
pub use r2::R2Store;
pub use resolution_survey::{RemiResolutionSurveyConfig, RemiResolutionSurveyOutcome};
pub use routes::{create_admin_router, create_external_admin_router, create_router};
pub use search::SearchEngine;
pub use security::BanList;
#[cfg(test)]
pub(crate) use startup::prepare_runtime_storage;
pub use startup::run_server_from_config;

use anyhow::{Context, Result};
use dashmap::DashMap;
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
    pub(crate) publication_coordinator: Arc<publication_coordinator::PublicationCoordinator>,
    pub(crate) catalog_gc_coordinator: Arc<Mutex<()>>,
    pub(crate) catalog_scratch_coordinator: Arc<catalog_capacity::CatalogScratchCoordinator>,
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
        let publication_coordinator =
            Arc::new(publication_coordinator::PublicationCoordinator::default());
        let catalog_gc_coordinator = Arc::new(Mutex::new(()));
        let catalog_scratch_coordinator =
            Arc::new(catalog_capacity::CatalogScratchCoordinator::default());
        let chunk_cache = ChunkCache::new(
            config.chunk_dir.clone(),
            config.cache_max_bytes,
            config.db_path.clone(),
        );
        let bounded_cache = BoundedCache::new(chunk_cache.clone());
        let archive_cpu = conary_core::ccs::CcsArchiveCpuAdmission::for_current_process();
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
        .with_archive_cpu_admission(archive_cpu)
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
            catalog_scratch_coordinator,
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

#[cfg(test)]
mod tests {
    use super::{
        build_http_client, ensure_admin_bootstrap_token, ensure_database_ready, open_runtime_db,
        runtime_lock::{RuntimeRootLock, RuntimeRootLockError},
        startup::{prepare_runtime_storage, run_with_runtime_lock},
    };

    fn test_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let db_path = tmp.path().join("conary.db");
        {
            let conn = rusqlite::Connection::open(&db_path).expect("open sqlite");
            conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
            conary_core::db::schema::ensure_current(&conn).expect("ensure current schema");
        }
        (tmp, db_path)
    }

    #[tokio::test]
    async fn ensure_admin_bootstrap_token_inserts_once() {
        let (_temp, db_path) = test_db_path();
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
}
