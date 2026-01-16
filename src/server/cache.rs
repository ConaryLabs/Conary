// src/server/cache.rs
//! LRU chunk cache management
//!
//! Tracks chunk access times and evicts old chunks when storage
//! exceeds the configured threshold.

use crate::server::ServerState;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Chunk cache manager
#[derive(Clone)]
pub struct ChunkCache {
    /// Root directory for chunk storage
    chunk_dir: PathBuf,
    /// Maximum cache size in bytes
    max_bytes: u64,
    /// Chunk TTL in days (chunks not accessed in this period are candidates for eviction)
    /// Reserved for future LRU eviction implementation
    #[allow(dead_code)]
    ttl_days: u32,
}

impl ChunkCache {
    pub fn new(chunk_dir: PathBuf, max_bytes: u64, ttl_days: u32) -> Self {
        Self {
            chunk_dir,
            max_bytes,
            ttl_days,
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
    pub async fn record_access(&self, hash: &str) -> Result<()> {
        // Update atime on the file (or use a separate tracking mechanism)
        // For now, just touch the file's mtime
        let path = self.chunk_path(hash);
        if path.exists() {
            // Use filetime crate or just rewrite metadata
            // For simplicity, we'll rely on atime if mounted with relatime
            // In production, we'd use a database table for tracking
            let _ = tokio::fs::File::open(&path).await?;
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

    /// Run LRU eviction
    ///
    /// Returns (chunks_evicted, bytes_freed)
    pub async fn run_eviction(&self) -> Result<(usize, u64)> {
        tracing::info!("Starting LRU eviction check");

        // Get current cache size
        let (total_size, chunks) = self.scan_chunks().await?;

        if total_size <= self.max_bytes {
            tracing::info!(
                "Cache size {} bytes is within limit {} bytes, no eviction needed",
                total_size,
                self.max_bytes
            );
            return Ok((0, 0));
        }

        let bytes_to_free = total_size - self.max_bytes;
        tracing::info!(
            "Cache size {} bytes exceeds limit {} bytes, need to free {} bytes",
            total_size,
            self.max_bytes,
            bytes_to_free
        );

        // Sort chunks by access time (oldest first)
        let mut chunks_with_time: Vec<_> = chunks
            .into_iter()
            .filter_map(|(path, size)| {
                let metadata = std::fs::metadata(&path).ok()?;
                let accessed = metadata.accessed().ok()?;
                Some((path, size, accessed))
            })
            .collect();

        chunks_with_time.sort_by_key(|(_, _, time)| *time);

        // Evict oldest chunks until we're under the limit
        let mut freed = 0u64;
        let mut evicted = 0usize;

        for (path, size, _) in chunks_with_time {
            if freed >= bytes_to_free {
                break;
            }

            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    freed += size;
                    evicted += 1;
                    tracing::debug!("Evicted chunk: {:?} ({} bytes)", path, size);
                }
                Err(e) => {
                    tracing::warn!("Failed to evict chunk {:?}: {}", path, e);
                }
            }
        }

        tracing::info!("Eviction complete: {} chunks, {} bytes freed", evicted, freed);
        Ok((evicted, freed))
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
                    let size = metadata.len();
                    total_size += size;
                    chunks.push((path, size));
                }
            }
        }

        Ok((total_size, chunks))
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
            if let Err(e) = state_guard.chunk_cache.run_eviction().await {
                tracing::error!("Eviction loop error: {}", e);
            }
        }

        // Clean up expired jobs
        {
            let mut state_guard = state.write().await;
            state_guard.job_manager.cleanup_expired();
        }
    }
}
