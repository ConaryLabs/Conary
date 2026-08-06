// conary-test/src/engine/qemu/deadline.rs
//! One monotonic runtime budget for a complete QEMU manifest step.

use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use thiserror::Error;
use tokio::process::Command;
use tokio::time::Instant;

use crate::container::backend::ExecResult;

/// A complete-step or QEMU-operation runtime budget was exhausted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(super) enum QemuTimeoutError {
    #[error("QEMU step timed out before {operation}; the complete step budget is {total:?}")]
    Step { operation: String, total: Duration },
    #[error("{operation} timed out after {timeout:?}")]
    Operation {
        operation: String,
        timeout: Duration,
    },
}

impl QemuTimeoutError {
    pub(super) fn operation(operation: impl Into<String>, timeout: Duration) -> Self {
        Self::Operation {
            operation: operation.into(),
            timeout,
        }
    }
}

/// Shared monotonic deadline consumed by every operation in one QEMU step.
#[derive(Debug)]
pub(super) struct QemuStepDeadline {
    expires_at: Instant,
    total: Duration,
    operation: Mutex<String>,
}

impl QemuStepDeadline {
    pub(super) fn new(total: Duration) -> Self {
        Self::from_start(Instant::now(), total)
    }

    fn from_start(started_at: Instant, total: Duration) -> Self {
        Self {
            expires_at: started_at + total,
            total,
            operation: Mutex::new("QEMU setup".to_string()),
        }
    }

    /// Return the smaller of the remaining step budget and an operation cap.
    ///
    /// The operation cap preserves the QEMU-specific per-operation limit, but
    /// it never resets the complete step budget.
    pub(super) fn remaining(
        &self,
        operation: impl Into<String>,
        operation_cap: Duration,
    ) -> Result<Duration, QemuTimeoutError> {
        self.remaining_at(Instant::now(), operation, operation_cap)
    }

    fn remaining_at(
        &self,
        now: Instant,
        operation: impl Into<String>,
        operation_cap: Duration,
    ) -> Result<Duration, QemuTimeoutError> {
        let operation = operation.into();
        self.operation
            .lock()
            .expect("QEMU deadline operation lock is not poisoned")
            .clone_from(&operation);
        if operation_cap.is_zero() {
            return Err(QemuTimeoutError::operation(operation, operation_cap));
        }
        let Some(remaining) = self.expires_at.checked_duration_since(now) else {
            return Err(QemuTimeoutError::Step {
                operation,
                total: self.total,
            });
        };
        if remaining.is_zero() {
            return Err(QemuTimeoutError::Step {
                operation,
                total: self.total,
            });
        }
        Ok(remaining.min(operation_cap))
    }

    fn timeout_result(&self) -> ExecResult {
        let operation = self
            .operation
            .lock()
            .expect("QEMU deadline operation lock is not poisoned");
        ExecResult {
            exit_code: 124,
            stdout: String::new(),
            stderr: format!(
                "QEMU step timed out during {operation}; the complete step budget is {:?}",
                self.total
            ),
        }
    }
}

/// Apply the manifest-effective hard deadline around the complete QEMU future.
///
/// Dropping the future drops its `kill_on_drop` QEMU child, so timeout returns
/// a typed failed result without leaving the VM detached.
pub(super) async fn enforce_qemu_step_timeout<F>(
    deadline: &QemuStepDeadline,
    step: F,
) -> Result<ExecResult>
where
    F: std::future::Future<Output = Result<ExecResult>>,
{
    let remaining = deadline
        .expires_at
        .checked_duration_since(Instant::now())
        .unwrap_or_default();
    if remaining.is_zero() {
        return Ok(deadline.timeout_result());
    }
    match tokio::time::timeout(remaining, step).await {
        Ok(result) => result,
        Err(_) => Ok(deadline.timeout_result()),
    }
}

/// Run a host command within one operation budget and terminate its complete
/// process group when that budget expires.
pub(super) async fn run_command_with_timeout(
    mut command: Command,
    timeout: Duration,
    description: &str,
) -> Result<ExecResult> {
    #[cfg(unix)]
    command.process_group(0);
    command.kill_on_drop(true);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let child = command
        .spawn()
        .with_context(|| format!("failed to start {description}"))?;
    let child_id = child.id();

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => {
            let output = output.with_context(|| format!("failed to wait for {description}"))?;
            Ok(ExecResult {
                exit_code: output.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
        Err(_) => {
            #[cfg(unix)]
            if let Some(child_id) = child_id.and_then(|id| i32::try_from(id).ok()) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(child_id),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            Ok(ExecResult {
                exit_code: 124,
                stdout: String::new(),
                stderr: format!("{description} timed out after {}s", timeout.as_secs()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successive_operations_consume_one_shared_budget() {
        let start = Instant::now();
        let deadline = QemuStepDeadline::from_start(start, Duration::from_millis(50));

        assert_eq!(
            deadline
                .remaining_at(start, "boot", Duration::from_millis(20))
                .unwrap(),
            Duration::from_millis(20)
        );
        assert_eq!(
            deadline
                .remaining_at(
                    start + Duration::from_millis(35),
                    "guest command",
                    Duration::from_millis(20),
                )
                .unwrap(),
            Duration::from_millis(15)
        );

        let error = deadline
            .remaining_at(
                start + Duration::from_millis(50),
                "guest copy",
                Duration::from_millis(20),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "QEMU step timed out before guest copy; the complete step budget is 50ms"
        );
    }

    #[test]
    fn operation_cap_never_extends_the_step_deadline() {
        let start = Instant::now();
        let deadline = QemuStepDeadline::from_start(start, Duration::from_secs(30));

        assert_eq!(
            deadline
                .remaining_at(
                    start + Duration::from_secs(10),
                    "system readiness",
                    Duration::from_secs(300),
                )
                .unwrap(),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn zero_operation_cap_is_an_immediate_typed_timeout() {
        let start = Instant::now();
        let deadline = QemuStepDeadline::from_start(start, Duration::from_secs(30));

        let error = deadline
            .remaining_at(start, "SSH readiness", Duration::ZERO)
            .unwrap_err();
        assert_eq!(error.to_string(), "SSH readiness timed out after 0ns");
    }

    #[tokio::test]
    async fn command_timeout_returns_failure_instead_of_hanging() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5"]);

        let result = run_command_with_timeout(command, Duration::from_millis(10), "slow command")
            .await
            .unwrap();

        assert_eq!(result.exit_code, 124);
        assert!(result.stderr.contains("slow command timed out"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn command_timeout_kills_descendant_process_group() {
        use std::path::PathBuf;

        let dir = tempfile::TempDir::new().unwrap();
        let pid_path = dir.path().join("descendant.pid");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "sleep 30 & child=$!; printf '%s' \"$child\" > \"$1\"; wait",
                "qemu-timeout-test",
            ])
            .arg(&pid_path);

        let result =
            run_command_with_timeout(command, Duration::from_secs(1), "process-group command")
                .await
                .unwrap();
        assert_eq!(result.exit_code, 124);

        let descendant_pid: u32 = std::fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        let proc_stat = PathBuf::from(format!("/proc/{descendant_pid}/stat"));
        for _ in 0..50 {
            let still_running = std::fs::read_to_string(&proc_stat)
                .ok()
                .and_then(|stat| stat.split_whitespace().nth(2).map(str::to_string))
                .is_some_and(|state| state != "Z");
            if !still_running {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed-out command left descendant process {descendant_pid} running");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn deadline_expiry_kills_the_owned_qemu_process() {
        use std::path::PathBuf;
        use std::process::Stdio;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let pid = Arc::new(AtomicU32::new(0));
        let owned_pid = Arc::clone(&pid);
        let step = async move {
            let mut child = tokio::process::Command::new("sh")
                .args(["-c", "sleep 30"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            owned_pid.store(child.id().unwrap(), Ordering::SeqCst);
            let status = child.wait().await.unwrap();
            Ok(ExecResult {
                exit_code: status.code().unwrap_or(1),
                stdout: String::new(),
                stderr: String::new(),
            })
        };

        let deadline = QemuStepDeadline::new(Duration::from_millis(20));
        let result = enforce_qemu_step_timeout(&deadline, step).await.unwrap();
        assert_eq!(result.exit_code, 124);
        assert_eq!(
            result.stderr,
            "QEMU step timed out during QEMU setup; the complete step budget is 20ms"
        );

        let child_pid = pid.load(Ordering::SeqCst);
        assert_ne!(child_pid, 0, "synthetic QEMU process was not spawned");
        let proc_stat = PathBuf::from(format!("/proc/{child_pid}/stat"));
        for _ in 0..50 {
            let still_running = std::fs::read_to_string(&proc_stat)
                .ok()
                .and_then(|stat| stat.split_whitespace().nth(2).map(str::to_string))
                .is_some_and(|state| state != "Z");
            if !still_running {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("deadline expiry left synthetic QEMU process {child_pid} running");
    }
}
