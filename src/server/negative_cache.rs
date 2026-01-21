// src/server/negative_cache.rs
//! Negative result caching for the Remi server
//!
//! When a package isn't found upstream, we cache the "not found" result
//! to avoid repeatedly hitting upstream for the same non-existent package.
//! This provides DoS protection and reduces upstream load.

use crate::server::ServerState;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Cache entry for negative results
#[derive(Debug, Clone)]
struct NegativeEntry {
    /// When this entry was created
    created_at: Instant,
    /// Number of requests that hit this entry
    hit_count: u64,
}

/// Negative cache for "not found" responses
///
/// Caches URLs/keys that returned 404 to avoid repeatedly checking upstream.
pub struct NegativeCache {
    /// Cache entries: key -> entry
    entries: RwLock<HashMap<String, NegativeEntry>>,
    /// Time-to-live for entries
    ttl: Duration,
}

impl NegativeCache {
    /// Create a new negative cache with the given TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    /// Check if a key is in the negative cache
    ///
    /// Returns true if the key was recently marked as "not found".
    pub async fn is_negative(&self, key: &str) -> bool {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key) {
            if entry.created_at.elapsed() < self.ttl {
                return true;
            }
        }
        false
    }

    /// Check and record a hit if the key is in the cache
    ///
    /// Returns true if the key was in the cache (and still valid).
    pub async fn check_and_record_hit(&self, key: &str) -> bool {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(key) {
            if entry.created_at.elapsed() < self.ttl {
                entry.hit_count += 1;
                return true;
            } else {
                // Entry expired, remove it
                entries.remove(key);
            }
        }
        false
    }

    /// Mark a key as "not found"
    pub async fn mark_negative(&self, key: &str) {
        let mut entries = self.entries.write().await;
        entries.insert(
            key.to_string(),
            NegativeEntry {
                created_at: Instant::now(),
                hit_count: 0,
            },
        );
    }

    /// Remove a key from the negative cache (e.g., when it becomes available)
    pub async fn invalidate(&self, key: &str) {
        let mut entries = self.entries.write().await;
        entries.remove(key);
    }

    /// Get the number of entries in the cache
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Check if the cache is empty
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }

    /// Get cache statistics
    pub async fn stats(&self) -> NegativeCacheStats {
        let entries = self.entries.read().await;
        let now = Instant::now();

        let mut total_hits = 0u64;
        let mut expired_count = 0usize;
        let mut active_count = 0usize;

        for entry in entries.values() {
            total_hits += entry.hit_count;
            if entry.created_at.elapsed() < self.ttl {
                active_count += 1;
            } else {
                expired_count += 1;
            }
        }

        NegativeCacheStats {
            total_entries: entries.len(),
            active_entries: active_count,
            expired_entries: expired_count,
            total_hits,
            ttl_secs: self.ttl.as_secs(),
            checked_at: now,
        }
    }

    /// Clean up expired entries
    pub async fn cleanup(&self) -> usize {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|_, entry| entry.created_at.elapsed() < self.ttl);
        before - entries.len()
    }
}

/// Statistics for the negative cache
#[derive(Debug, Clone)]
pub struct NegativeCacheStats {
    /// Total number of entries (including expired)
    pub total_entries: usize,
    /// Number of active (non-expired) entries
    pub active_entries: usize,
    /// Number of expired entries (pending cleanup)
    pub expired_entries: usize,
    /// Total number of cache hits
    pub total_hits: u64,
    /// TTL in seconds
    pub ttl_secs: u64,
    /// When these stats were collected
    pub checked_at: Instant,
}

/// Background cleanup loop for the negative cache
///
/// Runs every 5 minutes to remove expired entries.
pub async fn run_cleanup_loop(state: Arc<RwLock<ServerState>>) {
    let interval = Duration::from_secs(5 * 60); // 5 minutes

    loop {
        tokio::time::sleep(interval).await;

        let state_guard = state.read().await;
        let removed = state_guard.negative_cache.cleanup().await;
        if removed > 0 {
            tracing::debug!("Negative cache cleanup: removed {} expired entries", removed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_negative_cache_basic() {
        let cache = NegativeCache::new(Duration::from_secs(60));

        // Not in cache initially
        assert!(!cache.is_negative("foo").await);

        // Mark as negative
        cache.mark_negative("foo").await;

        // Now in cache
        assert!(cache.is_negative("foo").await);

        // Different key not in cache
        assert!(!cache.is_negative("bar").await);
    }

    #[tokio::test]
    async fn test_negative_cache_hit_count() {
        let cache = NegativeCache::new(Duration::from_secs(60));

        cache.mark_negative("test").await;

        // Multiple hits
        assert!(cache.check_and_record_hit("test").await);
        assert!(cache.check_and_record_hit("test").await);
        assert!(cache.check_and_record_hit("test").await);

        let stats = cache.stats().await;
        assert_eq!(stats.total_hits, 3);
    }

    #[tokio::test]
    async fn test_negative_cache_invalidate() {
        let cache = NegativeCache::new(Duration::from_secs(60));

        cache.mark_negative("test").await;
        assert!(cache.is_negative("test").await);

        cache.invalidate("test").await;
        assert!(!cache.is_negative("test").await);
    }

    #[tokio::test]
    async fn test_negative_cache_expiry() {
        let cache = NegativeCache::new(Duration::from_millis(50));

        cache.mark_negative("test").await;
        assert!(cache.is_negative("test").await);

        // Wait for expiry
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be expired
        assert!(!cache.is_negative("test").await);
    }

    #[tokio::test]
    async fn test_negative_cache_cleanup() {
        let cache = NegativeCache::new(Duration::from_millis(50));

        cache.mark_negative("a").await;
        cache.mark_negative("b").await;
        cache.mark_negative("c").await;

        assert_eq!(cache.len().await, 3);

        // Wait for expiry
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cleanup
        let removed = cache.cleanup().await;
        assert_eq!(removed, 3);
        assert_eq!(cache.len().await, 0);
    }
}
