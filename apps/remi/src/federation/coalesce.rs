// apps/remi/src/federation/coalesce.rs
//! Request coalescing (singleflight pattern)
//!
//! When multiple tasks request the same chunk concurrently, this module
//! ensures only one actual network request is made. Other tasks wait
//! for the result and share it.
//!
//! This prevents the "thundering herd" problem and reduces bandwidth
//! when many machines request the same chunk simultaneously.

use conary_core::{Error, Result};
use dashmap::DashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::broadcast;
use tracing::debug;

const MAX_INFLIGHT_REQUESTS: usize = 2048;

/// Cached result from a coalesced request
#[derive(Clone)]
enum CachedResult {
    /// Success with data
    Success(Vec<u8>),
    /// Failure with error message
    Failure(String),
}

/// Request coalescer implementing the singleflight pattern
///
/// When a fetch for a chunk is in progress, subsequent requests for
/// the same chunk will wait for the in-flight request rather than
/// making duplicate network calls.
pub struct RequestCoalescer {
    /// In-flight requests (hash -> broadcast sender)
    inflight: DashMap<String, broadcast::Sender<CachedResult>>,
    /// Count of coalesced (deduplicated) requests
    coalesced_count: AtomicU64,
}

impl RequestCoalescer {
    /// Create a new request coalescer
    pub fn new() -> Self {
        Self {
            inflight: DashMap::new(),
            coalesced_count: AtomicU64::new(0),
        }
    }

    /// Coalesce concurrent requests for the same chunk
    ///
    /// If another task is already fetching this chunk, wait for that result.
    /// Otherwise, execute the fetch function and broadcast the result to any
    /// waiting tasks.
    ///
    /// # Arguments
    ///
    /// * `hash` - The chunk hash (used as the deduplication key)
    /// * `fetch` - Function to execute if no in-flight request exists
    ///
    /// # Returns
    ///
    /// The chunk data if successful, or an error if the fetch failed.
    pub async fn coalesce<F, Fut>(&self, hash: &str, fetch: F) -> Result<Vec<u8>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        // Use DashMap's entry API for atomic check-then-insert to avoid a race
        // where two tasks both see no in-flight request and both start fetches.
        use dashmap::mapref::entry::Entry;

        let rx = match self.inflight.entry(hash.to_string()) {
            Entry::Occupied(e) => {
                // Another task is already fetching this chunk - subscribe and wait
                let mut rx = e.get().subscribe();
                drop(e); // Release the entry lock before awaiting

                debug!("Coalescing request for chunk {}", hash);
                self.coalesced_count.fetch_add(1, Ordering::Relaxed);

                match rx.recv().await {
                    Ok(CachedResult::Success(data)) => return Ok(data),
                    Ok(CachedResult::Failure(msg)) => {
                        return Err(Error::DownloadError(msg));
                    }
                    Err(_) => {
                        // Sender dropped without sending - fall through to retry
                        debug!("Coalesced request sender dropped, retrying");
                        None
                    }
                }
            }
            Entry::Vacant(e) => {
                if self.inflight.len() >= MAX_INFLIGHT_REQUESTS {
                    return Err(Error::DownloadError(format!(
                        "Too many in-flight federation requests (max {})",
                        MAX_INFLIGHT_REQUESTS
                    )));
                }
                // We are the leader - create broadcast channel and register
                let (tx, _rx) = broadcast::channel::<CachedResult>(1);
                e.insert(tx.clone());
                Some(tx)
            }
        };

        // If we got a sender, we are the leader. If None, the previous leader
        // dropped without sending, so we loop back through the entry API to
        // properly coalesce with any concurrent retries (avoids multiple tasks
        // all inserting new entries simultaneously).
        let tx = match rx {
            Some(tx) => tx,
            None => {
                // Re-enter through the entry API for proper coalescing
                match self.inflight.entry(hash.to_string()) {
                    dashmap::mapref::entry::Entry::Occupied(e) => {
                        // Another task beat us to the retry - subscribe
                        let mut rx = e.get().subscribe();
                        drop(e);
                        self.coalesced_count.fetch_add(1, Ordering::Relaxed);
                        match rx.recv().await {
                            Ok(CachedResult::Success(data)) => return Ok(data),
                            Ok(CachedResult::Failure(msg)) => {
                                return Err(Error::DownloadError(msg));
                            }
                            Err(_) => {
                                return Err(Error::DownloadError(
                                    "Coalesced request failed after leader retry".into(),
                                ));
                            }
                        }
                    }
                    dashmap::mapref::entry::Entry::Vacant(e) => {
                        if self.inflight.len() >= MAX_INFLIGHT_REQUESTS {
                            return Err(Error::DownloadError(format!(
                                "Too many in-flight federation requests (max {})",
                                MAX_INFLIGHT_REQUESTS
                            )));
                        }
                        let (tx, _rx) = broadcast::channel::<CachedResult>(1);
                        e.insert(tx.clone());
                        tx
                    }
                }
            }
        };

        // Execute the fetch
        let result = fetch().await;

        // Broadcast the result to any waiters
        let cached_result = match &result {
            Ok(data) => CachedResult::Success(data.clone()),
            Err(e) => CachedResult::Failure(e.to_string()),
        };

        // Send result (ignore errors if no receivers)
        let _ = tx.send(cached_result);

        // Remove from in-flight
        self.inflight.remove(hash);

        result
    }

    /// Get the count of coalesced (deduplicated) requests
    pub fn coalesced_count(&self) -> u64 {
        self.coalesced_count.load(Ordering::Relaxed)
    }

    /// Get the number of currently in-flight requests
    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }
}

impl Default for RequestCoalescer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_single_request() {
        let coalescer = RequestCoalescer::new();

        let result = coalescer
            .coalesce("hash1", || async { Ok(vec![1, 2, 3]) })
            .await;

        assert_eq!(result.unwrap(), vec![1, 2, 3]);
        assert_eq!(coalescer.coalesced_count(), 0);
    }

    #[tokio::test]
    async fn test_coalesced_requests() {
        let coalescer = Arc::new(RequestCoalescer::new());
        let call_count = Arc::new(AtomicUsize::new(0));

        let hash = "shared_hash";

        // Spawn multiple concurrent requests
        let mut handles = Vec::new();

        for _ in 0..5 {
            let coalescer = Arc::clone(&coalescer);
            let call_count = Arc::clone(&call_count);

            handles.push(tokio::spawn(async move {
                coalescer
                    .coalesce(hash, || {
                        let count = Arc::clone(&call_count);
                        async move {
                            // Simulate slow fetch
                            sleep(Duration::from_millis(100)).await;
                            count.fetch_add(1, Ordering::SeqCst);
                            Ok(vec![42])
                        }
                    })
                    .await
            }));
        }

        // Wait for all to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert_eq!(result.unwrap(), vec![42]);
        }

        // Only one actual fetch should have happened
        // (may be slightly more due to timing, but significantly fewer than 5)
        assert!(call_count.load(Ordering::SeqCst) < 3);
        // Some requests should have been coalesced
        assert!(coalescer.coalesced_count() > 0);
    }

    #[tokio::test]
    async fn test_different_hashes_not_coalesced() {
        let coalescer = RequestCoalescer::new();

        let result1 = coalescer
            .coalesce("hash1", || async { Ok(vec![1]) })
            .await
            .unwrap();

        let result2 = coalescer
            .coalesce("hash2", || async { Ok(vec![2]) })
            .await
            .unwrap();

        assert_eq!(result1, vec![1]);
        assert_eq!(result2, vec![2]);
        assert_eq!(coalescer.coalesced_count(), 0);
    }

    #[tokio::test]
    async fn test_error_propagation() {
        let coalescer = RequestCoalescer::new();

        let result = coalescer
            .coalesce("hash1", || async {
                Err::<Vec<u8>, _>(Error::NotFound("test error".to_string()))
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_inflight_cleanup() {
        let coalescer = RequestCoalescer::new();

        assert_eq!(coalescer.inflight_count(), 0);

        let _ = coalescer.coalesce("hash1", || async { Ok(vec![1]) }).await;

        // After completion, should be cleaned up
        assert_eq!(coalescer.inflight_count(), 0);
    }

    #[tokio::test]
    async fn test_rejects_new_requests_when_inflight_cap_reached() {
        let coalescer = RequestCoalescer::new();
        for n in 0..MAX_INFLIGHT_REQUESTS {
            let (tx, _rx) = broadcast::channel(1);
            coalescer.inflight.insert(format!("hash-{n}"), tx);
        }

        let err = coalescer
            .coalesce("overflow", || async { Ok(vec![1, 2, 3]) })
            .await
            .expect_err("new requests should be rejected once the in-flight map is full");
        assert!(
            err.to_string()
                .contains("Too many in-flight federation requests")
        );
    }
}
