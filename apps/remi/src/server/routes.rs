// apps/remi/src/server/routes.rs
//! Axum router configuration for the Remi server
//!
//! Phase 0 hardening:
//! - HEAD endpoint for chunk existence checks
//! - Batch endpoints for multi-chunk operations
//! - Compression disabled for chunk responses (already compressed/binary)
//! - Rate limiting per IP (optional)
//!
//! Phase 4 security:
//! - CORS restrictions for chunk/admin endpoints
//! - Rate limiting middleware
//! - Audit logging for federation requests
//! - Ban list for misbehaving IPs
//! - Separate admin listener on localhost only
//! - Cloudflare IP header extraction
//! - Recipe build moved to admin API

use crate::server::handlers::{
    admin as admin_handlers, artifacts, canonical, chunks, derivations, detail, federation, index,
    jobs, models, oci, openapi, packages, profiles, recipes, search, seeds, self_update, sparse,
    tuf,
};
use crate::server::security::RateLimiter;
use crate::server::{ServerConfig, ServerState};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, head, post, put},
};
use serde::Serialize;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower::Service;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, info, warn};

mod admin;
mod mcp;
mod public;

pub use admin::{create_admin_router, create_external_admin_router};
pub use public::create_router;

const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;

fn request_body_limit_bytes() -> usize {
    MAX_REQUEST_BODY_BYTES
}

/// Cloudflare IP ranges (IPv4)
/// These should be periodically updated from https://www.cloudflare.com/ips-v4
const CLOUDFLARE_IPV4_RANGES: &[&str] = &[
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
];

fn mcp_scope_error(request: &Request<Body>) -> Option<Response> {
    let scopes = request
        .extensions()
        .get::<crate::server::auth::TokenScopes>()
        .cloned()
        .map(axum::Extension);

    admin_handlers::check_scope(&scopes, crate::server::auth::Scope::Admin)
}

/// Check if an IP is in Cloudflare's ranges
fn is_cloudflare_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            let ip_u32 = u32::from(*ipv4);
            CLOUDFLARE_IPV4_RANGES.iter().any(|range| {
                parse_cidr(range).is_some_and(|(network, prefix_len)| {
                    let mask = if prefix_len == 0 {
                        0
                    } else {
                        !0u32 << (32 - prefix_len)
                    };
                    (ip_u32 & mask) == (network & mask)
                })
            })
        }
        IpAddr::V6(ipv6) => {
            // Cloudflare IPv6 CIDR ranges
            const CF_V6: &[(&[u16; 8], u32)] = &[
                (&[0x2400, 0xcb00, 0, 0, 0, 0, 0, 0], 32),
                (&[0x2606, 0x4700, 0, 0, 0, 0, 0, 0], 32),
                (&[0x2803, 0xf800, 0, 0, 0, 0, 0, 0], 32),
                (&[0x2405, 0xb500, 0, 0, 0, 0, 0, 0], 32),
                (&[0x2405, 0x8100, 0, 0, 0, 0, 0, 0], 32),
                (&[0x2a06, 0x98c0, 0, 0, 0, 0, 0, 0], 29),
                (&[0x2c0f, 0xf248, 0, 0, 0, 0, 0, 0], 32),
            ];
            let segments = ipv6.segments();
            CF_V6.iter().any(|(network, prefix_len)| {
                let prefix = *prefix_len;
                let full_segments = (prefix / 16) as usize;
                for i in 0..full_segments.min(8) {
                    if segments[i] != network[i] {
                        return false;
                    }
                }
                if full_segments < 8 {
                    let remaining_bits = prefix % 16;
                    if remaining_bits > 0 {
                        let mask = !0u16 << (16 - remaining_bits);
                        if (segments[full_segments] & mask) != (network[full_segments] & mask) {
                            return false;
                        }
                    }
                }
                true
            })
        }
    }
}

/// Check if a connection IP is from a trusted proxy source (loopback or private).
///
/// Only connections from these addresses are allowed to set proxy headers like
/// X-Forwarded-For or X-Real-IP. This prevents external clients from spoofing
/// client IP addresses.
fn is_trusted_proxy_source(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Parse a CIDR notation string into (network_u32, prefix_length)
fn parse_cidr(cidr: &str) -> Option<(u32, u32)> {
    let (ip_str, prefix_str) = cidr.split_once('/')?;
    let ip: std::net::Ipv4Addr = ip_str.parse().ok()?;
    let prefix_len: u32 = prefix_str.parse().ok()?;
    Some((u32::from(ip), prefix_len))
}

/// Extract client IP from request, handling Cloudflare proxy headers
///
/// Priority:
/// 1. CF-Connecting-IP header (if request is from Cloudflare IP)
/// 2. X-Forwarded-For first IP (if trusted proxy header is set)
/// 3. Direct connection IP
///
/// This is the single source of truth for client IP extraction. All
/// middleware (rate limiting, audit, auth) should use this function
/// or its async wrapper [`resolve_client_ip`] instead of reading
/// `ConnectInfo` directly.
pub fn extract_client_ip(
    headers: &HeaderMap,
    conn_ip: &IpAddr,
    trusted_proxy_header: Option<&str>,
) -> IpAddr {
    // Check if connection is from Cloudflare
    if is_cloudflare_ip(conn_ip) {
        // Try CF-Connecting-IP first
        if let Some(cf_ip) = headers
            .get("CF-Connecting-IP")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<IpAddr>().ok())
        {
            debug!(cf_ip = %cf_ip, conn_ip = %conn_ip, "Using CF-Connecting-IP");
            return cf_ip;
        }
    }

    // Check trusted proxy header if configured.
    // Only honor the header when the direct connection is from a loopback or
    // private address (i.e., the server is behind a reverse proxy). This
    // prevents external clients from spoofing the header to bypass rate
    // limiting and ban lists.
    if let Some(header_name) = trusted_proxy_header
        && is_trusted_proxy_source(conn_ip)
        && let Some(ip) = headers
            .get(header_name)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                // X-Forwarded-For can have multiple IPs, take the first (client)
                s.split(',')
                    .next()
                    .map(|ip| ip.trim().parse::<IpAddr>().ok())
            })
            .flatten()
    {
        debug!(header = header_name, ip = %ip, "Using trusted proxy header");
        return ip;
    }

    // Fall back to direct connection IP
    *conn_ip
}

/// Create CORS layer based on configuration
fn create_cors_layer(config: &ServerConfig, restricted: bool) -> CorsLayer {
    if !restricted {
        // Permissive CORS for public endpoints (health, federation directory, metadata)
        return CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::HEAD, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::ACCEPT]);
    }

    // Restricted CORS for chunk and admin endpoints
    if config.cors_allowed_origins.is_empty() {
        // No external origins allowed - same-origin only
        CorsLayer::new()
            .allow_methods([Method::GET, Method::HEAD, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::ACCEPT, header::AUTHORIZATION])
    } else {
        // Allow specific origins
        let origins: Vec<HeaderValue> = config
            .cors_allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();

        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([Method::GET, Method::HEAD, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::ACCEPT, header::AUTHORIZATION])
    }
}

/// Extract client IP by reading the trusted_proxy_header from state once.
///
/// Convenience wrapper around [`extract_client_ip`] for middleware that
/// already holds an `Arc<RwLock<ServerState>>`. Acquires a brief read
/// lock to fetch the trusted proxy header configuration.
pub async fn resolve_client_ip(
    state: &Arc<RwLock<ServerState>>,
    headers: &HeaderMap,
    conn_ip: &IpAddr,
) -> IpAddr {
    let state_guard = state.read().await;
    let trusted_header = state_guard.trusted_proxy_header.as_deref();
    extract_client_ip(headers, conn_ip, trusted_header)
}

/// Audit logging middleware with Cloudflare IP extraction
async fn audit_log_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let path = uri.path();
    let headers = request.headers().clone();
    let client_ip = resolve_client_ip(&state, &headers, &addr.ip()).await;

    // Only log federation and admin endpoints
    let should_log = path.starts_with("/v1/chunks")
        || path.starts_with("/v1/admin")
        || path.starts_with("/v1/federation");

    let start = Instant::now();
    let response = next.run(request).await;
    let elapsed = start.elapsed();

    if should_log {
        let status = response.status();
        info!(
            target: "audit",
            method = %method,
            path = %path,
            status = status.as_u16(),
            client_ip = %client_ip,
            conn_ip = %addr.ip(),
            // SAFETY: as_millis() returns u128 but u64 can hold ~585 million
            // years of milliseconds, so this cast is lossless in practice.
            latency_ms = elapsed.as_millis() as u64,
            "request"
        );
    }

    response
}

/// Rate limiting middleware with Cloudflare IP extraction
async fn rate_limit_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::State((limiter, state)): axum::extract::State<(
        Arc<RateLimiter>,
        Arc<RwLock<ServerState>>,
    )>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let headers = request.headers().clone();
    let client_ip = resolve_client_ip(&state, &headers, &addr.ip()).await;

    let ip = client_ip.to_string();

    if !limiter.check(&ip).await {
        warn!(ip = %ip, "Rate limit exceeded");
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    Ok(next.run(request).await)
}

/// Ban list enforcement middleware with Cloudflare IP extraction
async fn ban_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let _path = request.uri().path().to_string();
    let headers = request.headers().clone();

    // Get ban list from state
    let ban_list = state.read().await.ban_list.clone();
    let client_ip = resolve_client_ip(&state, &headers, &addr.ip()).await;

    // Check if banned
    if ban_list.is_banned(client_ip).await {
        warn!(ip = %client_ip, "Request rejected (banned)");
        return Err(StatusCode::FORBIDDEN);
    }

    // Process request
    let response = next.run(request).await;

    // Only count authentication/authorization failures (401/403) toward the
    // ban threshold. Other error codes (400, 404) are too noisy and would
    // cause false-positive bans from normal client errors.
    if (response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN)
        && ban_list.record_failure(client_ip).await
    {
        warn!(ip = %client_ip, "IP banned due to repeated auth failures");
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::net::Ipv4Addr;
    use tower::ServiceExt;

    #[test]
    fn test_cors_layer_restricted_no_origins() {
        let config = ServerConfig::default();
        // Default config has empty cors_allowed_origins
        let _cors = create_cors_layer(&config, true);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_cors_layer_restricted_with_origins() {
        let config = ServerConfig {
            cors_allowed_origins: vec!["https://example.com".to_string()],
            ..ServerConfig::default()
        };
        let _cors = create_cors_layer(&config, true);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_cors_layer_public() {
        let config = ServerConfig::default();
        let _cors = create_cors_layer(&config, false);
        // Public CORS should be permissive
    }

    #[test]
    fn test_request_body_limit_matches_task_29_cap() {
        assert_eq!(request_body_limit_bytes(), 16 * 1024 * 1024);
    }

    #[test]
    fn test_is_cloudflare_ip() {
        // Known Cloudflare IPs
        assert!(is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(104, 16, 0, 1))));
        assert!(is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(172, 64, 0, 1))));
        assert!(is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(162, 158, 0, 1))));

        // Non-Cloudflare IPs
        assert!(!is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 1
        ))));
        assert!(!is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn test_parse_cidr() {
        let (network, prefix) = parse_cidr("192.168.1.0/24").unwrap();
        assert_eq!(prefix, 24);
        assert_eq!(network, u32::from(Ipv4Addr::new(192, 168, 1, 0)));

        let (network, prefix) = parse_cidr("10.0.0.0/8").unwrap();
        assert_eq!(prefix, 8);
        assert_eq!(network, u32::from(Ipv4Addr::new(10, 0, 0, 0)));
    }

    #[test]
    fn test_extract_client_ip_direct() {
        let headers = HeaderMap::new();
        let conn_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));

        let result = extract_client_ip(&headers, &conn_ip, None);
        assert_eq!(result, conn_ip);
    }

    #[test]
    fn test_extract_client_ip_cf_header() {
        let mut headers = HeaderMap::new();
        headers.insert("CF-Connecting-IP", "203.0.113.50".parse().unwrap());

        // From Cloudflare IP
        let cf_ip = IpAddr::V4(Ipv4Addr::new(104, 16, 0, 1));
        let result = extract_client_ip(&headers, &cf_ip, None);
        assert_eq!(result, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)));

        // From non-Cloudflare IP (should ignore CF header)
        let non_cf_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let result = extract_client_ip(&headers, &non_cf_ip, None);
        assert_eq!(result, non_cf_ip);
    }

    #[test]
    fn test_extract_client_ip_trusted_header() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Real-IP", "10.20.30.40".parse().unwrap());

        let conn_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let result = extract_client_ip(&headers, &conn_ip, Some("X-Real-IP"));
        assert_eq!(result, IpAddr::V4(Ipv4Addr::new(10, 20, 30, 40)));
    }

    #[test]
    fn test_extract_client_ip_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Forwarded-For",
            "203.0.113.50, 198.51.100.1, 192.0.2.1".parse().unwrap(),
        );

        let conn_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let result = extract_client_ip(&headers, &conn_ip, Some("X-Forwarded-For"));
        // Should take the first IP (original client)
        assert_eq!(result, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)));
    }

    #[tokio::test]
    async fn test_mcp_route_rejects_unauthenticated_requests() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

        let response = app
            .oneshot(Request::builder().uri("/mcp").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(
            matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            "unauthenticated MCP requests should be rejected"
        );
    }

    #[tokio::test]
    async fn test_mcp_route_rejects_non_admin_scope() {
        let (_app, db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
        let token = "test-repo-reader-token-54321";
        let hash = crate::server::auth::hash_token(token);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::models::admin_token::create(&conn, "test-repo-reader", &hash, "repos:read").unwrap();
        drop(conn);

        let app = crate::server::handlers::admin::test_helpers::rebuild_app(&db_path);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
