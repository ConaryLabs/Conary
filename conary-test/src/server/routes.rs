// conary-test/src/server/routes.rs

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use crate::server::handlers;
use crate::server::mcp::TestMcpServer;
use crate::server::state::AppState;

pub fn create_router(state: AppState) -> Router {
    // MCP (Model Context Protocol) endpoint for LLM agent integration.
    let state_for_mcp = state.clone();
    let mcp_service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || Ok(TestMcpServer::new(state_for_mcp.clone())),
        Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ),
        Default::default(),
    );

    Router::new()
        .route("/v1/health", get(handlers::health))
        .route("/v1/suites", get(handlers::list_suites))
        .route("/v1/runs", post(handlers::start_run))
        .route("/v1/runs", get(handlers::list_runs))
        .route("/v1/runs/:id/stream", get(handlers::stream_run))
        .route("/v1/runs/:id/cancel", post(handlers::cancel_run))
        .route("/v1/runs/:id/artifacts", get(handlers::get_run_artifacts))
        .route(
            "/v1/runs/:id/tests/:test_id/rerun",
            post(handlers::rerun_test),
        )
        .route(
            "/v1/runs/:id/tests/:test_id/logs",
            get(handlers::get_test_logs),
        )
        .route("/v1/runs/{id}", get(handlers::get_run))
        .route("/v1/distros", get(handlers::list_distros))
        .route("/v1/images", get(handlers::list_images))
        .route("/v1/images/build", post(handlers::build_image))
        .route("/v1/cleanup", post(handlers::cleanup_containers))
        .nest_service("/mcp", mcp_service)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .uri("/v1/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_stream_route_exists() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .uri("/v1/runs/1/stream")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        // The SSE endpoint should return 200 (not 404).
        assert_ne!(response.status(), 404, "route should exist");
    }

    #[tokio::test]
    async fn test_not_found() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .uri("/v1/nonexistent")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn test_get_run_not_found() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .uri("/v1/runs/12345")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 404);
    }
}
