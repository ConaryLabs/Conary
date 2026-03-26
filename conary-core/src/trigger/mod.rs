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

    /// Execute a single trigger handler
    fn execute_handler(&self, trigger: &Trigger) -> Result<Option<String>> {
        let parts = shell_split(&trigger.handler).map_err(|e| {
            Error::TriggerError(format!(
                "Failed to parse handler '{}': {e}",
                trigger.handler
            ))
        })?;
        if parts.is_empty() {
            return Err(Error::TriggerError("Empty handler command".to_string()));
        }

        let cmd = parts[0].as_str();
        let args: Vec<&str> = parts[1..].iter().map(String::as_str).collect();

        debug!("Executing: {} {:?}", cmd, args);

        let root_string = self.root.to_string_lossy().into_owned();
        let child = Command::new(cmd)
            .args(args)
            .env("CONARY_TRIGGER_NAME", &trigger.name)
            .env("CONARY_ROOT", root_string)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::TriggerError(format!("Failed to spawn '{}': {}", cmd, e)))?;

        self.wait_and_capture(child, &trigger.name, cmd, None)
    }

    /// Execute a trigger handler inside a target root using chroot
    ///
    /// This method runs the handler inside the target filesystem, which is
    /// necessary for triggers to work correctly during bootstrap or when
    /// installing to a non-live filesystem.
    fn execute_handler_in_target(&self, trigger: &Trigger) -> Result<Option<String>> {
        let parts = shell_split(&trigger.handler).map_err(|e| {
            Error::TriggerError(format!(
                "Failed to parse handler '{}': {e}",
                trigger.handler
            ))
        })?;
        if parts.is_empty() {
            return Err(Error::TriggerError("Empty handler command".to_string()));
        }

        let cmd = parts[0].as_str();
        let args: Vec<&str> = parts[1..].iter().map(String::as_str).collect();

        debug!(
            "Executing in chroot {}: {} {:?}",
            self.root.display(),
            cmd,
            args
        );

        // Check if we have root privileges (required for chroot)
        if !nix::unistd::geteuid().is_root() {
            warn!(
                "Target root trigger execution requires root privileges, skipping '{}'",
                trigger.name
            );
            return Ok(Some(
                "Skipped: target root execution requires root privileges".to_string(),
            ));
        }

        let child = Command::new("chroot")
            .arg(self.root)
            .arg(cmd)
            .args(args)
            .env("CONARY_TRIGGER_NAME", &trigger.name)
            .env("CONARY_ROOT", "/") // From inside chroot, root is always /
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::TriggerError(format!("Failed to spawn chroot for '{}': {}", cmd, e))
            })?;

        self.wait_and_capture(child, &trigger.name, cmd, Some(self.root))
    }

    /// Wait for a spawned child process with timeout, capture output, and check status.
    ///
    /// If `chroot_path` is `Some`, error messages include the chroot context.
    fn wait_and_capture(
        &self,
        mut child: std::process::Child,
        trigger_name: &str,
        cmd: &str,
        chroot_path: Option<&Path>,
    ) -> Result<Option<String>> {
        match child.wait_timeout(self.timeout)? {
            Some(status) => {
                // Process already exited; read buffered output without
                // calling wait_with_output (which would double-wait).
                let stdout_bytes = child
                    .stdout
                    .take()
                    .map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stderr_bytes = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stdout = String::from_utf8_lossy(&stdout_bytes);
                let stderr = String::from_utf8_lossy(&stderr_bytes);

                // Log output
                if !stdout.is_empty() {
                    for line in stdout.lines() {
                        debug!("[{}] {}", trigger_name, line);
                    }
                }
                if !stderr.is_empty() {
                    for line in stderr.lines() {
                        warn!("[{}] {}", trigger_name, line);
                    }
                }

                if status.success() {
                    let combined = format!("{}{}", stdout, stderr);
                    Ok(if combined.is_empty() {
                        None
                    } else {
                        Some(combined)
                    })
                } else {
                    let code = status.code().unwrap_or(-1);
                    let context = match chroot_path {
                        Some(root) => format!(
                            "Handler '{}' failed with exit code {} (chroot: {}): {}",
                            cmd,
                            code,
                            root.display(),
                            stderr.trim()
                        ),
                        None => format!(
                            "Handler '{}' failed with exit code {}: {}",
                            cmd,
                            code,
                            stderr.trim()
                        ),
                    };
                    Err(Error::TriggerError(context))
                }
            }
            None => {
                // Timeout - kill the process
                let _ = child.kill();
                let context = match chroot_path {
                    Some(root) => format!(
                        "Handler '{}' timed out after {} seconds (chroot: {})",
                        cmd,
                        self.timeout.as_secs(),
                        root.display()
                    ),
                    None => format!(
                        "Handler '{}' timed out after {} seconds",
                        cmd,
                        self.timeout.as_secs()
                    ),
                };
                Err(Error::TriggerError(context))
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

/// Check if a handler command exists in a target root
///
/// For absolute paths, checks under the target root.
/// For non-absolute paths, checks common bin directories in target.
pub fn handler_exists_in_root(cmd: &str, root: &Path) -> bool {
    if cmd.is_empty() {
        return false;
    }

    // If it's an absolute path, check under target root
    if cmd.starts_with('/') {
        let target_path = match crate::filesystem::path::safe_join(root, cmd) {
            Ok(p) => p,
            Err(_) => return false, // Path traversal attempt -- handler doesn't exist
        };
        return target_path.exists();
    }

    // Otherwise, check common bin directories in target
    let search_paths = [
        "usr/bin",
        "usr/sbin",
        "bin",
        "sbin",
        "usr/local/bin",
        "usr/local/sbin",
    ];

    for search_path in &search_paths {
        let target_path = root.join(search_path).join(cmd);
        if target_path.exists() {
            return true;
        }
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
    /// Check if all triggers succeeded
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0
    }

    /// Total triggers processed
    pub fn total(&self) -> usize {
        self.succeeded + self.failed + self.skipped
    }
}

/// Split a command string into tokens, respecting single and double quotes.
///
/// Handles the common shell quoting rules:
/// - Unquoted tokens are split on whitespace
/// - Single-quoted strings preserve literal content (no escaping)
/// - Double-quoted strings allow `\"` and `\\` escapes
///
/// Returns an error for unterminated quotes.
fn shell_split(input: &str) -> std::result::Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '\\' if in_double => {
                // Inside double quotes, only \" and \\ are special
                if let Some(&next) = chars.peek() {
                    if next == '"' || next == '\\' {
                        current.push(chars.next().unwrap());
                    } else {
                        current.push('\\');
                    }
                } else {
                    current.push('\\');
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }

    if in_single {
        return Err("Unterminated single quote".to_string());
    }
    if in_double {
        return Err("Unterminated double quote".to_string());
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
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
