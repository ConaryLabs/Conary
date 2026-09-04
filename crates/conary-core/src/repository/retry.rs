// crates/conary-core/src/repository/retry.rs

//! Retry policy with exponential backoff and jitter.
//!
//! [`RetryConfig`] owns the attempt count and delay schedule. The retry loops
//! themselves live beside the operations they guard (the metadata, byte, and
//! streamed-download loops in `client.rs`), because each classifies retryable
//! failures differently and the streamed loop resumes partial transfers.

use rand::RngExt;
use std::time::Duration;

/// Configuration for retry behavior with exponential backoff and jitter.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of attempts (including the first).
    pub max_attempts: u32,
    /// Base delay between retries (doubles each attempt).
    pub base_delay: Duration,
    /// Maximum delay cap.
    pub max_delay: Duration,
    /// Jitter factor (0.0 to 1.0) -- adds random delay up to this fraction.
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter_factor: 0.25,
        }
    }
}

impl RetryConfig {
    /// Calculate the delay for a given attempt number (1-based).
    ///
    /// Uses exponential backoff: `min(base * 2^(n-1), max_delay) + jitter`
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1);
        let base_ms = self.base_delay.as_millis() as u64;
        let multiplier = 1u64.checked_shl(exp).unwrap_or(u64::MAX);
        let delay_ms = base_ms.saturating_mul(multiplier);
        let max_ms = self.max_delay.as_millis() as u64;
        let capped_ms = delay_ms.min(max_ms);

        let jitter_ms = if self.jitter_factor > 0.0 {
            let max_jitter = (capped_ms as f64 * self.jitter_factor) as u64;
            if max_jitter > 0 {
                rand::rng().random_range(0..=max_jitter)
            } else {
                0
            }
        } else {
            0
        };

        Duration::from_millis(capped_ms + jitter_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.base_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(30));
    }

    #[test]
    fn test_delay_exponential_no_jitter() {
        let config = RetryConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            jitter_factor: 0.0,
        };

        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(200));
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(400));
        assert_eq!(config.delay_for_attempt(4), Duration::from_millis(800));
    }

    #[test]
    fn test_delay_capped_at_max() {
        let config = RetryConfig {
            max_attempts: 10,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            jitter_factor: 0.0,
        };

        assert_eq!(config.delay_for_attempt(4), Duration::from_secs(5));
        assert_eq!(config.delay_for_attempt(10), Duration::from_secs(5));
    }

    #[test]
    fn test_delay_jitter_within_bounds() {
        let config = RetryConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(60),
            jitter_factor: 0.5,
        };

        for _ in 0..100 {
            let delay = config.delay_for_attempt(1);
            assert!(delay >= Duration::from_millis(1000));
            assert!(delay <= Duration::from_millis(1500));
        }
    }

    #[test]
    fn test_delay_large_attempt_no_overflow() {
        let config = RetryConfig {
            max_attempts: 100,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            jitter_factor: 0.0,
        };

        let delay = config.delay_for_attempt(64);
        assert_eq!(delay, Duration::from_secs(60));
    }
}
