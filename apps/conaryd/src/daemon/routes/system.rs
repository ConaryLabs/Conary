// apps/conaryd/src/daemon/routes/system.rs
//! Daemon health, metrics, and system operation routes.

use super::*;

pub(super) fn root_router() -> Router<SharedState> {
    Router::new().route("/health", get(health_handler))
}

pub(super) fn v1_router() -> Router<SharedState> {
    Router::new()
        .route("/version", get(version_handler))
        .route("/metrics", get(metrics_handler))
        .route("/system/states", get(list_states_handler))
        .route("/system/rollback", post(rollback_handler))
        .route("/system/verify", post(verify_handler))
        .route("/system/gc", post(gc_handler))
}

async fn health_handler(State(state): State<SharedState>) -> Json<HealthResponse> {
    let uptime_secs = state.uptime_secs();

    Json(HealthResponse {
        status: "healthy",
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs,
    })
}

async fn version_handler(State(_state): State<SharedState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION"),
        api_version: "1.0",
        build_date: option_env!("BUILD_DATE"),
        git_commit: option_env!("GIT_COMMIT"),
    })
}

async fn metrics_handler(State(state): State<SharedState>) -> String {
    let m = &state.metrics;

    format!(
        r#"# HELP conary_jobs_total Total jobs processed
# TYPE conary_jobs_total counter
conary_jobs_total {}

# HELP conary_jobs_running Currently running jobs
# TYPE conary_jobs_running gauge
conary_jobs_running {}

# HELP conary_jobs_completed Jobs completed successfully
# TYPE conary_jobs_completed counter
conary_jobs_completed {}

# HELP conary_jobs_failed Jobs that failed
# TYPE conary_jobs_failed counter
conary_jobs_failed {}

# HELP conary_jobs_cancelled Jobs that were cancelled
# TYPE conary_jobs_cancelled counter
conary_jobs_cancelled {}

# HELP conary_sse_connections Active SSE connections
# TYPE conary_sse_connections gauge
conary_sse_connections {}
"#,
        m.jobs_total.load(Ordering::Relaxed),
        m.jobs_running.load(Ordering::Relaxed),
        m.jobs_completed.load(Ordering::Relaxed),
        m.jobs_failed.load(Ordering::Relaxed),
        m.jobs_cancelled.load(Ordering::Relaxed),
        m.sse_connections.load(Ordering::Relaxed),
    )
}

async fn list_states_handler(State(_state): State<SharedState>) -> ApiResult<Json<Vec<()>>> {
    Ok(Json(vec![]))
}

async fn rollback_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    require_auth(&state.auth_checker, &creds, Action::Rollback)?;
    Err(not_implemented_error("Rollback not yet implemented"))
}

async fn verify_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
) -> ApiResult<Json<serde_json::Value>> {
    require_auth(&state.auth_checker, &creds, Action::Verify)?;
    Err(not_implemented_error(
        "System verification not yet implemented",
    ))
}

async fn gc_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
) -> ApiResult<Json<serde_json::Value>> {
    require_auth(&state.auth_checker, &creds, Action::GarbageCollect)?;
    Err(not_implemented_error(
        "Garbage collection not yet implemented",
    ))
}
