// conary-test/src/remi_stream.rs

//! Local CLI orchestration for Remi test-result streaming.

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::build_info::BuildInfo;
use crate::engine::runner::RemiStreamCtx;
use crate::engine::suite::{RunStatus, TestSuite};
use crate::paths;
use crate::remi_client::RemiClient;
use crate::wal::{self, Wal};

const WAL_FILENAME: &str = "results-wal.db";

/// One acknowledged Remi run associated with one local CLI suite invocation.
pub struct LocalRemiRun {
    context: RemiStreamCtx,
}

impl LocalRemiRun {
    /// Create and initialize a Remi run for a local CLI invocation.
    ///
    /// Missing Remi configuration and failed run creation are deliberately
    /// treated as optional observability paths. The caller receives `None`
    /// and continues with ordinary local test behavior in either case.
    pub async fn start(suite: &str, distro: &str, phase: u32) -> Option<Self> {
        let client = match RemiClient::from_env() {
            Ok(client) => client,
            Err(_) => {
                info!("Remi streaming disabled: no Remi env configured");
                return None;
            }
        };

        let build_info = BuildInfo::current();
        let source_commit = (!build_info.git_commit.is_empty()).then_some(build_info.git_commit);
        let source_commit_ref = source_commit.as_deref();
        let run_id = match client
            .create_run(suite, distro, phase, Some("local-cli"), source_commit_ref)
            .await
        {
            Ok(run_id) => run_id,
            Err(error) => {
                warn!(error = %error, "Remi streaming disabled: failed to create run");
                return None;
            }
        };

        let client = Arc::new(client);
        let context = RemiStreamCtx {
            remi_run_id: run_id,
            client,
            wal: open_wal(),
        };
        let local_run = Self { context };

        // Acknowledged reachability is the only gate that permits replaying
        // rows left by earlier invocations.
        local_run.flush_wal("startup").await;
        Some(local_run)
    }

    /// Return the runner context for per-test result streaming.
    pub fn context(&self) -> &RemiStreamCtx {
        &self.context
    }

    /// Flush this run's WAL, then close the acknowledged Remi run.
    ///
    /// `aborted` marks an invocation that ended on an error before every
    /// manifest ran. The run is still closed: an acknowledged run left at its
    /// `pending` default is indistinguishable from one still in flight, which
    /// is the opposite of what this surface exists to report.
    ///
    /// The flush runs first so a run is never marked terminal ahead of results
    /// it could still have delivered. It is ordering, not a gate — the status
    /// update is best-effort and proceeds whatever the flush outcome, because a
    /// run left open forever is worse than one whose last results arrive on a
    /// later flush.
    pub async fn finish(&self, suite: &TestSuite, aborted: bool) {
        self.flush_wal("final").await;

        let status = terminal_status(suite, aborted);
        let total = count_as_u32(suite.total());
        let passed = count_as_u32(suite.passed());
        let failed = count_as_u32(suite.failed());
        let skipped = count_as_u32(suite.skipped());
        if let Err(error) = self
            .context
            .client
            .update_run(
                self.context.remi_run_id,
                status,
                total,
                passed,
                failed,
                skipped,
            )
            .await
        {
            warn!(
                remi_run_id = self.context.remi_run_id,
                error = %error,
                "failed to update Remi run status"
            );
        }
    }

    async fn flush_wal(&self, reason: &str) {
        let Some(wal) = &self.context.wal else {
            return;
        };

        let outcome = {
            let wal_guard = wal.lock().await;
            wal::flush(&wal_guard, self.context.client.as_ref()).await
        };
        debug!(
            remi_run_id = self.context.remi_run_id,
            reason,
            flushed = outcome.flushed,
            failed = outcome.failed,
            discarded = outcome.discarded,
            purged = outcome.purged,
            "completed Remi result WAL flush"
        );
    }
}

fn open_wal() -> Option<Arc<tokio::sync::Mutex<Wal>>> {
    let state_dir = match paths::state_dir() {
        Ok(path) => path,
        Err(error) => {
            warn!(error = %error, "failed to resolve Remi result WAL state directory");
            return None;
        }
    };

    if let Err(error) = std::fs::create_dir_all(&state_dir) {
        warn!(
            path = %state_dir.display(),
            error = %error,
            "failed to create Remi result WAL state directory"
        );
        return None;
    }

    let wal_path = state_dir.join(WAL_FILENAME);
    info!(path = %wal_path.display(), "Remi result streaming enabled; using WAL");
    match Wal::open(&wal_path) {
        Ok(wal) => Some(Arc::new(tokio::sync::Mutex::new(wal))),
        Err(error) => {
            warn!(
                path = %wal_path.display(),
                error = %error,
                "failed to open Remi result WAL; continuing without buffering"
            );
            None
        }
    }
}

/// Project a finished local suite onto Remi's run-status vocabulary.
///
/// An invocation that `aborted` never finished its manifests, so its counts
/// describe only the part that ran and cannot say the run passed.
///
/// Whether a run counts as cancelled is [`TestSuite::finish`]'s decision, not a
/// second rule here: it is deliberately narrower than "some test was
/// cancelled", and two rules under one word would let the same run read
/// `completed` locally and `cancelled` on Remi. What this function adds is the
/// pass/fail judgement, which the local lifecycle status does not carry.
fn terminal_status(suite: &TestSuite, aborted: bool) -> &'static str {
    if aborted {
        "failed"
    } else if suite.status == RunStatus::Cancelled {
        "cancelled"
    } else if suite.has_blocking_results() {
        "failed"
    } else {
        "passed"
    }
}

fn count_as_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::suite::TestResult;
    use std::ffi::OsString;

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        fn new(names: &[&'static str]) -> Self {
            let lock = crate::test_support::lock_env();
            let values = names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect();
            Self {
                _lock: lock,
                values,
            }
        }

        fn clear(&self, name: &str) {
            // Environment mutation is unsafe under Rust 2024 because other
            // threads may observe the process environment concurrently. The
            // shared test lock makes this test's mutation serialized.
            unsafe {
                std::env::remove_var(name);
            }
        }

        fn set(&self, name: &str, value: &str) {
            unsafe {
                std::env::set_var(name, value);
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.values {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn no_remi_environment_does_not_create_a_wal() {
        let env = EnvGuard::new(&[
            "REMI_ADMIN_ENDPOINT",
            "REMI_ADMIN_TOKEN",
            "CONARY_TEST_STATE_DIR",
        ]);
        env.clear("REMI_ADMIN_ENDPOINT");
        env.clear("REMI_ADMIN_TOKEN");
        let state_dir = std::env::current_dir()
            .unwrap()
            .join(format!(".conary-test-remi-stream-{}", std::process::id()));
        assert!(!state_dir.exists());
        env.set("CONARY_TEST_STATE_DIR", state_dir.to_str().unwrap());

        assert!(
            LocalRemiRun::start("phase-1", "fedora44", 1)
                .await
                .is_none()
        );
        assert!(!state_dir.join(WAL_FILENAME).exists());
    }

    fn result(id: &str, status: crate::engine::suite::TestStatus) -> TestResult {
        TestResult {
            id: id.to_string(),
            name: id.to_string(),
            status,
            duration_ms: 1,
            message: None,
            stdout: None,
            stderr: None,
            attempts: Vec::new(),
        }
    }

    fn finished(results: Vec<TestResult>) -> TestSuite {
        let mut suite = TestSuite::new("phase-1", 1);
        for result in results {
            suite.record(result);
        }
        suite.finish();
        suite
    }

    #[test]
    fn terminal_status_matches_cli_outcome_categories() {
        use crate::engine::suite::TestStatus;

        assert_eq!(terminal_status(&finished(Vec::new()), false), "passed");
        assert_eq!(
            terminal_status(&finished(vec![result("T1", TestStatus::Passed)]), false),
            "passed"
        );
        assert_eq!(
            terminal_status(&finished(vec![result("T1", TestStatus::Failed)]), false),
            "failed"
        );
        assert_eq!(
            terminal_status(&finished(vec![result("T1", TestStatus::Skipped)]), false),
            "failed"
        );
    }

    /// An invocation that died before running every manifest is closed as
    /// failed, never left at the `pending` default where it is
    /// indistinguishable from a run still in flight.
    #[test]
    fn an_aborted_invocation_is_closed_as_failed_whatever_ran() {
        use crate::engine::suite::TestStatus;

        let all_passed = finished(vec![result("T1", TestStatus::Passed)]);
        assert_eq!(terminal_status(&all_passed, false), "passed");
        assert_eq!(terminal_status(&all_passed, true), "failed");

        let cancelled = finished(vec![result("T1", TestStatus::Cancelled)]);
        assert_eq!(terminal_status(&cancelled, false), "cancelled");
        assert_eq!(terminal_status(&cancelled, true), "failed");
    }

    /// A run is cancelled only when [`TestSuite::finish`] says so. Deriving
    /// cancellation here from `cancelled() > 0` instead would report a run as
    /// `cancelled` on Remi that the local report calls `completed`.
    #[test]
    fn a_partly_cancelled_run_is_not_reported_as_a_cancelled_run() {
        use crate::engine::suite::{RunStatus, TestStatus};

        let all_cancelled = finished(vec![result("T1", TestStatus::Cancelled)]);
        assert_eq!(all_cancelled.status, RunStatus::Cancelled);
        assert_eq!(terminal_status(&all_cancelled, false), "cancelled");

        let partly_cancelled = finished(vec![
            result("T1", TestStatus::Passed),
            result("T2", TestStatus::Cancelled),
        ]);
        assert_eq!(partly_cancelled.status, RunStatus::Completed);
        assert_eq!(terminal_status(&partly_cancelled, false), "failed");
    }
}
