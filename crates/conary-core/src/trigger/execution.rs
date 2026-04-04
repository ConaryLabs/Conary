// conary-core/src/trigger/execution.rs

use super::TriggerExecutor;
use crate::child_wait::wait_with_output;
use crate::db::models::Trigger;
use crate::error::{Error, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, warn};

impl TriggerExecutor<'_> {
    /// Execute a single trigger handler
    pub(super) fn execute_handler(&self, trigger: &Trigger) -> Result<Option<String>> {
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
            .env("CONARY_ROOT", "/")
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
        let outcome = wait_with_output(&mut child, self.timeout)?;
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
    }
}

/// Check if a handler command exists on the system
pub(super) fn handler_exists(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }

    if cmd.starts_with('/') {
        return Path::new(cmd).exists();
    }

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
    use super::handler_exists;

    #[test]
    fn test_handler_exists() {
        assert!(handler_exists("/bin/true") || handler_exists("/usr/bin/true"));
        assert!(!handler_exists("/nonexistent/command"));
        assert!(!handler_exists(""));
    }
}
