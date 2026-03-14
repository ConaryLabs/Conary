// conary-test/src/server/state.rs

use crate::config::distro::GlobalConfig;
use crate::engine::suite::TestSuite;
use crate::report::stream::TestEvent;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Mutex;

/// Metadata for a run that persists alongside the TestSuite.
#[derive(Debug, Clone)]
pub struct RunMeta {
    pub suite_name: String,
    pub distro: String,
    pub phase: u32,
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Maximum number of completed runs to retain in memory.
const MAX_RUNS: usize = 100;

/// Capacity of the broadcast channel for test events.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct AppState {
    pub config: GlobalConfig,
    pub manifest_dir: String,
    /// Concurrent map of run IDs to test suites. Replaces the previous
    /// `Arc<RwLock<HashMap>>` with lock-free per-shard concurrency.
    pub runs: Arc<DashMap<u64, TestSuite>>,
    /// Per-run metadata (suite name, distro, phase) for rerun lookups.
    pub run_meta: Arc<DashMap<u64, RunMeta>>,
    /// Per-run cancellation flags. Setting a flag to `true` signals the
    /// runner to stop executing tests for that run.
    pub cancellation_flags: Arc<DashMap<u64, Arc<AtomicBool>>>,
    /// Per-distro image build locks. Ensures only one image build runs at a
    /// time per distro, with the built tag cached for subsequent runs.
    pub image_locks: Arc<DashMap<String, Arc<Mutex<Option<String>>>>>,
    /// Broadcast channel for live test events (SSE streaming).
    pub event_tx: tokio::sync::broadcast::Sender<TestEvent>,
}

impl AppState {
    pub fn new(config: GlobalConfig, manifest_dir: String) -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            config,
            manifest_dir,
            runs: Arc::new(DashMap::new()),
            run_meta: Arc::new(DashMap::new()),
            image_locks: Arc::new(DashMap::new()),
            cancellation_flags: Arc::new(DashMap::new()),
            event_tx,
        }
    }

    /// Register a cancellation flag for a run. Returns the flag for the
    /// caller to pass into the runner.
    pub fn register_cancel_flag(&self, run_id: u64) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.cancellation_flags.insert(run_id, Arc::clone(&flag));
        flag
    }

    /// Signal cancellation for a run. Returns `true` if the run was found.
    pub fn cancel_run(&self, run_id: u64) -> bool {
        if let Some(flag) = self.cancellation_flags.get(&run_id) {
            flag.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Remove the cancellation flag for a completed run.
    pub fn remove_cancel_flag(&self, run_id: u64) {
        self.cancellation_flags.remove(&run_id);
    }

    /// Get the per-distro image build lock. Creates one if it doesn't exist.
    pub fn image_lock(&self, distro: &str) -> Arc<Mutex<Option<String>>> {
        self.image_locks
            .entry(distro.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    }

    pub fn next_run_id() -> u64 {
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    /// Insert a new run, evicting the oldest entry (by `started_at`) if the
    /// map has reached `MAX_RUNS`.
    pub fn insert_run(&self, run_id: u64, suite: TestSuite) {
        if self.runs.len() >= MAX_RUNS {
            self.evict_oldest_run();
        }
        self.runs.insert(run_id, suite);
    }

    /// Remove the run with the earliest `started_at` timestamp.
    fn evict_oldest_run(&self) {
        let oldest_key = self
            .runs
            .iter()
            .min_by_key(|entry| entry.value().started_at)
            .map(|entry| *entry.key());
        if let Some(key) = oldest_key {
            self.runs.remove(&key);
        }
    }

    /// Emit a test event on the broadcast channel. Silently ignores
    /// failures (e.g., no subscribers).
    pub fn emit_event(&self, event: TestEvent) {
        let _ = self.event_tx.send(event);
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
        assert!(state.runs.is_empty());
    }

    #[test]
    fn test_register_and_cancel_flag() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );

        let flag = state.register_cancel_flag(42);
        assert!(!flag.load(Ordering::Relaxed));

        // Cancel the run.
        assert!(state.cancel_run(42));
        assert!(flag.load(Ordering::Relaxed));

        // Cancel a non-existent run returns false.
        assert!(!state.cancel_run(999));
    }

    #[test]
    fn test_remove_cancel_flag() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );

        let _flag = state.register_cancel_flag(42);
        assert!(state.cancellation_flags.contains_key(&42));

        state.remove_cancel_flag(42);
        assert!(!state.cancellation_flags.contains_key(&42));
    }

    #[test]
    fn test_eviction_removes_oldest_run() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );

        // Insert MAX_RUNS + 1 entries to trigger eviction.
        for i in 0..=MAX_RUNS {
            let id = (i + 1) as u64;
            let suite = TestSuite::new(&format!("suite-{i}"), 1);
            state.insert_run(id, suite);
        }

        assert_eq!(state.runs.len(), MAX_RUNS);
        // The first inserted run (id=1) should have been evicted.
        assert!(!state.runs.contains_key(&1));
        // The latest run should still be present.
        assert!(state.runs.contains_key(&(MAX_RUNS as u64 + 1)));
    }

    #[test]
    fn broadcast_channel_delivers_events() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );

        let mut rx = state.event_tx.subscribe();

        state.emit_event(TestEvent::SuiteStarted {
            run_id: 1,
            suite: "smoke".to_string(),
            phase: 1,
            total: 5,
        });

        let event = rx.try_recv().unwrap();
        assert_eq!(event.run_id(), 1);
    }

    #[test]
    fn broadcast_no_subscribers_does_not_panic() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );

        // Emitting with no subscribers should not panic.
        state.emit_event(TestEvent::RunComplete {
            run_id: 99,
            passed: 0,
            failed: 0,
            skipped: 0,
        });
    }

    #[test]
    fn dashmap_concurrent_access() {
        let state = AppState::new(
            test_fixtures::test_global_config(),
            "/tmp/manifests".to_string(),
        );

        // Insert and read concurrently (simulated sequentially in a unit test).
        state.insert_run(1, TestSuite::new("suite-a", 1));
        state.insert_run(2, TestSuite::new("suite-b", 2));

        assert_eq!(state.runs.len(), 2);
        assert!(state.runs.get(&1).is_some());
        assert!(state.runs.get(&2).is_some());
        assert_eq!(state.runs.get(&1).unwrap().name, "suite-a");
    }
}
