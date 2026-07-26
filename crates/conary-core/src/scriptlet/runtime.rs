// conary-core/src/scriptlet/runtime.rs

use crate::capability::enforcement::EnforcementMode;
use crate::child_wait::wait_with_output;
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

pub(super) fn chroot_namespace_flags() -> nix::sched::CloneFlags {
    nix::sched::CloneFlags::CLONE_NEWNS
}

pub(super) fn chroot_mount_private_flags() -> nix::mount::MsFlags {
    nix::mount::MsFlags::MS_PRIVATE | nix::mount::MsFlags::MS_REC
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
    let outcome = wait_with_output(child, timeout)?;
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

/// Build a seccomp BPF filter for scriptlet execution
///
/// Uses the closed `scriptlet-v1` executor ABI with the given enforcement mode.
/// Enforce mode fails closed when the contract cannot be installed; Warn and
/// Audit retain an explicit diagnostic and may continue without a filter.
pub(super) fn build_scriptlet_seccomp() -> Result<seccompiler::BpfProgram> {
    use crate::capability::enforcement::seccomp_enforce::{
        self, SCRIPTLET_EXECUTOR_ABI_V1, scriptlet_executor_v1_capabilities,
    };

    if !seccomp_enforce::check_seccomp_support() {
        return scriptlet_seccomp_unavailable("kernel seccomp support is unavailable");
    }

    let caps = scriptlet_executor_v1_capabilities();

    match seccomp_enforce::build_seccomp_filter(&caps, EnforcementMode::Enforce) {
        Ok(bpf) => {
            info!(
                syscall_contract = SCRIPTLET_EXECUTOR_ABI_V1,
                "Built mandatory seccomp filter for scriptlet execution"
            );
            Ok(bpf)
        }
        Err(error) => {
            scriptlet_seccomp_unavailable(&format!("failed to build the executor filter: {error}"))
        }
    }
}

fn scriptlet_seccomp_unavailable(reason: &str) -> Result<seccompiler::BpfProgram> {
    use crate::capability::enforcement::seccomp_enforce::SCRIPTLET_EXECUTOR_ABI_V1;

    Err(Error::scriptlet(
        ScriptletFailureKind::EnforcementSetupFailed,
        format!(
            "Cannot enforce scriptlet syscall contract \
             {SCRIPTLET_EXECUTOR_ABI_V1}: {reason}"
        ),
    ))
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

    #[test]
    fn scriptlet_seccomp_is_mandatory() {
        if crate::capability::enforcement::seccomp_enforce::check_seccomp_support() {
            build_scriptlet_seccomp().expect("supported seccomp must build the mandatory filter");
        } else {
            assert!(
                build_scriptlet_seccomp().is_err(),
                "unsupported seccomp must reject lifecycle execution"
            );
        }
    }

    #[test]
    fn enforce_mode_fails_closed_when_scriptlet_seccomp_is_unavailable() {
        let error = scriptlet_seccomp_unavailable("forced test failure")
            .expect_err("enforce mode must not execute without scriptlet-v1");
        let message = error.to_string();
        assert!(message.contains("scriptlet-v1"));
        assert!(message.contains("forced test failure"));
    }

    #[test]
    fn test_chroot_namespace_flags_include_mount_namespace() {
        assert!(chroot_namespace_flags().contains(nix::sched::CloneFlags::CLONE_NEWNS));
    }

    #[test]
    fn test_chroot_mount_propagation_is_private_recursive() {
        let flags = chroot_mount_private_flags();
        assert!(flags.contains(nix::mount::MsFlags::MS_PRIVATE));
        assert!(flags.contains(nix::mount::MsFlags::MS_REC));
    }
}
