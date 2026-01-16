// src/server/routes.rs
//! Axum router configuration for the Refinery server

use crate::server::handlers::{chunks, index, jobs, packages};
use crate::server::ServerState;
use axum::{
    routing::{get, post},
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

    Router::new()
        // Health check
        .route("/health", get(health_check))
        // Repository index endpoints (Cloudflare-cached)
        .route("/v1/:distro/metadata", get(index::get_metadata))
        .route("/v1/:distro/metadata.sig", get(index::get_metadata_sig))
        // Package metadata endpoints (Cloudflare-cached, triggers conversion)
        .route("/v1/:distro/packages/:name", get(packages::get_package))
        // CCS package download (after conversion complete)
        .route("/v1/:distro/packages/:name/download", get(packages::download_package))
        // Chunk serving endpoints (direct, immutable)
        .route("/v1/chunks/:hash", get(chunks::get_chunk))
        // Conversion job status (for 202 Accepted polling)
        .route("/v1/jobs/:job_id", get(jobs::get_job_status))
        // Admin endpoints
        .route("/v1/admin/convert", post(packages::trigger_conversion))
        .route("/v1/admin/evict", post(chunks::trigger_eviction))
        // Layers
        .layer(CompressionLayer::new())
        .layer(cors)
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}
