// conary-server/src/server/routes.rs
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
    canonical, chunks, detail, federation, index, jobs, models, oci, packages, recipes, search,
    sparse, tuf,
};
use crate::server::security::RateLimiter;
use crate::server::{ServerConfig, ServerState};
use axum::{
    Json, Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, head, post, put},
};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, info, warn};

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
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let ip: std::net::Ipv4Addr = parts[0].parse().ok()?;
    let prefix_len: u32 = parts[1].parse().ok()?;
    Some((u32::from(ip), prefix_len))
}

/// Extract client IP from request, handling Cloudflare proxy headers
///
/// Priority:
/// 1. CF-Connecting-IP header (if request is from Cloudflare IP)
/// 2. X-Forwarded-For first IP (if trusted proxy header is set)
/// 3. Direct connection IP
fn extract_client_ip(
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

/// Extract client IP by reading the trusted_proxy_header from state once
async fn resolve_client_ip(
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
    let path = request.uri().path().to_string();
    let headers = request.headers().clone();

    // Get ban list from state
    let ban_list = state.read().await.ban_list.clone();
    let client_ip = resolve_client_ip(&state, &headers, &addr.ip()).await;
    let ip = client_ip.to_string();

    // Check if banned
    if ban_list.is_banned(&ip).await {
        warn!(ip = %ip, "Request rejected (banned)");
        return Err(StatusCode::FORBIDDEN);
    }

    // Process request
    let response = next.run(request).await;

    // Check for suspicious failures
    // 400 Bad Request (often malformed input/hash)
    // 401/403 (auth failures)
    // 404 on admin endpoints (probing)
    if (response.status() == StatusCode::BAD_REQUEST
        || response.status() == StatusCode::UNAUTHORIZED
        || response.status() == StatusCode::FORBIDDEN
        || (response.status() == StatusCode::NOT_FOUND && path.starts_with("/v1/admin")))
        && ban_list.record_failure(&ip).await
    {
        warn!(ip = %ip, "IP banned due to repeated failures");
    }

    Ok(response)
}

/// Create the main public application router
///
/// This router handles:
/// - Health checks
/// - Package metadata and downloads
/// - Chunk serving
/// - Federation discovery
/// - Job status polling
///
/// Admin endpoints are NOT included here - they're on a separate listener.
pub async fn create_router(state: Arc<RwLock<ServerState>>) -> Router {
    // Read config for router setup
    let config = {
        let guard = state.read().await;
        guard.config.clone()
    };

    // Create rate limiter if enabled
    let rate_limiter = Arc::new(RateLimiter::new(
        config.rate_limit_rps,
        config.rate_limit_burst,
    ));

    // Spawn periodic cleanup for the rate limiter to prevent unbounded HashMap growth
    if config.enable_rate_limit {
        let cleanup_limiter = Arc::clone(&rate_limiter);
        tokio::spawn(async move {
            let cleanup_interval = std::time::Duration::from_secs(300);
            let max_age = std::time::Duration::from_secs(300);
            loop {
                tokio::time::sleep(cleanup_interval).await;
                cleanup_limiter.cleanup(max_age).await;
            }
        });
    }

    // CORS layers
    let public_cors = create_cors_layer(&config, false);
    let restricted_cors = create_cors_layer(&config, true);

    // Compression layer - but we'll exclude chunk routes
    let compression = CompressionLayer::new();

    // Routes that should NOT be compressed (chunk data is already binary/compressed)
    // These get restricted CORS
    let chunk_routes = Router::new()
        // HEAD for existence checks (with Bloom filter protection)
        .route("/v1/chunks/:hash", head(chunks::head_chunk))
        // GET for chunk data
        .route("/v1/chunks/:hash", get(chunks::get_chunk))
        // Batch operations
        .route("/v1/chunks/find-missing", post(chunks::find_missing))
        .route("/v1/chunks/batch", post(chunks::batch_fetch))
        .layer(restricted_cors)
        .with_state(state.clone());

    // Public routes - permissive CORS (read-only, cacheable)
    let public_routes = Router::new()
        // Health check (enhanced)
        .route("/health", get(health_check))
        .route("/health/ready", get(readiness_check))
        // Federation discovery
        .route("/v1/federation/directory", get(federation::directory))
        // Repository index endpoints (Cloudflare-cached)
        .route("/v1/:distro/metadata", get(index::get_metadata))
        .route("/v1/:distro/metadata.sig", get(index::get_metadata_sig))
        // Package metadata endpoints (Cloudflare-cached, triggers conversion)
        .route("/v1/:distro/packages/:name", get(packages::get_package))
        // CCS package download (after conversion complete)
        .route(
            "/v1/:distro/packages/:name/download",
            get(packages::download_package),
        )
        // Delta manifest between two versions
        .route("/v1/:distro/packages/:name/delta", get(packages::get_delta))
        // Conversion job status (for 202 Accepted polling)
        .route("/v1/jobs/:job_id", get(jobs::get_job_status))
        // Recipe package download (read-only, after build complete)
        .route(
            "/v1/recipes/:name/:version/download",
            get(recipes::download_recipe_package),
        )
        // === Sparse Index (CDN-cacheable, crates.io-style) ===
        .route("/v1/index/:distro/:name", get(sparse::get_sparse_entry))
        .route("/v1/index/:distro", get(sparse::list_packages))
        // === Search ===
        .route("/v1/search", get(search::search_packages))
        .route("/v1/suggest", get(search::suggest_packages))
        // === Canonical Package Identity ===
        .route(
            "/v1/canonical/search",
            get(canonical::canonical_search),
        )
        .route("/v1/canonical/:name", get(canonical::canonical_lookup))
        .route("/v1/groups", get(canonical::groups_list))
        // === Model Collections (for remote include resolution) ===
        .route("/v1/models/:name", get(models::get_model))
        .route(
            "/v1/models/:name/signature",
            get(models::get_model_signature),
        )
        .route("/v1/models", get(models::list_models))
        // === Package Detail API ===
        .route(
            "/v1/packages/:distro/:name",
            get(detail::get_package_detail),
        )
        .route(
            "/v1/packages/:distro/:name/versions",
            get(detail::get_versions),
        )
        .route(
            "/v1/packages/:distro/:name/dependencies",
            get(detail::get_dependencies),
        )
        .route(
            "/v1/packages/:distro/:name/rdepends",
            get(detail::get_reverse_dependencies),
        )
        // === TUF Trust Metadata ===
        .route("/v1/:distro/tuf/timestamp.json", get(tuf::get_timestamp))
        .route("/v1/:distro/tuf/snapshot.json", get(tuf::get_snapshot))
        .route("/v1/:distro/tuf/targets.json", get(tuf::get_targets))
        .route("/v1/:distro/tuf/root.json", get(tuf::get_root))
        .route("/v1/:distro/tuf/:version", get(tuf::get_versioned_root))
        // === Statistics ===
        .route("/v1/stats/popular", get(detail::get_popular))
        .route("/v1/stats/recent", get(detail::get_recent))
        .route("/v1/stats/overview", get(detail::get_overview))
        // Prometheus metrics (public, for monitoring)
        .route("/metrics", get(prometheus_metrics))
        // === OCI Distribution API v2 ===
        .route("/v2/", get(oci::version_check))
        .route("/v2/_catalog", get(oci::catalog))
        // Catch-all for /v2/{name}/manifests/{ref}, /v2/{name}/blobs/{digest},
        // /v2/{name}/tags/list. Name can contain slashes so we use a wildcard.
        .route(
            "/v2/*path",
            get(oci::oci_catchall).head(oci::oci_catchall_head),
        )
        .layer(compression)
        .layer(public_cors)
        .with_state(state.clone());

    // SPA fallback: serve web frontend if configured
    // Must be a separate router so API routes take priority
    let web_routes = {
        let state_guard = state.read().await;
        state_guard.config.web_root.as_ref().map(|web_root| {
            Router::new().fallback_service(tower_http::services::ServeDir::new(web_root).fallback(
                tower_http::services::ServeFile::new(web_root.join("index.html")),
            ))
        })
    };

    // Build final router with middleware
    let mut app = Router::new().merge(chunk_routes).merge(public_routes);

    if let Some(web) = web_routes {
        app = app.merge(web);
    }

    // Add rate limiting if enabled
    if config.enable_rate_limit {
        app = app.route_layer(middleware::from_fn_with_state(
            (rate_limiter, state.clone()),
            rate_limit_middleware,
        ));
    }

    // Add ban list enforcement (always enabled)
    app = app.route_layer(middleware::from_fn_with_state(
        state.clone(),
        ban_middleware,
    ));

    // Add body size limit (16MB max for all requests)
    app = app.layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024));

    // Add audit logging if enabled
    if config.enable_audit_log {
        app = app.route_layer(middleware::from_fn_with_state(
            state.clone(),
            audit_log_middleware,
        ));
    }

    app
}

/// Create the admin router (localhost only)
///
/// This router handles privileged operations:
/// - Triggering conversions
/// - Cache management (stats, eviction)
/// - Bloom filter management
/// - Recipe builds (SSRF-sensitive)
/// - Server metrics and stats
///
/// SECURITY: This router should ONLY be bound to localhost (127.0.0.1).
/// Access from external networks should be via SSH tunnel.
pub fn create_admin_router(state: Arc<RwLock<ServerState>>) -> Router {
    Router::new()
        // Admin endpoints - no external CORS, localhost only
        .route("/health", get(|| async { "OK" }))
        // Conversion management
        .route("/v1/admin/convert", post(packages::trigger_conversion))
        // Cache management
        .route("/v1/admin/cache/stats", get(chunks::cache_stats))
        .route("/v1/admin/evict", post(chunks::trigger_eviction))
        // Bloom filter management
        .route("/v1/admin/bloom/rebuild", post(chunks::rebuild_bloom))
        // Metrics (detailed, internal)
        .route("/v1/admin/metrics", get(admin_metrics))
        .route("/v1/admin/metrics/prometheus", get(prometheus_metrics))
        // Negative cache stats
        .route("/v1/admin/negative-cache/stats", get(negative_cache_stats))
        .route("/v1/admin/negative-cache/clear", post(negative_cache_clear))
        // Recipe build (moved from public - SSRF vector)
        .route("/v1/admin/recipes/build", post(recipes::build_recipe))
        // Server info
        .route("/v1/admin/info", get(server_info))
        // Upstream metadata refresh
        .route("/v1/admin/refresh", post(refresh_upstream))
        // Model collection publishing
        .route("/v1/admin/models/:name", put(models::put_model))
        // TUF timestamp refresh
        .route(
            "/v1/admin/tuf/refresh-timestamp",
            post(tuf::refresh_timestamp),
        )
        .with_state(state)
}

/// Simple liveness check
async fn health_check() -> &'static str {
    "OK"
}

/// Readiness check - verifies DB and storage are accessible
async fn readiness_check(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Response {
    let state_guard = state.read().await;
    let config = &state_guard.config;

    // Check database is accessible
    let db_ok = config.db_path.exists()
        || config
            .db_path
            .parent()
            .is_some_and(|p| p.exists() && p.is_dir());

    // Check chunk directory is writable
    let chunk_dir_ok = config.chunk_dir.exists() && config.chunk_dir.is_dir();

    // Check cache directory is writable
    let cache_dir_ok = config.cache_dir.exists() && config.cache_dir.is_dir();

    // Check disk space (warn if < 10GB free)
    let disk_ok = check_disk_space(&config.chunk_dir, 10 * 1024 * 1024 * 1024);

    drop(state_guard);

    if db_ok && chunk_dir_ok && cache_dir_ok && disk_ok {
        (StatusCode::OK, "READY").into_response()
    } else {
        let details = ReadinessDetails {
            ready: false,
            db_accessible: db_ok,
            chunk_dir_ok,
            cache_dir_ok,
            disk_space_ok: disk_ok,
        };
        (StatusCode::SERVICE_UNAVAILABLE, Json(details)).into_response()
    }
}

#[derive(Serialize)]
struct ReadinessDetails {
    ready: bool,
    db_accessible: bool,
    chunk_dir_ok: bool,
    cache_dir_ok: bool,
    disk_space_ok: bool,
}

/// Check if a path has at least `min_bytes` of free space
fn check_disk_space(path: &std::path::Path, min_bytes: u64) -> bool {
    // Use statvfs on Unix
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let path_cstr = match CString::new(path.as_os_str().as_bytes()) {
            Ok(s) => s,
            Err(_) => return true, // Can't check, assume OK
        };

        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                #[allow(clippy::unnecessary_cast)]
                let free_bytes = stat.f_bavail as u64 * stat.f_bsize as u64;
                return free_bytes >= min_bytes;
            }
        }
        true // Can't check, assume OK
    }

    #[cfg(not(unix))]
    {
        let _ = (path, min_bytes);
        true // Can't check on non-Unix, assume OK
    }
}

/// Prometheus metrics endpoint
async fn prometheus_metrics(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> String {
    let state = state.read().await;
    state.metrics.to_prometheus()
}

/// Detailed admin metrics
async fn admin_metrics(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<AdminMetrics> {
    let state_guard = state.read().await;

    let metrics_snapshot = state_guard.metrics.snapshot();
    let negative_cache_stats = state_guard.negative_cache.stats().await;
    let job_stats = state_guard.job_manager.stats();

    Json(AdminMetrics {
        requests_total: metrics_snapshot.requests_total,
        cache_hits: metrics_snapshot.hits,
        cache_misses: metrics_snapshot.misses,
        hit_rate: metrics_snapshot.hit_rate,
        bloom_rejects: metrics_snapshot.bloom_rejects,
        bytes_served: metrics_snapshot.bytes_served,
        upstream_fetches: metrics_snapshot.upstream_fetches,
        upstream_errors: metrics_snapshot.upstream_errors,
        uptime_secs: metrics_snapshot.uptime_secs,
        negative_cache_entries: negative_cache_stats.active_entries,
        negative_cache_hits: negative_cache_stats.total_hits,
        jobs_pending: job_stats.pending,
        jobs_converting: job_stats.converting,
        jobs_completed: job_stats.completed,
        jobs_failed: job_stats.failed,
    })
}

#[derive(Serialize)]
struct AdminMetrics {
    requests_total: u64,
    cache_hits: u64,
    cache_misses: u64,
    hit_rate: f64,
    bloom_rejects: u64,
    bytes_served: u64,
    upstream_fetches: u64,
    upstream_errors: u64,
    uptime_secs: u64,
    negative_cache_entries: usize,
    negative_cache_hits: u64,
    jobs_pending: usize,
    jobs_converting: usize,
    jobs_completed: usize,
    jobs_failed: usize,
}

/// Negative cache statistics
async fn negative_cache_stats(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<serde_json::Value> {
    let state_guard = state.read().await;
    let stats = state_guard.negative_cache.stats().await;

    Json(serde_json::json!({
        "total_entries": stats.total_entries,
        "active_entries": stats.active_entries,
        "expired_entries": stats.expired_entries,
        "total_hits": stats.total_hits,
        "ttl_seconds": stats.ttl_secs,
    }))
}

/// Clear the negative cache (removes all entries, not just expired ones)
async fn negative_cache_clear(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<serde_json::Value> {
    let state_guard = state.read().await;
    let removed = state_guard.negative_cache.clear_all().await;

    Json(serde_json::json!({
        "cleared": true,
        "entries_removed": removed,
    }))
}

/// Server information
async fn server_info(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Json<serde_json::Value> {
    let state_guard = state.read().await;
    let config = &state_guard.config;

    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "bind_addr": config.bind_addr.to_string(),
        "db_path": config.db_path.display().to_string(),
        "chunk_dir": config.chunk_dir.display().to_string(),
        "max_concurrent_conversions": config.max_concurrent_conversions,
        "cache_max_bytes": config.cache_max_bytes,
        "bloom_filter_enabled": config.enable_bloom_filter,
        "rate_limit_enabled": config.enable_rate_limit,
        "trusted_proxy_header": state_guard.trusted_proxy_header.clone(),
    }))
}

/// Trigger upstream metadata refresh
async fn refresh_upstream() -> Json<serde_json::Value> {
    // TODO: Implement actual metadata refresh
    Json(serde_json::json!({
        "status": "not_implemented",
        "message": "Upstream refresh not yet implemented"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

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
}
