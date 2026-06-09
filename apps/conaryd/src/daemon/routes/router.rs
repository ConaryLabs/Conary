// apps/conaryd/src/daemon/routes/router.rs
//! Axum router assembly for conaryd routes.

use super::auth::auth_gate_middleware;
use super::types::SharedState;
use super::{events, query, system, transactions};
use axum::{Router, extract::DefaultBodyLimit, middleware};

pub(super) const DAEMON_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// Build the main router
pub fn build_router(state: SharedState) -> Router {
    system::root_router()
        .nest("/v1", build_v1_router(state.clone()))
        .with_state(state)
}

/// Build the v1 API router
fn build_v1_router(state: SharedState) -> Router<SharedState> {
    Router::new()
        .merge(system::v1_router())
        .merge(transactions::router())
        .merge(query::router())
        .merge(events::router())
        .layer(middleware::from_fn_with_state(state, auth_gate_middleware))
        .layer(DefaultBodyLimit::max(DAEMON_BODY_LIMIT_BYTES))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_state, current_process_creds, test_router};
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_v1_router_rejects_request_bodies_over_2mb() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());
        let oversized = "1,".repeat(DAEMON_BODY_LIMIT_BYTES / 2 + 64);
        let body = format!(
            "{{\"batch_size\":10,\"trove_ids\":[{}],\"types\":[],\"force\":false}}",
            oversized
        );

        let request = Request::builder()
            .method("POST")
            .uri("/v1/enhance")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_handler_nonexistent_route() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = Request::builder()
            .uri("/v1/does-not-exist")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
