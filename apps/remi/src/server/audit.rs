// apps/remi/src/server/audit.rs
//! Audit logging middleware for the external admin API.
//!
//! Captures all admin API requests with timing, token identity, and
//! (for write operations) request/response bodies.

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::auth::TokenName;

/// Derive a semantic action name from method + path.
///
/// Examples:
/// - POST /v1/admin/tokens -> "token.create"
/// - GET /v1/admin/repos -> "repo.list"
/// - DELETE /v1/admin/federation/peers/abc123 -> "federation.peer.delete"
pub fn derive_action(method: &str, path: &str) -> String {
    // Strip the /v1/admin/ prefix
    let rest = path.strip_prefix("/v1/admin/").unwrap_or(path);

    // Map known path patterns to semantic actions
    let resource = if rest.starts_with("tokens") {
        "token"
    } else if rest.starts_with("ci/mirror-sync") {
        "ci.mirror_sync"
    } else if rest.starts_with("ci/workflows") && rest.contains("/dispatch") {
        "ci.dispatch"
    } else if rest.starts_with("ci/") {
        "ci"
    } else if rest.starts_with("repos") {
        if rest.contains("/sync") {
            "repo.sync"
        } else {
            "repo"
        }
    } else if rest.starts_with("federation/config") {
        "federation.config"
    } else if rest.starts_with("federation/peers") {
        "federation.peer"
    } else if rest.starts_with("audit") {
        "audit"
    } else if rest.starts_with("events") {
        "events"
    } else if rest.starts_with("test-health") {
        "test.health"
    } else if rest.starts_with("test-runs") {
        "test.run"
    } else if rest.starts_with("test-fixtures") {
        "test.fixture"
    } else if rest.starts_with("test-artifacts") {
        "test.artifact"
    } else if rest.starts_with("packages") || rest.starts_with("convert") {
        "package"
    } else if rest.starts_with("openapi") {
        "openapi"
    } else {
        "unknown"
    };

    let verb = match method {
        "GET" => "read",
        "POST" => "create",
        "PUT" => "update",
        "DELETE" => "delete",
        _ => "unknown",
    };

    // Special cases where the resource already includes the verb
    if resource.ends_with("dispatch")
        || resource.ends_with("mirror_sync")
        || resource.ends_with("sync")
    {
        return resource.to_string();
    }

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

    let action = derive_action(&method, &path);

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
        assert_eq!(derive_action("POST", "/v1/admin/tokens"), "token.create");
        assert_eq!(derive_action("GET", "/v1/admin/tokens"), "token.read");
        assert_eq!(
            derive_action("DELETE", "/v1/admin/tokens/5"),
            "token.delete"
        );
    }

    #[test]
    fn test_derive_action_ci() {
        assert_eq!(derive_action("GET", "/v1/admin/ci/workflows"), "ci.read");
        assert_eq!(
            derive_action("POST", "/v1/admin/ci/workflows/ci.yaml/dispatch"),
            "ci.dispatch"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/ci/mirror-sync"),
            "ci.mirror_sync"
        );
    }

    #[test]
    fn test_derive_action_repos() {
        assert_eq!(derive_action("GET", "/v1/admin/repos"), "repo.read");
        assert_eq!(derive_action("POST", "/v1/admin/repos"), "repo.create");
        assert_eq!(
            derive_action("PUT", "/v1/admin/repos/fedora"),
            "repo.update"
        );
        assert_eq!(
            derive_action("DELETE", "/v1/admin/repos/fedora"),
            "repo.delete"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/repos/fedora/sync"),
            "repo.sync"
        );
    }

    #[test]
    fn test_derive_action_federation() {
        assert_eq!(
            derive_action("GET", "/v1/admin/federation/peers"),
            "federation.peer.read"
        );
        assert_eq!(
            derive_action("POST", "/v1/admin/federation/peers"),
            "federation.peer.create"
        );
        assert_eq!(
            derive_action("DELETE", "/v1/admin/federation/peers/abc"),
            "federation.peer.delete"
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
        assert_eq!(derive_action("GET", "/v1/admin/test-runs"), "test.run.read");
        assert_eq!(
            derive_action("POST", "/v1/admin/test-runs"),
            "test.run.create"
        );
        assert_eq!(
            derive_action("DELETE", "/v1/admin/test-runs/gc"),
            "test.run.delete"
        );
        assert_eq!(
            derive_action("GET", "/v1/admin/test-health"),
            "test.health.read"
        );
    }

    #[test]
    fn test_derive_action_artifacts() {
        assert_eq!(
            derive_action("PUT", "/v1/admin/test-fixtures/demo/sample.ccs"),
            "test.fixture.update"
        );
        assert_eq!(
            derive_action("PUT", "/v1/admin/test-artifacts/run-1/output.json"),
            "test.artifact.update"
        );
    }

    #[test]
    fn test_derive_action_packages() {
        assert_eq!(
            derive_action("POST", "/v1/admin/packages/fedora"),
            "package.create"
        );
        assert_eq!(derive_action("POST", "/v1/admin/convert"), "package.create");
    }

    #[test]
    fn test_derive_action_openapi() {
        assert_eq!(
            derive_action("GET", "/v1/admin/openapi.json"),
            "openapi.read"
        );
    }
}
