// apps/conaryd/src/daemon/routes/system.rs
//! Daemon health, metrics, and system operation routes.

use super::types::{HealthResponse, SharedState, VersionResponse};
use axum::{Router, extract::State, response::Json, routing::get};
use std::sync::atomic::Ordering;

pub(super) fn root_router() -> Router<SharedState> {
    Router::new().route("/health", get(health_handler))
}

pub(super) fn v1_router() -> Router<SharedState> {
    Router::new()
        .route("/version", get(version_handler))
        .route("/metrics", get(metrics_handler))
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
}
