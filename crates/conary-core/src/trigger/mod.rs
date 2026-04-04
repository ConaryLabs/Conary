// conary-core/src/trigger/mod.rs

//! Trigger execution for post-installation actions
//!
//! This module handles executing triggers after files are installed or removed.
//! Triggers are path-based handlers that run when files matching certain patterns
//! are modified.
//!
//! Key features:
//! - Pattern-based file matching (glob patterns)
//! - DAG-ordered execution (respects trigger dependencies)
//! - Deduplication (each trigger runs once per changeset, not per file)
//! - Timeout protection
//! - Handler existence checking (skip if handler not found)
//! - Target root support: triggers can run inside a target filesystem
//!
//! ## Target Root Support
//!
//! When installing to a target root (root != "/"), triggers are executed
//! inside a chroot rooted at the target path. This allows triggers to run
//! correctly during bootstrap or container image creation.

mod execution;

use crate::db::models::{ChangesetTrigger, Trigger, TriggerEngine};
use crate::error::Result;
use execution::{handler_exists, shell_split};
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info, warn};

pub use execution::handler_exists_in_root;

/// Default timeout for trigger execution (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Trigger executor handles running triggers after file operations
pub struct TriggerExecutor<'a> {
    conn: &'a Connection,
    root: &'a Path,
    timeout: Duration,
    dry_run: bool,
}

impl<'a> TriggerExecutor<'a> {
    /// Create a new trigger executor
    pub fn new(conn: &'a Connection, root: &'a Path) -> Self {
        Self {
            conn,
            root,
            timeout: DEFAULT_TIMEOUT,
            dry_run: false,
        }
    }

    /// Set custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Enable dry-run mode (don't actually execute triggers)
    pub fn dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Record which triggers need to run based on installed files
    pub fn record_triggers(
        &self,
        changeset_id: i64,
        file_paths: &[String],
    ) -> Result<Vec<Trigger>> {
        let engine = TriggerEngine::new(self.conn);
        engine.record_triggers(changeset_id, file_paths)
    }

    /// Check if we're operating on the live root
    fn is_live_root(&self) -> bool {
        self.root == Path::new("/")
    }

    /// Execute all pending triggers for a changeset
    pub fn execute_pending(&self, changeset_id: i64) -> Result<TriggerResults> {
        let engine = TriggerEngine::new(self.conn);
        let triggers = engine.get_execution_order(changeset_id)?;

        if triggers.is_empty() {
            debug!("No triggers to execute for changeset {}", changeset_id);
            return Ok(TriggerResults::default());
        }

        info!(
            "Executing {} trigger(s) for changeset {} (root: {})",
            triggers.len(),
            changeset_id,
            self.root.display()
        );

        let mut results = TriggerResults::default();

        for trigger in triggers {
            let trigger_id = trigger.id.unwrap_or(0);

            if self.dry_run {
                info!("  [DRY-RUN] Would execute trigger: {}", trigger.name);
                results.skipped += 1;
                continue;
            }

            // Check if handler exists (in target root if not live).
            // Surface parse errors (e.g. unterminated quotes) as warnings
            // instead of silently treating them as "not found".
            let handler_parts = match shell_split(&trigger.handler) {
                Ok(parts) => parts,
                Err(e) => {
                    let msg = format!("malformed handler: {e}");
                    warn!(
                        "Trigger '{}' has malformed handler '{}': {e}",
                        trigger.name, trigger.handler
                    );
                    // Persist failure in DB so get_execution_order() does not
                    // pick up the same broken trigger on the next run.
                    ChangesetTrigger::mark_failed(self.conn, changeset_id, trigger_id, &msg)?;
                    results.failed += 1;
                    results.errors.push(format!("{}: {msg}", trigger.name));
                    continue;
                }
            };
            let handler_cmd = handler_parts
                .first()
                .map(String::as_str)
                .unwrap_or_default();
            let handler_check = if self.is_live_root() {
                handler_exists(handler_cmd)
            } else {
                handler_exists_in_root(handler_cmd, self.root)
            };

            if !handler_check {
                info!(
                    "  [SKIP] Trigger '{}': handler '{}' not found{}",
                    trigger.name,
                    handler_cmd,
                    if self.is_live_root() {
                        ""
                    } else {
                        " in target root"
                    }
                );
                ChangesetTrigger::mark_completed(
                    self.conn,
                    changeset_id,
                    trigger_id,
                    Some(&format!("Skipped: handler '{}' not found", handler_cmd)),
                )?;
                results.skipped += 1;
                continue;
            }

            info!("  Running trigger: {} ({})", trigger.name, trigger.handler);
            ChangesetTrigger::mark_running(self.conn, changeset_id, trigger_id)?;

            let result = if self.is_live_root() {
                self.execute_handler(&trigger)
            } else {
                self.execute_handler_in_target(&trigger)
            };

            match result {
                Ok(output) => {
                    info!("  [OK] Trigger '{}' completed", trigger.name);
                    ChangesetTrigger::mark_completed(
                        self.conn,
                        changeset_id,
                        trigger_id,
                        output.as_deref(),
                    )?;
                    results.succeeded += 1;
                }
                Err(e) => {
                    warn!("  [FAIL] Trigger '{}': {}", trigger.name, e);
                    ChangesetTrigger::mark_failed(
                        self.conn,
                        changeset_id,
                        trigger_id,
                        &e.to_string(),
                    )?;
                    results.failed += 1;
                    results.errors.push(format!("{}: {}", trigger.name, e));
                }
            }
        }

        Ok(results)
    }
}

/// Results of trigger execution
#[derive(Debug, Default)]
pub struct TriggerResults {
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

impl TriggerResults {
    /// Check if all triggers succeeded
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0
    }

    /// Total triggers processed
    pub fn total(&self) -> usize {
        self.succeeded + self.failed + self.skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_results() {
        let results = TriggerResults {
            succeeded: 5,
            failed: 1,
            skipped: 2,
            ..Default::default()
        };

        assert!(!results.all_succeeded());
        assert_eq!(results.total(), 8);

        let results2 = TriggerResults {
            succeeded: 3,
            ..TriggerResults::default()
        };
        assert!(results2.all_succeeded());
    }
}
