// apps/remi/src/server/handlers/admin/tokens.rs
//! Token management handlers

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service::{self, ServiceError};
use crate::server::auth::{Scope, TokenScopes, json_error};

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

    use super::super::test_helpers::test_app;

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
        let config2 = crate::server::ServerConfig {
            db_path: db_path.clone(),
            chunk_dir: db_path.parent().unwrap().join("chunks"),
            cache_dir: db_path.parent().unwrap().join("cache"),
            ..Default::default()
        };
        let state2 = Arc::new(RwLock::new(
            crate::server::ServerState::new(config2).expect("test server state"),
        ));
        let app2 = crate::server::routes::create_external_admin_router(state2, None);

        // DELETE /v1/admin/tokens/:id
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/admin/tokens/{token_id}"))
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
