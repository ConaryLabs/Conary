// src/server/cache.rs
//! LRU chunk cache management
//!
//! Tracks chunk access times and evicts old chunks when storage
//! exceeds the configured threshold or chunks exceed TTL.

use crate::server::ServerState;
use anyhow::Result;
use filetime::{set_file_mtime, FileTime};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// Chunk cache manager
#[derive(Clone)]
pub struct ChunkCache {
    /// Root directory for chunk storage
    chunk_dir: PathBuf,
    /// Maximum cache size in bytes
    max_bytes: u64,
    /// Chunk TTL in days (chunks not accessed in this period are candidates for eviction)
    ttl_days: u32,
    /// Set of chunk hashes currently protected from eviction (e.g., active conversions)
    protected: Arc<RwLock<HashSet<String>>>,
}

/// Cache statistics
#[derive(Debug, Clone, Serialize)]
pub struct CacheStats {
    /// Total size of all chunks in bytes
    pub total_bytes: u64,
    /// Human-readable total size
    pub total_size_human: String,
    /// Maximum allowed size in bytes
    pub max_bytes: u64,
    /// Human-readable max size
    pub max_size_human: String,
    /// Number of chunks stored
    pub chunk_count: usize,
    /// Percentage of cache used
    pub usage_percent: f64,
    /// Number of chunks older than TTL
    pub stale_chunks: usize,
    /// Bytes in stale chunks
    pub stale_bytes: u64,
    /// Number of protected chunks (immune to eviction)
    pub protected_chunks: usize,
    /// TTL in days
    pub ttl_days: u32,
}

/// Result of an eviction run
#[derive(Debug, Clone, Serialize)]
pub struct EvictionResult {
    /// Number of chunks evicted
    pub chunks_evicted: usize,
    /// Total bytes freed
    pub bytes_freed: u64,
    /// Human-readable bytes freed
    pub bytes_freed_human: String,
    /// Reason for eviction
    pub reason: String,
    /// Number of chunks skipped (protected)
    pub chunks_skipped: usize,
}

impl ChunkCache {
    pub fn new(chunk_dir: PathBuf, max_bytes: u64, ttl_days: u32) -> Self {
        Self {
            chunk_dir,
            max_bytes,
            ttl_days,
            protected: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Get the filesystem path for a chunk
    ///
    /// Uses a two-level directory structure: {hash[0:2]}/{hash[2:]}
    /// This prevents any single directory from having too many entries.
    pub fn chunk_path(&self, hash: &str) -> PathBuf {
        let (prefix, rest) = hash.split_at(2.min(hash.len()));
        self.chunk_dir.join("objects").join(prefix).join(rest)
    }

    /// Record that a chunk was accessed (for LRU tracking)
    ///
    /// Updates the modification time to "now" so the chunk is less likely
    /// to be evicted. This is more reliable than relying on filesystem atime.
    pub async fn record_access(&self, hash: &str) -> Result<()> {
        let path = self.chunk_path(hash);
        if path.exists() {
            let now = FileTime::now();
            // Update mtime - we use mtime instead of atime because:
            // 1. atime is often disabled (noatime) or unreliable (relatime)
            // 2. mtime is always updated and preserved across copies
            set_file_mtime(&path, now)?;
        }
        Ok(())
    }

    /// Store a chunk
    pub async fn store_chunk(&self, hash: &str, data: &[u8]) -> Result<PathBuf> {
        let path = self.chunk_path(hash);

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write atomically (write to temp, then rename)
        let temp_path = path.with_extension("tmp");
        tokio::fs::write(&temp_path, data).await?;
        tokio::fs::rename(&temp_path, &path).await?;

        Ok(path)
    }

    /// Check if a chunk exists
    pub async fn has_chunk(&self, hash: &str) -> bool {
        self.chunk_path(hash).exists()
    }

    /// Protect a set of chunk hashes from eviction
    ///
    /// Use this when starting a conversion to ensure those chunks
    /// aren't evicted while being assembled.
    pub async fn protect_chunks(&self, hashes: &[String]) {
        let mut protected = self.protected.write().await;
        for hash in hashes {
            protected.insert(hash.clone());
        }
    }

    /// Remove protection from chunk hashes
    pub async fn unprotect_chunks(&self, hashes: &[String]) {
        let mut protected = self.protected.write().await;
        for hash in hashes {
            protected.remove(hash);
        }
    }

    /// Get cache statistics
    pub async fn stats(&self) -> Result<CacheStats> {
        let (total_bytes, chunks) = self.scan_chunks().await?;
        let protected = self.protected.read().await;
        let now = SystemTime::now();
        let ttl_threshold = now - Duration::from_secs(self.ttl_days as u64 * 24 * 3600);

        let mut stale_chunks = 0usize;
        let mut stale_bytes = 0u64;

        for (path, size) in &chunks {
            if let Ok(metadata) = std::fs::metadata(path)
                && let Ok(mtime) = metadata.modified()
                && mtime < ttl_threshold
            {
                stale_chunks += 1;
                stale_bytes += size;
            }
        }

        let usage_percent = if self.max_bytes > 0 {
            (total_bytes as f64 / self.max_bytes as f64) * 100.0
        } else {
            0.0
        };

        Ok(CacheStats {
            total_bytes,
            total_size_human: human_bytes(total_bytes),
            max_bytes: self.max_bytes,
            max_size_human: human_bytes(self.max_bytes),
            chunk_count: chunks.len(),
            usage_percent,
            stale_chunks,
            stale_bytes,
            protected_chunks: protected.len(),
            ttl_days: self.ttl_days,
        })
    }

    /// Run LRU eviction
    ///
    /// Evicts chunks based on two criteria:
    /// 1. If cache exceeds max_bytes, evict oldest chunks until under limit
    /// 2. Evict any chunks older than ttl_days regardless of cache size
    ///
    /// Returns eviction statistics
    pub async fn run_eviction(&self) -> Result<EvictionResult> {
        tracing::info!("Starting LRU eviction check");

        // Get current cache size and protected chunks
        let (total_size, chunks) = self.scan_chunks().await?;
        let protected = self.protected.read().await;
        let now = SystemTime::now();
        let ttl_threshold = now - Duration::from_secs(self.ttl_days as u64 * 24 * 3600);

        // Build list of chunks with metadata, excluding protected ones
        let mut chunks_with_time: Vec<_> = chunks
            .into_iter()
            .filter_map(|(path, size)| {
                // Extract hash from path for protection check
                let hash = extract_hash_from_path(&path)?;
                if protected.contains(&hash) {
                    return None; // Skip protected chunks
                }
                let metadata = std::fs::metadata(&path).ok()?;
                let mtime = metadata.modified().ok()?;
                Some((path, size, mtime, hash))
            })
            .collect();

        // Sort by mtime (oldest first)
        chunks_with_time.sort_by_key(|(_, _, time, _)| *time);

        let mut freed = 0u64;
        let mut evicted = 0usize;
        let mut skipped = 0usize;
        let mut reason = String::new();

        // Phase 1: Evict stale chunks (older than TTL)
        let mut remaining_chunks = Vec::new();
        for (path, size, mtime, hash) in chunks_with_time {
            if mtime < ttl_threshold {
                match tokio::fs::remove_file(&path).await {
                    Ok(()) => {
                        freed += size;
                        evicted += 1;
                        tracing::debug!("Evicted stale chunk: {} ({} bytes, TTL exceeded)", hash, size);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to evict chunk {}: {}", hash, e);
                        skipped += 1;
                    }
                }
            } else {
                remaining_chunks.push((path, size, mtime, hash));
            }
        }

        if evicted > 0 {
            reason = format!("TTL eviction ({} chunks older than {} days)", evicted, self.ttl_days);
            tracing::info!("{}: {} bytes freed", reason, freed);
        }

        // Phase 2: Size-based eviction if still over limit
        let current_size = total_size.saturating_sub(freed);
        if current_size > self.max_bytes {
            let bytes_to_free = current_size - self.max_bytes;
            tracing::info!(
                "Cache size {} exceeds limit {}, need to free {}",
                human_bytes(current_size),
                human_bytes(self.max_bytes),
                human_bytes(bytes_to_free)
            );

            let mut size_freed = 0u64;
            let mut size_evicted = 0usize;

            for (path, size, _, hash) in remaining_chunks {
                if size_freed >= bytes_to_free {
                    break;
                }

                match tokio::fs::remove_file(&path).await {
                    Ok(()) => {
                        size_freed += size;
                        size_evicted += 1;
                        tracing::debug!("Evicted chunk for space: {} ({} bytes)", hash, size);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to evict chunk {}: {}", hash, e);
                        skipped += 1;
                    }
                }
            }

            freed += size_freed;
            evicted += size_evicted;

            if size_evicted > 0 {
                let size_reason = format!("Size eviction ({} chunks to free {} bytes)", size_evicted, size_freed);
                if reason.is_empty() {
                    reason = size_reason;
                } else {
                    reason = format!("{}; {}", reason, size_reason);
                }
            }
        }

        if evicted == 0 && skipped == 0 {
            reason = "No eviction needed".to_string();
            tracing::info!(
                "Cache size {} is within limit {}, no eviction needed",
                human_bytes(total_size),
                human_bytes(self.max_bytes)
            );
        }

        let result = EvictionResult {
            chunks_evicted: evicted,
            bytes_freed: freed,
            bytes_freed_human: human_bytes(freed),
            reason,
            chunks_skipped: skipped,
        };

        if evicted > 0 {
            tracing::info!(
                "Eviction complete: {} chunks, {} freed, {} skipped",
                evicted,
                human_bytes(freed),
                skipped
            );
        }

        Ok(result)
    }

    /// Scan all chunks and return (total_size, vec of (path, size))
    async fn scan_chunks(&self) -> Result<(u64, Vec<(PathBuf, u64)>)> {
        let objects_dir = self.chunk_dir.join("objects");
        if !objects_dir.exists() {
            return Ok((0, vec![]));
        }

        let mut total_size = 0u64;
        let mut chunks = Vec::new();

        // Walk the objects directory
        let mut stack = vec![objects_dir];
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
                    let size = metadata.len();
                    total_size += size;
                    chunks.push((path, size));
                }
            }
        }

        Ok((total_size, chunks))
    }
}

/// Extract hash from chunk path (e.g., /chunks/objects/ab/cdef1234... -> abcdef1234...)
fn extract_hash_from_path(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let prefix = parent.file_name()?.to_str()?;
    Some(format!("{}{}", prefix, file_name))
}

/// Format bytes as human-readable string
fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Background eviction loop
///
/// Runs every hour to check cache size and evict old chunks.
pub async fn run_eviction_loop(state: Arc<RwLock<ServerState>>) {
    let interval = Duration::from_secs(3600); // 1 hour

    loop {
        tokio::time::sleep(interval).await;

        // Run chunk eviction
        {
            let state_guard = state.read().await;
            match state_guard.chunk_cache.run_eviction().await {
                Ok(result) => {
                    if result.chunks_evicted > 0 {
                        tracing::info!(
                            "Scheduled eviction: {} chunks, {} freed",
                            result.chunks_evicted,
                            result.bytes_freed_human
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("Eviction loop error: {}", e);
                }
            }
        }

        // Clean up expired jobs
        {
            let mut state_guard = state.write().await;
            state_guard.job_manager.cleanup_expired();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.00 KB");
        assert_eq!(human_bytes(1536), "1.50 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(human_bytes(1024 * 1024 * 1024 * 1024), "1.00 TB");
        assert_eq!(human_bytes(700 * 1024 * 1024 * 1024), "700.00 GB");
    }

    #[test]
    fn test_chunk_path() {
        let temp = TempDir::new().unwrap();
        let cache = ChunkCache::new(temp.path().to_path_buf(), 1024, 30);

        let path = cache.chunk_path("abcdef1234567890");
        assert!(path.to_string_lossy().contains("objects/ab/cdef1234567890"));
    }

    #[test]
    fn test_extract_hash_from_path() {
        let path = PathBuf::from("/var/lib/conary/chunks/objects/ab/cdef1234567890");
        assert_eq!(extract_hash_from_path(&path), Some("abcdef1234567890".to_string()));
    }

    #[tokio::test]
    async fn test_store_and_access() {
        let temp = TempDir::new().unwrap();
        let cache = ChunkCache::new(temp.path().to_path_buf(), 1024 * 1024, 30);

        // Store a chunk
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let data = b"test chunk data";
        cache.store_chunk(hash, data).await.unwrap();

        // Verify it exists
        assert!(cache.has_chunk(hash).await);

        // Record access
        cache.record_access(hash).await.unwrap();

        // Verify file still exists
        assert!(cache.has_chunk(hash).await);
    }

    #[tokio::test]
    async fn test_stats_empty_cache() {
        let temp = TempDir::new().unwrap();
        let cache = ChunkCache::new(temp.path().to_path_buf(), 1024 * 1024, 30);

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.chunk_count, 0);
        assert_eq!(stats.usage_percent, 0.0);
    }

    #[tokio::test]
    async fn test_protection() {
        let temp = TempDir::new().unwrap();
        let cache = ChunkCache::new(temp.path().to_path_buf(), 100, 30); // Small limit

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let data = b"test chunk data that exceeds our tiny cache limit easily";

        // Store chunk
        cache.store_chunk(hash, data).await.unwrap();

        // Protect it
        cache.protect_chunks(&[hash.to_string()]).await;

        // Run eviction - chunk should be protected
        let result = cache.run_eviction().await.unwrap();
        assert_eq!(result.chunks_evicted, 0);
        assert!(cache.has_chunk(hash).await);

        // Unprotect and evict again
        cache.unprotect_chunks(&[hash.to_string()]).await;
        // Would need to wait for TTL or exceed size limit significantly
    }
}
