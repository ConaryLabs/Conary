// src/server/bloom.rs
//! In-memory Bloom filter for chunk existence checks
//!
//! Protects the chunk endpoint from DoS via random hash probes.
//! A Bloom filter definitively says "not present" (no disk I/O needed)
//! but may have false positives (requires disk check).
//!
//! Trade-off: ~1.2MB memory for 1M chunks at 1% false positive rate.

use parking_lot::RwLock;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

/// Bloom filter for fast negative lookups
pub struct ChunkBloomFilter {
    /// Bit array (using u64 chunks for efficiency)
    bits: Vec<AtomicU64>,
    /// Number of bits in the filter
    num_bits: usize,
    /// Number of hash functions
    num_hashes: usize,
    /// Count of items added
    count: AtomicU64,
    /// Whether the filter needs rebuilding
    dirty: RwLock<bool>,
}

impl ChunkBloomFilter {
    /// Create a new Bloom filter sized for expected items with target false positive rate
    ///
    /// # Arguments
    /// * `expected_items` - Expected number of chunks to store
    /// * `false_positive_rate` - Target false positive rate (e.g., 0.01 for 1%)
    pub fn new(expected_items: usize, false_positive_rate: f64) -> Self {
        // Calculate optimal size: m = -n * ln(p) / (ln(2)^2)
        let ln2_squared = std::f64::consts::LN_2 * std::f64::consts::LN_2;
        let num_bits = (-(expected_items as f64) * false_positive_rate.ln() / ln2_squared).ceil() as usize;

        // Calculate optimal number of hash functions: k = (m/n) * ln(2)
        let num_hashes = ((num_bits as f64 / expected_items as f64) * std::f64::consts::LN_2).ceil() as usize;
        let num_hashes = num_hashes.clamp(1, 16); // Reasonable bounds

        // Round up to nearest u64 boundary
        let num_u64s = num_bits.div_ceil(64);
        let actual_bits = num_u64s * 64;

        tracing::info!(
            "Bloom filter: {} bits ({:.2} MB) for {} items, {} hashes, {:.2}% target FP rate",
            actual_bits,
            (num_u64s * 8) as f64 / 1024.0 / 1024.0,
            expected_items,
            num_hashes,
            false_positive_rate * 100.0
        );

        Self {
            bits: (0..num_u64s).map(|_| AtomicU64::new(0)).collect(),
            num_bits: actual_bits,
            num_hashes,
            count: AtomicU64::new(0),
            dirty: RwLock::new(false),
        }
    }

    /// Create with reasonable defaults (1M chunks, 1% FP rate)
    pub fn default_sized() -> Self {
        Self::new(1_000_000, 0.01)
    }

    /// Add a chunk hash to the filter
    pub fn add(&self, hash: &str) {
        for i in 0..self.num_hashes {
            let bit_index = self.hash_to_index(hash, i);
            let word_index = bit_index / 64;
            let bit_offset = bit_index % 64;

            if word_index < self.bits.len() {
                self.bits[word_index].fetch_or(1 << bit_offset, Ordering::Relaxed);
            }
        }
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Check if a chunk hash might be present
    ///
    /// Returns:
    /// - `false`: Definitely not present (safe to return 404 immediately)
    /// - `true`: Might be present (need to check disk)
    pub fn might_contain(&self, hash: &str) -> bool {
        for i in 0..self.num_hashes {
            let bit_index = self.hash_to_index(hash, i);
            let word_index = bit_index / 64;
            let bit_offset = bit_index % 64;

            if word_index >= self.bits.len() {
                return false;
            }

            let word = self.bits[word_index].load(Ordering::Relaxed);
            if (word & (1 << bit_offset)) == 0 {
                return false; // Definitely not present
            }
        }
        true // Might be present
    }

    /// Get the number of items added
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Clear the filter (for rebuild)
    pub fn clear(&self) {
        for word in &self.bits {
            word.store(0, Ordering::Relaxed);
        }
        self.count.store(0, Ordering::Relaxed);
    }

    /// Mark the filter as needing rebuild
    pub fn mark_dirty(&self) {
        *self.dirty.write() = true;
    }

    /// Check if filter needs rebuild
    pub fn is_dirty(&self) -> bool {
        *self.dirty.read()
    }

    /// Mark filter as clean after rebuild
    pub fn mark_clean(&self) {
        *self.dirty.write() = false;
    }

    /// Calculate bit index for a hash using double hashing
    fn hash_to_index(&self, hash: &str, k: usize) -> usize {
        // Use double hashing: h(k, i) = (h1 + i * h2) mod m
        let mut hasher1 = DefaultHasher::new();
        hash.hash(&mut hasher1);
        let h1 = hasher1.finish() as usize;

        let mut hasher2 = DefaultHasher::new();
        // Salt the second hash
        (hash, 0x517cc1b727220a95u64).hash(&mut hasher2);
        let h2 = hasher2.finish() as usize;

        (h1.wrapping_add(k.wrapping_mul(h2))) % self.num_bits
    }

    /// Estimated false positive rate based on current fill
    pub fn estimated_fp_rate(&self) -> f64 {
        let n = self.count.load(Ordering::Relaxed) as f64;
        let m = self.num_bits as f64;
        let k = self.num_hashes as f64;

        // FP rate = (1 - e^(-k*n/m))^k
        (1.0 - (-k * n / m).exp()).powf(k)
    }
}

impl Default for ChunkBloomFilter {
    fn default() -> Self {
        Self::default_sized()
    }
}

/// Statistics about the bloom filter
#[derive(Debug, Clone, serde::Serialize)]
pub struct BloomStats {
    /// Number of chunks in the filter
    pub count: u64,
    /// Size in bytes
    pub size_bytes: usize,
    /// Estimated false positive rate
    pub estimated_fp_rate: f64,
    /// Number of hash functions
    pub num_hashes: usize,
    /// Total bits
    pub num_bits: usize,
    /// Whether filter needs rebuild
    pub needs_rebuild: bool,
}

impl ChunkBloomFilter {
    /// Get filter statistics
    pub fn stats(&self) -> BloomStats {
        BloomStats {
            count: self.count(),
            size_bytes: self.bits.len() * 8,
            estimated_fp_rate: self.estimated_fp_rate(),
            num_hashes: self.num_hashes,
            num_bits: self.num_bits,
            needs_rebuild: self.is_dirty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_filter_basic() {
        let filter = ChunkBloomFilter::new(1000, 0.01);

        // Add some hashes
        filter.add("abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890");
        filter.add("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef");

        // Check presence
        assert!(filter.might_contain("abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"));
        assert!(filter.might_contain("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"));

        // Check absence (high probability)
        assert!(!filter.might_contain("0000000000000000000000000000000000000000000000000000000000000000"));

        assert_eq!(filter.count(), 2);
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let filter = ChunkBloomFilter::new(10000, 0.01);

        // Add 10000 items
        for i in 0..10000 {
            filter.add(&format!("{:064x}", i));
        }

        // Check false positives on items not added
        let mut false_positives = 0;
        for i in 10000..20000 {
            if filter.might_contain(&format!("{:064x}", i)) {
                false_positives += 1;
            }
        }

        // Should be around 1% (100 out of 10000), allow some variance
        let fp_rate = false_positives as f64 / 10000.0;
        assert!(fp_rate < 0.03, "False positive rate too high: {}", fp_rate);
    }

    #[test]
    fn test_bloom_filter_clear() {
        let filter = ChunkBloomFilter::new(100, 0.01);
        filter.add("test_hash_1234567890123456789012345678901234567890123456789012");
        assert_eq!(filter.count(), 1);

        filter.clear();
        assert_eq!(filter.count(), 0);
        assert!(!filter.might_contain("test_hash_1234567890123456789012345678901234567890123456789012"));
    }

    #[test]
    fn test_default_sizing() {
        let filter = ChunkBloomFilter::default_sized();
        // Should be sized for 1M items at 1% FP rate
        // ~9.6 million bits = ~1.2MB
        assert!(filter.num_bits >= 9_000_000);
        assert!(filter.bits.len() * 8 >= 1_000_000); // At least 1MB
    }
}
