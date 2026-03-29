// conary-server/src/server/security.rs
//! Security primitives for the Remi server
//!
//! Includes:
//! - Rate limiting (token bucket)
//! - IP Banning (failure tracking)

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::warn;

/// Maximum number of per-IP rate limit buckets before stale entries are evicted.
/// This bounds memory under scanning attacks where many unique IPs hit the server.
const RATE_LIMITER_MAX_BUCKETS: usize = 100_000;

/// Rate limiter state for per-IP tracking.
///
/// TODO: Consider replacing this hand-rolled token bucket with governor's
/// `DefaultKeyedRateLimiter`, which is already used for the admin API
/// (see `rate_limit.rs`). That would unify rate limiting across both the
/// public and admin APIs, reduce code, and gain governor's built-in GC.
/// The main blocker is plumbing governor into the public router's middleware
/// layer in `routes.rs`, which currently passes this type via `Extension`.
pub struct RateLimiter {
    /// Request counts per IP
    buckets: RwLock<HashMap<String, RateBucket>>,
    /// Requests per second limit
    rps: u32,
    /// Burst size
    burst: u32,
}

struct RateBucket {
    tokens: f64,
    last_update: Instant,
}

fn evict_rate_limit_entries(
    buckets: &mut HashMap<String, RateBucket>,
    ip: &str,
    now: Instant,
    max_buckets: usize,
) {
    if buckets.len() < max_buckets || buckets.contains_key(ip) {
        return;
    }

    let max_idle = Duration::from_secs(60);
    buckets.retain(|_, bucket| now.duration_since(bucket.last_update) < max_idle);

    if buckets.len() < max_buckets || buckets.contains_key(ip) {
        return;
    }

    if let Some(oldest_key) = buckets
        .iter()
        .min_by_key(|(_, bucket)| bucket.last_update)
        .map(|(key, _)| key.clone())
    {
        buckets.remove(&oldest_key);
    }
}

impl RateLimiter {
    pub fn new(rps: u32, burst: u32) -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
            rps,
            burst,
        }
    }

    /// Check if request should be allowed for this IP
    ///
    /// If the bucket map exceeds [`RATE_LIMITER_MAX_BUCKETS`], stale entries
    /// (older than 60 seconds) are evicted before inserting a new one.
    pub async fn check(&self, ip: &str) -> bool {
        let mut buckets = self.buckets.write().await;
        let now = Instant::now();

        evict_rate_limit_entries(&mut buckets, ip, now, RATE_LIMITER_MAX_BUCKETS);

        let bucket = buckets.entry(ip.to_string()).or_insert_with(|| RateBucket {
            tokens: self.burst as f64,
            last_update: now,
        });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last_update).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rps as f64).min(self.burst as f64);
        bucket.last_update = now;

        // Try to consume a token
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Clean up old entries (call periodically)
    pub async fn cleanup(&self, max_age: Duration) {
        let mut buckets = self.buckets.write().await;
        let now = Instant::now();
        buckets.retain(|_, bucket| now.duration_since(bucket.last_update) < max_age);
    }
}

/// Ban list for misbehaving IPs
///
/// Uses `IpAddr` keys for type-safe IP tracking, avoiding string-comparison
/// pitfalls (e.g., "::1" vs "0:0:0:0:0:0:0:1").
pub struct BanList {
    /// Active bans: IP -> Ban Creation Time
    bans: RwLock<HashMap<IpAddr, Instant>>,
    /// Recent failures: IP -> (Count, First Failure Time)
    failures: RwLock<HashMap<IpAddr, (u32, Instant)>>,
    /// Duration of a ban
    ban_duration: Duration,
    /// Number of failures before ban
    ban_threshold: u32,
    /// Time window to reset failures (if no failures for this long)
    failure_window: Duration,
}

impl BanList {
    pub fn new(ban_duration_secs: u64, ban_threshold: u32) -> Self {
        Self {
            bans: RwLock::new(HashMap::new()),
            failures: RwLock::new(HashMap::new()),
            ban_duration: Duration::from_secs(ban_duration_secs),
            ban_threshold,
            // Reset failures if quiet for 1 minute or ban duration, whichever is larger
            failure_window: Duration::from_secs(60).max(Duration::from_secs(ban_duration_secs)),
        }
    }

    /// Check if IP is banned
    pub async fn is_banned(&self, ip: IpAddr) -> bool {
        let bans = self.bans.read().await;
        if let Some(banned_at) = bans.get(&ip) {
            // Check if ban has expired
            if banned_at.elapsed() < self.ban_duration {
                return true;
            }
        }
        false
    }

    /// Record a failure for an IP (e.g., 404, 401, 500)
    /// Returns true if the IP is now banned
    pub async fn record_failure(&self, ip: IpAddr) -> bool {
        // If already banned, ignore
        if self.is_banned(ip).await {
            return true;
        }

        let mut failures = self.failures.write().await;
        let now = Instant::now();

        let (count, first_failure) = failures.entry(ip).or_insert((0, now));

        // Reset if window passed
        if now.duration_since(*first_failure) > self.failure_window {
            *count = 0;
            *first_failure = now;
        }

        *count += 1;

        if *count >= self.ban_threshold {
            // Threshold reached - BAN!
            drop(failures); // Drop lock before acquiring bans lock
            self.ban(ip).await;
            return true;
        }

        false
    }

    /// Ban an IP immediately
    ///
    /// Note: This acquires two separate locks (`bans` then `failures`). There is a minor
    /// TOCTOU race where concurrent callers could both pass the `is_banned` check in
    /// `record_failure` and call `ban` simultaneously. The worst case is a redundant ban
    /// insertion, which is harmless. Combining both maps into a single lock would add
    /// contention on the hot `is_banned` read path, so this trade-off is acceptable.
    pub async fn ban(&self, ip: IpAddr) {
        let mut bans = self.bans.write().await;
        let mut failures = self.failures.write().await;

        bans.insert(ip, Instant::now());
        // Clear failures
        failures.remove(&ip);

        warn!(
            ip = %ip,
            "IP banned for {} seconds",
            self.ban_duration.as_secs()
        );
    }

    /// Cleanup expired bans and old failure records
    pub async fn cleanup(&self) {
        let now = Instant::now();

        // Cleanup bans
        {
            let mut bans = self.bans.write().await;
            bans.retain(|_, banned_at| banned_at.elapsed() < self.ban_duration);
        }

        // Cleanup failures
        {
            let mut failures = self.failures.write().await;
            failures.retain(|_, (_, first_failure)| {
                now.duration_since(*first_failure) < self.failure_window
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let limiter = RateLimiter::new(10, 5); // 10 rps, burst of 5

        // First 5 requests should succeed (burst)
        for _ in 0..5 {
            assert!(limiter.check("192.168.1.1").await);
        }

        // 6th request should fail (burst exhausted)
        assert!(!limiter.check("192.168.1.1").await);

        // Different IP should still work
        assert!(limiter.check("192.168.1.2").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_refill() {
        let limiter = RateLimiter::new(100, 2); // 100 rps, burst of 2

        // Exhaust tokens
        assert!(limiter.check("test-ip").await);
        assert!(limiter.check("test-ip").await);
        assert!(!limiter.check("test-ip").await);

        // Wait a tiny bit for token refill (100 rps = 1 token per 10ms)
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Should have refilled at least 1 token
        assert!(limiter.check("test-ip").await);
    }

    #[tokio::test]
    async fn test_ban_list_manual() {
        let ban_list = BanList::new(1, 5); // 1 second ban, 5 failures
        let ip: IpAddr = "192.168.1.100".parse().unwrap();
        let other_ip: IpAddr = "192.168.1.200".parse().unwrap();

        // Not banned initially
        assert!(!ban_list.is_banned(ip).await);

        // Ban the IP
        ban_list.ban(ip).await;

        // Should be banned now
        assert!(ban_list.is_banned(ip).await);

        // Other IPs unaffected
        assert!(!ban_list.is_banned(other_ip).await);

        // Wait for ban to expire
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Should no longer be banned
        assert!(!ban_list.is_banned(ip).await);
    }

    #[tokio::test]
    async fn test_ban_list_failures() {
        let ban_list = BanList::new(60, 3); // 60s ban, 3 failures
        let ip: IpAddr = "10.0.0.1".parse().unwrap();

        assert!(!ban_list.record_failure(ip).await); // 1
        assert!(!ban_list.record_failure(ip).await); // 2
        assert!(ban_list.record_failure(ip).await); // 3 -> Ban

        assert!(ban_list.is_banned(ip).await);
    }

    #[tokio::test]
    async fn test_ban_list_cleanup() {
        let ban_list = BanList::new(1, 3); // 1 second ban
        let ip: IpAddr = "10.0.0.2".parse().unwrap();

        ban_list.ban(ip).await;

        // Wait and cleanup
        tokio::time::sleep(Duration::from_secs(2)).await;
        ban_list.cleanup().await;

        // Should be cleaned up
        let bans = ban_list.bans.read().await;
        assert!(bans.is_empty());
    }

    #[test]
    fn test_rate_limiter_eviction_evicts_oldest_when_capacity_remains_full() {
        let now = Instant::now();
        let mut buckets = HashMap::new();
        buckets.insert(
            "oldest".to_string(),
            RateBucket {
                tokens: 1.0,
                last_update: now - Duration::from_secs(5),
            },
        );
        buckets.insert(
            "newer".to_string(),
            RateBucket {
                tokens: 1.0,
                last_update: now - Duration::from_secs(1),
            },
        );

        evict_rate_limit_entries(&mut buckets, "incoming", now, 2);

        assert_eq!(buckets.len(), 1);
        assert!(!buckets.contains_key("oldest"));
        assert!(buckets.contains_key("newer"));
    }
}
