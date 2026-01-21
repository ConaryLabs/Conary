// src/server/routes.rs
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

use crate::server::handlers::{chunks, federation, index, jobs, packages, recipes};
use crate::server::security::RateLimiter;
use crate::server::{ServerConfig, ServerState};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, HeaderValue, Method, Request, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, head, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

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

/// Audit logging middleware
async fn audit_log_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let path = uri.path();

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
            client_ip = %addr.ip(),
            latency_ms = elapsed.as_millis() as u64,
            "federation request"
        );
    }

    response
}

/// Rate limiting middleware
async fn rate_limit_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::State(limiter): axum::extract::State<Arc<RateLimiter>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let ip = addr.ip().to_string();

    if !limiter.check(&ip).await {
        warn!(ip = %ip, "Rate limit exceeded");
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    Ok(next.run(request).await)
}

/// Ban list enforcement middleware
async fn ban_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let ip = addr.ip().to_string();
    let path = request.uri().path().to_string();
    
    // Get ban list from state
    let state_guard = state.read().await;
    let ban_list = state_guard.ban_list.clone();
    drop(state_guard);

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

/// Create the main application router
pub fn create_router(state: Arc<RwLock<ServerState>>) -> Router {
    // We need to read config synchronously for router setup
    // Use a blocking approach since this runs once at startup
    let config = {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let guard = state.read().await;
            guard.config.clone()
        })
    };

    // Create rate limiter if enabled
    let rate_limiter = Arc::new(RateLimiter::new(config.rate_limit_rps, config.rate_limit_burst));

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
        .layer(restricted_cors.clone())
        .with_state(state.clone());

    // Admin routes - restricted CORS
    let admin_routes = Router::new()
        // Admin endpoints
        .route("/v1/admin/convert", post(packages::trigger_conversion))
        .route("/v1/admin/cache/stats", get(chunks::cache_stats))
        .route("/v1/admin/evict", post(chunks::trigger_eviction))
        .route("/v1/admin/bloom/rebuild", post(chunks::rebuild_bloom))
        .route("/v1/admin/metrics/prometheus", get(prometheus_metrics))
        .layer(restricted_cors)
        .with_state(state.clone());

    // Public routes - permissive CORS (read-only, cacheable)
    let public_routes = Router::new()
        // Health check
        .route("/health", get(health_check))
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
        // Conversion job status (for 202 Accepted polling)
        .route("/v1/jobs/:job_id", get(jobs::get_job_status))
        // Recipe build endpoints
        .route("/v1/recipes/build", post(recipes::build_recipe))
        .route(
            "/v1/recipes/:name/:version/download",
            get(recipes::download_recipe_package),
        )
        .layer(compression)
        .layer(public_cors)
        .with_state(state.clone()); // Use state for public routes too

    // Build final router with middleware
    let mut app = Router::new()
        .merge(chunk_routes)
        .merge(admin_routes)
        .merge(public_routes);

    // Add rate limiting if enabled
    if config.enable_rate_limit {
        app = app
            .route_layer(middleware::from_fn_with_state(
                rate_limiter,
                rate_limit_middleware,
            ));
    }

    // Add ban list enforcement (always enabled)
    app = app.route_layer(middleware::from_fn_with_state(
        state.clone(),
        ban_middleware,
    ));

    // Add audit logging if enabled
    if config.enable_audit_log {
        app = app.route_layer(middleware::from_fn(audit_log_middleware));
    }

    app
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

/// Prometheus metrics endpoint
async fn prometheus_metrics(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> String {
    let state = state.read().await;
    state.metrics.to_prometheus()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cors_layer_restricted_no_origins() {
        let config = ServerConfig::default();
        // Default config has empty cors_allowed_origins
        let _cors = create_cors_layer(&config, true);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_cors_layer_restricted_with_origins() {
        let mut config = ServerConfig::default();
        config.cors_allowed_origins = vec!["https://example.com".to_string()];
        let _cors = create_cors_layer(&config, true);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_cors_layer_public() {
        let config = ServerConfig::default();
        let _cors = create_cors_layer(&config, false);
        // Public CORS should be permissive
    }
}