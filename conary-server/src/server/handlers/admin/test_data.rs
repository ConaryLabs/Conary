// conary-server/src/server/handlers/admin/test_data.rs
//! Test data API handlers

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service::{self, PushTestResultData, ServiceError};
use crate::server::auth::{Scope, TokenScopes, json_error};

use super::check_scope;

// ---------------------------------------------------------------------------
// Request / query types
// ---------------------------------------------------------------------------

/// Request body for creating a new test run.
#[derive(Debug, Deserialize)]
pub struct CreateTestRunRequest {
    pub suite: String,
    pub distro: String,
    pub phase: u32,
    pub triggered_by: Option<String>,
    pub source_commit: Option<String>,
}

/// Request body for updating a test run's status.
#[derive(Debug, Deserialize)]
pub struct UpdateTestRunRequest {
    pub status: String,
    pub total: Option<u32>,
    pub passed: Option<u32>,
    pub failed: Option<u32>,
    pub skipped: Option<u32>,
}

/// Query parameters for listing test runs.
#[derive(Debug, Default, Deserialize)]
pub struct ListTestRunsQuery {
    pub limit: Option<u32>,
    pub cursor: Option<i64>,
    pub suite: Option<String>,
    pub distro: Option<String>,
    pub status: Option<String>,
}

/// Query parameters for fetching test logs.
#[derive(Debug, Default, Deserialize)]
pub struct TestLogsQuery {
    pub stream: Option<String>,
    pub step_index: Option<u32>,
}

/// Query parameters for garbage-collecting old test runs.
#[derive(Debug, Default, Deserialize)]
pub struct TestGcQuery {
    pub older_than_days: Option<u32>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /v1/admin/test-runs
///
/// Create a new test run. Returns the newly created run with HTTP 201.
pub async fn create_test_run(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<CreateTestRunRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::create_test_run(
        &state,
        body.suite,
        body.distro,
        body.phase,
        body.triggered_by,
        body.source_commit,
    )
    .await
    {
        Ok(run) => (StatusCode::CREATED, Json(run)).into_response(),
        Err(ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(e) => {
            tracing::error!("Failed to create test run: {e}");
            json_error(500, "Failed to create test run", "INTERNAL_ERROR")
        }
    }
}

/// PUT /v1/admin/test-runs/:id
///
/// Update the status and optional aggregate counts of a test run.
pub async fn update_test_run(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<i64>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<UpdateTestRunRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::update_test_run_status(
        &state,
        id,
        body.status,
        body.total,
        body.passed,
        body.failed,
        body.skipped,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(ServiceError::NotFound(msg)) => json_error(404, &msg, "NOT_FOUND"),
        Err(ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(e) => {
            tracing::error!("Failed to update test run {id}: {e}");
            json_error(500, "Failed to update test run", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/test-runs/:id/results
///
/// Push a test result (with steps and logs) into an existing run.
pub async fn push_test_result(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<i64>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<PushTestResultData>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::push_test_result(&state, id, body).await {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(ServiceError::NotFound(msg)) | Err(ServiceError::Internal(msg))
            if msg.contains("not found") =>
        {
            json_error(404, &msg, "NOT_FOUND")
        }
        Err(ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(e) => {
            tracing::error!("Failed to push test result for run {id}: {e}");
            json_error(500, "Failed to push test result", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/test-runs
///
/// List test runs with optional filters and cursor-based pagination.
pub async fn list_test_runs(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<ListTestRunsQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    let limit = query.limit.unwrap_or(20).min(100);

    match admin_service::list_test_runs(
        &state,
        limit,
        query.cursor,
        query.suite,
        query.distro,
        query.status,
    )
    .await
    {
        Ok(runs) => Json(runs).into_response(),
        Err(e) => {
            tracing::error!("Failed to list test runs: {e}");
            json_error(500, "Failed to list test runs", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/test-runs/:id
///
/// Get a test run with all its results.
pub async fn get_test_run(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<i64>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::get_test_run_detail(&state, id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(ServiceError::NotFound(msg)) => json_error(404, &msg, "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to get test run {id}: {e}");
            json_error(500, "Failed to get test run", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/test-runs/:id/tests/:test_id
///
/// Get a single test result with its steps and logs.
pub async fn get_test_detail(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((id, test_id)): Path<(i64, String)>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::get_test_detail(&state, id, test_id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(ServiceError::NotFound(msg)) => json_error(404, &msg, "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to get test detail for run {id}: {e}");
            json_error(500, "Failed to get test detail", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/test-runs/:id/tests/:test_id/logs
///
/// Get log entries for a specific test, optionally filtered by stream or step index.
pub async fn get_test_logs(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((id, test_id)): Path<(i64, String)>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<TestLogsQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::get_test_logs(&state, id, test_id, query.stream, query.step_index).await {
        Ok(logs) => Json(logs).into_response(),
        Err(ServiceError::NotFound(msg)) => json_error(404, &msg, "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to get test logs for run {id}: {e}");
            json_error(500, "Failed to get test logs", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/test-health
///
/// Return a health summary of recent test activity.
pub async fn test_health(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::test_health(&state).await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => {
            tracing::error!("Failed to get test health: {e}");
            json_error(500, "Failed to get test health", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/test-runs/gc
///
/// Delete test runs older than the specified number of days.
/// Defaults to 90 days if `older_than_days` is not provided.
pub async fn test_gc(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<TestGcQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    let older_than_days = query.older_than_days.unwrap_or(90);

    match admin_service::test_gc(&state, older_than_days).await {
        Ok(deleted) => Json(serde_json::json!({
            "deleted": deleted,
            "older_than_days": older_than_days,
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to garbage-collect test runs: {e}");
            json_error(500, "Failed to garbage-collect test runs", "INTERNAL_ERROR")
        }
    }
}
