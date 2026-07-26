// apps/remi/src/server/handlers/chunks/admin.rs

use super::*;

// === Admin/Stats Endpoints ===

/// GET /v1/admin/cache/stats
///
/// Get cache statistics
pub async fn cache_stats(State(state): State<Arc<RwLock<ServerState>>>) -> impl IntoResponse {
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
pub async fn trigger_eviction(State(state): State<Arc<RwLock<ServerState>>>) -> impl IntoResponse {
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

            state.publish_event(
                "cache",
                serde_json::json!({
                    "action": "eviction_triggered",
                }),
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
pub async fn rebuild_bloom(State(state): State<Arc<RwLock<ServerState>>>) -> impl IntoResponse {
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
            let new_bloom =
                crate::server::bloom::ChunkBloomFilter::new(expected_count.max(100_000), 0.01);

            // Scan and add all chunks
            let objects_dir = state.config.chunk_dir.join("objects");
            if let Ok(hashes) = scan_chunk_hashes(&objects_dir).await {
                for hash in &hashes {
                    new_bloom.add(hash);
                }
                tracing::info!("Bloom filter rebuilt with {} chunks", new_bloom.count());
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
///
/// NOTE: This function is also called from `server/mod.rs` (Bloom filter init)
/// and from `rebuild_bloom_filter` in this module. If similar scanning logic
/// is needed elsewhere, reuse this function rather than duplicating the walk.
// TODO: Consider moving to a shared `chunk_store` module if more callers appear.
pub(crate) async fn scan_chunk_hashes(
    objects_dir: &std::path::Path,
) -> std::io::Result<Vec<String>> {
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
pub(crate) fn extract_hash_from_path(path: &std::path::Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let prefix = parent.file_name()?.to_str()?;
    Some(format!("{}{}", prefix, file_name))
}
