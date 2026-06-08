// conary-core/src/scriptlet/process.rs

use super::ScriptletExecutor;
use super::runtime::{
    apply_sanitized_command_env, build_scriptlet_seccomp, chroot_mount_private_flags,
    chroot_namespace_flags, current_seccomp_mode, log_script_output, wait_and_capture,
    write_executable_script,
};
use crate::capability::enforcement::EnforcementMode;
use crate::container::Sandbox;
use crate::error::{Error, Result};
use std::fs;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};

impl ScriptletExecutor {
    /// Execute scriptlet in sandbox on live root
    pub(super) fn execute_sandbox_live(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        // Protected live-root mode gives scriptlets private writable /etc and
        // /var layers, then overlays selected host identity files read-only.
        // Setup failures are fatal so this mode never silently downgrades to
        // host-writable /etc or /var.
        let mut sandbox = Sandbox::new(self.live_sandbox_config()?);
        let (code, stdout, stderr) = sandbox.execute(interpreter, content, args, env)?;

        log_script_output(phase, &stdout, &stderr);

        if code == 0 {
            info!("{} scriptlet completed successfully (sandboxed)", phase);
            Ok(())
        } else {
            Err(Error::ScriptletError(format!(
                "{} scriptlet failed with exit code {} (sandboxed)",
                phase, code
            )))
        }
    }

    /// Execute scriptlet inside a target root using chroot/container
    ///
    /// This is the key method for bootstrap support. It runs the scriptlet
    /// inside the target filesystem using either:
    /// - chroot (requires root, simpler)
    /// - namespace container (more isolation)
    pub(super) fn execute_in_target(
        &self,
        phase: &str,
        interpreter: &str,
        interpreter_args: &[String],
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        let temp_dir = TempDir::new()?;
        let script_path = temp_dir.path().join("scriptlet.sh");
        write_executable_script(&script_path, content)?;

        // Copy script into target root temporarily
        let target_script_dir = self.root.join("tmp/conary-scriptlets");
        fs::create_dir_all(&target_script_dir)?;
        let target_script_path = target_script_dir.join("scriptlet.sh");
        fs::copy(&script_path, &target_script_path)?;

        // Build chroot command
        // Using unshare for isolation when available, falling back to plain chroot
        let result = if nix::unistd::geteuid().is_root() {
            self.execute_with_chroot(
                phase,
                interpreter,
                interpreter_args,
                &target_script_path,
                args,
                env,
            )
        } else {
            // Non-root: try unshare with user namespace, fall back to error
            warn!("Target root scriptlet execution requires root privileges or user namespaces");
            Err(Error::ScriptletError(format!(
                "Cannot execute {} scriptlet in target root without root privileges",
                phase
            )))
        };

        // Cleanup
        let _ = fs::remove_file(&target_script_path);
        let _ = fs::remove_dir(&target_script_dir);

        result
    }

    /// Execute scriptlet using native chroot + seccomp (requires root)
    ///
    /// Uses `pre_exec` to chroot and apply seccomp in the child process,
    /// instead of spawning the external `chroot` command. This enables
    /// syscall filtering via seccomp-BPF for defense-in-depth.
    pub(super) fn execute_with_chroot(
        &self,
        phase: &str,
        interpreter: &str,
        interpreter_args: &[String],
        script_path: &Path,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        // Script path relative to chroot
        let script_in_chroot = script_path.strip_prefix(&self.root).unwrap_or(script_path);
        let script_in_chroot = format!("/{}", script_in_chroot.display());
        let root = self.root.clone();

        // Build seccomp BPF filter in parent process (avoids allocation after fork)
        let seccomp_mode = current_seccomp_mode();
        let bpf_filter = build_scriptlet_seccomp(seccomp_mode);
        let seccomp_enabled = bpf_filter.is_some();

        debug!(
            "Executing in chroot {}: {} {} {:?} (seccomp: {})",
            self.root.display(),
            interpreter,
            script_in_chroot,
            args,
            if seccomp_enabled {
                "enabled"
            } else {
                "unavailable"
            }
        );

        // Use interpreter directly with pre_exec for native chroot + seccomp
        let mut cmd = Command::new(interpreter);
        cmd.args(interpreter_args)
            .arg(&script_in_chroot)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env {
            cmd.env(*key, *value);
        }

        // Safety: pre_exec runs between fork and exec in the child process.
        // All operations (chroot, chdir, prctl, seccomp) are async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                // 1. Isolate mount topology before entering the target root.
                nix::sched::unshare(chroot_namespace_flags())
                    .map_err(|e| std::io::Error::other(format!("unshare failed: {e}")))?;
                nix::mount::mount::<str, str, str, str>(
                    None,
                    "/",
                    None,
                    chroot_mount_private_flags(),
                    None,
                )
                .map_err(|e| std::io::Error::other(format!("mount --make-rprivate failed: {e}")))?;

                // 2. chroot into the target root
                nix::unistd::chroot(&root).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("chroot failed: {e}"),
                    )
                })?;
                nix::unistd::chdir("/")
                    .map_err(|e| std::io::Error::other(format!("chdir failed: {e}")))?;

                // 3. Set NO_NEW_PRIVS (required for unprivileged seccomp)
                let ret = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
                if ret != 0 {
                    return Err(std::io::Error::last_os_error());
                }

                // 4. Apply seccomp filter
                if let Some(ref filter) = bpf_filter
                    && seccompiler::apply_filter(filter).is_err()
                {
                    // Use raw write of a static string -- no heap allocation,
                    // safe after fork in a multi-threaded process.
                    const MSG: &[u8] = b"[conary] seccomp filter application failed\n";
                    let _ = libc::write(2, MSG.as_ptr().cast(), MSG.len());

                    if seccomp_mode == EnforcementMode::Enforce {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "seccomp filter application failed in enforce mode",
                        ));
                    }
                }

                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            Error::ScriptletError(format!("Failed to spawn scriptlet in chroot: {e}"))
        })?;

        let context = format!(
            " (chroot: {}, seccomp: {})",
            self.root.display(),
            if seccomp_enabled {
                "enabled"
            } else {
                "unavailable"
            }
        );

        wait_and_capture(&mut child, self.timeout, phase, &context)
    }

    /// Execute scriptlet directly without sandbox
    pub(super) fn execute_direct(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        self.execute_direct_with_options(phase, interpreter, &[], content, args, env, self.timeout)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_direct_with_options(
        &self,
        phase: &str,
        interpreter: &str,
        interpreter_args: &[String],
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<()> {
        let temp_dir = TempDir::new()?;
        let script_path = temp_dir.path().join("scriptlet.sh");
        write_executable_script(&script_path, content)?;

        debug!(
            "Executing script: {} {} {:?}",
            interpreter,
            script_path.display(),
            args
        );

        let mut cmd = Command::new(interpreter);
        cmd.args(interpreter_args)
            .arg(&script_path)
            .args(args)
            .stdin(Stdio::null()) // CRITICAL: Prevent stdin hangs
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_sanitized_command_env(&mut cmd, env);

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::ScriptletError(format!("Failed to spawn scriptlet: {}", e)))?;

        wait_and_capture(&mut child, timeout, phase, "")
    }
}

#[cfg(test)]
mod tests {
    use super::super::runtime::ENV_LOCK;
    use super::super::{PackageFormat, SandboxMode, ScriptletExecutor};
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn test_execute_basic_success() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "echo hello",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_script_failure() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "exit 42",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed with exit code 42"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_execute_none_sandbox_runs_directly() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb)
                .with_sandbox_mode(SandboxMode::None);

        // Verify it runs and can produce output without error
        let result = executor.execute_direct(
            "pre-install",
            "/bin/sh",
            "echo 'running unsandboxed'; true",
            &["install".to_string()],
            &[
                ("CONARY_PACKAGE_NAME", "test-pkg"),
                ("CONARY_PHASE", "pre-install"),
            ],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_timeout() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_timeout(Duration::from_secs(1))
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "sleep 30",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"), "unexpected error: {}", err);
    }

    #[test]
    fn test_execute_with_env_vars() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "my-package", "2.5.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        // Script that checks environment variables are set
        let script = r#"
            test "$CONARY_PACKAGE_NAME" = "my-package" || exit 1
            test "$CONARY_PACKAGE_VERSION" = "2.5.0" || exit 2
        "#;

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            script,
            &["1".to_string()],
            &[
                ("CONARY_PACKAGE_NAME", "my-package"),
                ("CONARY_PACKAGE_VERSION", "2.5.0"),
            ],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_direct_clears_host_environment() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("CONARY_SCRIPTLET_LEAK", "host-secret");
        }

        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "test -z \"$CONARY_SCRIPTLET_LEAK\"",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );

        unsafe {
            std::env::remove_var("CONARY_SCRIPTLET_LEAK");
        }

        assert!(
            result.is_ok(),
            "direct scriptlet execution should not inherit host environment variables: {result:?}"
        );
    }

    #[test]
    fn test_execute_direct_captures_stdout_stderr_without_echild() {
        // Exercises the take-handles-before-wait pattern that prevents
        // ECHILD on double-wait when the child produces output.
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let script = r#"
            echo "stdout line"
            echo "stderr line" >&2
        "#;

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            script,
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(
            result.is_ok(),
            "Script with stdout/stderr should complete without ECHILD: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn test_execute_direct_timeout_no_double_wait_panic() {
        // The timeout path kills the child and returns an error.
        // Before the fix, calling wait_with_output after wait_timeout could
        // panic with ECHILD. This test verifies the timeout path is safe.
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_timeout(Duration::from_secs(1))
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "sleep 30",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timed out"),
            "Expected timeout error, got: {}",
            err
        );
    }

    #[test]
    fn test_execute_with_chroot_requires_root() {
        // Non-root users cannot chroot. Verify execute_in_target returns
        // an appropriate error when not running as root.
        if nix::unistd::geteuid().is_root() {
            // Skip this test when running as root; the root test below covers it.
            return;
        }

        let temp_dir = tempfile::TempDir::new().unwrap();
        let target_root = temp_dir.path();

        // Create minimal structure expected by execute_in_target
        std::fs::create_dir_all(target_root.join("tmp")).unwrap();
        std::fs::create_dir_all(target_root.join("bin")).unwrap();

        let executor = ScriptletExecutor::new(target_root, "test-pkg", "1.0.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_in_target(
            "post-install",
            "/bin/sh",
            &[],
            "echo hello",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("root privileges"),
            "Expected root-required error, got: {}",
            err
        );
    }
}
