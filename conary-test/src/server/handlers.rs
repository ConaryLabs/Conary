// conary-test/src/server/handlers.rs

use crate::config::load_manifest;
use crate::engine::suite::TestSuite;
use crate::report::json::to_json_value;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

pub async fn health() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct SuiteInfo {
    name: String,
    phase: u32,
    test_count: usize,
}

pub async fn list_suites(State(state): State<AppState>) -> impl IntoResponse {
    let manifest_dir = std::path::Path::new(&state.manifest_dir);

    let entries = match std::fs::read_dir(manifest_dir) {
        Ok(entries) => entries,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("cannot read manifest dir: {e}")})),
            );
        }
    };

    let mut suites = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml")
            && let Ok(manifest) = load_manifest(&path)
        {
            suites.push(SuiteInfo {
                name: manifest.suite.name,
                phase: manifest.suite.phase,
                test_count: manifest.test.len(),
            });
        }
    }

    suites.sort_by(|a, b| a.name.cmp(&b.name));
    (StatusCode::OK, Json(serde_json::json!(suites)))
}

#[derive(Deserialize)]
pub struct StartRunRequest {
    pub suite: String,
    pub distro: String,
    pub phase: u32,
}

#[derive(Serialize)]
struct StartRunResponse {
    run_id: u64,
    status: String,
}

pub async fn start_run(
    State(state): State<AppState>,
    Json(req): Json<StartRunRequest>,
) -> impl IntoResponse {
    if !state.distros_contain(&req.distro) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("unknown distro: {}", req.distro)})),
        );
    }

    let run_id = AppState::next_run_id();
    let suite = TestSuite::new(&req.suite, req.phase);

    state.insert_run(run_id, suite).await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!(StartRunResponse {
            run_id,
            status: "pending".to_string(),
        })),
    )
}

#[derive(Serialize)]
struct RunSummary {
    run_id: u64,
    suite: String,
    phase: u32,
    status: String,
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
}

pub async fn list_runs(State(state): State<AppState>) -> impl IntoResponse {
    let runs = state.runs.read().await;
    let mut summaries: Vec<RunSummary> = runs
        .iter()
        .map(|(&id, suite)| RunSummary {
            run_id: id,
            suite: suite.name.clone(),
            phase: suite.phase,
            status: suite.status.as_str().to_string(),
            total: suite.total(),
            passed: suite.passed(),
            failed: suite.failed(),
            skipped: suite.skipped(),
        })
        .collect();

    summaries.sort_by_key(|s| s.run_id);
    Json(serde_json::json!(summaries))
}

pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let runs = state.runs.read().await;
    match runs.get(&id) {
        Some(suite) => match to_json_value(suite) {
            Ok(value) => (StatusCode::OK, Json(value)),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("report error: {e}")})),
            ),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "run not found"})),
        ),
    }
}

#[derive(Serialize)]
struct DistroInfo {
    name: String,
    remi_distro: String,
    repo_name: String,
}

pub async fn list_distros(State(state): State<AppState>) -> impl IntoResponse {
    let mut distros: Vec<DistroInfo> = state
        .config
        .distros
        .iter()
        .map(|(name, cfg)| DistroInfo {
            name: name.clone(),
            remi_distro: cfg.remi_distro.clone(),
            repo_name: cfg.repo_name.clone(),
        })
        .collect();

    distros.sort_by(|a, b| a.name.cmp(&b.name));
    Json(serde_json::json!(distros))
}

impl AppState {
    fn distros_contain(&self, name: &str) -> bool {
        self.config.distros.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::create_router;
    use crate::test_fixtures;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health() {
        let response = health().await;
        assert_eq!(response, "ok");
    }

    #[tokio::test]
    async fn test_start_run_unknown_distro() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .method("POST")
            .uri("/v1/runs")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"suite":"smoke","distro":"nonexistent","phase":1}"#,
            ))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_start_run_valid_distro() {
        let state = test_fixtures::test_app_state();
        let app = create_router(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/v1/runs")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"suite":"smoke","distro":"fedora43","phase":1}"#,
            ))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let runs = state.runs.read().await;
        assert_eq!(runs.len(), 1);
    }

    #[tokio::test]
    async fn test_get_run_not_found() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .uri("/v1/runs/9999")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_list_runs_empty() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .uri("/v1/runs")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let arr: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(arr.is_empty());
    }

    #[tokio::test]
    async fn test_list_distros() {
        let app = create_router(test_fixtures::test_app_state());
        let req = Request::builder()
            .uri("/v1/distros")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let arr: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "fedora43");
    }
}
