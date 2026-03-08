// conary-server/src/server/handlers/admin.rs
//! Handlers for the external admin API

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use rusqlite::OptionalExtension;

use crate::server::auth::{TokenScopes, generate_token, hash_token, json_error};
use crate::server::ServerState;

/// Validate a path parameter against a safe pattern.
///
/// Rejects values containing slashes, `..`, null bytes, or characters
/// outside `[a-zA-Z0-9._-]`. Returns a 400 Bad Request response on failure.
fn validate_path_param(value: &str, param_name: &str) -> Option<Response> {
    if value.is_empty()
        || value.contains('/')
        || value.contains("..")
        || value.contains('\0')
        || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        Some(json_error(
            400,
            &format!("Invalid {param_name}: must match [a-zA-Z0-9._-]+"),
            "INVALID_PARAMETER",
        ))
    } else {
        None
    }
}

/// Request body for creating a new admin token.
#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    /// Human-readable name for the token (1-128 characters).
    pub name: String,
    /// Comma-separated scopes (defaults to "admin" if omitted).
    pub scopes: Option<String>,
}

/// Response body after successfully creating a token.
#[derive(Debug, Serialize)]
pub struct CreateTokenResponse {
    pub id: i64,
    pub name: String,
    pub token: String,
    pub scopes: String,
}

/// Check that the caller has the required scope, returning an error response if not.
fn check_scope(scopes: &Option<axum::Extension<TokenScopes>>, required: &str) -> Option<Response> {
    match scopes {
        Some(axum::Extension(s)) if s.has_scope(required) => None,
        Some(_) => Some(json_error(403, "Insufficient scope", "INSUFFICIENT_SCOPE")),
        None => Some(json_error(401, "Not authenticated", "UNAUTHORIZED")),
    }
}

/// POST /v1/admin/tokens
///
/// Create a new admin API token. Requires the "admin" scope.
/// Returns the plaintext token exactly once -- it cannot be retrieved again.
pub async fn create_token(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<CreateTokenRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "admin") {
        return err;
    }

    // Validate name length
    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 {
        return json_error(
            400,
            "Token name must be 1-128 characters",
            "INVALID_NAME",
        );
    }

    let scopes_str = body.scopes.unwrap_or_else(|| "admin".to_string());

    // Generate and hash the token
    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let name_owned = name.to_string();
    let scopes_clone = scopes_str.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::admin_token::create(&conn, &name_owned, &token_hash, &scopes_clone)
    })
    .await;

    match result {
        Ok(Ok(id)) => {
            let resp = CreateTokenResponse {
                id,
                name: name.to_string(),
                token: raw_token,
                scopes: scopes_str,
            };
            (StatusCode::CREATED, Json(resp)).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to create admin token: {}", e);
            json_error(
                500,
                "Failed to create token",
                "DB_ERROR",
            )
        }
        Err(e) => {
            tracing::error!("Task join error creating admin token: {}", e);
            json_error(
                500,
                "Internal error",
                "INTERNAL_ERROR",
            )
        }
    }
}

/// GET /v1/admin/tokens
///
/// List all admin API tokens. Token hashes are redacted.
/// Requires the "admin" scope.
pub async fn list_tokens(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "admin") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::admin_token::list(&conn)
    })
    .await;

    match result {
        Ok(Ok(tokens)) => Json(tokens).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to list admin tokens: {}", e);
            json_error(
                500,
                "Failed to list tokens",
                "DB_ERROR",
            )
        }
        Err(e) => {
            tracing::error!("Task join error listing admin tokens: {}", e);
            json_error(
                500,
                "Internal error",
                "INTERNAL_ERROR",
            )
        }
    }
}

/// DELETE /v1/admin/tokens/:id
///
/// Delete an admin API token by ID. Returns 204 on success, 404 if not found.
/// Requires the "admin" scope.
pub async fn delete_token(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<i64>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "admin") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::admin_token::delete(&conn, id)
    })
    .await;

    match result {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => json_error(404, "Token not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to delete admin token {}: {}", id, e);
            json_error(
                500,
                "Failed to delete token",
                "DB_ERROR",
            )
        }
        Err(e) => {
            tracing::error!("Task join error deleting admin token: {}", e);
            json_error(
                500,
                "Internal error",
                "INTERNAL_ERROR",
            )
        }
    }
}

// ---------------------------------------------------------------------------
// CI proxy helpers
// ---------------------------------------------------------------------------

/// Proxy a GET request to Forgejo API, returning parsed JSON.
async fn forgejo_get(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
) -> Result<serde_json::Value, Response> {
    let (url, token, client) = {
        let s = state.read().await;
        let base = s.forgejo_url.as_ref().ok_or_else(|| {
            json_error(
                503,
                "Forgejo not configured",
                "UPSTREAM_ERROR",
            )
        })?;
        let token = s.forgejo_token.clone().unwrap_or_default();
        (
            format!("{}/api/v1{path}", base.trim_end_matches('/')),
            token,
            s.http_client.clone(),
        )
    };

    let resp = client
        .get(&url)
        .header("Authorization", format!("token {token}"))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Forgejo proxy error: {e}");
            json_error(
                502,
                "Forgejo unreachable",
                "UPSTREAM_ERROR",
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(json_error(
            502,
            &format!("Forgejo returned {status}"),
            "UPSTREAM_ERROR",
        ));
    }

    resp.json().await.map_err(|e| {
        tracing::error!("Forgejo response parse error: {e}");
        json_error(
            502,
            "Invalid Forgejo response",
            "UPSTREAM_ERROR",
        )
    })
}

/// Proxy a POST request to Forgejo API, returning parsed JSON.
async fn forgejo_post(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, Response> {
    let (url, token, client) = {
        let s = state.read().await;
        let base = s.forgejo_url.as_ref().ok_or_else(|| {
            json_error(
                503,
                "Forgejo not configured",
                "UPSTREAM_ERROR",
            )
        })?;
        let token = s.forgejo_token.clone().unwrap_or_default();
        (
            format!("{}/api/v1{path}", base.trim_end_matches('/')),
            token,
            s.http_client.clone(),
        )
    };

    let resp = client
        .post(&url)
        .header("Authorization", format!("token {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Forgejo proxy error: {e}");
            json_error(
                502,
                "Forgejo unreachable",
                "UPSTREAM_ERROR",
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(json_error(
            502,
            &format!("Forgejo returned {status}"),
            "UPSTREAM_ERROR",
        ));
    }

    // Some Forgejo POST responses return 204 with no body
    let status = resp.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(serde_json::json!({"status": "ok"}));
    }

    resp.json().await.map_err(|e| {
        tracing::error!("Forgejo response parse error: {e}");
        json_error(
            502,
            "Invalid Forgejo response",
            "UPSTREAM_ERROR",
        )
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
    if let Some(err) = check_scope(&scopes, "ci:read") {
        return err;
    }
    match forgejo_get(&state, "/repos/peter/Conary/actions/workflows").await {
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
    if let Some(err) = check_scope(&scopes, "ci:read") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "workflow name") {
        return err;
    }
    match forgejo_get(
        &state,
        &format!("/repos/peter/Conary/actions/workflows/{name}/runs"),
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
    if let Some(err) = check_scope(&scopes, "ci:read") {
        return err;
    }
    match forgejo_get(&state, &format!("/repos/peter/Conary/actions/runs/{id}")).await {
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
    if let Some(err) = check_scope(&scopes, "ci:read") {
        return err;
    }

    let (url, token, client) = {
        let s = state.read().await;
        let base = match s.forgejo_url.as_ref() {
            Some(b) => b.clone(),
            None => {
                return json_error(
                    503,
                    "Forgejo not configured",
                    "UPSTREAM_ERROR",
                );
            }
        };
        let token = s.forgejo_token.clone().unwrap_or_default();
        (
            format!(
                "{}/api/v1/repos/peter/Conary/actions/runs/{id}/logs",
                base.trim_end_matches('/')
            ),
            token,
            s.http_client.clone(),
        )
    };

    let resp = match client
        .get(&url)
        .header("Authorization", format!("token {token}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Forgejo proxy error: {e}");
            return json_error(
                502,
                "Forgejo unreachable",
                "UPSTREAM_ERROR",
            );
        }
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return json_error(
            502,
            &format!("Forgejo returned {status}"),
            "UPSTREAM_ERROR",
        );
    }

    match resp.text().await {
        Ok(text) => (
            StatusCode::OK,
            [("content-type", "text/plain; charset=utf-8")],
            text,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Forgejo log body error: {e}");
            json_error(
                502,
                "Invalid Forgejo response",
                "UPSTREAM_ERROR",
            )
        }
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
    if let Some(err) = check_scope(&scopes, "ci:trigger") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "workflow name") {
        return err;
    }
    match forgejo_post(
        &state,
        &format!("/repos/peter/Conary/actions/workflows/{name}/dispatches"),
        serde_json::json!({"ref": "main"}),
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
    if let Some(err) = check_scope(&scopes, "ci:trigger") {
        return err;
    }
    match forgejo_post(
        &state,
        "/repos/peter/Conary/mirror-sync",
        serde_json::json!({}),
    )
    .await
    {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

// ========== Repository Management ==========

/// Request body for creating or updating a repository.
#[derive(Debug, Deserialize)]
pub struct RepoRequest {
    pub name: Option<String>,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub gpg_check: Option<bool>,
    pub metadata_expire: Option<i32>,
}

/// Response body for repository endpoints.
#[derive(Debug, Serialize)]
pub struct RepoResponse {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub gpg_check: bool,
    pub metadata_expire: i32,
    pub last_sync: Option<String>,
    pub created_at: Option<String>,
}

impl From<conary_core::db::models::Repository> for RepoResponse {
    fn from(r: conary_core::db::models::Repository) -> Self {
        Self {
            id: r.id.unwrap_or(0),
            name: r.name,
            url: r.url,
            content_url: r.content_url,
            enabled: r.enabled,
            priority: r.priority,
            gpg_check: r.gpg_check,
            metadata_expire: r.metadata_expire,
            last_sync: r.last_sync,
            created_at: r.created_at,
        }
    }
}

/// GET /v1/admin/repos
///
/// List all configured repositories. Requires the "repos:read" scope.
pub async fn list_repos(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:read") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::Repository::list_all(&conn)
    })
    .await;

    match result {
        Ok(Ok(repos)) => {
            let response: Vec<RepoResponse> = repos.into_iter().map(RepoResponse::from).collect();
            Json(response).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to list repos: {}", e);
            json_error(500, "Failed to list repositories", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error listing repos: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/repos
///
/// Add a new repository. Requires the "repos:write" scope.
pub async fn create_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<RepoRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }

    let name = match body.name.as_deref() {
        Some(n) => n.trim(),
        None => return json_error(400, "Name is required", "INVALID_INPUT"),
    };
    if name.is_empty() || name.len() > 128 {
        return json_error(400, "Repository name must be 1-128 characters", "INVALID_NAME");
    }
    let name = name.to_string();
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    let url = body.url.trim().to_string();
    if url.is_empty() {
        return json_error(400, "Repository URL must not be empty", "INVALID_URL");
    }
    if url::Url::parse(&url).is_err() {
        return json_error(400, "Invalid URL format", "INVALID_INPUT");
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let content_url = body.content_url;
    let enabled = body.enabled.unwrap_or(true);
    let priority = body.priority.unwrap_or(0);
    let gpg_check = body.gpg_check.unwrap_or(true);
    let metadata_expire = body.metadata_expire.unwrap_or(3600);

    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let mut repo =
            conary_core::db::models::Repository::new(name_clone, url);
        repo.content_url = content_url;
        repo.enabled = enabled;
        repo.priority = priority;
        repo.gpg_check = gpg_check;
        repo.metadata_expire = metadata_expire;
        repo.insert(&conn)?;
        Ok::<_, conary_core::Error>(repo)
    })
    .await;

    match result {
        Ok(Ok(repo)) => {
            let guard = state.read().await;
            guard.publish_event(
                "repo.created",
                serde_json::json!({"name": &name}),
            );
            drop(guard);
            (StatusCode::CREATED, Json(RepoResponse::from(repo))).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to create repo: {}", e);
            json_error(500, "Failed to create repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error creating repo: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/repos/:name
///
/// Get details for a specific repository. Requires the "repos:read" scope.
pub async fn get_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:read") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::Repository::find_by_name(&conn, &name)
    })
    .await;

    match result {
        Ok(Ok(Some(repo))) => Json(RepoResponse::from(repo)).into_response(),
        Ok(Ok(None)) => json_error(404, "Repository not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to get repo: {}", e);
            json_error(500, "Failed to get repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error getting repo: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// PUT /v1/admin/repos/:name
///
/// Update an existing repository configuration. Requires the "repos:write" scope.
pub async fn update_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<RepoRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    if body.url.trim().is_empty() {
        return json_error(400, "URL is required", "INVALID_INPUT");
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let repo =
            conary_core::db::models::Repository::find_by_name(&conn, &name)?;
        let mut repo = match repo {
            Some(r) => r,
            None => return Ok::<_, conary_core::Error>(None),
        };

        repo.url = body.url.trim().to_string();
        repo.content_url = body.content_url;
        if let Some(enabled) = body.enabled {
            repo.enabled = enabled;
        }
        if let Some(priority) = body.priority {
            repo.priority = priority;
        }
        if let Some(gpg_check) = body.gpg_check {
            repo.gpg_check = gpg_check;
        }
        if let Some(metadata_expire) = body.metadata_expire {
            repo.metadata_expire = metadata_expire;
        }
        repo.update(&conn)?;
        Ok(Some(repo))
    })
    .await;

    match result {
        Ok(Ok(Some(repo))) => {
            let guard = state.read().await;
            guard.publish_event(
                "repo.updated",
                serde_json::json!({"name": &repo.name}),
            );
            drop(guard);
            Json(RepoResponse::from(repo)).into_response()
        }
        Ok(Ok(None)) => json_error(404, "Repository not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to update repo: {}", e);
            json_error(500, "Failed to update repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error updating repo: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/repos/:name
///
/// Remove a repository. Requires the "repos:write" scope.
pub async fn delete_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let repo =
            conary_core::db::models::Repository::find_by_name(&conn, &name_clone)?;
        match repo {
            Some(r) => {
                let id = r.id.ok_or_else(|| conary_core::Error::MissingId("Repository has no ID".to_string()))?;
                conary_core::db::models::Repository::delete(&conn, id)?;
                Ok::<_, conary_core::Error>(true)
            }
            None => Ok(false),
        }
    })
    .await;

    match result {
        Ok(Ok(true)) => {
            let guard = state.read().await;
            guard.publish_event(
                "repo.deleted",
                serde_json::json!({"name": &name}),
            );
            drop(guard);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(false)) => json_error(404, "Repository not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to delete repo {}: {}", name, e);
            json_error(500, "Failed to delete repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error deleting repo: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/repos/:name/sync
///
/// Trigger a manual sync for a repository. Currently a stub that verifies
/// the repo exists and publishes a `repo.sync_requested` event.
/// Requires the "repos:write" scope.
pub async fn sync_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::Repository::find_by_name(&conn, &name_clone)
    })
    .await;

    match result {
        Ok(Ok(Some(_))) => {
            let guard = state.read().await;
            guard.publish_event(
                "repo.sync_requested",
                serde_json::json!({"name": &name}),
            );
            drop(guard);
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"status": "sync_requested", "name": name})),
            )
                .into_response()
        }
        Ok(Ok(None)) => json_error(404, "Repository not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to find repo for sync: {}", e);
            json_error(500, "Failed to sync repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error syncing repo: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

// ========== SSE Event Stream ==========

#[derive(Deserialize)]
pub struct EventsQuery {
    pub filter: Option<String>,
}

pub async fn sse_events(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<EventsQuery>,
) -> Response {
    // Any valid token can subscribe
    if scopes.is_none() {
        return json_error(401, "Not authenticated", "UNAUTHORIZED");
    }

    let filters: Option<Vec<String>> = query.filter.map(|f| {
        f.split(',').map(|s| s.trim().to_string()).collect()
    });

    let rx = {
        let s = state.read().await;
        s.admin_events.subscribe()
    };

    let stream = async_stream::stream! {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(ref filters) = filters
                        && !filters.iter().any(|f| f == &event.event_type)
                    {
                        continue;
                    }
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok::<_, std::convert::Infallible>(
                        axum::response::sse::Event::default()
                            .event(&event.event_type)
                            .data(data)
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("SSE client lagged by {} events", n);
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

// ========== Federation Management ==========

/// Response body for federation peer endpoints.
#[derive(Debug, Serialize)]
pub struct PeerResponse {
    pub id: String,
    pub endpoint: String,
    pub node_name: Option<String>,
    pub tier: String,
    pub first_seen: String,
    pub last_seen: String,
    pub latency_ms: f64,
    pub success_count: i64,
    pub failure_count: i64,
    pub consecutive_failures: i64,
    pub is_enabled: bool,
}

/// Request body for adding a federation peer.
#[derive(Debug, Deserialize)]
pub struct AddPeerRequest {
    pub endpoint: String,
    pub tier: Option<String>,
    pub node_name: Option<String>,
}

/// Health status for a peer.
#[derive(Debug, Serialize)]
pub struct PeerHealthStatus {
    pub success_rate: f64,
    pub total_requests: i64,
    pub status: String,
}

/// Detailed health response for a single peer.
#[derive(Debug, Serialize)]
pub struct PeerHealthResponse {
    pub peer: PeerResponse,
    pub health: PeerHealthStatus,
}

/// GET /v1/admin/federation/peers
///
/// List all federation peers with their health status. Requires "federation:read".
pub async fn list_peers(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:read") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let mut stmt = conn.prepare(
            "SELECT id, endpoint, node_name, tier, first_seen, last_seen,
                    latency_ms, success_count, failure_count, consecutive_failures, is_enabled
             FROM federation_peers
             ORDER BY tier, endpoint",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(PeerResponse {
                id: row.get(0)?,
                endpoint: row.get(1)?,
                node_name: row.get(2)?,
                tier: row.get(3)?,
                first_seen: row.get::<_, String>(4)?,
                last_seen: row.get::<_, String>(5)?,
                latency_ms: row.get::<_, i64>(6)? as f64,
                success_count: row.get(7)?,
                failure_count: row.get(8)?,
                consecutive_failures: row.get(9)?,
                is_enabled: row.get::<_, i64>(10)? != 0,
            })
        })?;

        Ok::<_, conary_core::Error>(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .await;

    match result {
        Ok(Ok(peers)) => Json(peers).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to list federation peers: {}", e);
            json_error(500, "Failed to list peers", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error listing peers: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/federation/peers
///
/// Add a new federation peer. Requires "federation:write".
pub async fn add_peer(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<AddPeerRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:write") {
        return err;
    }

    let endpoint = body.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return json_error(400, "Endpoint must not be empty", "INVALID_ENDPOINT");
    }

    // Validate URL
    if url::Url::parse(&endpoint).is_err() {
        return json_error(400, "Invalid endpoint URL", "INVALID_URL");
    }

    let tier = body.tier.unwrap_or_else(|| "leaf".to_string());
    if !["leaf", "cell_hub", "region_hub"].contains(&tier.as_str()) {
        return json_error(
            400,
            "Tier must be one of: leaf, cell_hub, region_hub",
            "INVALID_TIER",
        );
    }

    let node_name = body.node_name;
    let peer_id = conary_core::hash::sha256(endpoint.as_bytes());
    let now = chrono::Utc::now().to_rfc3339();

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let peer_id_clone = peer_id.clone();
    let endpoint_clone = endpoint.clone();
    let tier_clone = tier.clone();
    let node_name_clone = node_name.clone();
    let now_clone = now.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conn.execute(
            "INSERT INTO federation_peers (id, endpoint, node_name, tier, first_seen, last_seen,
             latency_ms, success_count, failure_count, consecutive_failures, is_enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, 0, 0, 1)",
            rusqlite::params![peer_id_clone, endpoint_clone, node_name_clone, tier_clone, now_clone, now_clone],
        )?;
        Ok::<_, conary_core::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let guard = state.read().await;
            guard.publish_event(
                "federation.peer_added",
                serde_json::json!({"id": &peer_id, "endpoint": &endpoint}),
            );
            drop(guard);

            let response = PeerResponse {
                id: peer_id,
                endpoint,
                node_name,
                tier,
                first_seen: now.clone(),
                last_seen: now,
                latency_ms: 0.0,
                success_count: 0,
                failure_count: 0,
                consecutive_failures: 0,
                is_enabled: true,
            };
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("UNIQUE constraint") {
                json_error(409, "Peer with this endpoint already exists", "DUPLICATE_PEER")
            } else {
                tracing::error!("Failed to add peer: {}", e);
                json_error(500, "Failed to add peer", "DB_ERROR")
            }
        }
        Err(e) => {
            tracing::error!("Task join error adding peer: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/federation/peers/:id
///
/// Remove a federation peer. Requires "federation:write".
pub async fn delete_peer(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:write") {
        return err;
    }
    if let Some(err) = validate_path_param(&id, "peer id") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let id_clone = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let affected = conn.execute(
            "DELETE FROM federation_peers WHERE id = ?1",
            rusqlite::params![id_clone],
        )?;
        Ok::<_, conary_core::Error>(affected)
    })
    .await;

    match result {
        Ok(Ok(0)) => json_error(404, "Peer not found", "NOT_FOUND"),
        Ok(Ok(_)) => {
            let guard = state.read().await;
            guard.publish_event(
                "federation.peer_removed",
                serde_json::json!({"id": &id}),
            );
            drop(guard);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to delete peer {}: {}", id, e);
            json_error(500, "Failed to delete peer", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error deleting peer: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/federation/peers/:id/health
///
/// Get detailed health for a specific peer. Requires "federation:read".
pub async fn peer_health(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:read") {
        return err;
    }
    if let Some(err) = validate_path_param(&id, "peer id") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let peer = conn
            .query_row(
                "SELECT id, endpoint, node_name, tier, first_seen, last_seen,
                        latency_ms, success_count, failure_count, consecutive_failures, is_enabled
                 FROM federation_peers WHERE id = ?1",
                rusqlite::params![id],
                |row| {
                    Ok(PeerResponse {
                        id: row.get(0)?,
                        endpoint: row.get(1)?,
                        node_name: row.get(2)?,
                        tier: row.get(3)?,
                        first_seen: row.get::<_, String>(4)?,
                        last_seen: row.get::<_, String>(5)?,
                        latency_ms: row.get::<_, i64>(6)? as f64,
                        success_count: row.get(7)?,
                        failure_count: row.get(8)?,
                        consecutive_failures: row.get(9)?,
                        is_enabled: row.get::<_, i64>(10)? != 0,
                    })
                },
            )
            .optional()?;
        Ok::<_, conary_core::Error>(peer)
    })
    .await;

    match result {
        Ok(Ok(Some(peer))) => {
            let total = peer.success_count + peer.failure_count;
            let success_rate = if total > 0 {
                peer.success_count as f64 / total as f64
            } else {
                1.0
            };
            let status = if peer.consecutive_failures > 5 {
                "unhealthy"
            } else if peer.consecutive_failures > 0 {
                "degraded"
            } else {
                "healthy"
            };

            let health = PeerHealthStatus {
                success_rate,
                total_requests: total,
                status: status.to_string(),
            };
            Json(PeerHealthResponse { peer, health }).into_response()
        }
        Ok(Ok(None)) => json_error(404, "Peer not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to get peer health: {}", e);
            json_error(500, "Failed to get peer health", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error getting peer health: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/federation/config
///
/// Get the current federation configuration. Requires "federation:read".
/// Returns defaults if no config has been stored yet.
pub async fn get_federation_config(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:read") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let json_str: Option<String> = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'federation_config'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None); // Table may not exist -- treat as missing

        let config: crate::federation::FederationConfig = match json_str {
            Some(s) => serde_json::from_str(&s).unwrap_or_default(),
            None => crate::federation::FederationConfig::default(),
        };
        Ok::<_, conary_core::Error>(config)
    })
    .await;

    match result {
        Ok(Ok(config)) => Json(config).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to get federation config: {}", e);
            json_error(500, "Failed to get federation config", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error getting federation config: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// PUT /v1/admin/federation/config
///
/// Update the federation configuration. Requires "federation:write".
/// The request body must be a valid `FederationConfig` JSON object.
pub async fn update_federation_config(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:write") {
        return err;
    }

    // Validate by deserializing into FederationConfig
    let config: crate::federation::FederationConfig = match serde_json::from_value(body.clone()) {
        Ok(c) => c,
        Err(e) => {
            return json_error(
                400,
                &format!("Invalid federation config: {e}"),
                "INVALID_CONFIG",
            );
        }
    };

    let json_str = serde_json::to_string(&config).unwrap_or_default();

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('federation_config', ?1)",
            rusqlite::params![json_str],
        )?;
        Ok::<_, conary_core::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let guard = state.read().await;
            guard.publish_event(
                "federation.config_updated",
                serde_json::json!({"enabled": config.enabled}),
            );
            drop(guard);
            Json(config).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to update federation config: {}", e);
            json_error(500, "Failed to update federation config", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error updating federation config: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}
// ========== Audit Log ==========

/// Query parameters for the audit log endpoint.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub action: Option<String>,
    pub since: Option<String>,
    pub token_name: Option<String>,
}

/// Query parameters for purging audit entries.
#[derive(Debug, Deserialize)]
pub struct PurgeQuery {
    pub before: String,
}

/// GET /v1/admin/audit
pub async fn query_audit(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<AuditQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "admin") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::audit_log::query(
            &conn,
            query.limit,
            query.action.as_deref(),
            query.since.as_deref(),
            query.token_name.as_deref(),
        )
    })
    .await;
    match result {
        Ok(Ok(entries)) => Json(entries).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to query audit log: {e}");
            json_error(500, "Failed to query audit log", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error querying audit: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/audit
pub async fn purge_audit(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<PurgeQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "admin") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let before = query.before.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::audit_log::purge(&conn, &before)
    })
    .await;
    match result {
        Ok(Ok(deleted)) => {
            Json(serde_json::json!({"deleted": deleted, "before": query.before})).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to purge audit log: {e}");
            json_error(500, "Failed to purge audit log", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error purging audit: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    /// Build an axum app backed by a temporary database with one pre-seeded
    /// admin token (`test-admin-token-12345`, scopes = `admin`).
    ///
    /// Returns the router and the database path so callers can inspect DB
    /// state if needed.  The `tempfile::TempDir` is leaked intentionally --
    /// tests are short-lived and the OS reclaims the directory on process
    /// exit.
    async fn test_app() -> (axum::Router, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        // Initialize DB with full schema
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .unwrap();
            conary_core::db::schema::migrate(&conn).unwrap();
        }

        let mut config = crate::server::ServerConfig::default();
        config.db_path = db_path.clone();
        config.chunk_dir = tmp.path().join("chunks");
        config.cache_dir = tmp.path().join("cache");
        std::fs::create_dir_all(&config.chunk_dir).unwrap();
        std::fs::create_dir_all(&config.cache_dir).unwrap();

        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));

        // Build the external admin router (includes auth middleware)
        let app = crate::server::routes::create_external_admin_router(state);

        // Seed a bootstrap token for tests
        let test_token = "test-admin-token-12345";
        let hash = crate::server::auth::hash_token(test_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin")
                .unwrap();
        }

        // Leak the TempDir so it outlives the test (cleaned up at process exit)
        std::mem::forget(tmp);

        (app, db_path)
    }

    // ------------------------------------------------------------------
    // Integration tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_unauthenticated_request_rejected() {
        let (app, _db) = test_app().await;
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/tokens")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_invalid_token_rejected() {
        let (app, _db) = test_app().await;
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/tokens")
                    .header("Authorization", "Bearer wrong-token")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_valid_token_list_tokens() {
        let (app, _db) = test_app().await;
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/tokens")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_and_delete_token() {
        let (app, db_path) = test_app().await;

        // POST /v1/admin/tokens to create a new token
        let create_body = serde_json::json!({"name": "new-token", "scopes": "ci:read"});
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/tokens")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Parse the response body
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["name"], "new-token");
        assert_eq!(body["scopes"], "ci:read");
        let token_id = body["id"].as_i64().expect("id should be an integer");

        // Build a fresh router (oneshot consumes the app)
        let mut config2 = crate::server::ServerConfig::default();
        config2.db_path = db_path.clone();
        config2.chunk_dir = db_path.parent().unwrap().join("chunks");
        config2.cache_dir = db_path.parent().unwrap().join("cache");
        let state2 = Arc::new(RwLock::new(crate::server::ServerState::new(config2)));
        let app2 = crate::server::routes::create_external_admin_router(state2);

        // DELETE /v1/admin/tokens/:id
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(&format!("/v1/admin/tokens/{token_id}"))
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[test]
    fn test_json_error_format() {
        let resp = json_error(400, "bad input", "INVALID");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_create_token_request_deserialize() {
        let json = r#"{"name": "ci-key", "scopes": "ci:read,ci:trigger"}"#;
        let req: CreateTokenRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "ci-key");
        assert_eq!(req.scopes.unwrap(), "ci:read,ci:trigger");
    }

    #[test]
    fn test_create_token_request_default_scopes() {
        let json = r#"{"name": "admin-key"}"#;
        let req: CreateTokenRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "admin-key");
        assert!(req.scopes.is_none());
    }

    #[test]
    fn test_check_scope_admin_granted() {
        let scopes = Some(axum::Extension(TokenScopes("admin".to_string())));
        assert!(check_scope(&scopes, "admin").is_none());
    }

    #[test]
    fn test_check_scope_insufficient() {
        let scopes = Some(axum::Extension(TokenScopes("ci:read".to_string())));
        let resp = check_scope(&scopes, "admin").unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_check_scope_missing() {
        let resp = check_scope(&None, "admin").unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Helper to rebuild a fresh router against the same DB (oneshot consumes the app).
    fn rebuild_app(db_path: &std::path::Path) -> axum::Router {
        let mut config = crate::server::ServerConfig::default();
        config.db_path = db_path.to_path_buf();
        config.chunk_dir = db_path.parent().unwrap().join("chunks");
        config.cache_dir = db_path.parent().unwrap().join("cache");
        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));
        crate::server::routes::create_external_admin_router(state)
    }

    #[tokio::test]
    async fn test_repo_crud_lifecycle() {
        let (app, db_path) = test_app().await;

        // Create a repo
        let create_body = serde_json::json!({
            "name": "fedora",
            "url": "https://mirrors.example.com/fedora",
            "enabled": true,
            "priority": 10
        });
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["name"], "fedora");
        assert_eq!(body["priority"], 10);

        // List repos and verify it appears
        let app2 = rebuild_app(&db_path);
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let repos = body.as_array().expect("should be an array");
        assert!(repos.iter().any(|r| r["name"] == "fedora"));

        // Get single repo
        let app3 = rebuild_app(&db_path);
        let resp = app3
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Update repo
        let app4 = rebuild_app(&db_path);
        let update_body = serde_json::json!({
            "name": "fedora",
            "url": "https://mirrors2.example.com/fedora",
            "priority": 20
        });
        let resp = app4
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["priority"], 20);

        // Delete repo
        let app5 = rebuild_app(&db_path);
        let resp = app5
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify it is gone
        let app6 = rebuild_app(&db_path);
        let resp = app6
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_repo_scope_enforcement() {
        let (app, db_path) = test_app().await;

        // Create a token with only ci:read scope
        let ci_token = "ci-read-only-token-67890";
        let hash = crate::server::auth::hash_token(ci_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "ci-reader", &hash, "ci:read")
                .unwrap();
        }

        // GET /v1/admin/repos with ci:read scope should be 403
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos")
                    .header("Authorization", format!("Bearer {ci_token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_federation_peer_lifecycle() {
        let (app, db_path) = test_app().await;
        let token = "test-admin-token-12345";

        // Add a peer
        let add_body = serde_json::json!({
            "endpoint": "https://peer1.example.com:7891",
            "tier": "leaf",
            "node_name": "peer1"
        });
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/federation/peers")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(add_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["endpoint"], "https://peer1.example.com:7891");
        assert_eq!(body["tier"], "leaf");
        assert_eq!(body["is_enabled"], true);
        let peer_id = body["id"].as_str().expect("id should be a string").to_string();

        // List peers and verify it appears
        let app2 = rebuild_app(&db_path);
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/federation/peers")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let peers = body.as_array().expect("should be an array");
        assert!(peers.iter().any(|p| p["id"] == peer_id));

        // Delete the peer
        let app3 = rebuild_app(&db_path);
        let resp = app3
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(&format!("/v1/admin/federation/peers/{peer_id}"))
                    .header("Authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify it is gone (health check returns 404)
        let app4 = rebuild_app(&db_path);
        let resp = app4
            .oneshot(
                axum::http::Request::builder()
                    .uri(&format!("/v1/admin/federation/peers/{peer_id}/health"))
                    .header("Authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
