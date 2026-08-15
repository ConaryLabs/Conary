// apps/remi/src/server/cache.rs
//! Local chunk cache storage and access bookkeeping.
//!
//! Eviction is owned by `bounded_cache`, which verifies durable R2 presence
//! before removing any local object.

use crate::server::ServerState;
use crate::server::handlers::human_bytes;
use anyhow::Result;
use conary_core::db::models::ChunkAccess;
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
    /// Number of protected chunks (immune to eviction)
    pub protected_chunks: usize,
}

impl ChunkCache {
    pub fn new(chunk_dir: PathBuf, max_bytes: u64, db_path: PathBuf) -> Self {
        Self {
            chunk_dir,
            max_bytes,
            db_path,
        }
    }

    pub(crate) fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub(crate) fn db_path(&self) -> &std::path::Path {
        &self.db_path
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
            let conn = crate::server::open_runtime_db(&db_path)?;
            ChunkAccess::record_access(&conn, &hash)?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }

    /// Store a chunk, verifying its content hash matches the expected hash.
    pub async fn store_chunk(&self, hash: &str, data: &[u8]) -> Result<PathBuf> {
        // Verify content hash before writing to prevent storage of corrupted data
        let computed = conary_core::hash::sha256(data);
        anyhow::ensure!(
            computed == hash,
            "chunk hash mismatch: expected {hash}, got {computed}"
        );

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
            let conn = crate::server::open_runtime_db(&db_path)?;
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
            if let Ok(conn) = crate::server::open_runtime_db(&db_path) {
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
            if let Ok(conn) = crate::server::open_runtime_db(&db_path) {
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
            let conn = crate::server::open_runtime_db(&db_path)?;
            ChunkAccess::get_stats(&conn)
        })
        .await??;

        let usage_percent = if self.max_bytes > 0 {
            (stats.total_bytes as f64 / self.max_bytes as f64) * 100.0
        } else {
            0.0
        };

        Ok(CacheStats {
            total_bytes: stats.total_bytes,
            total_size_human: human_bytes(stats.total_bytes),
            max_bytes: self.max_bytes,
            max_size_human: human_bytes(self.max_bytes),
            chunk_count: stats.total_chunks,
            usage_percent,
            protected_chunks: stats.protected_chunks,
        })
    }
}

/// Background eviction loop
///
/// Runs every hour to enforce the configured local cache bound.
pub async fn run_eviction_loop(state: Arc<RwLock<ServerState>>) {
    let interval = Duration::from_secs(3600); // 1 hour

    loop {
        tokio::time::sleep(interval).await;

        let (bounded_cache, r2_store, bloom_filter) = {
            let state_guard = state.read().await;
            (
                state_guard.bounded_cache.clone(),
                state_guard.r2_store.clone(),
                state_guard.bloom_filter.clone(),
            )
        };
        if let Some(r2_store) = r2_store {
            match bounded_cache.enforce(r2_store.as_ref()).await {
                Ok(result) => {
                    if result.objects_evicted > 0 {
                        if let Some(bloom) = bloom_filter {
                            bloom.mark_dirty();
                        }
                        tracing::info!(
                            "Scheduled bounded eviction: {} chunks, {} bytes freed",
                            result.objects_evicted,
                            result.bytes_freed
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("Bounded cache enforcement error: {}", e);
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
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    fn test_hash(data: &[u8]) -> String {
        conary_core::hash::sha256(data)
    }

    fn create_test_db() -> NamedTempFile {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        temp_file
    }

    #[tokio::test]
    async fn test_store_and_stats() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            db_file.path().to_path_buf(),
        );

        let data = b"test chunk data";
        let hash = test_hash(data);

        cache.store_chunk(&hash, data).await.unwrap();

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
            db_file.path().to_path_buf(),
        );

        let data = b"hello world chunk content";
        let hash = test_hash(data);

        let stored_path = cache.store_chunk(&hash, data).await.unwrap();

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
            db_file.path().to_path_buf(),
        );

        let data = b"chunk data";
        let hash = test_hash(data);

        // Before storing - cache miss
        assert!(!cache.has_chunk(&hash).await);

        // After storing - cache hit
        cache.store_chunk(&hash, data).await.unwrap();
        assert!(cache.has_chunk(&hash).await);
    }

    #[test]
    fn test_chunk_path_structure() {
        let cache = ChunkCache::new(
            PathBuf::from("/data/chunks"),
            1024 * 1024,
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
            db_file.path().to_path_buf(),
        );

        // Store three chunks of different sizes
        let chunk_a = vec![0u8; 1000];
        let chunk_b = vec![0u8; 2000];
        let chunk_c = vec![0u8; 3000];
        cache
            .store_chunk(&test_hash(&chunk_a), &chunk_a)
            .await
            .unwrap();
        cache
            .store_chunk(&test_hash(&chunk_b), &chunk_b)
            .await
            .unwrap();
        cache
            .store_chunk(&test_hash(&chunk_c), &chunk_c)
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
            db_file.path().to_path_buf(),
        );

        // Store 5000 bytes => 50% usage
        let data = vec![0u8; 5000];
        cache.store_chunk(&test_hash(&data), &data).await.unwrap();

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.max_bytes, max_bytes);
        assert!((stats.usage_percent - 50.0).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_stats_zero_max_bytes() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            0, // zero max
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
            db_file.path().to_path_buf(),
        );

        let data = b"protected data";
        let hash = test_hash(data);
        cache.store_chunk(&hash, data).await.unwrap();

        // Protect the chunk
        cache.protect_chunks(std::slice::from_ref(&hash)).await;

        // Verify it's protected via stats
        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.protected_chunks, 1);

        // Unprotect
        cache.unprotect_chunks(std::slice::from_ref(&hash)).await;

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
            db_file.path().to_path_buf(),
        );

        let data = b"access test";
        let hash = test_hash(data);
        cache.store_chunk(&hash, data).await.unwrap();

        // Record additional accesses
        cache.record_access(&hash).await.unwrap();
        cache.record_access(&hash).await.unwrap();

        // Verify via DB directly (access_count should be 3: 1 from store + 2 from record_access)
        let conn = rusqlite::Connection::open(db_file.path()).unwrap();
        let found = ChunkAccess::find_by_hash(&conn, &hash).unwrap().unwrap();
        assert_eq!(found.access_count, 3);
    }

    #[tokio::test]
    async fn test_store_chunk_atomic_write() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            db_file.path().to_path_buf(),
        );

        let data = b"atomic write test data";
        let hash = test_hash(data);

        cache.store_chunk(&hash, data).await.unwrap();

        // Verify no .tmp file is left behind (atomic rename should clean up)
        let chunk_path = cache.chunk_path(&hash);
        let tmp_path = chunk_path.with_extension("tmp");
        assert!(!tmp_path.exists());
        assert!(chunk_path.exists());
    }

    #[tokio::test]
    async fn test_store_chunk_rejects_hash_mismatch() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024,
            db_file.path().to_path_buf(),
        );

        let original = b"first version";
        let hash = test_hash(original);

        cache.store_chunk(&hash, original).await.unwrap();

        let err = cache
            .store_chunk(&hash, b"second version")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("chunk hash mismatch"));

        // The original chunk should remain unchanged.
        let chunk_path = cache.chunk_path(&hash);
        let content = tokio::fs::read(&chunk_path).await.unwrap();
        assert_eq!(content, original);
    }

    #[tokio::test]
    async fn test_stats_human_readable_fields() {
        let db_file = create_test_db();
        let temp_dir = tempfile::TempDir::new().unwrap();
        let max_bytes = 1024 * 1024; // 1MB
        let cache = ChunkCache::new(
            temp_dir.path().to_path_buf(),
            max_bytes,
            db_file.path().to_path_buf(),
        );

        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.total_size_human, "0 B");
        assert_eq!(stats.max_size_human, "1.00 MB");
        assert_eq!(stats.chunk_count, 0);
    }
}
