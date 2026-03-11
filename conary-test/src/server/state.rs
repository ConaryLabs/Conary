// conary-test/src/server/state.rs

use crate::config::distro::GlobalConfig;
use crate::engine::suite::TestSuite;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Maximum number of completed runs to retain in memory.
const MAX_RUNS: usize = 100;

#[derive(Clone)]
pub struct AppState {
    pub config: GlobalConfig,
    pub manifest_dir: String,
    pub runs: Arc<RwLock<HashMap<u64, TestSuite>>>,
}

impl AppState {
    pub fn new(config: GlobalConfig, manifest_dir: String) -> Self {
        Self {
            config,
            manifest_dir,
            runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn next_run_id() -> u64 {
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    /// Insert a new run, evicting the oldest entry (by `started_at`) if the
    /// map has reached `MAX_RUNS`.
    pub async fn insert_run(&self, run_id: u64, suite: TestSuite) {
        let mut runs = self.runs.write().await;
        if runs.len() >= MAX_RUNS {
            // Find the key with the earliest started_at timestamp.
            if let Some(oldest_key) = runs
                .iter()
                .min_by_key(|(_, s)| s.started_at)
                .map(|(&k, _)| k)
            {
                runs.remove(&oldest_key);
            }
        }
        runs.insert(run_id, suite);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;

    #[test]
    fn test_next_run_id_increments() {
        let id1 = AppState::next_run_id();
        let id2 = AppState::next_run_id();
        assert!(id2 > id1);
    }

    #[test]
    fn test_new_state_has_empty_runs() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );
        let runs = state.runs.try_read().unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn test_eviction_removes_oldest_run() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );

        // Insert MAX_RUNS + 1 entries to trigger eviction.
        for i in 0..=MAX_RUNS {
            let id = (i + 1) as u64;
            let suite = TestSuite::new(&format!("suite-{i}"), 1);
            state.insert_run(id, suite).await;
        }

        let runs = state.runs.read().await;
        assert_eq!(runs.len(), MAX_RUNS);
        // The first inserted run (id=1) should have been evicted.
        assert!(!runs.contains_key(&1));
        // The latest run should still be present.
        assert!(runs.contains_key(&(MAX_RUNS as u64 + 1)));
    }
}
