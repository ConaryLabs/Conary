// src/server/routes.rs
//! Axum router configuration for the Remi server
//!
//! Phase 0 hardening:
//! - HEAD endpoint for chunk existence checks
//! - Batch endpoints for multi-chunk operations
//! - Compression disabled for chunk responses (already compressed/binary)
//! - Rate limiting per IP (optional)

use crate::server::handlers::{chunks, federation, index, jobs, packages, recipes};
use crate::server::ServerState;
use axum::{
    routing::{get, head, post},
    Router,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};

/// Create the main application router
pub fn create_router(state: Arc<RwLock<ServerState>>) -> Router {
    // CORS configuration - permissive for now
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Compression layer - but we'll exclude chunk routes
    let compression = CompressionLayer::new();

    // Routes that should NOT be compressed (chunk data is already binary/compressed)
    let chunk_routes = Router::new()
        // HEAD for existence checks (with Bloom filter protection)
        .route("/v1/chunks/:hash", head(chunks::head_chunk))
        // GET for chunk data
        .route("/v1/chunks/:hash", get(chunks::get_chunk))
        // Batch operations
        .route("/v1/chunks/find-missing", post(chunks::find_missing))
        .route("/v1/chunks/batch", post(chunks::batch_fetch))
        .with_state(state.clone());

    // Routes that benefit from compression
    let compressed_routes = Router::new()
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
        // Admin endpoints
        .route("/v1/admin/convert", post(packages::trigger_conversion))
        .route("/v1/admin/cache/stats", get(chunks::cache_stats))
        .route("/v1/admin/evict", post(chunks::trigger_eviction))
        .route("/v1/admin/bloom/rebuild", post(chunks::rebuild_bloom))
        .layer(compression)
        .with_state(state);

    // Merge routers - chunk routes come first (no compression)
    Router::new()
        .merge(chunk_routes)
        .merge(compressed_routes)
        .layer(cors)
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check() {
        let config = crate::server::ServerConfig::default();
        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
