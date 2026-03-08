// conary-server/src/server/auth.rs
//! Token authentication for the Remi Admin API
//!
//! Provides bearer token authentication with SHA-256 hashing,
//! scope-based authorization, and axum middleware integration.

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::Rng;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;

/// Wrapper for token scopes stored in request extensions.
///
/// Scopes are comma-separated strings. The special "admin" scope grants
/// access to everything.
#[derive(Clone, Debug)]
pub struct TokenScopes(pub String);

/// The name of the authenticated token, stored in request extensions.
#[derive(Clone, Debug)]
pub struct TokenName(pub String);

impl TokenScopes {
    /// Check if this token has the required scope.
    ///
    /// The "admin" scope grants access to everything. Otherwise, the
    /// required scope must appear as an exact match in the comma-separated list.
    pub fn has_scope(&self, required: &str) -> bool {
        self.0.split(',').any(|s| {
            let t = s.trim();
            t == "admin" || t == required
        })
    }
}

/// Valid token scopes for the admin API.
pub const VALID_SCOPES: &[&str] = &[
    "admin",
    "ci:read",
    "ci:trigger",
    "repos:read",
    "repos:write",
    "federation:read",
    "federation:write",
];

/// Validate that all scopes in a comma-separated string are valid.
/// Returns Err with the first invalid scope found.
pub fn validate_scopes(scopes: &str) -> Result<(), String> {
    for scope in scopes.split(',') {
        let trimmed = scope.trim();
        if !VALID_SCOPES.contains(&trimmed) {
            return Err(trimmed.to_string());
        }
    }
    Ok(())
}

/// Hash a raw token using SHA-256, returning a 64-character hex string.
pub fn hash_token(raw: &str) -> String {
    conary_core::hash::sha256(raw.as_bytes())
}

/// Generate a cryptographically random token (32 bytes, 64 hex chars).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

/// Extract a bearer token from the Authorization header.
///
/// Expects the format "Bearer <token>". Returns `None` if the header
/// is missing, malformed, or uses a different auth scheme.
pub fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Axum middleware that authenticates requests via bearer token.
///
/// On success, stores [`TokenScopes`] in request extensions and updates
/// `last_used_at` in the background. On failure, returns a 401 JSON error.
pub async fn auth_middleware(
    State(state): State<Arc<RwLock<ServerState>>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // Extract rate limiters, client IP, and db_path in a single lock acquisition
    let (limiters, client_ip, db_path) = {
        let s = state.read().await;
        let limiters = s.rate_limiters.clone();
        let ip = crate::server::rate_limit::extract_ip(&request);
        let db_path = s.config.db_path.clone();
        (limiters, ip, db_path)
    };

    let token = match extract_bearer(request.headers()) {
        Some(t) => t.to_owned(),
        None => {
            tracing::warn!("Auth failed: missing or invalid Authorization header");
            if let Some(ref l) = limiters
                && crate::server::rate_limit::check_auth_failure(l, client_ip)
            {
                return json_error(429, "Too many authentication failures", "RATE_LIMITED");
            }
            return json_error(401, "Missing or invalid Authorization header", "UNAUTHORIZED");
        }
    };

    let token_hash = hash_token(&token);

    let hash_for_lookup = token_hash.clone();
    let db_path_for_lookup = db_path.clone();
    let lookup_result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path_for_lookup)?;
        conary_core::db::models::admin_token::find_by_hash(&conn, &hash_for_lookup)
    })
    .await;

    let token_record = match lookup_result {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => {
            tracing::warn!("Auth failed: unknown token (hash prefix: {}...)", &token_hash[..8]);
            if let Some(ref l) = limiters
                && crate::server::rate_limit::check_auth_failure(l, client_ip)
            {
                return json_error(429, "Too many authentication failures", "RATE_LIMITED");
            }
            return json_error(401, "Invalid token", "INVALID_TOKEN");
        }
        Ok(Err(e)) => {
            tracing::warn!("Auth failed: database error: {}", e);
            return json_error(401, "Authentication failed", "AUTH_ERROR");
        }
        Err(e) => {
            tracing::warn!("Auth failed: task join error: {}", e);
            return json_error(401, "Authentication failed", "AUTH_ERROR");
        }
    };

    // Store scopes in request extensions
    let scopes = TokenScopes(token_record.scopes.clone());
    request.extensions_mut().insert(scopes);
    request.extensions_mut().insert(TokenName(token_record.name.clone()));

    // Update last_used_at in background (fire-and-forget, reusing db_path)
    let bg_db_path = db_path;
    let bg_id = token_record.id;
    tokio::task::spawn_blocking(move || {
        if let Ok(conn) = conary_core::db::open(&bg_db_path)
            && let Err(e) = conary_core::db::models::admin_token::touch(&conn, bg_id)
        {
            tracing::warn!("Failed to update token last_used_at: {}", e);
        }
    });

    next.run(request).await
}

/// Build a JSON error response with the given status code, message, and error code.
pub(crate) fn json_error(status: u16, message: &str, code: &str) -> Response {
    let body = serde_json::json!({
        "error": message,
        "code": code,
    });

    (
        axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_token_deterministic() {
        let hash1 = hash_token("my-secret-token");
        let hash2 = hash_token("my-secret-token");
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn test_hash_token_different_inputs() {
        let hash1 = hash_token("token-a");
        let hash2 = hash_token("token-b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_generate_token_length() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
    }

    #[test]
    fn test_generate_token_unique() {
        let token1 = generate_token();
        let token2 = generate_token();
        assert_ne!(token1, token2);
    }

    #[test]
    fn test_token_scopes_admin_grants_all() {
        let scopes = TokenScopes("admin".to_string());
        assert!(scopes.has_scope("repos:write"));
        assert!(scopes.has_scope("ci:read"));
        assert!(scopes.has_scope("anything"));
    }

    #[test]
    fn test_token_scopes_specific() {
        let scopes = TokenScopes("ci:read,ci:trigger".to_string());
        assert!(scopes.has_scope("ci:read"));
        assert!(scopes.has_scope("ci:trigger"));
        assert!(!scopes.has_scope("repos:write"));
    }

    #[test]
    fn test_extract_bearer_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer mytoken123".parse().unwrap());
        assert_eq!(extract_bearer(&headers), Some("mytoken123"));
    }

    #[test]
    fn test_extract_bearer_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_bearer(&headers), None);
    }

    #[test]
    fn test_extract_bearer_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Basic abc123".parse().unwrap());
        assert_eq!(extract_bearer(&headers), None);
    }

    #[test]
    fn test_validate_scopes_valid() {
        assert!(validate_scopes("admin").is_ok());
        assert!(validate_scopes("ci:read,ci:trigger").is_ok());
        assert!(validate_scopes("repos:read, repos:write").is_ok());
    }

    #[test]
    fn test_validate_scopes_invalid() {
        let err = validate_scopes("admin,bogus").unwrap_err();
        assert_eq!(err, "bogus");
    }
}
