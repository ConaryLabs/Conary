// conary-core/src/trigger/execution.rs

use super::TriggerExecutor;
use crate::child_wait::wait_with_output_process_group;
use crate::db::models::Trigger;
use crate::error::{Error, Result};
use crate::scriptlet::configure_target_command_boundary;
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, warn};

impl TriggerExecutor<'_> {
    /// Execute one exact trigger argv inside the materialized selected root.
    pub(super) fn execute_handler_in_target(&self, trigger: &Trigger) -> Result<Option<String>> {
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

        let mut command = Command::new(cmd);
        command
            .args(args)
            .env_clear()
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            .env("SHELL", "/bin/sh")
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("CONARY_TRIGGER_NAME", &trigger.name)
            .env("CONARY_ROOT", "/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_target_command_boundary(&mut command, self.root).map_err(|error| {
            Error::TriggerError(format!(
                "Failed to establish selected-root boundary for '{}': {error}",
                trigger.name
            ))
        })?;
        let child = command.spawn().map_err(|e| {
            Error::TriggerError(format!(
                "Failed to spawn '{}' in selected root {}: {}",
                cmd,
                self.root.display(),
                e
            ))
        })?;

        self.wait_and_capture(child, &trigger.name, cmd, self.root)
    }

    /// Wait for a spawned child process with timeout, capture output, and check status.
    ///
    fn wait_and_capture(
        &self,
        mut child: std::process::Child,
        trigger_name: &str,
        cmd: &str,
        target_root: &Path,
    ) -> Result<Option<String>> {
        let outcome = wait_with_output_process_group(&mut child, self.timeout)?;
        let stdout = String::from_utf8_lossy(&outcome.stdout);
        let stderr = String::from_utf8_lossy(&outcome.stderr);

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

        if outcome.timed_out {
            Err(Error::TriggerError(format!(
                "Handler '{}' timed out after {} seconds (selected root: {})",
                cmd,
                self.timeout.as_secs(),
                target_root.display()
            )))
        } else {
            let status = outcome
                .status
                .expect("child wait helper must return a status when not timed out");

            if status.success() {
                let combined = format!("{}{}", stdout, stderr);
                Ok(if combined.is_empty() {
                    None
                } else {
                    Some(combined)
                })
            } else {
                let code = status.code().unwrap_or(-1);
                Err(Error::TriggerError(format!(
                    "Handler '{}' failed with exit code {} (selected root: {}): {}",
                    cmd,
                    code,
                    target_root.display(),
                    stderr.trim()
                )))
            }
        }
    }
}

/// Check if a handler command exists in a target root
///
/// For absolute paths, checks under the target root.
/// For non-absolute paths, checks common bin directories in target.
pub fn handler_exists_in_root(cmd: &str, root: &Path) -> bool {
    if cmd.is_empty() {
        return false;
    }

    if cmd.starts_with('/') {
        let target_path = match crate::filesystem::path::safe_join(root, cmd) {
            Ok(path) => path,
            Err(_) => return false,
        };
        return target_path.exists();
    }

    let search_paths = [
        "usr/bin",
        "usr/sbin",
        "bin",
        "sbin",
        "usr/local/bin",
        "usr/local/sbin",
    ];

    for search_path in search_paths {
        let target_path = root.join(search_path).join(cmd);
        if target_path.exists() {
            return true;
        }
    }

    false
}

/// Split a command string into tokens, respecting single and double quotes.
///
/// Handles the common shell quoting rules:
/// - Unquoted tokens are split on whitespace
/// - Single-quoted strings preserve literal content (no escaping)
/// - Double-quoted strings allow `\"` and `\\` escapes
///
/// Returns an error for unterminated quotes.
pub(super) fn shell_split(input: &str) -> std::result::Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if in_double => {
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
    use super::handler_exists_in_root;

    #[test]
    fn handler_lookup_is_confined_to_the_selected_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr/bin")).unwrap();
        std::fs::write(root.path().join("usr/bin/fixture-trigger"), b"fixture").unwrap();
        assert!(handler_exists_in_root("fixture-trigger", root.path()));
        assert!(handler_exists_in_root(
            "/usr/bin/fixture-trigger",
            root.path()
        ));
        assert!(!handler_exists_in_root("/bin/true", root.path()));
        assert!(!handler_exists_in_root("", root.path()));
    }
}
