// apps/remi/src/server/routes/public.rs
//! Public Remi router assembly and readiness endpoints.

use super::*;

/// Create the main public application router
///
/// This router handles:
/// - Health checks
/// - Package metadata and downloads
/// - Chunk serving
/// - Federation discovery
/// - Job status polling
///
/// Admin endpoints are NOT included here - they're on a separate listener.
pub async fn create_router(state: Arc<RwLock<ServerState>>) -> Router {
    let config = {
        let guard = state.read().await;
        guard.config.clone()
    };

    let rate_limiter = Arc::new(RateLimiter::new(
        config.rate_limit_rps,
        config.rate_limit_burst,
    ));

    if config.enable_rate_limit {
        let cleanup_limiter = Arc::clone(&rate_limiter);
        tokio::spawn(async move {
            let cleanup_interval = std::time::Duration::from_secs(300);
            let max_age = std::time::Duration::from_secs(300);
            loop {
                tokio::time::sleep(cleanup_interval).await;
                cleanup_limiter.cleanup(max_age).await;
            }
        });
    }

    let public_cors = create_cors_layer(&config, false);
    let restricted_cors = create_cors_layer(&config, true);
    let compression = CompressionLayer::new().compress_when(
        DefaultPredicate::new().and(NotForContentType::const_new("application/octet-stream")),
    );

    let chunk_routes = Router::new()
        .route("/v1/chunks/{hash}", head(chunks::head_chunk))
        .route("/v1/chunks/{hash}", get(chunks::get_chunk))
        .route("/v1/chunks/find-missing", post(chunks::find_missing))
        .route("/v1/chunks/batch", post(chunks::batch_fetch))
        .layer(restricted_cors)
        .with_state(state.clone());

    let public_routes = Router::new()
        .route("/health", get(health_check))
        .route("/health/ready", get(readiness_check))
        .route("/v1/federation/directory", get(federation::directory))
        .route("/v1/{distro}/metadata", get(index::get_metadata))
        .route("/v1/{distro}/packages/{name}", get(packages::get_package))
        .route(
            "/v1/{distro}/packages/{name}/download",
            get(packages::download_package),
        )
        .route(
            "/v1/{distro}/packages/{name}/delta",
            get(packages::get_delta),
        )
        .route("/v1/jobs/{job_id}", get(jobs::get_job_status))
        .route(
            "/v1/recipes/{name}/{version}/download",
            get(recipes::download_recipe_package),
        )
        .route("/v1/index/{distro}/{name}", get(sparse::get_sparse_entry))
        .route("/v1/index/{distro}", get(sparse::list_packages))
        .route("/v1/search", get(search::search_packages))
        .route("/v1/suggest", get(search::suggest_packages))
        .route("/v1/canonical/map", get(canonical::canonical_map))
        .route("/v1/canonical/search", get(canonical::canonical_search))
        .route("/v1/canonical/{name}", get(canonical::canonical_lookup))
        .route("/v1/groups", get(canonical::groups_list))
        .route("/v1/models/{name}", get(models::get_model))
        .route(
            "/v1/models/{name}/signature",
            get(models::get_model_signature),
        )
        .route("/v1/models", get(models::list_models))
        .route(
            "/v1/packages/{distro}/{name}",
            get(detail::get_package_detail),
        )
        .route(
            "/v1/packages/{distro}/{name}/versions",
            get(detail::get_versions),
        )
        .route(
            "/v1/packages/{distro}/{name}/dependencies",
            get(detail::get_dependencies),
        )
        .route(
            "/v1/packages/{distro}/{name}/rdepends",
            get(detail::get_reverse_dependencies),
        )
        .route("/v1/{distro}/tuf/timestamp.json", get(tuf::get_timestamp))
        .route("/v1/{distro}/tuf/snapshot.json", get(tuf::get_snapshot))
        .route("/v1/{distro}/tuf/targets.json", get(tuf::get_targets))
        .route("/v1/{distro}/tuf/root.json", get(tuf::get_root))
        .route("/v1/{distro}/tuf/{version}", get(tuf::get_versioned_root))
        .route("/v1/ccs/conary/latest", get(self_update::get_latest))
        .route("/v1/ccs/conary/versions", get(self_update::get_versions))
        .route(
            "/v1/ccs/conary/{version}",
            get(self_update::get_version_info),
        )
        .route(
            "/v1/ccs/conary/{version}/download",
            get(self_update::download),
        )
        .route(
            "/test-fixtures/{*path}",
            get(artifacts::get_fixture).head(artifacts::head_fixture),
        )
        .route(
            "/test-artifacts/{*path}",
            get(artifacts::get_test_artifact).head(artifacts::head_test_artifact),
        )
        .route(
            "/v1/derivations/probe",
            post(derivations::probe_derivations),
        )
        .route(
            "/v1/derivations/{derivation_id}",
            get(derivations::get_derivation)
                .head(derivations::head_derivation)
                .put(derivations::put_derivation),
        )
        .route("/v1/seeds/latest", get(seeds::get_latest_seed))
        .route("/v1/seeds", get(seeds::list_seeds))
        .route(
            "/v1/seeds/{seed_id}",
            get(seeds::get_seed).put(seeds::put_seed),
        )
        .route("/v1/seeds/{seed_id}/image", get(seeds::get_seed_image))
        .route(
            "/v1/profiles/{profile_hash}",
            get(profiles::get_profile).put(profiles::put_profile),
        )
        .route("/v1/stats/popular", get(detail::get_popular))
        .route("/v1/stats/recent", get(detail::get_recent))
        .route("/v1/stats/overview", get(detail::get_overview))
        .route("/metrics", get(prometheus_metrics))
        .route("/v2/", get(oci::version_check))
        .route("/v2/_catalog", get(oci::catalog))
        .route(
            "/v2/{*path}",
            get(oci::oci_catchall).head(oci::oci_catchall_head),
        )
        .layer(compression)
        .layer(public_cors)
        .with_state(state.clone());

    let web_routes = {
        let state_guard = state.read().await;
        state_guard.config.web_root.as_ref().map(|web_root| {
            Router::new().fallback_service(tower_http::services::ServeDir::new(web_root).fallback(
                tower_http::services::ServeFile::new(web_root.join("index.html")),
            ))
        })
    };

    let mut app = Router::new().merge(chunk_routes).merge(public_routes);

    if let Some(web) = web_routes {
        app = app.merge(web);
    }

    if config.enable_rate_limit {
        app = app.route_layer(middleware::from_fn_with_state(
            (rate_limiter, state.clone()),
            rate_limit_middleware,
        ));
    }

    app = app.route_layer(middleware::from_fn_with_state(
        state.clone(),
        ban_middleware,
    ));

    app = app.layer(axum::extract::DefaultBodyLimit::max(
        request_body_limit_bytes(),
    ));

    if config.enable_audit_log {
        app = app.route_layer(middleware::from_fn_with_state(
            state.clone(),
            audit_log_middleware,
        ));
    }

    app
}

async fn health_check() -> &'static str {
    "OK"
}

/// Report whether this server can actually answer requests.
///
/// Every probe fails closed: an absent database, a wrong schema revision, a
/// missing directory, insufficient free space, or a probe that could not run at
/// all all produce 503. Deploy verification and the release pipeline consume
/// this endpoint, so it must never report readiness it has not established.
/// The evaluation itself lives in `server::readiness`, where each failure mode
/// is covered by a focused test.
async fn readiness_check(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Response {
    let inputs = {
        let state_guard = state.read().await;
        let config = &state_guard.config;
        ReadinessInputs {
            db_path: config.db_path.clone(),
            chunk_dir: config.chunk_dir.clone(),
            cache_dir: config.cache_dir.clone(),
            min_free_bytes: config.readiness_min_free_bytes,
        }
    };

    // Probes open a database and stat filesystems; keep them off the runtime.
    let report = match tokio::task::spawn_blocking(move || readiness::evaluate(&inputs)).await {
        Ok(report) => report,
        Err(error) => {
            // The probe task itself failed, so readiness is unknown. Unknown is
            // not ready.
            tracing::error!("readiness probe task failed: {error}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "readiness probe did not complete",
            )
                .into_response();
        }
    };

    if report.ready {
        (StatusCode::OK, Json(report)).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
    }
}

pub(super) async fn prometheus_metrics(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> String {
    let state = state.read().await;
    state.metrics.to_prometheus()
}
