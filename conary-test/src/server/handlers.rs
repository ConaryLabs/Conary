// conary-test/src/server/handlers.rs

use crate::server::service;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

pub async fn health() -> &'static str {
    "ok"
}

pub async fn list_suites(State(state): State<AppState>) -> impl IntoResponse {
    match service::list_suites(&state) {
        Ok(suites) => (StatusCode::OK, Json(serde_json::json!(suites))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("cannot read manifest dir: {e}")})),
        ),
    }
}

#[derive(Deserialize)]
pub struct StartRunRequest {
    pub suite: String,
    pub distro: String,
    pub phase: u32,
}

pub async fn start_run(
    State(state): State<AppState>,
    Json(req): Json<StartRunRequest>,
) -> impl IntoResponse {
    match service::start_run(&state, &req.suite, &req.distro, req.phase).await {
        Ok(result) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "run_id": result.run_id,
                "status": "pending",
            })),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn list_runs(State(state): State<AppState>) -> impl IntoResponse {
    let summaries = service::list_runs(&state, usize::MAX).await;
    // HTTP handler sorts ascending by run_id for backwards compatibility.
    let mut summaries = summaries;
    summaries.sort_by_key(|s| s.run_id);
    Json(serde_json::json!(summaries))
}

pub async fn get_run(State(state): State<AppState>, Path(id): Path<u64>) -> impl IntoResponse {
    match service::get_run(&state, id).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "run not found"})),
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("report error: {msg}")})),
                )
            }
        }
    }
}

pub async fn list_distros(State(state): State<AppState>) -> impl IntoResponse {
    let distros = service::list_distros(&state);
    Json(serde_json::json!(distros))
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
