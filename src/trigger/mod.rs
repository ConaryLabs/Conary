// src/trigger/mod.rs

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

use crate::db::models::{ChangesetTrigger, Trigger, TriggerEngine};
use crate::error::{Error, Result};
use rusqlite::Connection;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tracing::{debug, info, warn};
use wait_timeout::ChildExt;

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
    pub fn record_triggers(&self, changeset_id: i64, file_paths: &[String]) -> Result<Vec<Trigger>> {
        let engine = TriggerEngine::new(self.conn);
        engine.record_triggers(changeset_id, file_paths)
    }

    /// Execute all pending triggers for a changeset
    pub fn execute_pending(&self, changeset_id: i64) -> Result<TriggerResults> {
        // Safety check: Don't run triggers on non-root installs
        if self.root != Path::new("/") {
            warn!(
                "Skipping triggers: execution in non-root install paths ({}) is not yet supported",
                self.root.display()
            );
            return Ok(TriggerResults::skipped());
        }

        let engine = TriggerEngine::new(self.conn);
        let triggers = engine.get_execution_order(changeset_id)?;

        if triggers.is_empty() {
            debug!("No triggers to execute for changeset {}", changeset_id);
            return Ok(TriggerResults::empty());
        }

        info!("Executing {} trigger(s) for changeset {}", triggers.len(), changeset_id);

        let mut results = TriggerResults::new();

        for trigger in triggers {
            let trigger_id = trigger.id.unwrap_or(0);

            if self.dry_run {
                info!("  [DRY-RUN] Would execute trigger: {}", trigger.name);
                results.skipped += 1;
                continue;
            }

            // Check if handler exists
            let handler_cmd = trigger.handler.split_whitespace().next().unwrap_or("");
            if !handler_exists(handler_cmd) {
                info!(
                    "  [SKIP] Trigger '{}': handler '{}' not found",
                    trigger.name, handler_cmd
                );
                ChangesetTrigger::mark_completed(
                    self.conn, changeset_id, trigger_id,
                    Some(&format!("Skipped: handler '{}' not found", handler_cmd))
                )?;
                results.skipped += 1;
                continue;
            }

            info!("  Running trigger: {} ({})", trigger.name, trigger.handler);
            ChangesetTrigger::mark_running(self.conn, changeset_id, trigger_id)?;

            match self.execute_handler(&trigger) {
                Ok(output) => {
                    info!("  [OK] Trigger '{}' completed", trigger.name);
                    ChangesetTrigger::mark_completed(self.conn, changeset_id, trigger_id, output.as_deref())?;
                    results.succeeded += 1;
                }
                Err(e) => {
                    warn!("  [FAIL] Trigger '{}': {}", trigger.name, e);
                    ChangesetTrigger::mark_failed(self.conn, changeset_id, trigger_id, &e.to_string())?;
                    results.failed += 1;
                    results.errors.push(format!("{}: {}", trigger.name, e));
                }
            }
        }

        Ok(results)
    }

    /// Execute a single trigger handler
    fn execute_handler(&self, trigger: &Trigger) -> Result<Option<String>> {
        // Parse handler command
        let parts: Vec<&str> = trigger.handler.split_whitespace().collect();
        if parts.is_empty() {
            return Err(Error::TriggerError("Empty handler command".to_string()));
        }

        let cmd = parts[0];
        let args = &parts[1..];

        debug!("Executing: {} {:?}", cmd, args);

        // Execute with timeout and stdin nullification
        let mut child = Command::new(cmd)
            .args(args)
            .env("CONARY_TRIGGER_NAME", &trigger.name)
            .env("CONARY_ROOT", self.root.to_string_lossy().as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::TriggerError(format!("Failed to spawn '{}': {}", cmd, e)))?;

        // Wait with timeout
        match child.wait_timeout(self.timeout)? {
            Some(status) => {
                let output = child.wait_with_output()?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                // Log output
                if !stdout.is_empty() {
                    for line in stdout.lines() {
                        debug!("[{}] {}", trigger.name, line);
                    }
                }
                if !stderr.is_empty() {
                    for line in stderr.lines() {
                        warn!("[{}] {}", trigger.name, line);
                    }
                }

                if status.success() {
                    let combined = format!("{}{}", stdout, stderr);
                    Ok(if combined.is_empty() { None } else { Some(combined) })
                } else {
                    let code = status.code().unwrap_or(-1);
                    Err(Error::TriggerError(format!(
                        "Handler '{}' failed with exit code {}: {}",
                        cmd, code, stderr.trim()
                    )))
                }
            }
            None => {
                // Timeout - kill the process
                let _ = child.kill();
                Err(Error::TriggerError(format!(
                    "Handler '{}' timed out after {} seconds",
                    cmd,
                    self.timeout.as_secs()
                )))
            }
        }
    }
}

/// Check if a handler command exists on the system
fn handler_exists(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }

    // If it's an absolute path, check file existence
    if cmd.starts_with('/') {
        return Path::new(cmd).exists();
    }

    // Otherwise, check if it's in PATH
    if let Ok(output) = Command::new("which").arg(cmd).output() {
        return output.status.success();
    }

    false
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
    fn new() -> Self {
        Self::default()
    }

    fn empty() -> Self {
        Self::default()
    }

    fn skipped() -> Self {
        Self {
            skipped: 1,
            ..Self::default()
        }
    }

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
    fn test_handler_exists() {
        // Common commands that should exist
        assert!(handler_exists("/bin/true") || handler_exists("/usr/bin/true"));

        // Commands that shouldn't exist
        assert!(!handler_exists("/nonexistent/command"));
        assert!(!handler_exists(""));
    }

    #[test]
    fn test_trigger_results() {
        let mut results = TriggerResults::new();
        results.succeeded = 5;
        results.failed = 1;
        results.skipped = 2;

        assert!(!results.all_succeeded());
        assert_eq!(results.total(), 8);

        let results2 = TriggerResults { succeeded: 3, ..TriggerResults::default() };
        assert!(results2.all_succeeded());
    }
}
