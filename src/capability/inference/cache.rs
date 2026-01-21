// src/capability/inference/cache.rs
//! Inference result caching
//!
//! Caches capability inference results by file content hash to avoid
//! re-analyzing packages with identical content. This is particularly
//! useful when multiple packages share common files or when re-converting
//! the same package.

use super::InferredCapabilities;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Maximum number of cached entries
const DEFAULT_MAX_ENTRIES: usize = 1000;

/// Default TTL for cache entries (1 hour)
const DEFAULT_TTL_SECS: u64 = 3600;

/// Cached inference entry
#[derive(Clone)]
struct CacheEntry {
    capabilities: InferredCapabilities,
    created_at: Instant,
    hits: u32,
}

/// Thread-safe inference cache
pub struct InferenceCache {
    entries: Arc<RwLock<HashMap<String, CacheEntry>>>,
    max_entries: usize,
    ttl: Duration,
}

impl Default for InferenceCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, Duration::from_secs(DEFAULT_TTL_SECS))
    }
}

impl InferenceCache {
    /// Create a new cache with specified limits
    pub fn new(max_entries: usize, ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::with_capacity(max_entries / 2))),
            max_entries,
            ttl,
        }
    }

    /// Create a cache with no TTL (entries don't expire)
    pub fn no_expiry(max_entries: usize) -> Self {
        Self::new(max_entries, Duration::MAX)
    }

    /// Compute a cache key from package content
    ///
    /// The key is a SHA256 hash of:
    /// - Package name
    /// - Package version
    /// - All file content hashes (sorted for consistency)
    pub fn compute_key(
        package_name: &str,
        package_version: &str,
        file_hashes: &[&str],
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(package_name.as_bytes());
        hasher.update(b"|");
        hasher.update(package_version.as_bytes());
        hasher.update(b"|");

        // Sort hashes for consistent key generation
        let mut sorted_hashes: Vec<_> = file_hashes.to_vec();
        sorted_hashes.sort();
        for hash in sorted_hashes {
            hasher.update(hash.as_bytes());
            hasher.update(b",");
        }

        format!("{:x}", hasher.finalize())
    }

    /// Get cached inference result
    pub fn get(&self, key: &str) -> Option<InferredCapabilities> {
        let mut entries = self.entries.write().ok()?;

        if let Some(entry) = entries.get_mut(key) {
            // Check TTL
            if entry.created_at.elapsed() > self.ttl {
                entries.remove(key);
                return None;
            }

            entry.hits += 1;
            return Some(entry.capabilities.clone());
        }

        None
    }

    /// Store inference result in cache
    pub fn put(&self, key: String, capabilities: InferredCapabilities) {
        if let Ok(mut entries) = self.entries.write() {
            // Evict if at capacity
            if entries.len() >= self.max_entries {
                self.evict_lru(&mut entries);
            }

            entries.insert(
                key,
                CacheEntry {
                    capabilities,
                    created_at: Instant::now(),
                    hits: 0,
                },
            );
        }
    }

    /// Get or compute inference result
    ///
    /// If the key is in cache, returns cached result.
    /// Otherwise, calls the provided function to compute the result and caches it.
    pub fn get_or_compute<F>(&self, key: &str, compute: F) -> InferredCapabilities
    where
        F: FnOnce() -> InferredCapabilities,
    {
        if let Some(cached) = self.get(key) {
            return cached;
        }

        let result = compute();
        self.put(key.to_string(), result.clone());
        result
    }

    /// Evict least recently used entry
    fn evict_lru(&self, entries: &mut HashMap<String, CacheEntry>) {
        // Find entry with lowest hit count and oldest creation time
        if let Some((key, _)) = entries
            .iter()
            .min_by_key(|(_, e)| (e.hits, std::cmp::Reverse(e.created_at)))
            .map(|(k, v)| (k.clone(), v.clone()))
        {
            entries.remove(&key);
        }
    }

    /// Remove expired entries
    pub fn cleanup_expired(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.retain(|_, entry| entry.created_at.elapsed() <= self.ttl);
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        if let Ok(entries) = self.entries.read() {
            let total_hits: u32 = entries.values().map(|e| e.hits).sum();
            CacheStats {
                entries: entries.len(),
                max_entries: self.max_entries,
                total_hits,
            }
        } else {
            CacheStats::default()
        }
    }

    /// Clear all cache entries
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
    }
}

/// Cache statistics
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    pub entries: usize,
    pub max_entries: usize,
    pub total_hits: u32,
}

/// Global inference cache instance
static GLOBAL_CACHE: std::sync::LazyLock<InferenceCache> =
    std::sync::LazyLock::new(InferenceCache::default);

/// Get the global inference cache
pub fn global_cache() -> &'static InferenceCache {
    &GLOBAL_CACHE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::inference::{Confidence, ConfidenceScore, InferenceSource};

    fn make_test_caps() -> InferredCapabilities {
        InferredCapabilities {
            confidence: ConfidenceScore::new(Confidence::High),
            source: InferenceSource::WellKnown,
            tier_used: 1,
            rationale: "test".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_cache_put_get() {
        let cache = InferenceCache::default();
        let caps = make_test_caps();

        cache.put("test-key".to_string(), caps.clone());

        let retrieved = cache.get("test-key").unwrap();
        assert_eq!(retrieved.source, InferenceSource::WellKnown);
    }

    #[test]
    fn test_cache_miss() {
        let cache = InferenceCache::default();
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_compute_key() {
        let key1 = InferenceCache::compute_key("nginx", "1.0.0", &["hash1", "hash2"]);
        let key2 = InferenceCache::compute_key("nginx", "1.0.0", &["hash2", "hash1"]);
        // Order shouldn't matter
        assert_eq!(key1, key2);

        let key3 = InferenceCache::compute_key("nginx", "1.0.1", &["hash1", "hash2"]);
        // Different version = different key
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_get_or_compute() {
        let cache = InferenceCache::default();
        let mut computed = false;

        let result = cache.get_or_compute("new-key", || {
            computed = true;
            make_test_caps()
        });

        assert!(computed);
        assert_eq!(result.source, InferenceSource::WellKnown);

        // Second call should use cache
        computed = false;
        let result2 = cache.get_or_compute("new-key", || {
            computed = true;
            make_test_caps()
        });

        assert!(!computed); // Should not have computed again
        assert_eq!(result2.source, InferenceSource::WellKnown);
    }

    #[test]
    fn test_cache_eviction() {
        let cache = InferenceCache::new(3, Duration::from_secs(3600));

        cache.put("key1".to_string(), make_test_caps());
        cache.put("key2".to_string(), make_test_caps());
        cache.put("key3".to_string(), make_test_caps());

        // Access key1 to increase its hit count
        cache.get("key1");
        cache.get("key1");

        // Adding a 4th should evict key2 or key3 (lowest hits)
        cache.put("key4".to_string(), make_test_caps());

        assert!(cache.get("key1").is_some()); // Should still be there (higher hits)
        assert!(cache.get("key4").is_some()); // Newly added

        // Either key2 or key3 should be evicted
        let key2_exists = cache.get("key2").is_some();
        let key3_exists = cache.get("key3").is_some();
        assert!(
            !key2_exists || !key3_exists,
            "One of key2 or key3 should have been evicted"
        );
    }

    #[test]
    fn test_cache_stats() {
        let cache = InferenceCache::default();

        cache.put("key1".to_string(), make_test_caps());
        cache.get("key1");
        cache.get("key1");

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.total_hits, 2);
    }

    #[test]
    fn test_global_cache() {
        let cache = global_cache();
        let stats = cache.stats();
        assert!(stats.max_entries > 0);
    }
}
