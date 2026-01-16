// src/server/handlers/chunks.rs
//! Chunk serving endpoint - fast, dumb file server
//!
//! This endpoint serves raw chunks from the CAS store.
//! No conversion logic here - if a chunk is missing, return 404.
//! Chunks are immutable and infinitely cacheable.

use crate::server::ServerState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;
use tokio::fs::File;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

/// GET /v1/chunks/:hash
///
/// Serves a chunk by its content hash. Returns:
/// - 200 OK with chunk data and immutable cache headers
/// - 404 Not Found if chunk doesn't exist
pub async fn get_chunk(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(hash): Path<String>,
) -> Response {
    // Validate hash format (64 hex chars for SHA-256)
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return (StatusCode::BAD_REQUEST, "Invalid chunk hash format").into_response();
    }

    let state = state.read().await;
    let chunk_path = state.chunk_cache.chunk_path(&hash);

    // Check if chunk exists
    if !chunk_path.exists() {
        return (StatusCode::NOT_FOUND, "Chunk not found").into_response();
    }

    // Open file for streaming
    let file = match File::open(&chunk_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Failed to open chunk {}: {}", hash, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read chunk").into_response();
        }
    };

    // Get file size for Content-Length
    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to get chunk metadata {}: {}", hash, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read chunk").into_response();
        }
    };

    // Update access time for LRU tracking (fire and forget)
    let hash_clone = hash.clone();
    let cache = state.chunk_cache.clone();
    tokio::spawn(async move {
        if let Err(e) = cache.record_access(&hash_clone).await {
            tracing::warn!("Failed to record chunk access: {}", e);
        }
    });

    // Stream the file
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, metadata.len())
        // Immutable cache - chunks never change (content-addressed)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ETAG, format!("\"{}\"", hash))
        .body(body)
        .unwrap()
}

/// GET /v1/admin/cache/stats
///
/// Get cache statistics
pub async fn cache_stats(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> impl IntoResponse {
    let state = state.read().await;

    match state.chunk_cache.stats().await {
        Ok(stats) => Json(stats).into_response(),
        Err(e) => {
            tracing::error!("Failed to get cache stats: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to get stats: {}", e)).into_response()
        }
    }
}

/// POST /v1/admin/evict
///
/// Manually trigger LRU eviction (admin endpoint)
pub async fn trigger_eviction(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> impl IntoResponse {
    let state = state.read().await;

    match state.chunk_cache.run_eviction().await {
        Ok(result) => {
            tracing::info!(
                "Manual eviction: {} chunks, {} freed",
                result.chunks_evicted,
                result.bytes_freed_human
            );
            Json(result).into_response()
        }
        Err(e) => {
            tracing::error!("Eviction failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Eviction failed: {}", e)).into_response()
        }
    }
}
