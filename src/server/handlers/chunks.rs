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
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
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

/// Parse HTTP Range header
/// Returns (start, end) if valid, None otherwise
/// Only supports single byte ranges like "bytes=0-1023"
fn parse_range_header(range_header: &str, file_size: u64) -> Option<(u64, u64)> {
    // Must start with "bytes="
    let range = range_header.strip_prefix("bytes=")?;

    // We only support a single range (not multiple ranges)
    if range.contains(',') {
        return None;
    }

    let parts: Vec<&str> = range.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start: u64;
    let end: u64;

    if parts[0].is_empty() {
        // Suffix range: "-500" means last 500 bytes
        let suffix_len: u64 = parts[1].parse().ok()?;
        if suffix_len == 0 || suffix_len > file_size {
            return None;
        }
        start = file_size - suffix_len;
        end = file_size - 1;
    } else if parts[1].is_empty() {
        // Open-ended range: "500-" means from byte 500 to end
        start = parts[0].parse().ok()?;
        if start >= file_size {
            return None;
        }
        end = file_size - 1;
    } else {
        // Closed range: "0-499"
        start = parts[0].parse().ok()?;
        end = parts[1].parse().ok()?;
        if start > end || start >= file_size {
            return None;
        }
        // Clamp end to file size
        let end = end.min(file_size - 1);
        return Some((start, end));
    }

    Some((start, end))
}

/// GET /v1/chunks/:hash
///
/// Serves a chunk by its content hash. Returns:
/// - 200 OK with chunk data and immutable cache headers
/// - 206 Partial Content for Range requests
/// - 416 Range Not Satisfiable for invalid ranges
/// - 404 Not Found if chunk doesn't exist
pub async fn get_chunk(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(hash): Path<String>,
    headers: HeaderMap,
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
            return pull_through_fetch(state, &hash, None).await;
        }

        return (StatusCode::NOT_FOUND, "Chunk not found").into_response();
    }

    let chunk_path = state_guard.chunk_cache.chunk_path(&hash);

    // Check if chunk exists locally
    if !chunk_path.exists() {
        // Try pull-through if configured
        if state_guard.config.upstream_url.is_some() {
            drop(state_guard);
            return pull_through_fetch(state, &hash, None).await;
        }
        state_guard.metrics.record_miss();
        return (StatusCode::NOT_FOUND, "Chunk not found").into_response();
    }

    // Open file for streaming
    let mut file = match File::open(&chunk_path).await {
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

    let file_size = metadata.len();

    // Update access time for LRU tracking (fire and forget)
    let hash_clone = hash.clone();
    let cache = state_guard.chunk_cache.clone();
    tokio::spawn(async move {
        if let Err(e) = cache.record_access(&hash_clone).await {
            tracing::warn!("Failed to record chunk access: {}", e);
        }
    });

    // Check for Range header
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    if let Some(range_str) = range_header {
        // Parse range
        let range = match parse_range_header(range_str, file_size) {
            Some(r) => r,
            None => {
                // Invalid range - return 416 Range Not Satisfiable
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{}", file_size))
                    .body(Body::empty())
                    .unwrap();
            }
        };

        let (start, end) = range;
        let content_length = end - start + 1;

        // Seek to start position
        if let Err(e) = file.seek(std::io::SeekFrom::Start(start)).await {
            tracing::error!("Failed to seek in chunk {}: {}", hash, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read chunk").into_response();
        }

        // Read the range
        let mut buffer = vec![0u8; content_length as usize];
        if let Err(e) = file.read_exact(&mut buffer).await {
            tracing::error!("Failed to read chunk range {}: {}", hash, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read chunk").into_response();
        }

        state_guard.metrics.record_hit();
        state_guard.metrics.record_bytes_served(content_length);

        tracing::debug!(
            "Range request for chunk {}: bytes {}-{}/{}",
            hash, start, end, file_size
        );

        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, content_length)
            .header(
                header::CONTENT_RANGE,
                format!("bytes {}-{}/{}", start, end, file_size),
            )
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            .header(header::ETAG, format!("\"{}\"", hash))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::from(buffer))
            .unwrap();
    }

    // No Range header - serve full content
    state_guard.metrics.record_hit();
    state_guard.metrics.record_bytes_served(file_size);

    // Stream the file
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, file_size)
        // Immutable cache - chunks never change (content-addressed)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ETAG, format!("\"{}\"", hash))
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .unwrap()
}

/// Pull-through caching: fetch from upstream and store locally
/// Optional range parameter for Range request passthrough (not currently used)
async fn pull_through_fetch(
    state: Arc<RwLock<ServerState>>,
    hash: &str,
    _range: Option<(u64, u64)>,
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
    /// Response format: "multipart" (default, efficient) or "json" (legacy, base64)
    #[serde(default)]
    pub format: Option<String>,
}

/// POST /v1/chunks/batch
///
/// Fetch multiple chunks in a single request.
/// Returns multipart response by default for efficiency.
///
/// Response formats:
/// - `multipart` (default): Efficient binary transfer with multipart/mixed
/// - `json`: Legacy JSON with base64-encoded chunks (for compatibility)
///
/// Multipart format:
/// ```text
/// Content-Type: multipart/mixed; boundary=chunk-boundary
/// --chunk-boundary
/// X-Chunk-Hash: abc123...
/// Content-Length: 65536
/// <raw bytes>
/// --chunk-boundary
/// X-Chunk-Hash: def456...
/// Content-Length: 32768
/// <raw bytes>
/// --chunk-boundary--
/// ```
pub async fn batch_fetch(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<BatchFetchRequest>,
) -> impl IntoResponse {
    const MAX_BATCH_SIZE: usize = 100;
    const BOUNDARY: &str = "chunk-boundary-7f3e9a2b";

    if request.hashes.len() > MAX_BATCH_SIZE {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Too many hashes (max {})", MAX_BATCH_SIZE) })),
        )
            .into_response();
    }

    let format = request.format.as_deref().unwrap_or("multipart");
    let state = state.read().await;

    // Collect chunk data
    let mut chunks_data: Vec<(String, Vec<u8>)> = Vec::new();
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
                chunks_data.push((hash.clone(), data));
            }
            Err(_) => {
                missing.push(hash.clone());
            }
        }
    }

    // Return JSON format if requested
    if format == "json" {
        use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

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

        let chunks: Vec<ChunkData> = chunks_data
            .into_iter()
            .map(|(hash, data)| ChunkData {
                size: data.len() as u64,
                data: BASE64.encode(&data),
                hash,
            })
            .collect();

        return Json(BatchResponse {
            chunks,
            missing,
            invalid,
        })
        .into_response();
    }

    // Build multipart response
    let mut body_parts: Vec<u8> = Vec::new();

    // Add metadata header as first part (JSON with missing/invalid info)
    if !missing.is_empty() || !invalid.is_empty() {
        body_parts.extend_from_slice(format!("--{}\r\n", BOUNDARY).as_bytes());
        body_parts.extend_from_slice(b"Content-Type: application/json\r\n");
        body_parts.extend_from_slice(b"X-Part-Type: metadata\r\n\r\n");
        let metadata = serde_json::json!({
            "missing": missing,
            "invalid": invalid,
        });
        body_parts.extend_from_slice(metadata.to_string().as_bytes());
        body_parts.extend_from_slice(b"\r\n");
    }

    // Add each chunk as a binary part
    for (hash, data) in chunks_data {
        body_parts.extend_from_slice(format!("--{}\r\n", BOUNDARY).as_bytes());
        body_parts.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
        body_parts.extend_from_slice(format!("X-Chunk-Hash: {}\r\n", hash).as_bytes());
        body_parts.extend_from_slice(format!("Content-Length: {}\r\n\r\n", data.len()).as_bytes());
        body_parts.extend_from_slice(&data);
        body_parts.extend_from_slice(b"\r\n");
    }

    // End boundary
    body_parts.extend_from_slice(format!("--{}--\r\n", BOUNDARY).as_bytes());

    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/mixed; boundary={}", BOUNDARY),
        )
        .header(header::CONTENT_LENGTH, body_parts.len())
        .body(Body::from(body_parts))
        .unwrap()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_header_closed_range() {
        // bytes=0-499 (first 500 bytes)
        let result = parse_range_header("bytes=0-499", 1000);
        assert_eq!(result, Some((0, 499)));

        // bytes=500-999 (last 500 bytes)
        let result = parse_range_header("bytes=500-999", 1000);
        assert_eq!(result, Some((500, 999)));

        // bytes=0-0 (first byte only)
        let result = parse_range_header("bytes=0-0", 1000);
        assert_eq!(result, Some((0, 0)));
    }

    #[test]
    fn test_parse_range_header_open_ended() {
        // bytes=500- (from byte 500 to end)
        let result = parse_range_header("bytes=500-", 1000);
        assert_eq!(result, Some((500, 999)));

        // bytes=0- (entire file)
        let result = parse_range_header("bytes=0-", 1000);
        assert_eq!(result, Some((0, 999)));
    }

    #[test]
    fn test_parse_range_header_suffix() {
        // bytes=-500 (last 500 bytes)
        let result = parse_range_header("bytes=-500", 1000);
        assert_eq!(result, Some((500, 999)));

        // bytes=-1 (last byte)
        let result = parse_range_header("bytes=-1", 1000);
        assert_eq!(result, Some((999, 999)));

        // bytes=-1000 (entire file via suffix)
        let result = parse_range_header("bytes=-1000", 1000);
        assert_eq!(result, Some((0, 999)));
    }

    #[test]
    fn test_parse_range_header_clamp_to_file_size() {
        // bytes=0-9999 should clamp end to file size - 1
        let result = parse_range_header("bytes=0-9999", 1000);
        assert_eq!(result, Some((0, 999)));
    }

    #[test]
    fn test_parse_range_header_invalid() {
        // No bytes= prefix
        assert!(parse_range_header("0-499", 1000).is_none());

        // Start > end
        assert!(parse_range_header("bytes=500-100", 1000).is_none());

        // Start >= file_size
        assert!(parse_range_header("bytes=1000-", 1000).is_none());
        assert!(parse_range_header("bytes=1001-", 1000).is_none());

        // Suffix larger than file
        assert!(parse_range_header("bytes=-1001", 1000).is_none());

        // Zero suffix
        assert!(parse_range_header("bytes=-0", 1000).is_none());

        // Multiple ranges (not supported)
        assert!(parse_range_header("bytes=0-100,200-300", 1000).is_none());

        // Invalid format
        assert!(parse_range_header("bytes=abc-def", 1000).is_none());
        assert!(parse_range_header("bytes=", 1000).is_none());
        assert!(parse_range_header("bytes=-", 1000).is_none());
    }

    #[test]
    fn test_is_valid_hash() {
        // Valid 64-char hex hash
        assert!(is_valid_hash("abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"));

        // Wrong length
        assert!(!is_valid_hash("abcdef"));
        assert!(!is_valid_hash(""));

        // Invalid characters
        assert!(!is_valid_hash("ghijklmnopqrstuv1234567890abcdef1234567890abcdef1234567890abcd"));
        assert!(!is_valid_hash("ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF12345678901")); // too long
    }

    #[test]
    fn test_extract_hash_from_path() {
        use std::path::Path;

        let path = Path::new("/chunks/objects/ab/cdef1234");
        assert_eq!(extract_hash_from_path(path), Some("abcdef1234".to_string()));

        let path = Path::new("/ab/cdef");
        assert_eq!(extract_hash_from_path(path), Some("abcdef".to_string()));
    }
}
