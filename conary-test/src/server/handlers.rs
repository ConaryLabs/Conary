// conary-test/src/server/handlers.rs

use crate::error::ConaryTestError;
use crate::error_taxonomy::{self, StructuredError};
use crate::server::service;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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
) -> Response {
    match service::start_run(&state, &req.suite, &req.distro, req.phase) {
        Ok(result) => {
            // Spawn the actual test execution in a background task.
            service::spawn_run(&state, result.run_id, &req.suite, &req.distro, req.phase);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "run_id": result.run_id,
                    "status": "pending",
                })),
            )
                .into_response()
        }
        Err(ConaryTestError::Config(ref msg)) if msg.contains("unknown distro") => {
            error_taxonomy::unknown_distro(&req.distro).into_response()
        }
        Err(ConaryTestError::Config(_)) => {
            error_taxonomy::unknown_suite(&req.suite).into_response()
        }
        Err(e) => StructuredError::from(e).into_response(),
    }
}

pub async fn list_runs(State(state): State<AppState>) -> Response {
    // Try Remi first if configured.
    if let Some(ref client) = state.remi_client {
        match client.list_runs(100, None, None, None, None).await {
            Ok(data) => return (StatusCode::OK, Json(data)).into_response(),
            Err(e) => {
                tracing::debug!("Remi proxy failed for list_runs, falling back to local: {e}");
            }
        }
    }

    // Fall back to in-memory DashMap.
    let mut summaries = service::list_runs(&state, usize::MAX);
    // HTTP handler sorts ascending by run_id for backwards compatibility.
    summaries.sort_by_key(|s| s.run_id);
    Json(serde_json::json!(summaries)).into_response()
}

pub async fn get_run(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    // Try Remi first if configured.
    if let Some(ref client) = state.remi_client {
        match client.get_run(id as i64).await {
            Ok(data) => return (StatusCode::OK, Json(data)).into_response(),
            Err(e) => {
                tracing::debug!("Remi proxy failed for get_run {id}, falling back to local: {e}");
            }
        }
    }

    // Fall back to in-memory DashMap.
    match service::get_run(&state, id) {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(ConaryTestError::RunNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(error_taxonomy::run_not_found(id)),
        )
            .into_response(),
        Err(e) => StructuredError::from(e).into_response(),
    }
}

pub async fn list_distros(State(state): State<AppState>) -> impl IntoResponse {
    let distros = service::list_distros(&state);
    Json(serde_json::json!(distros))
}

pub async fn cancel_run(State(state): State<AppState>, Path(id): Path<u64>) -> impl IntoResponse {
    match service::cancel_run(&state, id) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"run_id": id, "status": "cancelled"})),
        ),
        Err(ConaryTestError::RunNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "run not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn rerun_test(
    State(state): State<AppState>,
    Path((id, test_id)): Path<(u64, String)>,
) -> impl IntoResponse {
    match service::rerun_test(&state, id, &test_id) {
        Ok(rerun) => {
            // Spawn execution using the original suite's manifest.
            service::spawn_run(
                &state,
                rerun.run_id,
                &rerun.suite_name,
                &rerun.distro,
                rerun.phase,
            );
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "original_run_id": id,
                    "test_id": test_id,
                    "new_run_id": rerun.run_id,
                    "status": "pending",
                })),
            )
        }
        Err(ConaryTestError::RunNotFound(_) | ConaryTestError::TestNotFound { .. }) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn get_test_logs(
    State(state): State<AppState>,
    Path((id, test_id)): Path<(u64, String)>,
) -> Response {
    // Try Remi first if configured.
    if let Some(ref client) = state.remi_client {
        match client.get_logs(id as i64, &test_id, None, None).await {
            Ok(data) => return (StatusCode::OK, Json(data)).into_response(),
            Err(e) => {
                tracing::debug!(
                    "Remi proxy failed for get_test_logs {id}/{test_id}, falling back to local: {e}"
                );
            }
        }
    }

    // Fall back to in-memory DashMap.
    match service::get_test_logs(&state, id, &test_id) {
        Ok(logs) => (StatusCode::OK, Json(serde_json::json!(logs))).into_response(),
        Err(ConaryTestError::RunNotFound(_) | ConaryTestError::TestNotFound { .. }) => (
            StatusCode::NOT_FOUND,
            Json(error_taxonomy::test_not_found(id, &test_id)),
        )
            .into_response(),
        Err(e) => StructuredError::from(e).into_response(),
    }
}

pub async fn get_run_artifacts(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match service::get_run_artifacts(&state, id) {
        Ok(artifacts) => (StatusCode::OK, Json(serde_json::json!(artifacts))),
        Err(ConaryTestError::RunNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "run not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

#[derive(Deserialize)]
pub struct BuildImageRequest {
    pub distro: String,
}

pub async fn build_image(
    State(state): State<AppState>,
    Json(req): Json<BuildImageRequest>,
) -> Response {
    match service::build_image(&state, &req.distro).await {
        Ok(tag) => (
            StatusCode::OK,
            Json(serde_json::json!({"distro": req.distro, "image": tag, "status": "built"})),
        )
            .into_response(),
        Err(ConaryTestError::Config(_)) => {
            error_taxonomy::unknown_distro(&req.distro).into_response()
        }
        Err(e) => StructuredError::from(e).into_response(),
    }
}

pub async fn list_images(State(state): State<AppState>) -> impl IntoResponse {
    match service::list_images(&state).await {
        Ok(images) => (StatusCode::OK, Json(serde_json::json!(images))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn cleanup_containers(State(state): State<AppState>) -> impl IntoResponse {
    match service::cleanup_containers(&state).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// SSE endpoint that streams live test events for a specific run.
///
/// Subscribes to the broadcast channel, filters events by `run_id`, and
/// streams them as `text/event-stream`. The stream ends when the
/// `RunComplete` event is received or the channel closes.
pub async fn stream_run(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> axum::response::Response {
    let rx = state.event_tx.subscribe();

    let stream = async_stream::stream! {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    // Only emit events for the requested run.
                    if event.run_id() != id {
                        continue;
                    }
                    let is_complete = matches!(event, crate::report::stream::TestEvent::RunComplete { .. });
                    let event_name = event.event_name();
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok::<_, std::convert::Infallible>(
                        axum::response::sse::Event::default()
                            .event(event_name)
                            .data(data)
                    );
                    if is_complete {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("SSE client for run {id} lagged by {n} events");
                    yield Ok(
                        axum::response::sse::Event::default()
                            .event("error")
                            .data(format!(r#"{{"error":"Lagged by {n} events","code":"LAGGED"}}"#))
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    axum::response::sse::Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(30))
                .text("ping"),
        )
        .into_response()
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
        let app = create_router(test_fixtures::test_app_state(), None);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/runs")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"suite":"smoke","distro":"nonexistent","phase":1}"#,
            ))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        // Config errors (unknown distro) return 422 with structured error.
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"], "unknown_distro");
        assert_eq!(parsed["category"], "config");
        assert!(!parsed["transient"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_start_run_valid_distro() {
        let state = test_fixtures::test_app_state();
        let app = create_router(state.clone(), None);
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

        assert_eq!(state.runs.len(), 1);
    }

    #[tokio::test]
    async fn test_get_run_not_found() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let req = Request::builder()
            .uri("/v1/runs/9999")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_list_runs_empty() {
        let app = create_router(test_fixtures::test_app_state(), None);
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
        let app = create_router(test_fixtures::test_app_state(), None);
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

    #[tokio::test]
    async fn test_cancel_run_not_found() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/runs/9999/cancel")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_cancel_run_success() {
        let state = test_fixtures::test_app_state();
        // Create a run.
        let suite = crate::engine::suite::TestSuite::new("smoke", 1);
        state.insert_run(42, suite);
        let _flag = state.register_cancel_flag(42);

        let app = create_router(state, None);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/runs/42/cancel")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rerun_test_not_found() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/runs/9999/tests/T01/rerun")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_test_logs_not_found() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let req = Request::builder()
            .uri("/v1/runs/9999/tests/T01/logs")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_run_artifacts_not_found() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let req = Request::builder()
            .uri("/v1/runs/9999/artifacts")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_stream_run_returns_sse() {
        use crate::report::stream::TestEvent;

        let state = test_fixtures::test_app_state();
        let app = create_router(state.clone(), None);

        // Spawn the request in background, then send an event.
        let handle = tokio::spawn(async move {
            let req = Request::builder()
                .uri("/v1/runs/42/stream")
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap()
        });

        // Send a RunComplete event to close the stream.
        tokio::task::yield_now().await;
        state.emit_event(TestEvent::RunComplete {
            run_id: 42,
            passed: 1,
            failed: 0,
            skipped: 0,
        });

        let response = handle.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("text/event-stream"),
            "Expected text/event-stream, got: {ct}"
        );
    }
}
