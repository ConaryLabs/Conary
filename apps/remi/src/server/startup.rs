// apps/remi/src/server/startup.rs

use super::*;
use std::future::Future;

pub(crate) fn prepare_runtime_storage(
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
    super::operator::create_runtime_storage_directories(remi_config)?;
    ensure_database_ready(&server_config.db_path)?;
    runtime_session::begin(&server_config.db_path)?;
    tracing::info!("  Runtime reader session: installed with stale-reader recovery");
    Ok(runtime_lock)
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

pub(super) fn run_with_runtime_lock<F, Fut>(
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
        deleted_conversion_proofs = catalog_gc.deleted_conversion_proofs,
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
                // Rebuild from the exact signed public universe in background.
                // Until that succeeds, the persisted index has no serving
                // authority and search returns typed unavailability.
                let rebuild_engine = Arc::clone(&engine);
                let rebuild_db = server_config.db_path.clone();
                let rebuild_catalog_authority = state.read().await.catalog_authority.clone();
                tokio::task::spawn_blocking(move || {
                    match public_universe::PublicUniverseSnapshot::load(&rebuild_db) {
                        Ok(public_universe::PublicUniverseLoadOutcome::Current(universe)) => {
                            if let Err(error) = rebuild_engine.rebuild_from_universe(
                                &rebuild_db,
                                &rebuild_catalog_authority,
                                &universe,
                            ) {
                                rebuild_engine.mark_unavailable();
                                tracing::error!(%error, "Failed to rebuild public search index");
                            }
                        }
                        Ok(public_universe::PublicUniverseLoadOutcome::NoActiveUniverse) => {
                            tracing::info!(
                                "Search index remains unavailable until a signed universe is active"
                            )
                        }
                        Ok(
                            public_universe::PublicUniverseLoadOutcome::ObsoleteUniverseSchema {
                                found,
                                required,
                            },
                        ) => {
                            tracing::info!(
                                found,
                                required,
                                "Search index remains unavailable while the public universe schema is rebuilt"
                            )
                        }
                        Ok(public_universe::PublicUniverseLoadOutcome::ObsoleteProfileSchema) => {
                            tracing::info!(
                                "Search index remains unavailable while obsolete public profiles are rebuilt"
                            )
                        }
                        Err(error) => {
                            tracing::error!(%error, "Failed to load public search authority")
                        }
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
