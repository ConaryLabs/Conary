// conary-server/src/server/auth.rs
//! Token authentication for the Remi Admin API
//!
//! Provides bearer token authentication with SHA-256 hashing,
//! scope-based authorization, and axum middleware integration.

use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::Rng;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::rate_limit::AdminRateLimiters;

/// Minimum interval between `touch()` DB writes for the same token (5 minutes).
const TOUCH_DEBOUNCE_SECS: u64 = 300;

/// Maximum number of entries in TOUCH_CACHE before we evict stale entries.
/// With a 5-minute debounce, 10K entries covers well beyond any realistic
/// concurrent token count while bounding memory usage.
const TOUCH_CACHE_MAX_ENTRIES: usize = 10_000;

/// In-memory cache of the last time each token ID was touched.
///
/// Shared across requests via a `lazy_static`-style global. Using a `Mutex`
/// is fine here because the critical section is just a HashMap lookup/insert
/// (sub-microsecond).
///
/// Bounded to [`TOUCH_CACHE_MAX_ENTRIES`]: when the limit is reached, entries
/// older than [`TOUCH_DEBOUNCE_SECS`] are evicted. If the cache is still full
/// after eviction, the oldest entry is removed to make room.
static TOUCH_CACHE: std::sync::LazyLock<Mutex<HashMap<i64, Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

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
    pub fn has_scope(&self, required: Scope) -> bool {
        let required_str = required.as_str();
        self.0.split(',').any(|s| {
            let t = s.trim();
            t == "admin" || t == required_str
        })
    }
}

/// Typed representation of a valid token scope.
///
/// Scopes are stored as strings in SQLite but validated at API boundaries
/// using this enum. Use [`Scope::as_str`] for DB/API serialization and
/// [`Scope::parse`] for deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Admin,
    CiRead,
    CiTrigger,
    ReposRead,
    ReposWrite,
    FederationRead,
    FederationWrite,
}

impl Scope {
    /// All valid scope variants.
    pub const ALL: &[Scope] = &[
        Scope::Admin,
        Scope::CiRead,
        Scope::CiTrigger,
        Scope::ReposRead,
        Scope::ReposWrite,
        Scope::FederationRead,
        Scope::FederationWrite,
    ];

    /// Return the wire-format string for this scope.
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Admin => "admin",
            Scope::CiRead => "ci:read",
            Scope::CiTrigger => "ci:trigger",
            Scope::ReposRead => "repos:read",
            Scope::ReposWrite => "repos:write",
            Scope::FederationRead => "federation:read",
            Scope::FederationWrite => "federation:write",
        }
    }

    /// Parse a wire-format string into a scope, returning `None` for unknown values.
    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "admin" => Some(Scope::Admin),
            "ci:read" => Some(Scope::CiRead),
            "ci:trigger" => Some(Scope::CiTrigger),
            "repos:read" => Some(Scope::ReposRead),
            "repos:write" => Some(Scope::ReposWrite),
            "federation:read" => Some(Scope::FederationRead),
            "federation:write" => Some(Scope::FederationWrite),
            _ => None,
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validate that all scopes in a comma-separated string are valid.
///
/// Returns `Err` with the first invalid scope found. Empty strings — either
/// a bare `""` or empty segments produced by leading/trailing/doubled commas
/// like `",admin"` or `"admin,"` — are explicitly rejected so that callers
/// can never store a token with a meaningless blank scope.
pub fn validate_scopes(scopes: &str) -> Result<(), String> {
    if scopes.is_empty() {
        return Err(String::new());
    }
    for scope in scopes.split(',') {
        let trimmed = scope.trim();
        if trimmed.is_empty() {
            return Err(String::new());
        }
        if Scope::parse(trimmed).is_none() {
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
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

/// Extract a bearer token from the Authorization header.
///
/// Expects the format "Bearer <token>". Returns `None` if the header
/// is missing, malformed, or uses a different auth scheme.
pub fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.is_empty() { None } else { Some(token) }
}

/// Axum middleware that authenticates requests via bearer token.
///
/// On success, stores [`TokenScopes`] in request extensions and updates
/// `last_used_at` in the background. On failure, returns a 401 JSON error.
pub async fn auth_middleware(
    State(state): State<Arc<RwLock<ServerState>>>,
    limiters: Option<Extension<Arc<AdminRateLimiters>>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // Rate limiters come from an Extension layer (set once at startup),
    // so we only need the RwLock for db_path and trusted_proxy_header.
    let limiters = limiters.map(|Extension(l)| l);
    let (db_path, trusted_proxy_header) = {
        let s = state.read().await;
        (s.config.db_path.clone(), s.trusted_proxy_header.clone())
    };
    // Use the proxy-aware IP extraction so that rate limiting and auth
    // failure tracking use the real client IP, not the proxy's IP.
    let client_ip =
        crate::server::rate_limit::extract_ip_with_proxy(&request, trusted_proxy_header.as_deref());

    let token = match extract_bearer(request.headers()) {
        Some(t) => t.to_owned(),
        None => {
            tracing::warn!("Auth failed: missing or invalid Authorization header");
            // Check rate limit BEFORE consuming a token to prevent N+1 attempts
            if let Some(ref l) = limiters
                && crate::server::rate_limit::check_auth_failure(l, client_ip)
            {
                return json_error(429, "Too many authentication failures", "RATE_LIMITED");
            }
            return json_error(
                401,
                "Missing or invalid Authorization header",
                "UNAUTHORIZED",
            );
        }
    };

    let token_hash = hash_token(&token);

    let hash_for_lookup = token_hash.clone();
    let db_path_for_lookup = db_path.clone();
    let lookup_result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path_for_lookup)?;
        conary_core::db::models::admin_token::find_by_hash(&conn, &hash_for_lookup)
    })
    .await;

    let token_record = match lookup_result {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => {
            tracing::warn!(
                "Auth failed: unknown token (hash prefix: {}...)",
                &token_hash[..8]
            );
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
    request
        .extensions_mut()
        .insert(TokenName(token_record.name.clone()));

    // Update last_used_at in background, debounced to avoid excessive DB writes.
    // Only touch if the last touch was more than TOUCH_DEBOUNCE_SECS ago.
    let bg_db_path = db_path;
    let bg_id = token_record.id;
    let should_touch = {
        let mut cache = TOUCH_CACHE.lock().expect("TOUCH_CACHE poisoned");
        let now = Instant::now();
        let debounce = std::time::Duration::from_secs(TOUCH_DEBOUNCE_SECS);
        match cache.get(&bg_id) {
            Some(last) if now.duration_since(*last) < debounce => false,
            _ => {
                // Evict stale entries when the cache is at capacity
                if cache.len() >= TOUCH_CACHE_MAX_ENTRIES {
                    cache.retain(|_, last| now.duration_since(*last) < debounce);
                }
                // If still full after eviction, drop the oldest entry
                if cache.len() >= TOUCH_CACHE_MAX_ENTRIES
                    && let Some(&oldest_id) = cache
                        .iter()
                        .min_by_key(|(_, instant)| *instant)
                        .map(|(id, _)| id)
                {
                    cache.remove(&oldest_id);
                }
                cache.insert(bg_id, now);
                true
            }
        }
    };

    if should_touch {
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = conary_core::db::open_fast(&bg_db_path)
                && let Err(e) = conary_core::db::models::admin_token::touch(&conn, bg_id)
            {
                tracing::warn!("Failed to update token last_used_at: {}", e);
            }
        });
    }

    next.run(request).await
}

/// Build a JSON error response with the given status code, message, and error code.
pub(crate) fn json_error(status: u16, message: &str, code: &str) -> Response {
    let body = serde_json::json!({
        "error": message,
        "code": code,
    });

    (
        axum::http::StatusCode::from_u16(status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
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
        assert!(scopes.has_scope(Scope::ReposWrite));
        assert!(scopes.has_scope(Scope::CiRead));
        assert!(scopes.has_scope(Scope::FederationRead));
    }

    #[test]
    fn test_token_scopes_specific() {
        let scopes = TokenScopes("ci:read,ci:trigger".to_string());
        assert!(scopes.has_scope(Scope::CiRead));
        assert!(scopes.has_scope(Scope::CiTrigger));
        assert!(!scopes.has_scope(Scope::ReposWrite));
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

    #[test]
    fn test_validate_scopes_empty_string_rejected() {
        assert!(validate_scopes("").is_err());
    }

    #[test]
    fn test_validate_scopes_empty_segment_rejected() {
        // Trailing comma produces an empty segment
        assert!(validate_scopes("admin,").is_err());
        // Leading comma
        assert!(validate_scopes(",admin").is_err());
        // Doubled comma
        assert!(validate_scopes("admin,,ci:read").is_err());
    }
}
