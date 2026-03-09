// conary-test/src/server/routes.rs

use axum::Router;
use axum::routing::{get, post};

use crate::server::handlers;
use crate::server::state::AppState;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(handlers::health))
        .route("/v1/suites", get(handlers::list_suites))
        .route("/v1/runs", post(handlers::start_run))
        .route("/v1/runs", get(handlers::list_runs))
        .route("/v1/runs/{id}", get(handlers::get_run))
        .route("/v1/distros", get(handlers::list_distros))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::distro::{GlobalConfig, PathsConfig, RemiConfig, SetupConfig};
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let config = GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://localhost".to_string(),
            },
            paths: PathsConfig {
                db: "/tmp/test.db".to_string(),
                conary_bin: "/usr/bin/conary".to_string(),
                results_dir: "/tmp/results".to_string(),
                fixture_dir: None,
            },
            setup: SetupConfig::default(),
            distros: HashMap::new(),
            fixtures: None,
        };
        AppState::new(config, "/tmp/manifests".to_string())
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = create_router(test_state());
        let req = Request::builder()
            .uri("/v1/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_not_found() {
        let app = create_router(test_state());
        let req = Request::builder()
            .uri("/v1/nonexistent")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn test_get_run_not_found() {
        let app = create_router(test_state());
        let req = Request::builder()
            .uri("/v1/runs/12345")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 404);
    }
}
