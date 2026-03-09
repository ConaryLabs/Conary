// conary-test/src/server/state.rs

use crate::config::distro::GlobalConfig;
use crate::engine::suite::TestSuite;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

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
        let state = AppState::new(test_fixtures::test_global_config(), "/tmp/manifests".to_string());
        let runs = state.runs.try_read().unwrap();
        assert!(runs.is_empty());
    }
}
