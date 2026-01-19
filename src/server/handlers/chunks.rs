// src/server/handlers/chunks.rs
//! Chunk serving endpoint - fast, dumb file server
//!
//! This endpoint serves raw chunks from the CAS store.
//! No conversion logic here - if a chunk is missing, return 404.
//! Chunks are immutable and infinitely cacheable.
//!
//! Phase 0 hardening:
//! - HEAD endpoint with Bloom filter protection
//! - Batch endpoints for finding missing chunks
//! - Pull-through caching (fetch from upstream on miss)
//! - Metrics tracking

use crate::server::ServerState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs::File;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

/// Validate chunk hash format (64 hex chars for SHA-256)
fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// HEAD /v1/chunks/:hash
///
/// Check if a chunk exists without transferring data.
/// Uses Bloom filter to quickly reject definite misses without disk I/O.
/// Returns:
/// - 200 OK with Content-Length and ETag (chunk exists)
/// - 404 Not Found (chunk doesn't exist)
pub async fn head_chunk(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(hash): Path<String>,
) -> Response {
    // Validate hash format
    if !is_valid_hash(&hash) {
        return (StatusCode::BAD_REQUEST, "Invalid chunk hash format").into_response();
    }

    let state = state.read().await;

    // First check Bloom filter - definite "no" avoids disk I/O
    if let Some(ref bloom) = state.bloom_filter
        && !bloom.might_contain(&hash)
    {
        state.metrics.record_bloom_reject();
        return (StatusCode::NOT_FOUND, "Chunk not found").into_response();
    }

    // Bloom says "maybe" - check disk
    let chunk_path = state.chunk_cache.chunk_path(&hash);

    match tokio::fs::metadata(&chunk_path).await {
        Ok(metadata) => {
            state.metrics.record_hit();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, metadata.len())
                .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                .header(header::ETAG, format!("\"{}\"", hash))
                .header(header::ACCEPT_RANGES, "bytes")
                .body(Body::empty())
                .unwrap()
        }
        Err(_) => {
            state.metrics.record_miss();
            (StatusCode::NOT_FOUND, "Chunk not found").into_response()
        }
    }
}

/// GET /v1/chunks/:hash
///
/// Serves a chunk by its content hash. Returns:
/// - 200 OK with chunk data and immutable cache headers
/// - 404 Not Found if chunk doesn't exist
pub async fn get_chunk(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(hash): Path<String>,
) -> Response {
    // Validate hash format
    if !is_valid_hash(&hash) {
        return (StatusCode::BAD_REQUEST, "Invalid chunk hash format").into_response();
    }

    let state_guard = state.read().await;

    // First check Bloom filter
    if let Some(ref bloom) = state_guard.bloom_filter
        && !bloom.might_contain(&hash)
    {
        state_guard.metrics.record_bloom_reject();

        // Try pull-through if configured
        if state_guard.config.upstream_url.is_some() {
            drop(state_guard);
            return pull_through_fetch(state, &hash).await;
        }

        return (StatusCode::NOT_FOUND, "Chunk not found").into_response();
    }

    let chunk_path = state_guard.chunk_cache.chunk_path(&hash);

    // Check if chunk exists locally
    if !chunk_path.exists() {
        // Try pull-through if configured
        if state_guard.config.upstream_url.is_some() {
            drop(state_guard);
            return pull_through_fetch(state, &hash).await;
        }
        state_guard.metrics.record_miss();
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
    let cache = state_guard.chunk_cache.clone();
    tokio::spawn(async move {
        if let Err(e) = cache.record_access(&hash_clone).await {
            tracing::warn!("Failed to record chunk access: {}", e);
        }
    });

    state_guard.metrics.record_hit();
    state_guard.metrics.record_bytes_served(metadata.len());

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
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .unwrap()
}

/// Pull-through caching: fetch from upstream and store locally
async fn pull_through_fetch(
    state: Arc<RwLock<ServerState>>,
    hash: &str,
) -> Response {
    let state_guard = state.read().await;

    let upstream_url = match &state_guard.config.upstream_url {
        Some(url) => url.clone(),
        None => return (StatusCode::NOT_FOUND, "Chunk not found").into_response(),
    };

    tracing::debug!("Pull-through fetch for chunk {} from {}", hash, upstream_url);
    state_guard.metrics.record_upstream_fetch();

    // Build upstream URL
    let fetch_url = format!("{}/v1/chunks/{}", upstream_url.trim_end_matches('/'), hash);

    // Fetch from upstream
    let client = &state_guard.http_client;
    let response = match client.get(&fetch_url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to fetch chunk {} from upstream: {}", hash, e);
            return (StatusCode::NOT_FOUND, "Chunk not found").into_response();
        }
    };

    if !response.status().is_success() {
        state_guard.metrics.record_miss();
        return (StatusCode::NOT_FOUND, "Chunk not found").into_response();
    }

    // Get the data
    let data = match response.bytes().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Failed to read chunk {} from upstream: {}", hash, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read chunk").into_response();
        }
    };

    // Verify hash before storing
    let computed_hash = sha2_hash(&data);
    if computed_hash != hash {
        tracing::error!(
            "Hash mismatch for chunk from upstream: expected {}, got {}",
            hash,
            computed_hash
        );
        return (StatusCode::INTERNAL_SERVER_ERROR, "Chunk hash mismatch").into_response();
    }

    // Store locally
    let cache = state_guard.chunk_cache.clone();
    let hash_owned = hash.to_string();
    let data_clone = data.clone();

    // Update bloom filter
    if let Some(ref bloom) = state_guard.bloom_filter {
        bloom.add(hash);
    }

    // Store in background (don't block response)
    tokio::spawn(async move {
        if let Err(e) = cache.store_chunk(&hash_owned, &data_clone).await {
            tracing::warn!("Failed to store pull-through chunk {}: {}", hash_owned, e);
        }
    });

    state_guard.metrics.record_hit();
    state_guard.metrics.record_bytes_served(data.len() as u64);

    // Return the data
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, data.len())
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ETAG, format!("\"{}\"", hash))
        .header(header::ACCEPT_RANGES, "bytes")
        .body(Body::from(data))
        .unwrap()
}

/// Compute SHA-256 hash of data
fn sha2_hash(data: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let hash = Sha256::digest(data);
    hex::encode(hash)
}

// === Batch Endpoints ===

/// Request body for find-missing endpoint
#[derive(Debug, Deserialize)]
pub struct FindMissingRequest {
    /// List of chunk hashes to check
    pub hashes: Vec<String>,
}

/// Response for find-missing endpoint
#[derive(Debug, Serialize)]
pub struct FindMissingResponse {
    /// Hashes that are missing (not in cache)
    pub missing: Vec<String>,
    /// Hashes that are present
    pub found: Vec<String>,
    /// Number of invalid hashes skipped
    pub invalid_count: usize,
}

/// POST /v1/chunks/find-missing
///
/// Check which chunks are missing from the cache.
/// Useful for clients to determine what needs to be uploaded.
pub async fn find_missing(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<FindMissingRequest>,
) -> impl IntoResponse {
    if request.hashes.len() > 10000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Too many hashes (max 10000)" })),
        )
            .into_response();
    }

    let state = state.read().await;

    let mut missing = Vec::new();
    let mut found = Vec::new();
    let mut invalid_count = 0;

    for hash in &request.hashes {
        if !is_valid_hash(hash) {
            invalid_count += 1;
            continue;
        }

        // Use Bloom filter for quick rejection
        if let Some(ref bloom) = state.bloom_filter
            && !bloom.might_contain(hash)
        {
            missing.push(hash.clone());
            continue;
        }

        // Check disk
        let path = state.chunk_cache.chunk_path(hash);
        if path.exists() {
            found.push(hash.clone());
        } else {
            missing.push(hash.clone());
        }
    }

    Json(FindMissingResponse {
        missing,
        found,
        invalid_count,
    })
    .into_response()
}

/// Request body for batch fetch endpoint
#[derive(Debug, Deserialize)]
pub struct BatchFetchRequest {
    /// List of chunk hashes to fetch
    pub hashes: Vec<String>,
}

/// POST /v1/chunks/batch
///
/// Fetch multiple chunks in a single request.
/// Returns multipart response with each chunk.
///
/// Note: This is a simplified implementation that returns JSON with base64-encoded chunks.
/// A production implementation might use proper multipart encoding.
pub async fn batch_fetch(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<BatchFetchRequest>,
) -> impl IntoResponse {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    if request.hashes.len() > 100 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Too many hashes (max 100)" })),
        )
            .into_response();
    }

    let state = state.read().await;

    #[derive(Serialize)]
    struct ChunkData {
        hash: String,
        data: String, // Base64 encoded
        size: u64,
    }

    #[derive(Serialize)]
    struct BatchResponse {
        chunks: Vec<ChunkData>,
        missing: Vec<String>,
        invalid: Vec<String>,
    }

    let mut chunks = Vec::new();
    let mut missing = Vec::new();
    let mut invalid = Vec::new();

    for hash in &request.hashes {
        if !is_valid_hash(hash) {
            invalid.push(hash.clone());
            continue;
        }

        let path = state.chunk_cache.chunk_path(hash);
        match tokio::fs::read(&path).await {
            Ok(data) => {
                state.metrics.record_hit();
                state.metrics.record_bytes_served(data.len() as u64);
                chunks.push(ChunkData {
                    hash: hash.clone(),
                    data: BASE64.encode(&data),
                    size: data.len() as u64,
                });
            }
            Err(_) => {
                missing.push(hash.clone());
            }
        }
    }

    Json(BatchResponse {
        chunks,
        missing,
        invalid,
    })
    .into_response()
}

// === Admin/Stats Endpoints ===

/// GET /v1/admin/cache/stats
///
/// Get cache statistics
pub async fn cache_stats(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> impl IntoResponse {
    let state = state.read().await;

    #[derive(Serialize)]
    struct CacheStatsResponse {
        cache: crate::server::cache::CacheStats,
        #[serde(skip_serializing_if = "Option::is_none")]
        bloom: Option<crate::server::bloom::BloomStats>,
        metrics: crate::server::metrics::MetricsSnapshot,
    }

    match state.chunk_cache.stats().await {
        Ok(cache_stats) => {
            let bloom_stats = state.bloom_filter.as_ref().map(|b| b.stats());
            let metrics = state.metrics.snapshot();

            Json(CacheStatsResponse {
                cache: cache_stats,
                bloom: bloom_stats,
                metrics,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to get cache stats: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get stats: {}", e),
            )
                .into_response()
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
            // Mark bloom filter dirty after eviction
            if let Some(ref bloom) = state.bloom_filter {
                bloom.mark_dirty();
            }

            tracing::info!(
                "Manual eviction: {} chunks, {} freed",
                result.chunks_evicted,
                result.bytes_freed_human
            );
            Json(result).into_response()
        }
        Err(e) => {
            tracing::error!("Eviction failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Eviction failed: {}", e),
            )
                .into_response()
        }
    }
}

/// POST /v1/admin/bloom/rebuild
///
/// Rebuild the Bloom filter from disk
pub async fn rebuild_bloom(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> impl IntoResponse {
    let mut state = state.write().await;

    if state.bloom_filter.is_none() {
        return (StatusCode::BAD_REQUEST, "Bloom filter not enabled").into_response();
    }

    tracing::info!("Rebuilding Bloom filter from disk");

    // Scan chunks and rebuild
    match state.chunk_cache.stats().await {
        Ok(stats) => {
            // Create new filter sized for current chunk count (with headroom)
            let expected_count = (stats.chunk_count as f64 * 1.5) as usize;
            let new_bloom = crate::server::bloom::ChunkBloomFilter::new(
                expected_count.max(100_000),
                0.01,
            );

            // Scan and add all chunks
            let objects_dir = state.config.chunk_dir.join("objects");
            if let Ok(hashes) = scan_chunk_hashes(&objects_dir).await {
                for hash in &hashes {
                    new_bloom.add(hash);
                }
                tracing::info!(
                    "Bloom filter rebuilt with {} chunks",
                    new_bloom.count()
                );
            }

            new_bloom.mark_clean();
            state.bloom_filter = Some(Arc::new(new_bloom));

            Json(serde_json::json!({
                "status": "ok",
                "chunks_indexed": state.bloom_filter.as_ref().map(|b| b.count()).unwrap_or(0)
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to scan chunks for bloom rebuild: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to rebuild: {}", e),
            )
                .into_response()
        }
    }
}

/// Scan directory for chunk hashes
async fn scan_chunk_hashes(objects_dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let mut hashes = Vec::new();

    if !objects_dir.exists() {
        return Ok(hashes);
    }

    let mut stack = vec![objects_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = entry.metadata().await?;

            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                // Skip temp files
                if path.extension().is_some_and(|ext| ext == "tmp") {
                    continue;
                }

                // Extract hash from path
                if let Some(hash) = extract_hash_from_path(&path) {
                    hashes.push(hash);
                }
            }
        }
    }

    Ok(hashes)
}

/// Extract hash from chunk path (e.g., /chunks/objects/ab/cdef... -> abcdef...)
fn extract_hash_from_path(path: &std::path::Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let prefix = parent.file_name()?.to_str()?;
    Some(format!("{}{}", prefix, file_name))
}
