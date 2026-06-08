// conary-core/src/scriptlet/runtime.rs

use crate::capability::enforcement::EnforcementMode;
use crate::child_wait::wait_with_output;
use crate::error::{Error, Result};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{info, warn};

static SECCOMP_WARN_OVERRIDE: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// Write script content to a file and set it executable (mode 0o700).
///
/// Delegates to [`crate::container::write_executable_script`].
pub(super) fn write_executable_script(path: &Path, content: &str) -> Result<()> {
    crate::container::write_executable_script(path, content)
}

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
        let code = status.code().unwrap_or(-1);
        Err(Error::ScriptletError(format!(
            "{} scriptlet failed with exit code {}{}",
            phase, code, context
        )))
    }
}

pub fn set_seccomp_warn_override(enabled: bool) {
    SECCOMP_WARN_OVERRIDE.store(enabled, Ordering::Relaxed);
}

pub(super) fn current_seccomp_mode() -> EnforcementMode {
    if SECCOMP_WARN_OVERRIDE.load(Ordering::Relaxed) {
        EnforcementMode::Warn
    } else {
        EnforcementMode::Enforce
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
        Err(Error::ScriptletError(format!(
            "{} scriptlet timed out after {} seconds{}{}",
            phase,
            timeout.as_secs(),
            context,
            suffix
        )))
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
/// Uses the Scriptlet profile with the given enforcement mode.
/// Returns `None` if seccomp is not supported on this kernel.
pub(super) fn build_scriptlet_seccomp(mode: EnforcementMode) -> Option<seccompiler::BpfProgram> {
    use crate::capability::SyscallCapabilities;
    use crate::capability::enforcement::seccomp_enforce;

    if !seccomp_enforce::check_seccomp_support() {
        return None;
    }

    let caps = SyscallCapabilities {
        profile: Some("scriptlet".to_string()),
        allow: Vec::new(),
        deny: Vec::new(),
    };

    match seccomp_enforce::build_seccomp_filter(&caps, mode) {
        Ok(bpf) => {
            info!("Built seccomp filter for scriptlet execution ({mode} mode)");
            Some(bpf)
        }
        Err(e) => {
            warn!("Failed to build scriptlet seccomp filter: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::enforcement::EnforcementMode;

    #[test]
    fn test_build_scriptlet_seccomp_returns_filter() {
        // On Linux with seccomp support, this should return Some(bpf).
        // On other platforms or kernels without seccomp, it returns None.
        let result = build_scriptlet_seccomp(EnforcementMode::Warn);
        // We cannot assert Some unconditionally (CI may lack seccomp),
        // but we verify the function does not panic and returns a valid option.
        if crate::capability::enforcement::seccomp_enforce::check_seccomp_support() {
            assert!(
                result.is_some(),
                "build_scriptlet_seccomp should return Some when seccomp is supported"
            );
        } else {
            assert!(
                result.is_none(),
                "build_scriptlet_seccomp should return None when seccomp is unsupported"
            );
        }
    }

    #[test]
    fn test_current_seccomp_mode_defaults_to_enforce() {
        set_seccomp_warn_override(false);
        assert_eq!(current_seccomp_mode(), EnforcementMode::Enforce);
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
