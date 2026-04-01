// apps/remi/src/server/routes/admin.rs
//! Remi admin router assembly and admin-only handlers.

use super::mcp;
use super::public::prometheus_metrics;
use super::*;
use crate::server::handlers::admin as admin_handlers;

async fn require_localhost(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !addr.ip().is_loopback() {
        warn!(
            ip = %addr.ip(),
            "Rejected non-loopback connection to internal admin API"
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

pub fn create_admin_router(state: Arc<RwLock<ServerState>>) -> Router {
    Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/v1/admin/convert", post(packages::trigger_conversion))
        .route("/v1/admin/cache/stats", get(chunks::cache_stats))
        .route("/v1/admin/evict", post(chunks::trigger_eviction))
        .route("/v1/admin/bloom/rebuild", post(chunks::rebuild_bloom))
        .route("/v1/admin/metrics", get(admin_metrics))
        .route("/v1/admin/metrics/prometheus", get(prometheus_metrics))
        .route("/v1/admin/negative-cache/stats", get(negative_cache_stats))
        .route("/v1/admin/negative-cache/clear", post(negative_cache_clear))
        .route("/v1/admin/recipes/build", post(recipes::build_recipe))
        .route("/v1/admin/info", get(server_info))
        .route("/v1/admin/refresh", post(refresh_upstream))
        .route("/v1/admin/models/{name}", put(models::put_model))
        .route(
            "/v1/admin/tuf/refresh-timestamp",
            post(tuf::refresh_timestamp),
        )
        .route(
            "/v1/admin/packages/{distro}",
            post(admin_handlers::upload_package),
        )
        .route_layer(middleware::from_fn(require_localhost))
        .with_state(state)
}

pub fn create_external_admin_router(
    state: Arc<RwLock<ServerState>>,
    rate_limiters: Option<Arc<crate::server::rate_limit::AdminRateLimiters>>,
) -> Router {
    let protected = Router::new()
        .route("/v1/admin/tokens", post(admin_handlers::create_token))
        .route("/v1/admin/tokens", get(admin_handlers::list_tokens))
        .route(
            "/v1/admin/tokens/{id}",
            delete(admin_handlers::delete_token),
        )
        .route(
            "/v1/admin/test-fixtures/{*path}",
            put(admin_handlers::upload_fixture),
        )
        .route(
            "/v1/admin/test-artifacts/{*path}",
            put(admin_handlers::upload_test_artifact),
        )
        .route(
            "/v1/admin/packages/{distro}",
            post(admin_handlers::upload_package),
        )
        .route(
            "/v1/admin/ci/workflows",
            get(admin_handlers::ci_list_workflows),
        )
        .route(
            "/v1/admin/ci/workflows/{name}/runs",
            get(admin_handlers::ci_list_runs),
        )
        .route("/v1/admin/ci/runs/{id}", get(admin_handlers::ci_get_run))
        .route(
            "/v1/admin/ci/runs/{id}/logs",
            get(admin_handlers::ci_get_logs),
        )
        .route(
            "/v1/admin/ci/workflows/{name}/dispatch",
            post(admin_handlers::ci_dispatch),
        )
        .route(
            "/v1/admin/ci/mirror-sync",
            post(admin_handlers::ci_mirror_sync),
        )
        .route("/v1/admin/repos", get(admin_handlers::list_repos))
        .route("/v1/admin/repos", post(admin_handlers::create_repo))
        .route("/v1/admin/repos/{name}", get(admin_handlers::get_repo))
        .route("/v1/admin/repos/{name}", put(admin_handlers::update_repo))
        .route(
            "/v1/admin/repos/{name}",
            delete(admin_handlers::delete_repo),
        )
        .route(
            "/v1/admin/repos/{name}/sync",
            post(admin_handlers::sync_repo),
        )
        .route("/v1/admin/refresh", post(admin_handlers::refresh_repos))
        .route(
            "/v1/admin/federation/peers",
            get(admin_handlers::list_peers),
        )
        .route("/v1/admin/federation/peers", post(admin_handlers::add_peer))
        .route(
            "/v1/admin/federation/peers/{id}",
            delete(admin_handlers::delete_peer),
        )
        .route(
            "/v1/admin/federation/peers/{id}/health",
            get(admin_handlers::peer_health),
        )
        .route(
            "/v1/admin/federation/config",
            get(admin_handlers::get_federation_config),
        )
        .route(
            "/v1/admin/federation/config",
            put(admin_handlers::update_federation_config),
        )
        .route(
            "/v1/admin/test-runs/gc",
            delete(admin_handlers::test_data::test_gc),
        )
        .route(
            "/v1/admin/test-health",
            get(admin_handlers::test_data::test_health),
        )
        .route(
            "/v1/admin/test-runs",
            post(admin_handlers::test_data::create_test_run)
                .get(admin_handlers::test_data::list_test_runs),
        )
        .route(
            "/v1/admin/test-runs/{id}",
            get(admin_handlers::test_data::get_test_run)
                .put(admin_handlers::test_data::update_test_run),
        )
        .route(
            "/v1/admin/test-runs/{id}/results",
            post(admin_handlers::test_data::push_test_result),
        )
        .route(
            "/v1/admin/test-runs/{id}/tests/{test_id}",
            get(admin_handlers::test_data::get_test_detail),
        )
        .route(
            "/v1/admin/test-runs/{id}/tests/{test_id}/logs",
            get(admin_handlers::test_data::get_test_logs),
        )
        .route(
            "/v1/admin/audit",
            get(admin_handlers::query_audit).delete(admin_handlers::purge_audit),
        )
        .route("/v1/admin/events", get(admin_handlers::sse_events))
        .merge(mcp::create_mcp_router(state.clone()))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::server::audit::audit_middleware,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::server::auth::auth_middleware,
        ));

    let unprotected = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/v1/admin/openapi.json", get(openapi::openapi_spec));

    let mut router = unprotected
        .merge(protected)
        .route_layer(middleware::from_fn(
            crate::server::rate_limit::rate_limit_middleware,
        ))
        .with_state(state);

    if let Some(limiters) = rate_limiters {
        router = router.layer(axum::Extension(limiters));
    }

    router
}

async fn admin_metrics(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<AdminMetrics> {
    let state_guard = state.read().await;

    let metrics_snapshot = state_guard.metrics.snapshot();
    let negative_cache_stats = state_guard.negative_cache.stats().await;
    let job_stats = state_guard.job_manager.stats();

    Json(AdminMetrics {
        requests_total: metrics_snapshot.requests_total,
        cache_hits: metrics_snapshot.hits,
        cache_misses: metrics_snapshot.misses,
        hit_rate: metrics_snapshot.hit_rate,
        bloom_rejects: metrics_snapshot.bloom_rejects,
        bytes_served: metrics_snapshot.bytes_served,
        upstream_fetches: metrics_snapshot.upstream_fetches,
        upstream_errors: metrics_snapshot.upstream_errors,
        uptime_secs: metrics_snapshot.uptime_secs,
        negative_cache_entries: negative_cache_stats.active_entries,
        negative_cache_hits: negative_cache_stats.total_hits,
        jobs_pending: job_stats.pending,
        jobs_converting: job_stats.converting,
        jobs_completed: job_stats.completed,
        jobs_failed: job_stats.failed,
    })
}

#[derive(Serialize)]
struct AdminMetrics {
    requests_total: u64,
    cache_hits: u64,
    cache_misses: u64,
    hit_rate: f64,
    bloom_rejects: u64,
    bytes_served: u64,
    upstream_fetches: u64,
    upstream_errors: u64,
    uptime_secs: u64,
    negative_cache_entries: usize,
    negative_cache_hits: u64,
    jobs_pending: usize,
    jobs_converting: usize,
    jobs_completed: usize,
    jobs_failed: usize,
}

async fn negative_cache_stats(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<serde_json::Value> {
    let state_guard = state.read().await;
    let stats = state_guard.negative_cache.stats().await;

    Json(serde_json::json!({
        "total_entries": stats.total_entries,
        "active_entries": stats.active_entries,
        "expired_entries": stats.expired_entries,
        "total_hits": stats.total_hits,
        "ttl_seconds": stats.ttl_secs,
    }))
}

async fn negative_cache_clear(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<serde_json::Value> {
    let state_guard = state.read().await;
    let removed = state_guard.negative_cache.clear_all().await;

    Json(serde_json::json!({
        "cleared": true,
        "entries_removed": removed,
    }))
}

async fn server_info(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<serde_json::Value> {
    let state_guard = state.read().await;
    let config = &state_guard.config;

    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "bind_addr": config.bind_addr.to_string(),
        "db_configured": config.db_path.exists(),
        "chunk_dir_configured": config.chunk_dir.exists(),
        "max_concurrent_conversions": config.max_concurrent_conversions,
        "cache_max_bytes": config.cache_max_bytes,
        "bloom_filter_enabled": config.enable_bloom_filter,
        "rate_limit_enabled": config.enable_rate_limit,
        "trusted_proxy_header_set": state_guard.trusted_proxy_header.is_some(),
    }))
}

async fn refresh_upstream(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<admin_handlers::RefreshQuery>,
) -> Response {
    match crate::server::admin_service::refresh_repositories(&state, query.force).await {
        Ok(results) => {
            let synced = results.iter().filter(|r| !r.skipped).count();
            let skipped = results.iter().filter(|r| r.skipped).count();

            {
                let guard = state.read().await;
                guard.publish_event(
                    "repos.refreshed",
                    serde_json::json!({
                        "force": query.force,
                        "synced": synced,
                        "skipped": skipped,
                    }),
                );
            }

            Json(serde_json::json!({
                "status": "ok",
                "force": query.force,
                "synced": synced,
                "skipped": skipped,
                "results": results
                    .into_iter()
                    .map(|r| serde_json::json!({
                        "name": r.name,
                        "packages_synced": r.packages_synced,
                        "skipped": r.skipped,
                    }))
                    .collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!("Upstream refresh failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "status": "error",
                    "message": e.to_string(),
                })),
            )
                .into_response()
        }
    }
}
