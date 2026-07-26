// conary-core/src/scriptlet/runtime.rs

use crate::child_wait::wait_with_output_process_group;
use crate::error::{Error, Result, ScriptletFailureKind};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus};
use std::time::Duration;
use tracing::{info, warn};

pub(super) fn apply_sanitized_command_env(cmd: &mut Command, env: &[(&str, &str)]) {
    cmd.env_clear()
        .env("HOME", "/root")
        .env("TERM", "dumb")
        .env("LANG", "C.UTF-8")
        .env("SHELL", "/bin/sh");

    if !env.iter().any(|(key, _)| *key == "PATH") {
        cmd.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
    }

    for (key, value) in env {
        cmd.env(*key, *value);
    }
}

/// Log captured stdout/stderr lines with a phase prefix.
pub(super) fn log_script_output(phase: &str, stdout: &str, stderr: &str) {
    if !stdout.is_empty() {
        for line in stdout.lines() {
            info!("[{}] {}", phase, line);
        }
    }
    if !stderr.is_empty() {
        for line in stderr.lines() {
            warn!("[{}] {}", phase, line);
        }
    }
}

/// Check an exit status from a scriptlet and return an appropriate error.
fn check_scriptlet_status(phase: &str, status: ExitStatus, context: &str) -> Result<()> {
    if status.success() {
        info!("{} scriptlet completed successfully{}", phase, context);
        Ok(())
    } else {
        let failure = match (status.code(), status.signal()) {
            (Some(code), _) => format!("failed with exit code {code}"),
            (None, Some(signal)) => format!("terminated by signal {signal}"),
            (None, None) => "terminated without an exit code or signal".to_string(),
        };
        Err(Error::scriptlet(
            ScriptletFailureKind::ScriptExited,
            format!("{phase} scriptlet {failure}{context}"),
        ))
    }
}

/// Wait for a child process to exit (with timeout), capture its stdout/stderr,
/// log the output, and check the exit status.
///
/// Takes the stdout/stderr pipe handles before waiting so that draining them
/// after the child exits is safe and cannot race with a double-wait (ECHILD).
pub(super) fn wait_and_capture(
    child: &mut std::process::Child,
    timeout: Duration,
    phase: &str,
    context: &str,
) -> Result<()> {
    let outcome = wait_with_output_process_group(child, timeout)?;
    let stdout = String::from_utf8_lossy(&outcome.stdout);
    let stderr = String::from_utf8_lossy(&outcome.stderr);

    log_script_output(phase, &stdout, &stderr);

    if outcome.timed_out {
        let signal = outcome
            .status
            .and_then(|status| ExitStatusExt::signal(&status));
        let suffix = signal
            .map(|sig| format!(" (killed with signal {sig})"))
            .unwrap_or_default();
        Err(Error::scriptlet(
            ScriptletFailureKind::ScriptTimedOut,
            format!(
                "{} scriptlet timed out after {} seconds{}{}",
                phase,
                timeout.as_secs(),
                context,
                suffix
            ),
        ))
    } else {
        check_scriptlet_status(
            phase,
            outcome
                .status
                .expect("child wait helper must return a status when not timed out"),
            context,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_terminated_scriptlet_reports_the_exact_signal() {
        let status = ExitStatus::from_raw(libc::SIGSYS);
        let error = check_scriptlet_status("post-install", status, " (package: example)")
            .expect_err("signal termination must fail the scriptlet");

        assert!(error.to_string().contains(&format!(
            "post-install scriptlet terminated by signal {} (package: example)",
            libc::SIGSYS
        )));
    }
}
