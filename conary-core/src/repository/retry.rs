// conary-core/src/repository/retry.rs

//! Consolidated retry logic with exponential backoff.
//!
//! The repository module had retry loops duplicated across `client.rs`,
//! `remi.rs` (chunk downloads, poll retries), and `mirror_selector.rs`.
//! This module provides a single canonical implementation.
//!
//! # Example
//! ```ignore
//! use conary_core::repository::retry::{RetryConfig, with_retry};
//!
//! let config = RetryConfig::default();
//! let result = with_retry(&config, || {
//!     download_something()
//! })?;
//! ```

use crate::error::Result;
use rand::Rng;
use std::time::Duration;
use tracing::warn;

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
    /// Create a config for quick operations (shorter delays).
    pub fn quick() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(5),
            jitter_factor: 0.25,
        }
    }

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

/// Execute a fallible operation with retry and exponential backoff.
///
/// Calls `op` up to `config.max_attempts` times. On failure, sleeps for
/// an exponentially increasing duration before the next attempt. Returns
/// the first successful result or the last error.
///
/// # Example
/// ```ignore
/// let data = with_retry(&RetryConfig::default(), || {
///     client.get(url).send()?.bytes().map_err(Into::into)
/// })?;
/// ```
pub fn with_retry<T, F>(config: &RetryConfig, mut op: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut last_err = None;

    for attempt in 1..=config.max_attempts {
        match op() {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt < config.max_attempts {
                    let delay = config.delay_for_attempt(attempt);
                    warn!(
                        "Attempt {}/{} failed: {e}, retrying in {delay:?}",
                        attempt, config.max_attempts
                    );
                    std::thread::sleep(delay);
                }
                last_err = Some(e);
            }
        }
    }

    Err(last_err.expect("max_attempts must be >= 1"))
}

/// Async version of [`with_retry`] for use in async contexts.
///
/// Uses `tokio::time::sleep` instead of `std::thread::sleep`.
pub async fn with_retry_async<T, F, Fut>(config: &RetryConfig, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_err = None;

    for attempt in 1..=config.max_attempts {
        match op().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt < config.max_attempts {
                    let delay = config.delay_for_attempt(attempt);
                    warn!(
                        "Attempt {}/{} failed: {e}, retrying in {delay:?}",
                        attempt, config.max_attempts
                    );
                    tokio::time::sleep(delay).await;
                }
                last_err = Some(e);
            }
        }
    }

    Err(last_err.expect("max_attempts must be >= 1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::cell::Cell;

    #[test]
    fn test_default_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.base_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(30));
    }

    #[test]
    fn test_quick_config() {
        let config = RetryConfig::quick();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.base_delay, Duration::from_millis(500));
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

    #[test]
    fn test_with_retry_succeeds_first_try() {
        let config = RetryConfig {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            jitter_factor: 0.0,
        };

        let result = with_retry(&config, || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_with_retry_succeeds_after_failures() {
        let config = RetryConfig {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            jitter_factor: 0.0,
        };

        let count = Cell::new(0);
        let result = with_retry(&config, || {
            let n = count.get() + 1;
            count.set(n);
            if n < 3 {
                Err(Error::DownloadError(format!("attempt {n}")))
            } else {
                Ok("success")
            }
        });

        assert_eq!(result.unwrap(), "success");
        assert_eq!(count.get(), 3);
    }

    #[test]
    fn test_with_retry_exhausts_attempts() {
        let config = RetryConfig {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            jitter_factor: 0.0,
        };

        let count = Cell::new(0);
        let result: Result<()> = with_retry(&config, || {
            count.set(count.get() + 1);
            Err(Error::DownloadError("always fails".to_string()))
        });

        assert!(result.is_err());
        assert_eq!(count.get(), 2);
    }
}
