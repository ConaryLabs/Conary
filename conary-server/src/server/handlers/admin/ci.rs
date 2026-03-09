// conary-server/src/server/handlers/admin/ci.rs
//! CI proxy handlers for Forgejo integration

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::auth::{Scope, TokenScopes, json_error};
use crate::server::forgejo::FORGEJO_REPO_PATH;
use crate::server::ServerState;

use super::{check_scope, validate_path_param};

// ---------------------------------------------------------------------------
// CI proxy helpers
// ---------------------------------------------------------------------------

/// Map a [`crate::server::forgejo::ForgejoError`] to an axum JSON error response.
///
/// Uses the upstream HTTP status if available; defaults to 502 Bad Gateway.
fn forgejo_err_to_response(e: crate::server::forgejo::ForgejoError) -> Response {
    let status = e.status.unwrap_or(502);
    json_error(status, &e.message, "UPSTREAM_ERROR")
}

/// Call `crate::server::forgejo::get` and parse the response text as JSON.
async fn forgejo_get_json(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
) -> Result<serde_json::Value, Response> {
    let text = crate::server::forgejo::get(state, path)
        .await
        .map_err(forgejo_err_to_response)?;
    serde_json::from_str(&text).map_err(|e| {
        tracing::error!("Forgejo response parse error: {e}");
        json_error(502, "Invalid Forgejo response", "UPSTREAM_ERROR")
    })
}

/// Call `crate::server::forgejo::post` and parse the response text as JSON.
async fn forgejo_post_json(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, Response> {
    let text = crate::server::forgejo::post(state, path, Some(body))
        .await
        .map_err(forgejo_err_to_response)?;
    serde_json::from_str(&text).map_err(|e| {
        tracing::error!("Forgejo response parse error: {e}");
        json_error(502, "Invalid Forgejo response", "UPSTREAM_ERROR")
    })
}

// ---------------------------------------------------------------------------
// CI proxy handlers
// ---------------------------------------------------------------------------

/// GET /v1/admin/ci/workflows
///
/// List all CI workflows from Forgejo. Requires the "ci:read" scope.
pub async fn ci_list_workflows(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::CiRead) {
        return err;
    }
    match forgejo_get_json(&state, &format!("{FORGEJO_REPO_PATH}/actions/workflows")).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

/// GET /v1/admin/ci/workflows/:name/runs
///
/// List runs for a specific workflow. Requires the "ci:read" scope.
pub async fn ci_list_runs(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::CiRead) {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "workflow name") {
        return err;
    }
    match forgejo_get_json(
        &state,
        &format!("{FORGEJO_REPO_PATH}/actions/workflows/{name}/runs"),
    )
    .await
    {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

/// GET /v1/admin/ci/runs/:id
///
/// Get details for a specific CI run. Requires the "ci:read" scope.
pub async fn ci_get_run(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<i64>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::CiRead) {
        return err;
    }
    match forgejo_get_json(&state, &format!("{FORGEJO_REPO_PATH}/actions/runs/{id}")).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

/// GET /v1/admin/ci/runs/:id/logs
///
/// Fetch raw logs for a CI run. Returns plain text. Requires the "ci:read" scope.
pub async fn ci_get_logs(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<i64>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::CiRead) {
        return err;
    }

    let path = format!("{FORGEJO_REPO_PATH}/actions/runs/{id}/logs");
    match crate::server::forgejo::get(&state, &path).await {
        Ok(text) => (
            StatusCode::OK,
            [("content-type", "text/plain; charset=utf-8")],
            text,
        )
            .into_response(),
        Err(e) => forgejo_err_to_response(e),
    }
}

/// POST /v1/admin/ci/workflows/:name/dispatch
///
/// Trigger a workflow dispatch on the main branch. Requires the "ci:trigger" scope.
pub async fn ci_dispatch(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::CiTrigger) {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "workflow name") {
        return err;
    }
    let body = serde_json::json!({"ref": "main"});
    match forgejo_post_json(
        &state,
        &format!("{FORGEJO_REPO_PATH}/actions/workflows/{name}/dispatches"),
        &body,
    )
    .await
    {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

/// POST /v1/admin/ci/mirror-sync
///
/// Trigger a mirror sync from GitHub. Requires the "ci:trigger" scope.
pub async fn ci_mirror_sync(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::CiTrigger) {
        return err;
    }
    let body = serde_json::json!({});
    match forgejo_post_json(&state, &format!("{FORGEJO_REPO_PATH}/mirror-sync"), &body).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}
