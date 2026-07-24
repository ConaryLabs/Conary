// apps/remi/src/server/handlers/chunks.rs
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
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

/// Maximum size for a single range request (64 MB)
/// Prevents OOM from malicious Range headers requesting the entire file into memory.
const MAX_RANGE_SIZE: u64 = 64 * 1024 * 1024;

/// Validate chunk hash format (64 lowercase hex chars for SHA-256).
///
/// Only lowercase hex is accepted to match the CAS on-disk format and avoid
/// ambiguity between "ABCD..." and "abcd..." referring to the same chunk.
pub(crate) fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Normalize a hash to lowercase for consistent CAS path lookup.
///
/// Callers that receive hashes from external sources (e.g., OCI digests)
/// should normalize before passing to CAS operations.
pub(crate) fn normalize_hash(hash: &str) -> String {
    hash.to_ascii_lowercase()
}

/// Build a chunk response with standard immutable-cache headers.
///
/// Every chunk response shares the same CONTENT_TYPE, CACHE_CONTROL, ETAG,
/// and ACCEPT_RANGES headers. Callers only need to add status-specific
/// headers (Content-Length, Content-Range) and the body.
fn chunk_response_builder(hash: &str, status: StatusCode) -> axum::http::response::Builder {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ETAG, format!("\"{}\"", hash))
        .header(header::ACCEPT_RANGES, "bytes")
}

/// Shorthand for a 404 "Chunk not found" response.
fn chunk_not_found() -> Response {
    (StatusCode::NOT_FOUND, "Chunk not found").into_response()
}

async fn chunk_allowed_by_public_gate(
    db_path: std::path::PathBuf,
    hash: String,
) -> std::result::Result<bool, Response> {
    match tokio::task::spawn_blocking(move || {
        crate::server::publication::local_chunk_servable_by_public_gate(&db_path, &hash)
    })
    .await
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::error!("Failed to check chunk publication reachability: {error}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response())
        }
        Err(error) => {
            tracing::error!("Chunk reachability task failed: {error}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response())
        }
    }
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
    let hash = normalize_hash(&hash);

    let db_path = {
        let state = state.read().await;
        state.config.db_path.clone()
    };
    match chunk_allowed_by_public_gate(db_path, hash.clone()).await {
        Ok(true) => {}
        Ok(false) => return chunk_not_found(),
        Err(response) => return response,
    }

    let state = state.read().await;

    // First check Bloom filter - definite "no" avoids disk I/O
    if let Some(ref bloom) = state.bloom_filter
        && !bloom.might_contain(&hash)
    {
        state.metrics.record_bloom_reject();
        return chunk_not_found();
    }

    // Bloom says "maybe" - check disk
    let chunk_path = state.chunk_cache.chunk_path(&hash);

    match tokio::fs::metadata(&chunk_path).await {
        Ok(metadata) => {
            state.metrics.record_hit();
            chunk_response_builder(&hash, StatusCode::OK)
                .header(header::CONTENT_LENGTH, metadata.len())
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => {
            state.metrics.record_miss();
            chunk_not_found()
        }
    }
}

/// Parse HTTP Range header
/// Returns (start, end) if valid, None otherwise
/// Only supports single byte ranges like "bytes=0-1023"
fn parse_range_header(range_header: &str, file_size: u64) -> Option<(u64, u64)> {
    let range = range_header.strip_prefix("bytes=")?;

    if range.contains(',') {
        return None;
    }

    let (left, right) = range.split_once('-')?;

    let (start, end) = if left.is_empty() {
        // Suffix range: "-500" means last 500 bytes
        let suffix_len: u64 = right.parse().ok()?;
        if suffix_len == 0 || suffix_len > file_size {
            return None;
        }
        (file_size - suffix_len, file_size - 1)
    } else if right.is_empty() {
        // Open-ended range: "500-" means from byte 500 to end
        let start: u64 = left.parse().ok()?;
        if start >= file_size {
            return None;
        }
        (start, file_size - 1)
    } else {
        // Closed range: "0-499"
        let start: u64 = left.parse().ok()?;
        let end: u64 = right.parse().ok()?;
        if start > end || start >= file_size {
            return None;
        }
        (start, end.min(file_size - 1))
    };

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
    let hash = normalize_hash(&hash);

    let db_path = {
        let state_guard = state.read().await;
        state_guard.config.db_path.clone()
    };
    match chunk_allowed_by_public_gate(db_path, hash.clone()).await {
        Ok(true) => {}
        Ok(false) => return chunk_not_found(),
        Err(response) => return response,
    }

    let state_guard = state.read().await;

    // First check Bloom filter
    if let Some(ref bloom) = state_guard.bloom_filter
        && !bloom.might_contain(&hash)
    {
        state_guard.metrics.record_bloom_reject();

        if state_guard.config.upstream_url.is_some() {
            drop(state_guard);
            return pull_through_fetch(state, &hash, None).await;
        }

        return chunk_not_found();
    }

    let chunk_path = state_guard.chunk_cache.chunk_path(&hash);

    // Check if chunk exists locally
    if !chunk_path.exists() {
        if state_guard.config.upstream_url.is_some() {
            drop(state_guard);
            return pull_through_fetch(state, &hash, None).await;
        }
        state_guard.metrics.record_miss();
        return chunk_not_found();
    }

    // R2 redirect: if enabled and not a Range request, redirect to presigned R2 URL
    let is_range_request = headers.contains_key(header::RANGE);
    if !is_range_request
        && state_guard.r2_redirect
        && let Some(ref r2_store) = state_guard.r2_store
    {
        match r2_store.presign_get(&hash, 3600).await {
            Ok(presigned_url) => {
                state_guard.metrics.record_hit();
                // Record approximate file size from metadata
                if let Ok(meta) = tokio::fs::metadata(&chunk_path).await {
                    state_guard.metrics.record_bytes_served(meta.len());
                }
                return Response::builder()
                    .status(StatusCode::TEMPORARY_REDIRECT)
                    .header(header::LOCATION, &presigned_url)
                    .header(header::CACHE_CONTROL, "public, max-age=3600")
                    .body(Body::empty())
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
            Err(e) => {
                tracing::warn!("R2 presign failed for {}, serving locally: {}", hash, e);
                // Fall through to normal local serving
            }
        }
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
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
        };

        let (start, end) = range;
        let content_length = end - start + 1;

        // Reject ranges that exceed MAX_RANGE_SIZE to prevent OOM
        if content_length > MAX_RANGE_SIZE {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{}", file_size))
                .body(Body::from(format!(
                    "Range too large ({} bytes, max {} bytes)",
                    content_length, MAX_RANGE_SIZE
                )))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }

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
            hash,
            start,
            end,
            file_size
        );

        return chunk_response_builder(&hash, StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_LENGTH, content_length)
            .header(
                header::CONTENT_RANGE,
                format!("bytes {}-{}/{}", start, end, file_size),
            )
            .body(Body::from(buffer))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    // No Range header - serve full content
    state_guard.metrics.record_hit();
    state_guard.metrics.record_bytes_served(file_size);

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    chunk_response_builder(&hash, StatusCode::OK)
        .header(header::CONTENT_LENGTH, file_size)
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Pull-through caching: fetch from upstream and store locally.
///
/// Uses request coalescing to prevent thundering herd: when multiple clients
/// request the same uncached chunk simultaneously, only the first triggers an
/// upstream fetch. Subsequent requests wait for that fetch to complete, then
/// serve the now-cached chunk from disk.
async fn pull_through_fetch(
    state: Arc<RwLock<ServerState>>,
    hash: &str,
    _range: Option<(u64, u64)>,
) -> Response {
    let state_guard = state.read().await;

    let upstream_url = match &state_guard.config.upstream_url {
        Some(url) => url.clone(),
        None => return chunk_not_found(),
    };

    let inflight = Arc::clone(&state_guard.inflight_fetches);

    // Check if another request is already fetching this chunk
    if let Some(entry) = inflight.get(hash) {
        let mut rx = entry.value().subscribe();
        drop(entry);
        drop(state_guard);

        tracing::debug!(
            "Coalescing request for chunk {} (waiting for in-flight fetch)",
            hash
        );

        // Wait for the in-flight fetch to complete (ignore send errors -- the
        // sender may have been dropped if the fetch failed, which closes the
        // channel and causes RecvError)
        let _ = rx.recv().await;

        // Now try to serve from disk (the first fetch should have stored it)
        let state_guard = state.read().await;
        let chunk_path = state_guard.chunk_cache.chunk_path(hash);
        if chunk_path.exists() {
            let file = match tokio::fs::File::open(&chunk_path).await {
                Ok(f) => f,
                Err(_) => return chunk_not_found(),
            };
            let metadata = match file.metadata().await {
                Ok(m) => m,
                Err(_) => return chunk_not_found(),
            };
            let file_size = metadata.len();
            state_guard.metrics.record_hit();
            state_guard.metrics.record_bytes_served(file_size);
            let stream = ReaderStream::new(file);
            return chunk_response_builder(hash, StatusCode::OK)
                .header(header::CONTENT_LENGTH, file_size)
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }

        // Fetch failed or chunk was not stored -- fall through to 404
        return chunk_not_found();
    }

    // First request for this chunk: register ourselves as the in-flight fetcher.
    // Use a broadcast channel so all waiters get notified when we finish.
    let (tx, _) = tokio::sync::broadcast::channel(1);
    inflight.insert(hash.to_string(), tx.clone());

    // Drop guard: if this future is cancelled (client disconnect, timeout, etc.),
    // remove the in-flight entry and notify waiters so they don't hang forever.
    struct InflightGuard {
        map: Arc<DashMap<String, tokio::sync::broadcast::Sender<()>>>,
        key: String,
        tx: tokio::sync::broadcast::Sender<()>,
        defused: bool,
    }
    impl Drop for InflightGuard {
        fn drop(&mut self) {
            if !self.defused {
                self.map.remove(&self.key);
                let _ = self.tx.send(());
            }
        }
    }
    let mut _cleanup = InflightGuard {
        map: Arc::clone(&inflight),
        key: hash.to_string(),
        tx: tx.clone(),
        defused: false,
    };

    tracing::debug!(
        "Pull-through fetch for chunk {} from {}",
        hash,
        upstream_url
    );
    state_guard.metrics.record_upstream_fetch();

    // Build upstream URL
    let fetch_url = format!("{}/v1/chunks/{}", upstream_url.trim_end_matches('/'), hash);

    // Fetch from upstream
    let client = &state_guard.http_client;
    let response = match client
        .get(&fetch_url)
        .header("accept-encoding", "identity")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to fetch chunk {} from upstream: {}", hash, e);
            return chunk_not_found();
        }
    };

    if !response.status().is_success() {
        state_guard.metrics.record_miss();
        return chunk_not_found();
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
    let computed_hash = conary_core::hash::sha256(&data);
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

    // Defuse the drop guard -- the background task will handle cleanup instead.
    _cleanup.defused = true;

    // Store in background (don't block response), then notify waiters
    let inflight_bg = Arc::clone(&inflight);
    let hash_bg = hash.to_string();
    tokio::spawn(async move {
        if let Err(e) = cache.store_chunk(&hash_owned, &data_clone).await {
            tracing::warn!("Failed to store pull-through chunk {}: {}", hash_owned, e);
        }
        // Remove from in-flight map and notify all waiting requests
        inflight_bg.remove(&hash_bg);
        let _ = tx.send(());
    });

    state_guard.metrics.record_hit();
    state_guard.metrics.record_bytes_served(data.len() as u64);

    chunk_response_builder(hash, StatusCode::OK)
        .header(header::CONTENT_LENGTH, data.len())
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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

    let (db_path, bloom_filter, chunk_cache) = {
        let state = state.read().await;
        (
            state.config.db_path.clone(),
            state.bloom_filter.clone(),
            state.chunk_cache.clone(),
        )
    };

    let mut missing = Vec::new();
    let mut found = Vec::new();
    let mut invalid_count = 0;

    for raw_hash in &request.hashes {
        if !is_valid_hash(raw_hash) {
            invalid_count += 1;
            continue;
        }
        let hash = normalize_hash(raw_hash);

        match chunk_allowed_by_public_gate(db_path.clone(), hash.clone()).await {
            Ok(true) => {}
            Ok(false) => {
                missing.push(hash);
                continue;
            }
            Err(response) => return response,
        }

        // Use Bloom filter for quick rejection
        if let Some(ref bloom) = bloom_filter
            && !bloom.might_contain(&hash)
        {
            missing.push(hash);
            continue;
        }

        // Check disk
        let path = chunk_cache.chunk_path(&hash);
        if path.exists() {
            found.push(hash);
        } else {
            missing.push(hash);
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
            Json(
                serde_json::json!({ "error": format!("Too many hashes (max {})", MAX_BATCH_SIZE) }),
            ),
        )
            .into_response();
    }

    /// Maximum aggregate response size for batch fetch (256 MB).
    const MAX_BATCH_BYTES: u64 = 256 * 1024 * 1024;

    let format = request.format.as_deref().unwrap_or("multipart");
    let (db_path, chunk_cache, metrics) = {
        let state = state.read().await;
        (
            state.config.db_path.clone(),
            state.chunk_cache.clone(),
            state.metrics.clone(),
        )
    };

    // Collect chunk data with aggregate size cap
    let mut chunks_data: Vec<(String, Vec<u8>)> = Vec::new();
    let mut missing = Vec::new();
    let mut invalid = Vec::new();
    let mut total_bytes: u64 = 0;

    for raw_hash in &request.hashes {
        if !is_valid_hash(raw_hash) {
            invalid.push(raw_hash.clone());
            continue;
        }
        let hash = normalize_hash(raw_hash);

        match chunk_allowed_by_public_gate(db_path.clone(), hash.clone()).await {
            Ok(true) => {}
            Ok(false) => {
                missing.push(hash);
                continue;
            }
            Err(response) => return response,
        }

        let path = chunk_cache.chunk_path(&hash);
        match tokio::fs::read(&path).await {
            Ok(data) => {
                total_bytes += data.len() as u64;
                if total_bytes > MAX_BATCH_BYTES {
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        Json(serde_json::json!({
                            "error": format!(
                                "Aggregate response exceeds size limit ({} MB)",
                                MAX_BATCH_BYTES / (1024 * 1024)
                            )
                        })),
                    )
                        .into_response();
                }
                metrics.record_hit();
                metrics.record_bytes_served(data.len() as u64);
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
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

mod admin;
#[cfg(test)]
use admin::extract_hash_from_path;
pub(crate) use admin::scan_chunk_hashes;
pub use admin::{cache_stats, rebuild_bloom, trigger_eviction};

#[cfg(test)]
#[path = "chunks/tests.rs"]
mod tests;
