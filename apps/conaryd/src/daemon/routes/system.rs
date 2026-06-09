// apps/conaryd/src/daemon/routes/system.rs
//! Daemon health, metrics, and system operation routes.

use super::auth::require_auth;
use super::errors::{ApiResult, not_implemented_error};
use super::types::{HealthResponse, SharedState, VersionResponse};
use crate::daemon::auth::{Action, PeerCredentials};
use axum::{
    Router,
    extract::{Extension, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use std::sync::atomic::Ordering;

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
    Err(not_implemented_error(
        "System state listing is not implemented in conaryd preview. Use the CLI directly.",
    ))
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

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        body_bytes, body_json, create_test_state, current_process_creds, test_router,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_handler_health_returns_200() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["status"], "healthy");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert!(json["uptime_secs"].is_number());
        assert!(json.get("pid").is_none());
    }

    #[tokio::test]
    async fn test_handler_version_returns_info() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/version")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["api_version"], "1.0");
        assert!(json.get("schema_version").is_none());
    }

    #[tokio::test]
    async fn test_handler_metrics_returns_prometheus_format() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/metrics")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let text = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(text.contains("conary_jobs_total"));
        assert!(text.contains("conary_jobs_running"));
        assert!(text.contains("conary_sse_connections"));
    }

    #[tokio::test]
    async fn test_handler_list_states_not_implemented() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/system/states")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
        assert_eq!(
            json["detail"],
            "System state listing is not implemented in conaryd preview. Use the CLI directly."
        );
    }

    #[tokio::test]
    async fn test_handler_rollback_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/system/rollback")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
    }

    #[tokio::test]
    async fn test_handler_verify_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/system/verify")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
    }

    #[tokio::test]
    async fn test_handler_gc_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/system/gc")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
    }

    #[tokio::test]
    async fn test_handler_system_endpoints_require_auth() {
        let (state, _dir) = create_test_state();
        // No credentials
        let app = test_router(state, None);

        for endpoint in &["/v1/system/rollback", "/v1/system/verify", "/v1/system/gc"] {
            let app_clone = app.clone();
            let request = Request::builder()
                .method("POST")
                .uri(*endpoint)
                .body(Body::empty())
                .unwrap();

            let response = app_clone.oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "Expected 403 for {}",
                endpoint
            );
        }
    }
}
