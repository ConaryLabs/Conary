// src/server/metrics.rs
//! Server metrics tracking
//!
//! Simple atomic counters for request/response statistics.
//! These can be exposed via the admin stats endpoint and optionally
//! integrated with Prometheus or other monitoring systems.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Server metrics collector
#[derive(Default)]
pub struct ServerMetrics {
    /// Total requests
    requests_total: AtomicU64,
    /// Cache hits
    hits: AtomicU64,
    /// Cache misses
    misses: AtomicU64,
    /// Bloom filter rejects (definite misses without disk I/O)
    bloom_rejects: AtomicU64,
    /// Bytes served
    bytes_served: AtomicU64,
    /// Pull-through fetches from upstream
    upstream_fetches: AtomicU64,
    /// Upstream fetch errors
    upstream_errors: AtomicU64,
    /// Server start time
    start_time: std::sync::OnceLock<Instant>,
}

impl ServerMetrics {
    /// Create new metrics collector
    pub fn new() -> Self {
        let metrics = Self::default();
        let _ = metrics.start_time.set(Instant::now());
        metrics
    }

    /// Record a cache hit
    pub fn record_hit(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss
    pub fn record_miss(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a Bloom filter rejection (avoided disk I/O)
    pub fn record_bloom_reject(&self) {
        self.bloom_rejects.fetch_add(1, Ordering::Relaxed);
    }

    /// Record bytes served
    pub fn record_bytes_served(&self, bytes: u64) {
        self.bytes_served.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record an upstream fetch attempt
    pub fn record_upstream_fetch(&self) {
        self.upstream_fetches.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an upstream fetch error
    pub fn record_upstream_error(&self) {
        self.upstream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current metrics snapshot
    pub fn snapshot(&self) -> MetricsSnapshot {
        let uptime = self
            .start_time
            .get()
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO);

        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            (hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        MetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            hits,
            misses,
            hit_rate,
            bloom_rejects: self.bloom_rejects.load(Ordering::Relaxed),
            bytes_served: self.bytes_served.load(Ordering::Relaxed),
            bytes_served_human: human_bytes(self.bytes_served.load(Ordering::Relaxed)),
            upstream_fetches: self.upstream_fetches.load(Ordering::Relaxed),
            upstream_errors: self.upstream_errors.load(Ordering::Relaxed),
            uptime_secs: uptime.as_secs(),
        }
    }

    /// Reset all counters (for testing)
    #[cfg(test)]
    pub fn reset(&self) {
        self.requests_total.store(0, Ordering::Relaxed);
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.bloom_rejects.store(0, Ordering::Relaxed);
        self.bytes_served.store(0, Ordering::Relaxed);
        self.upstream_fetches.store(0, Ordering::Relaxed);
        self.upstream_errors.store(0, Ordering::Relaxed);
    }
}

/// Snapshot of current metrics
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    /// Total requests processed
    pub requests_total: u64,
    /// Cache hits
    pub hits: u64,
    /// Cache misses
    pub misses: u64,
    /// Hit rate percentage
    pub hit_rate: f64,
    /// Requests rejected by Bloom filter (no disk I/O needed)
    pub bloom_rejects: u64,
    /// Total bytes served
    pub bytes_served: u64,
    /// Human-readable bytes served
    pub bytes_served_human: String,
    /// Pull-through fetches from upstream
    pub upstream_fetches: u64,
    /// Upstream fetch errors
    pub upstream_errors: u64,
    /// Server uptime in seconds
    pub uptime_secs: u64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_basic() {
        let metrics = ServerMetrics::new();

        metrics.record_hit();
        metrics.record_hit();
        metrics.record_miss();
        metrics.record_bytes_served(1000);
        metrics.record_bloom_reject();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.hits, 2);
        assert_eq!(snapshot.misses, 1);
        assert_eq!(snapshot.bytes_served, 1000);
        assert_eq!(snapshot.bloom_rejects, 1);
        assert!((snapshot.hit_rate - 66.67).abs() < 1.0);
    }

    #[test]
    fn test_hit_rate_zero_requests() {
        let metrics = ServerMetrics::new();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.hit_rate, 0.0);
    }
}
