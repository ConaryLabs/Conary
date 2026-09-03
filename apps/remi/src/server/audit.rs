// apps/remi/src/server/audit.rs
//! Audit logging middleware for the external admin API.
//!
//! Captures all admin API requests with timing, token identity, and
//! (for write operations) request/response bodies.

use axum::body::Body;
use axum::extract::{MatchedPath, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::auth::TokenName;

/// Derive a stable action from the matched axum route and HTTP method.
pub fn derive_action(method: &str, matched_path: &str) -> String {
    let resource = matched_path
        .strip_prefix("/v1/admin/")
        .unwrap_or(matched_path)
        .split('/')
        .filter(|segment| !segment.is_empty() && !segment.starts_with('{'))
        .map(|segment| segment.replace('-', "_"))
        .collect::<Vec<_>>()
        .join(".");
    let verb = match method {
        "GET" => "read",
        "POST" => "create",
        "PUT" => "update",
        "DELETE" => "delete",
        _ => "unknown",
    };

    format!("{resource}.{verb}")
}

/// Audit logging middleware.
///
/// Captures request details, passes to the handler, then logs the result
/// asynchronously. For write operations (POST/PUT/DELETE), also captures
/// request and response bodies.
pub async fn audit_middleware(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let start = Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let matched_path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| path.clone());
    let is_write = matches!(method.as_str(), "POST" | "PUT" | "DELETE");

    // Extract token name from extensions (set by auth middleware)
    let token_name = request
        .extensions()
        .get::<TokenName>()
        .map(|tn| tn.0.clone());

    // Extract db_path and trusted proxy header before running the handler
    // so we don't need to acquire the RwLock after the response is built.
    let (db_path, trusted_proxy_header) = {
        let s = state.read().await;
        (s.config.db_path.clone(), s.trusted_proxy_header.clone())
    };

    // Extract client IP using the proxy-aware shared helper so that
    // audit logs record the real client IP, not the proxy's IP.
    let source_ip = Some(
        crate::server::rate_limit::extract_ip_with_proxy(&request, trusted_proxy_header.as_deref())
            .to_string(),
    );

    // Maximum number of bytes to log from request/response bodies.
    // Larger payloads (e.g. package uploads) are truncated to avoid
    // excessive DB storage and memory usage in audit logs.
    const AUDIT_BODY_MAX: usize = 4096;

    // For write operations, capture the request body for audit logging.
    //
    // Only buffer the body if Content-Length indicates it fits in AUDIT_BODY_MAX.
    // Large uploads (package/artifact uploads up to 512 MB) pass through
    // without buffering -- we log the size but not the content.
    let (request, request_body) = if is_write {
        let content_len = request
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());

        // Only buffer when we know the body is small. If Content-Length is
        // absent (chunked uploads), skip buffering to avoid consuming and
        // losing a large streamed body.
        let should_buffer = content_len.is_some_and(|len| len <= AUDIT_BODY_MAX);

        if should_buffer {
            let (parts, body) = request.into_parts();
            // Safe: body is at most AUDIT_BODY_MAX bytes (or unknown/small)
            match axum::body::to_bytes(body, AUDIT_BODY_MAX).await {
                Ok(bytes) => {
                    let logged = String::from_utf8_lossy(&bytes).into_owned();
                    let new_body = Body::from(bytes);
                    (Request::from_parts(parts, new_body), Some(logged))
                }
                Err(_) => {
                    // Content-Length was absent but body exceeded AUDIT_BODY_MAX.
                    // Body is consumed; reconstruct as empty. This only affects
                    // chunked-encoded small writes with no Content-Length, which
                    // is rare for admin API calls.
                    let new_body = Body::empty();
                    (
                        Request::from_parts(parts, new_body),
                        Some("[body exceeded audit limit, stream consumed]".to_string()),
                    )
                }
            }
        } else {
            // Large or unknown-size upload -- don't buffer, just log what we know
            let logged = match content_len {
                Some(len) => format!("[body too large for audit: {len} bytes]"),
                None => "[chunked body, size unknown -- not buffered for audit]".to_string(),
            };
            (request, Some(logged))
        }
    } else {
        (request, None)
    };

    // Run the actual handler
    let response = next.run(request).await;
    // SAFETY: as_millis() returns u128 but i64 can hold ~292 million years of
    // milliseconds, so this cast is lossless for any real request duration.
    let duration_ms = start.elapsed().as_millis() as i64;
    let status_code = response.status().as_u16() as i32;

    // For write operations, capture the response body for audit logging.
    // Response bodies from admin handlers are JSON and typically small.
    // Use a generous limit (1 MB) to avoid losing the response on overflow.
    const RESPONSE_READ_LIMIT: usize = 1024 * 1024;
    let (response, response_body) = if is_write {
        let (parts, body) = response.into_parts();
        match axum::body::to_bytes(body, RESPONSE_READ_LIMIT).await {
            Ok(bytes) => {
                let body_str = String::from_utf8_lossy(&bytes);
                let logged = if body_str.len() > AUDIT_BODY_MAX {
                    format!(
                        "{}... [truncated, {} bytes total]",
                        &body_str[..AUDIT_BODY_MAX],
                        bytes.len()
                    )
                } else {
                    body_str.into_owned()
                };
                let new_body = Body::from(bytes);
                (Response::from_parts(parts, new_body), Some(logged))
            }
            Err(_) => {
                // Response exceeded 1 MB -- very unusual for admin API.
                // Body is consumed; skip logging.
                let new_body = Body::empty();
                (Response::from_parts(parts, new_body), None)
            }
        }
    } else {
        (response, None)
    };

    let action = derive_action(&method, &matched_path);

    // Log asynchronously -- don't block the response
    tokio::task::spawn_blocking(move || {
        if let Ok(conn) = conary_core::db::open_fast(&db_path)
            && let Err(e) = conary_core::db::models::audit_log::insert(
                &conn,
                token_name.as_deref(),
                &action,
                &method,
                &path,
                status_code,
                request_body.as_deref(),
                response_body.as_deref(),
                source_ip.as_deref(),
                Some(duration_ms),
            )
        {
            tracing::warn!("Failed to write audit log: {e}");
        }
    });

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_action_tokens() {
        assert_eq!(derive_action("POST", "/v1/admin/tokens"), "tokens.create");
        assert_eq!(derive_action("GET", "/v1/admin/tokens"), "tokens.read");
        assert_eq!(
            derive_action("DELETE", "/v1/admin/tokens/{id}"),
            "tokens.delete"
        );
    }

    #[test]
    fn test_derive_action_ci() {
        assert_eq!(
            derive_action("GET", "/v1/admin/ci/workflows"),
            "ci.workflows.read"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/ci/workflows/{workflow}/dispatch"),
            "ci.workflows.dispatch.create"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/ci/mirror-sync"),
            "ci.mirror_sync.create"
        );
    }

    #[test]
    fn test_derive_action_repos() {
        assert_eq!(derive_action("GET", "/v1/admin/repos"), "repos.read");
        assert_eq!(derive_action("POST", "/v1/admin/repos"), "repos.create");
        assert_eq!(
            derive_action("PUT", "/v1/admin/repos/{name}"),
            "repos.update"
        );
        assert_eq!(
            derive_action("DELETE", "/v1/admin/repos/{name}"),
            "repos.delete"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/repos/{name}/sync"),
            "repos.sync.create"
        );
    }

    #[test]
    fn test_derive_action_federation() {
        assert_eq!(
            derive_action("GET", "/v1/admin/federation/peers"),
            "federation.peers.read"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/federation/peers"),
            "federation.peers.create"
        );
        assert_eq!(
            derive_action("DELETE", "/v1/admin/federation/peers/{id}"),
            "federation.peers.delete"
        );
        assert_eq!(
            derive_action("GET", "/v1/admin/federation/config"),
            "federation.config.read"
        );
        assert_eq!(
            derive_action("PUT", "/v1/admin/federation/config"),
            "federation.config.update"
        );
    }

    #[test]
    fn test_derive_action_audit() {
        assert_eq!(derive_action("GET", "/v1/admin/audit"), "audit.read");
        assert_eq!(derive_action("DELETE", "/v1/admin/audit"), "audit.delete");
    }

    #[test]
    fn test_derive_action_test_data() {
        assert_eq!(
            derive_action("GET", "/v1/admin/test-runs"),
            "test_runs.read"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/test-runs"),
            "test_runs.create"
        );
        assert_eq!(
            derive_action("DELETE", "/v1/admin/test-runs/gc"),
            "test_runs.gc.delete"
        );
        assert_eq!(
            derive_action("GET", "/v1/admin/test-health"),
            "test_health.read"
        );
    }

    #[test]
    fn test_derive_action_artifacts() {
        assert_eq!(
            derive_action("PUT", "/v1/admin/test-fixtures/{*path}"),
            "test_fixtures.update"
        );
        assert_eq!(
            derive_action("PUT", "/v1/admin/test-artifacts/{*path}"),
            "test_artifacts.update"
        );
    }

    #[test]
    fn test_derive_action_packages() {
        assert_eq!(
            derive_action("POST", "/v1/admin/releases/{distro}"),
            "releases.create"
        );
        assert_eq!(derive_action("POST", "/v1/admin/convert"), "convert.create");
    }

    #[test]
    fn test_derive_action_openapi() {
        assert_eq!(
            derive_action("GET", "/v1/admin/openapi.json"),
            "openapi.json.read"
        );
    }
}
