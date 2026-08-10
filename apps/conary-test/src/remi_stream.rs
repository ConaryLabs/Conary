// conary-test/src/remi_stream.rs

//! Local CLI orchestration for Remi test-result streaming.

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::build_info::BuildInfo;
use crate::engine::runner::RemiStreamCtx;
use crate::engine::suite::TestSuite;
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

    /// Flush this run's WAL before closing the acknowledged Remi run.
    pub async fn finish(&self, suite: &TestSuite) {
        // Do not mark a run terminal while its own failed result deliveries
        // are still buffered.
        self.flush_wal("final").await;

        let status = terminal_status(suite);
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

fn terminal_status(suite: &TestSuite) -> &'static str {
    if suite.cancelled() > 0 {
        "cancelled"
    } else if suite.failed() > 0 || suite.skipped() > 0 || !suite.corpus_all_completed() {
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

    #[test]
    fn terminal_status_matches_cli_outcome_categories() {
        let mut passed = TestSuite::new("phase-1", 1);
        passed.finish();
        assert_eq!(terminal_status(&passed), "passed");
    }
}
