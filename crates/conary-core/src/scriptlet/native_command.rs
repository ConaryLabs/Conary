// conary-core/src/scriptlet/native_command.rs

//! Exact argv execution for formally parsed native transaction hooks.

use super::ScriptletExecutor;
use super::activation_capture::ActivationCaptureSession;
use super::boot_runtime_capture::BootRuntimeCaptureSession;
use super::process::{TargetRootScript, configure_target_command_boundary_with_mounts};
use super::runtime::{apply_sanitized_command_env, log_script_output};
use crate::child_wait::wait_with_output_process_group;
use crate::error::{Error, Result, ScriptletFailureKind};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeCommandStatus {
    Success,
    ExitCode(i32),
    Signal(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeCommandOutput {
    pub(super) status: NativeCommandStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

impl ScriptletExecutor {
    /// Validate a formally parsed transaction command before the transaction
    /// runs any lifecycle event or mutates package state.
    pub fn preflight_native_command(&self, argv: &[String]) -> Result<()> {
        let (requested_program, _) = argv.split_first().ok_or_else(|| {
            Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                "native transaction hook has an empty argv",
            )
        })?;
        if argv.iter().any(|argument| argument.contains('\0')) {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                "native transaction hook argv contains NUL",
            ));
        }

        self.require_target_root()?;
        resolve_target_program(&self.root, requested_program, &[])?;
        if !nix::unistd::geteuid().is_root() {
            return Err(Error::scriptlet(
                ScriptletFailureKind::SandboxSetupUnavailable,
                format!(
                    "native transaction hook execution in target root {} requires root privileges",
                    self.root.display()
                ),
            ));
        }
        Ok(())
    }

    /// Execute a formally parsed package-manager hook without a shell.
    ///
    /// The caller supplies the exact argv and stdin from the native transaction
    /// planner. This function deliberately does not infer behavior from a
    /// command string or script body.
    pub fn execute_native_command(&self, phase: &str, argv: &[String], stdin: &[u8]) -> Result<()> {
        match self.execute_native_command_status(phase, argv, stdin)? {
            NativeCommandStatus::Success => Ok(()),
            NativeCommandStatus::ExitCode(code) => Err(Error::scriptlet(
                ScriptletFailureKind::ScriptExited,
                format!("{phase} native transaction hook failed with exit code {code}"),
            )),
            NativeCommandStatus::Signal(signal) => Err(Error::scriptlet(
                ScriptletFailureKind::ScriptExited,
                format!("{phase} native transaction hook terminated by signal {signal}"),
            )),
        }
    }

    pub(super) fn execute_native_command_status(
        &self,
        phase: &str,
        argv: &[String],
        stdin: &[u8],
    ) -> Result<NativeCommandStatus> {
        Ok(self
            .execute_native_command_output(phase, argv, stdin, &[], true)?
            .status)
    }

    /// Execute a parsed command in the same target root as its scriptlet and
    /// return its bytes without treating a non-zero status as a transport
    /// failure. RPM's `%()` macro consumes this exact distinction.
    pub(super) fn execute_native_command_capture(
        &self,
        phase: &str,
        argv: &[String],
        stdin: &[u8],
        environment: &[(&str, &str)],
    ) -> Result<NativeCommandOutput> {
        self.execute_native_command_output(phase, argv, stdin, environment, false)
    }

    /// Execute an exact argv with the lifecycle entry's validated environment.
    ///
    /// This is used by source package ABIs whose package manager invokes a
    /// configured program directly instead of executing a generated script
    /// file. The argv remains the caller's typed contract.
    pub(super) fn execute_native_command_with_environment(
        &self,
        phase: &str,
        argv: &[String],
        stdin: &[u8],
        environment: &[(&str, &str)],
    ) -> Result<()> {
        match self
            .execute_native_command_output(phase, argv, stdin, environment, true)?
            .status
        {
            NativeCommandStatus::Success => Ok(()),
            NativeCommandStatus::ExitCode(code) => Err(Error::scriptlet(
                ScriptletFailureKind::ScriptExited,
                format!("{phase} native lifecycle command failed with exit code {code}"),
            )),
            NativeCommandStatus::Signal(signal) => Err(Error::scriptlet(
                ScriptletFailureKind::ScriptExited,
                format!("{phase} native lifecycle command terminated by signal {signal}"),
            )),
        }
    }

    fn execute_native_command_output(
        &self,
        phase: &str,
        argv: &[String],
        stdin: &[u8],
        environment: &[(&str, &str)],
        log_output: bool,
    ) -> Result<NativeCommandOutput> {
        let (requested_program, args) = argv.split_first().ok_or_else(|| {
            Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                "native transaction hook has an empty argv",
            )
        })?;
        if argv.iter().any(|argument| argument.contains('\0')) {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                "native transaction hook argv contains NUL",
            ));
        }

        let program = resolve_target_program(&self.root, requested_program, environment)?;
        let mut stdin_file = tempfile::tempfile()?;
        stdin_file.write_all(stdin)?;
        stdin_file.seek(SeekFrom::Start(0))?;

        let mut command = Command::new(&program);
        command
            .args(args)
            .stdin(Stdio::from(stdin_file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut command_environment = vec![
            ("CONARY_PACKAGE_NAME", self.package_name.as_str()),
            ("CONARY_PACKAGE_VERSION", self.package_version.as_str()),
        ];
        command_environment.extend_from_slice(environment);

        self.require_target_root()?;
        apply_sanitized_command_env(&mut command, &command_environment);

        let program_in_root = self.root.join(program.trim_start_matches('/'));
        if !program_in_root.exists() {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ProgramUnavailable,
                format!(
                    "native transaction hook program {} is missing from target root {}",
                    program,
                    self.root.display()
                ),
            ));
        }
        // Direct-output commands include RPM macro substitutions and embedded
        // Lua process calls. They share the same runtime-effect boundary as
        // logged transaction hooks, so capture them even when their stdout is
        // returned to the source interpreter instead of logged.
        let mut capture_staging = TargetRootScript::stage(&self.root, b"")?;
        let capture =
            ActivationCaptureSession::stage(&mut capture_staging, &self.root, environment)?;
        let mut boot_capture = BootRuntimeCaptureSession::stage(
            &mut capture_staging,
            &self.root,
            environment,
            self.timeout,
            self.lifecycle_bridge.as_ref(),
        )?;
        let mut mounts = capture.mounts().to_vec();
        mounts.extend_from_slice(boot_capture.mounts());
        boot_capture.configure_command(&mut command)?;
        let boundary =
            configure_target_command_boundary_with_mounts(&mut command, &self.root, &mounts)?;
        tracing::debug!(
            phase,
            program,
            target_root = %self.root.display(),
            boundary = boundary.contract,
            "executing exact native argv in the lifecycle target boundary"
        );

        let mut child = command.spawn().map_err(|error| {
            Error::scriptlet(
                ScriptletFailureKind::ProcessSetupFailed,
                format!("failed to spawn native transaction hook {program}: {error}"),
            )
        })?;
        boot_capture.child_spawned();
        let outcome = wait_with_output_process_group(&mut child, self.timeout)?;
        if log_output {
            log_script_output(
                phase,
                &String::from_utf8_lossy(&outcome.stdout),
                &String::from_utf8_lossy(&outcome.stderr),
            );
        } else if !outcome.stderr.is_empty() {
            tracing::warn!(
                target: "conary::rpm_macro",
                phase,
                stderr = %String::from_utf8_lossy(&outcome.stderr),
                "RPM macro command wrote to stderr"
            );
        }
        if outcome.timed_out {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ScriptTimedOut,
                format!(
                    "{phase} native transaction hook timed out after {} seconds",
                    self.timeout.as_secs()
                ),
            ));
        }
        let status = outcome
            .status
            .expect("child wait helper returns status when it did not time out");
        let status = if status.success() {
            NativeCommandStatus::Success
        } else if let Some(code) = status.code() {
            NativeCommandStatus::ExitCode(code)
        } else {
            NativeCommandStatus::Signal(status.signal().unwrap_or(-1))
        };
        let boot_invocations = boot_capture.finish()?;
        if status == NativeCommandStatus::Success {
            self.activation_invocations
                .lock()
                .expect("activation invocation collector poisoned")
                .extend(capture.finish()?);
            self.boot_runtime_invocations
                .lock()
                .expect("boot runtime invocation collector poisoned")
                .extend(boot_invocations);
        }
        Ok(NativeCommandOutput {
            status,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
        })
    }
}

fn resolve_target_program(
    root: &Path,
    requested: &str,
    environment: &[(&str, &str)],
) -> Result<String> {
    if requested.contains('/') {
        let guest = if requested.starts_with('/') {
            requested.to_string()
        } else {
            format!("/{requested}")
        };
        ensure_target_executable(root, &guest)?;
        return Ok(guest);
    }

    let path = environment
        .iter()
        .rev()
        .find_map(|(key, value)| (*key == "PATH").then_some(*value))
        .unwrap_or("/usr/sbin:/usr/bin:/sbin:/bin");
    for directory in path.split(':') {
        if !directory.starts_with('/') {
            continue;
        }
        let guest = format!("{}/{}", directory.trim_end_matches('/'), requested);
        let host = root.join(guest.trim_start_matches('/'));
        if is_executable_file(&host) {
            return Ok(guest);
        }
    }
    Err(Error::scriptlet(
        ScriptletFailureKind::ProgramUnavailable,
        format!("native transaction hook program {requested} is missing from target PATH {path}"),
    ))
}

fn ensure_target_executable(root: &Path, guest: &str) -> Result<()> {
    let host = root.join(guest.trim_start_matches('/'));
    if is_executable_file(&host) {
        Ok(())
    } else {
        Err(Error::scriptlet(
            ScriptletFailureKind::ProgramUnavailable,
            format!(
                "native transaction hook program {guest} is missing from target root {}",
                root.display()
            ),
        ))
    }
}

fn is_executable_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scriptlet::PackageFormat;

    fn executor(root: &Path) -> ScriptletExecutor {
        ScriptletExecutor::new(root, "fixture", "1", PackageFormat::Rpm)
    }

    #[test]
    fn transaction_command_preflight_resolves_the_exact_program() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let shell = bin.join("sh");
        std::fs::write(&shell, b"#!/bin/sh\n").unwrap();
        let mut permissions = shell.metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&shell, permissions).unwrap();
        assert_eq!(
            resolve_target_program(root.path(), "/bin/sh", &[]).unwrap(),
            "/bin/sh"
        );
    }

    #[test]
    fn transaction_command_preflight_rejects_missing_or_malformed_programs() {
        let root = tempfile::tempdir().unwrap();
        let executor = executor(root.path());
        assert!(executor.preflight_native_command(&[]).is_err());
        assert!(
            executor
                .preflight_native_command(&["/definitely/missing/conary-hook".to_string()])
                .is_err()
        );
        assert!(
            executor
                .preflight_native_command(&["/bin/sh\0bad".to_string()])
                .is_err()
        );
    }

    #[test]
    fn transaction_command_preflight_rejects_the_host_root() {
        let executor = executor(Path::new("/"));
        let error = executor
            .preflight_native_command(&["/bin/sh".to_string()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("host root '/' is not an execution target"));
    }
}
