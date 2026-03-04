// src/server/cache.rs
//! LRU chunk cache management
//!
//! Tracks chunk access times and evicts old chunks when storage
//! exceeds the configured threshold or chunks exceed TTL.
//!
//! Uses a database-backed LRU index (chunk_access table) for O(1) stats
//! and efficient eviction queries, replacing legacy mtime-based scanning.

use crate::db::models::ChunkAccess;
use crate::server::ServerState;
use anyhow::Result;
use serde::Serialize;
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
    ttl_days: u32,
    /// Path to database for tracking access
    db_path: PathBuf,
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
    /// Number of chunks skipped (protected/error)
    pub chunks_skipped: usize,
}

impl ChunkCache {
    pub fn new(chunk_dir: PathBuf, max_bytes: u64, ttl_days: u32, db_path: PathBuf) -> Self {
        Self {
            chunk_dir,
            max_bytes,
            ttl_days,
            db_path,
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
    /// Updates the `last_accessed` timestamp and `access_count` in the database.
    pub async fn record_access(&self, hash: &str) -> Result<()> {
        let db_path = self.db_path.clone();
        let hash = hash.to_string();

        // Run DB operation in blocking task since rusqlite is sync
        tokio::task::spawn_blocking(move || {
            let conn = crate::db::open(&db_path)?;
            ChunkAccess::record_access(&conn, &hash)?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;

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

        // Update DB record
        let db_path = self.db_path.clone();
        let hash_owned = hash.to_string();
        let size = data.len() as i64;

        tokio::task::spawn_blocking(move || {
            let conn = crate::db::open(&db_path)?;
            let chunk = ChunkAccess::new(hash_owned, size);
            chunk.upsert(&conn)?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(path)
    }

    /// Check if a chunk exists
    pub async fn has_chunk(&self, hash: &str) -> bool {
        self.chunk_path(hash).exists()
    }

    /// Protect a set of chunk hashes from eviction
    pub async fn protect_chunks(&self, hashes: &[String]) {
        let db_path = self.db_path.clone();
        let hashes = hashes.to_vec();

        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = crate::db::open(&db_path) {
                let _ = ChunkAccess::protect_chunks(&conn, &hashes);
            }
        })
        .await
        .ok();
    }

    /// Remove protection from chunk hashes
    pub async fn unprotect_chunks(&self, hashes: &[String]) {
        let db_path = self.db_path.clone();
        let hashes = hashes.to_vec();

        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = crate::db::open(&db_path) {
                let _ = ChunkAccess::unprotect_chunks(&conn, &hashes);
            }
        })
        .await
        .ok();
    }

    /// Get cache statistics
    pub async fn stats(&self) -> Result<CacheStats> {
        let db_path = self.db_path.clone();

        let stats = tokio::task::spawn_blocking(move || {
            let conn = crate::db::open(&db_path)?;
            ChunkAccess::get_stats(&conn)
        })
        .await??;

        let usage_percent = if self.max_bytes > 0 {
            (stats.total_bytes as f64 / self.max_bytes as f64) * 100.0
        } else {
            0.0
        };

        // TODO: Efficiently get stale stats from DB without full scan?
        // For now reporting 0 for stale to keep this fast O(1)
        // A full stale count would require `SELECT COUNT(*) WHERE last_accessed < ...`
        // which is fast enough with index, let's add it if needed.

        Ok(CacheStats {
            total_bytes: stats.total_bytes,
            total_size_human: human_bytes(stats.total_bytes),
            max_bytes: self.max_bytes,
            max_size_human: human_bytes(self.max_bytes),
            chunk_count: stats.total_chunks,
            usage_percent,
            stale_chunks: 0, // Not querying this to keep stats extremely fast
            stale_bytes: 0,
            protected_chunks: stats.protected_chunks,
            ttl_days: self.ttl_days,
        })
    }

    /// Run LRU eviction
    ///
    /// Evicts chunks based on two criteria:
    /// 1. Stale chunks: older than ttl_days
    /// 2. Size limit: if cache > max_bytes, evict oldest chunks until under limit
    ///
    /// Uses DB index for efficient candidate selection.
    pub async fn run_eviction(&self) -> Result<EvictionResult> {
        tracing::info!("Starting DB-backed LRU eviction check");

        let db_path = self.db_path.clone();
        let max_bytes = self.max_bytes;
        let ttl_days = self.ttl_days;
        let self_clone = self.clone();

        tokio::task::spawn_blocking(move || {
            let conn = crate::db::open(&db_path)?;

            let mut freed = 0u64;
            let mut evicted = 0usize;
            let mut skipped = 0usize;
            let mut reason = String::new();

            // Phase 1: Evict stale chunks
            // Calculate cutoff time
            let now = std::time::SystemTime::now();
            let cutoff = now - Duration::from_secs(ttl_days as u64 * 24 * 3600);
            let datetime: chrono::DateTime<chrono::Utc> = cutoff.into();
            let cutoff_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();

            let stale_chunks = ChunkAccess::get_stale_chunks(&conn, &cutoff_str)?;

            if !stale_chunks.is_empty() {
                reason = format!(
                    "TTL eviction ({} chunks older than {} days)",
                    stale_chunks.len(),
                    ttl_days
                );

                for chunk in stale_chunks {
                    // Delete file first
                    let path = self_clone.chunk_path(&chunk.hash);
                    if path.exists()
                        && let Err(e) = std::fs::remove_file(&path)
                    {
                        tracing::warn!("Failed to delete chunk file {}: {}", chunk.hash, e);
                        skipped += 1;
                        continue;
                    }

                    // Delete from DB
                    if let Err(e) = ChunkAccess::delete(&conn, &chunk.hash) {
                        tracing::warn!("Failed to delete chunk db record {}: {}", chunk.hash, e);
                        // If file is gone but DB record remains, it's a "ghost" record.
                        // Ideally we should handle this, but for now just warn.
                    } else {
                        freed += chunk.size_bytes as u64;
                        evicted += 1;
                    }
                }
            }

            // Phase 2: Size-based eviction
            let stats = ChunkAccess::get_stats(&conn)?;
            let current_size = stats.total_bytes;

            if current_size > max_bytes {
                let bytes_to_free = current_size - max_bytes;
                tracing::info!(
                    "Cache size {} exceeds limit {}, need to free {}",
                    human_bytes(current_size),
                    human_bytes(max_bytes),
                    human_bytes(bytes_to_free)
                );

                let size_reason = format!(
                    "Size limit exceeded (need {} freed)",
                    human_bytes(bytes_to_free)
                );
                if reason.is_empty() {
                    reason = size_reason;
                } else {
                    reason = format!("{}; {}", reason, size_reason);
                }

                // Get LRU chunks - fetch enough to cover the deficit + buffer
                // Estimate count based on avg chunk size (say 64KB)
                let avg_size = if stats.total_chunks > 0 {
                    current_size / stats.total_chunks as u64
                } else {
                    65536
                };
                let chunks_needed = (bytes_to_free / avg_size) as usize + 100;

                let lru_chunks = ChunkAccess::get_lru_chunks(&conn, chunks_needed)?;
                let mut size_freed_phase2 = 0u64;

                for chunk in lru_chunks {
                    if size_freed_phase2 >= bytes_to_free {
                        break;
                    }

                    let path = self_clone.chunk_path(&chunk.hash);
                    if path.exists()
                        && let Err(e) = std::fs::remove_file(&path)
                    {
                        tracing::warn!("Failed to delete chunk file {}: {}", chunk.hash, e);
                        skipped += 1;
                        continue;
                    }

                    if let Err(e) = ChunkAccess::delete(&conn, &chunk.hash) {
                        tracing::warn!("Failed to delete chunk db record {}: {}", chunk.hash, e);
                    } else {
                        size_freed_phase2 += chunk.size_bytes as u64;
                        freed += chunk.size_bytes as u64;
                        evicted += 1;
                    }
                }
            }

            if evicted == 0 && skipped == 0 {
                reason = "No eviction needed".to_string();
            }

            Ok(EvictionResult {
                chunks_evicted: evicted,
                bytes_freed: freed,
                bytes_freed_human: human_bytes(freed),
                reason,
                chunks_skipped: skipped,
            })
        })
        .await?
    }
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
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> NamedTempFile {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        temp_file
    }

    #[tokio::test]
    async fn test_store_and_stats() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let data = b"test chunk data";

        cache.store_chunk(hash, data).await.unwrap();

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.chunk_count, 1);
        assert_eq!(stats.total_bytes, data.len() as u64);
    }

    #[tokio::test]
    async fn test_store_and_retrieve_chunk() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        let hash = "aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899";
        let data = b"hello world chunk content";

        let stored_path = cache.store_chunk(hash, data).await.unwrap();

        // Verify the file exists at the returned path
        assert!(stored_path.exists());

        // Verify we can read back the exact data
        let read_back = tokio::fs::read(&stored_path).await.unwrap();
        assert_eq!(read_back, data);
    }

    #[tokio::test]
    async fn test_has_chunk_hit_and_miss() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        let hash = "1122334455667788990011223344556677889900aabbccddeeff001122334455";

        // Before storing - cache miss
        assert!(!cache.has_chunk(hash).await);

        // After storing - cache hit
        cache.store_chunk(hash, b"chunk data").await.unwrap();
        assert!(cache.has_chunk(hash).await);
    }

    #[test]
    fn test_chunk_path_structure() {
        let cache = ChunkCache::new(
            PathBuf::from("/data/chunks"),
            1024 * 1024,
            30,
            PathBuf::from("/data/db.sqlite"),
        );

        let hash = "abcdef1234567890";
        let path = cache.chunk_path(hash);

        // Should use 2-character prefix directory
        assert_eq!(
            path,
            PathBuf::from("/data/chunks/objects/ab/cdef1234567890")
        );
    }

    #[test]
    fn test_chunk_path_short_hash() {
        let cache = ChunkCache::new(
            PathBuf::from("/data/chunks"),
            1024 * 1024,
            30,
            PathBuf::from("/data/db.sqlite"),
        );

        // Very short hash (edge case)
        let hash = "ab";
        let path = cache.chunk_path(hash);
        assert_eq!(path, PathBuf::from("/data/chunks/objects/ab/"));
    }

    #[tokio::test]
    async fn test_store_multiple_chunks_stats() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        // Store three chunks of different sizes
        cache
            .store_chunk(
                "aaaa000000000000000000000000000000000000000000000000000000000001",
                &[0u8; 1000],
            )
            .await
            .unwrap();
        cache
            .store_chunk(
                "bbbb000000000000000000000000000000000000000000000000000000000002",
                &[0u8; 2000],
            )
            .await
            .unwrap();
        cache
            .store_chunk(
                "cccc000000000000000000000000000000000000000000000000000000000003",
                &[0u8; 3000],
            )
            .await
            .unwrap();

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.chunk_count, 3);
        assert_eq!(stats.total_bytes, 6000);
    }

    #[tokio::test]
    async fn test_stats_usage_percent() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let max_bytes = 10000u64;
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            max_bytes,
            30,
            db_file.path().to_path_buf(),
        );

        // Store 5000 bytes => 50% usage
        cache
            .store_chunk(
                "dd00000000000000000000000000000000000000000000000000000000000001",
                &[0u8; 5000],
            )
            .await
            .unwrap();

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.max_bytes, max_bytes);
        assert!((stats.usage_percent - 50.0).abs() < 0.1);
        assert_eq!(stats.ttl_days, 30);
    }

    #[tokio::test]
    async fn test_stats_zero_max_bytes() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            0, // zero max
            30,
            db_file.path().to_path_buf(),
        );

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.usage_percent, 0.0);
    }

    #[tokio::test]
    async fn test_protect_and_unprotect_chunks() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        let hash = "ee00000000000000000000000000000000000000000000000000000000000001";
        cache.store_chunk(hash, b"protected data").await.unwrap();

        // Protect the chunk
        cache.protect_chunks(&[hash.to_string()]).await;

        // Verify it's protected via stats
        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.protected_chunks, 1);

        // Unprotect
        cache.unprotect_chunks(&[hash.to_string()]).await;

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.protected_chunks, 0);
    }

    #[tokio::test]
    async fn test_record_access_updates_count() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        let hash = "ff00000000000000000000000000000000000000000000000000000000000001";
        cache.store_chunk(hash, b"access test").await.unwrap();

        // Record additional accesses
        cache.record_access(hash).await.unwrap();
        cache.record_access(hash).await.unwrap();

        // Verify via DB directly (access_count should be 3: 1 from store + 2 from record_access)
        let conn = rusqlite::Connection::open(db_file.path()).unwrap();
        let found = ChunkAccess::find_by_hash(&conn, hash).unwrap().unwrap();
        assert_eq!(found.access_count, 3);
    }

    #[tokio::test]
    async fn test_eviction_no_eviction_needed() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024, // 1MB max
            30,
            db_file.path().to_path_buf(),
        );

        // Store a small chunk - well within limits
        cache
            .store_chunk(
                "1100000000000000000000000000000000000000000000000000000000000001",
                b"small chunk",
            )
            .await
            .unwrap();

        let result = cache.run_eviction().await.unwrap();
        assert_eq!(result.chunks_evicted, 0);
        assert_eq!(result.bytes_freed, 0);
        assert_eq!(result.reason, "No eviction needed");
    }

    #[tokio::test]
    async fn test_eviction_size_based() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            500, // Very small max: 500 bytes
            365, // Long TTL so we don't trigger TTL eviction
            db_file.path().to_path_buf(),
        );

        // Store chunks totaling more than 500 bytes
        cache
            .store_chunk(
                "2200000000000000000000000000000000000000000000000000000000000001",
                &[0u8; 200],
            )
            .await
            .unwrap();
        cache
            .store_chunk(
                "3300000000000000000000000000000000000000000000000000000000000002",
                &[0u8; 200],
            )
            .await
            .unwrap();
        cache
            .store_chunk(
                "4400000000000000000000000000000000000000000000000000000000000003",
                &[0u8; 200],
            )
            .await
            .unwrap();

        // Total: 600 bytes, max: 500 bytes => should evict at least 100 bytes
        let result = cache.run_eviction().await.unwrap();
        assert!(result.chunks_evicted > 0);
        assert!(result.bytes_freed > 0);
        assert!(result.reason.contains("Size limit exceeded"));
    }

    #[tokio::test]
    async fn test_eviction_skips_protected_chunks() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            300, // Very small max
            365,
            db_file.path().to_path_buf(),
        );

        let hash1 = "5500000000000000000000000000000000000000000000000000000000000001";
        let hash2 = "6600000000000000000000000000000000000000000000000000000000000002";

        cache.store_chunk(hash1, &[0u8; 200]).await.unwrap();
        cache.store_chunk(hash2, &[0u8; 200]).await.unwrap();

        // Protect one chunk
        cache.protect_chunks(&[hash1.to_string()]).await;

        // Total: 400 bytes, max: 300 bytes
        let _result = cache.run_eviction().await.unwrap();

        // The protected chunk should still exist
        assert!(cache.has_chunk(hash1).await);
    }

    #[tokio::test]
    async fn test_store_chunk_atomic_write() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        let hash = "7700000000000000000000000000000000000000000000000000000000000001";
        let data = b"atomic write test data";

        cache.store_chunk(hash, data).await.unwrap();

        // Verify no .tmp file is left behind (atomic rename should clean up)
        let chunk_path = cache.chunk_path(hash);
        let tmp_path = chunk_path.with_extension("tmp");
        assert!(!tmp_path.exists());
        assert!(chunk_path.exists());
    }

    #[tokio::test]
    async fn test_store_overwrite_existing_chunk() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            30,
            db_file.path().to_path_buf(),
        );

        let hash = "8800000000000000000000000000000000000000000000000000000000000001";

        // Store same hash twice with different data (content-addressed should overwrite)
        cache.store_chunk(hash, b"first version").await.unwrap();
        cache.store_chunk(hash, b"second version").await.unwrap();

        // Read back - should have the second version
        let chunk_path = cache.chunk_path(hash);
        let content = tokio::fs::read(&chunk_path).await.unwrap();
        assert_eq!(content, b"second version");
    }

    // --- human_bytes tests ---

    #[test]
    fn test_human_bytes_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1023), "1023 B");
    }

    #[test]
    fn test_human_bytes_kb() {
        assert_eq!(human_bytes(1024), "1.00 KB");
        assert_eq!(human_bytes(1536), "1.50 KB");
    }

    #[test]
    fn test_human_bytes_mb() {
        assert_eq!(human_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.00 MB");
    }

    #[test]
    fn test_human_bytes_gb() {
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(human_bytes(700 * 1024 * 1024 * 1024), "700.00 GB");
    }

    #[test]
    fn test_human_bytes_tb() {
        assert_eq!(human_bytes(1024 * 1024 * 1024 * 1024), "1.00 TB");
    }

    #[tokio::test]
    async fn test_stats_human_readable_fields() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let max_bytes = 1024 * 1024; // 1MB
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            max_bytes,
            30,
            db_file.path().to_path_buf(),
        );

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.total_size_human, "0 B");
        assert_eq!(stats.max_size_human, "1.00 MB");
        assert_eq!(stats.chunk_count, 0);
    }
}
