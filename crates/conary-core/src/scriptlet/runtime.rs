// conary-core/src/scriptlet/runtime.rs

use crate::capability::enforcement::EnforcementMode;
use crate::error::{Error, Result};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{info, warn};
use wait_timeout::ChildExt;

static SECCOMP_WARN_OVERRIDE: AtomicBool = AtomicBool::new(false);

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

pub(super) fn set_seccomp_warn_override(enabled: bool) {
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
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    match child.wait_timeout(timeout)? {
        Some(status) => {
            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            if let Some(ref mut out) = stdout_handle {
                let _ = std::io::Read::read_to_end(out, &mut stdout_bytes);
            }
            if let Some(ref mut err) = stderr_handle {
                let _ = std::io::Read::read_to_end(err, &mut stderr_bytes);
            }
            log_script_output(
                phase,
                &String::from_utf8_lossy(&stdout_bytes),
                &String::from_utf8_lossy(&stderr_bytes),
            );
            check_scriptlet_status(phase, status, context)
        }
        None => {
            let _ = child.kill();
            let status = child.wait().ok();
            let signal = status.and_then(|status| ExitStatusExt::signal(&status));
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
        }
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
