// conary-server/src/server/handlers/admin.rs
//! Handlers for the external admin API

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

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
}
