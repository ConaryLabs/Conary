// conary-server/src/server/handlers/admin/tokens.rs
//! Token management handlers

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::admin_service::{self, ServiceError};
use crate::server::auth::{Scope, TokenScopes, json_error};
use crate::server::ServerState;

use super::check_scope;

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

/// POST /v1/admin/tokens
///
/// Create a new admin API token. Requires the "admin" scope.
/// Returns the plaintext token exactly once -- it cannot be retrieved again.
pub async fn create_token(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<CreateTokenRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::create_token(&state, &body.name, body.scopes.as_deref()).await {
        Ok(created) => {
            let resp = CreateTokenResponse {
                id: created.id,
                name: created.name,
                token: created.raw_token,
                scopes: created.scopes,
            };
            (StatusCode::CREATED, Json(resp)).into_response()
        }
        Err(ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(e) => {
            tracing::error!("Failed to create admin token: {e}");
            json_error(500, "Failed to create token", "INTERNAL_ERROR")
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
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::list_tokens(&state).await {
        Ok(tokens) => Json(tokens).into_response(),
        Err(e) => {
            tracing::error!("Failed to list admin tokens: {e}");
            json_error(500, "Failed to list tokens", "INTERNAL_ERROR")
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
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::delete_token(&state, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => json_error(404, "Token not found", "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to delete admin token {id}: {e}");
            json_error(500, "Failed to delete token", "INTERNAL_ERROR")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
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
        let app = crate::server::routes::create_external_admin_router(state, None);

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
        let app2 = crate::server::routes::create_external_admin_router(state2, None);

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
        assert!(check_scope(&scopes, Scope::Admin).is_none());
    }

    #[test]
    fn test_check_scope_insufficient() {
        let scopes = Some(axum::Extension(TokenScopes("ci:read".to_string())));
        let resp = check_scope(&scopes, Scope::Admin).unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_check_scope_missing() {
        let resp = check_scope(&None, Scope::Admin).unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
