// conary-core/src/scriptlet/mod.rs

//! Scriptlet execution for package install/remove hooks
//!
//! This module handles executing package scriptlets with cross-distro support
//! for RPM, DEB, and Arch packages. Key features:
//!
//! - Distro-specific argument handling:
//!   - RPM: Integer count ($1=1 install, $1=2 upgrade, $1=0 remove)
//!   - DEB: Action words per Debian Policy ($1=install/configure/remove/upgrade)
//!   - Arch: Version strings ($1=new_version, $2=old_version for upgrades)
//! - Arch .INSTALL function wrapper generation
//! - Timeout protection (60 seconds)
//! - stdin nullification to prevent hangs
//! - Target root support: scriptlets can run inside a target filesystem
//! - Optional container isolation for untrusted scripts
//!
//! ## Target Root Support
//!
//! When installing to a target root (root != "/"), scriptlets are executed
//! inside a chroot or container rooted at the target path. This allows:
//! - Bootstrap: Running package scripts during system construction
//! - Container images: Populating rootfs without affecting host
//! - Offline installations: Installing packages into mounted filesystems
//!
//! The target root must have a working shell and interpreter for scriptlets
//! to execute successfully.

use crate::capability::enforcement::EnforcementMode;
use crate::container::Sandbox;
use crate::error::{Error, Result};
use anyhow::{Result as AnyhowResult, bail};
use runtime::{
    apply_sanitized_command_env, build_scriptlet_seccomp, chroot_mount_private_flags,
    chroot_namespace_flags, current_seccomp_mode, log_script_output, wait_and_capture,
    write_executable_script,
};
use std::fs;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};

mod arguments;
mod executor;
mod outcome;
mod phases;
mod runtime;
mod sandbox;
mod types;

pub use executor::ScriptletExecutor;
pub use outcome::{ScriptletFailureKind, ScriptletFailureOutcome, ScriptletOutcome};
pub use phases::{phase_from_string, phase_to_string};
pub use sandbox::{EffectiveSandbox, SandboxMode};
pub use types::{ExecutionMode, PackageFormat};

const LEGACY_MIN_TIMEOUT_MS: u64 = 1_000;
const LEGACY_MAX_TIMEOUT_MS: u64 = 300_000;
const LEGACY_SAFE_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const DANGEROUS_LEGACY_ENV_KEYS: [&str; 6] = [
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "BASH_ENV",
    "ENV",
    "PYTHONPATH",
    "PATH",
];
/// Executor-facing view of a legacy bundle entry.
pub struct LegacyScriptletExecution<'a> {
    pub entry_id: &'a str,
    pub phase: &'a str,
    pub interpreter: &'a str,
    pub interpreter_args: &'a [String],
    pub body: String,
    pub body_sha256: String,
    pub body_encoding: Option<&'a str>,
    pub native_args: &'a [String],
    pub native_environment: &'a [String],
    pub stdin_contract: Option<&'a str>,
    pub chroot_contract: Option<&'a str>,
    pub timeout_ms: u64,
}

/// Runtime values needed to resolve native package-manager invocation contracts.
pub struct LegacyInvocationRuntime<'a> {
    pub mode: &'a ExecutionMode,
    pub old_version: Option<&'a str>,
    pub new_version: Option<&'a str>,
    pub package_instance_count: Option<u32>,
}

impl ScriptletExecutor {
    /// Preflight a legacy bundle entry before mutation or temporary-file writes.
    pub fn preflight_legacy_entry(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> AnyhowResult<()> {
        self.validate_legacy_execution_contracts(execution, runtime)
    }

    /// Execute a legacy bundle entry and return typed outcome metadata.
    pub fn execute_legacy_entry_with_outcome(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> ScriptletOutcome {
        let requested_sandbox_mode = self.sandbox_mode;
        let effective_sandbox = self.effective_sandbox(false);

        if let Err(error) = self.preflight_legacy_entry(execution, runtime) {
            return self.failure_outcome(
                execution.phase,
                ScriptletFailureKind::SandboxSetupUnavailable,
                requested_sandbox_mode,
                effective_sandbox,
                error.to_string(),
            );
        }

        let script_content = match decode_legacy_body(execution) {
            Ok(script_content) => script_content,
            Err(error) => {
                return self.failure_outcome(
                    execution.phase,
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error.to_string(),
                );
            }
        };
        let args = match self.derive_legacy_native_args(execution, runtime) {
            Ok(args) => args,
            Err(error) => {
                return self.failure_outcome(
                    execution.phase,
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error.to_string(),
                );
            }
        };
        let env = match self.legacy_environment(execution) {
            Ok(env) => env,
            Err(error) => {
                return self.failure_outcome(
                    execution.phase,
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error.to_string(),
                );
            }
        };
        let env_refs: Vec<(&str, &str)> = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let use_sandbox = self.should_use_sandbox(&script_content);
        let effective_sandbox = self.effective_sandbox(use_sandbox);
        let executor = self.clone_with_timeout(Duration::from_millis(execution.timeout_ms));

        let result = if executor.is_live_root() {
            if use_sandbox {
                executor.execute_sandbox_live(
                    execution.phase,
                    execution.interpreter,
                    &script_content,
                    &args,
                    &env_refs,
                )
            } else {
                executor.execute_direct_with_options(
                    execution.phase,
                    execution.interpreter,
                    execution.interpreter_args,
                    &script_content,
                    &args,
                    &env_refs,
                    Duration::from_millis(execution.timeout_ms),
                )
            }
        } else {
            let interpreter_check_path = executor
                .root
                .join(execution.interpreter.trim_start_matches('/'));
            if !interpreter_check_path.exists() {
                warn!(
                    "Interpreter {} not found in target root {}, skipping {} legacy scriptlet",
                    execution.interpreter,
                    executor.root.display(),
                    execution.phase
                );
                return ScriptletOutcome::Skipped {
                    phase: execution.phase.to_string(),
                    requested_sandbox_mode,
                    effective_sandbox,
                };
            }
            executor.execute_in_target(
                execution.phase,
                execution.interpreter,
                execution.interpreter_args,
                &script_content,
                &args,
                &env_refs,
            )
        };

        match result {
            Ok(()) => ScriptletOutcome::Success {
                phase: execution.phase.to_string(),
                requested_sandbox_mode,
                effective_sandbox,
            },
            Err(error) => executor.failure_from_error(
                execution.phase,
                requested_sandbox_mode,
                effective_sandbox,
                error,
            ),
        }
    }

    fn validate_legacy_execution_contracts(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> AnyhowResult<()> {
        if execution.timeout_ms < LEGACY_MIN_TIMEOUT_MS
            || execution.timeout_ms > LEGACY_MAX_TIMEOUT_MS
        {
            bail!(
                "TimeoutOutOfRange: legacy entry '{}' timeout_ms {} is outside {}..={}",
                execution.entry_id,
                execution.timeout_ms,
                LEGACY_MIN_TIMEOUT_MS,
                LEGACY_MAX_TIMEOUT_MS
            );
        }

        let script_content = decode_legacy_body(execution)?;
        let use_sandbox = self.should_use_sandbox(&script_content);
        self.validate_legacy_interpreter_args(execution, use_sandbox)?;
        self.derive_legacy_native_args(execution, runtime)?;
        self.legacy_environment(execution)?;
        validate_stdin_contract(execution)?;
        validate_chroot_contract(execution)?;

        let interpreter_check_path = if self.is_live_root() {
            PathBuf::from(execution.interpreter)
        } else {
            self.root
                .join(execution.interpreter.trim_start_matches('/'))
        };

        if !interpreter_check_path.exists() {
            if self.is_live_root() {
                bail!(
                    "SandboxRequirementUnsupported: Interpreter not found: {}. Cannot execute legacy entry '{}'.",
                    execution.interpreter,
                    execution.entry_id
                );
            }
            return Ok(());
        }

        if self.is_live_root() && use_sandbox {
            self.preflight_protected_live_sandbox()
                .map_err(|error| anyhow::anyhow!("SandboxRequirementUnsupported: {error}"))?;
        }

        Ok(())
    }

    fn validate_legacy_interpreter_args(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        use_sandbox: bool,
    ) -> AnyhowResult<()> {
        for arg in execution.interpreter_args {
            if arg.contains('\0') {
                bail!(
                    "NativeArgsContractUnsupported: legacy entry '{}' has an interpreter arg containing NUL",
                    execution.entry_id
                );
            }
        }

        if self.is_live_root() && use_sandbox && !execution.interpreter_args.is_empty() {
            bail!(
                "NativeArgsContractUnsupported: legacy interpreter_args are unsupported with protected live-root sandboxing in Goal 6"
            );
        }

        Ok(())
    }

    fn derive_legacy_native_args(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> AnyhowResult<Vec<String>> {
        if execution.native_args.is_empty() {
            return Ok(self.get_args(runtime.mode, execution.phase));
        }

        let mut args = Vec::with_capacity(execution.native_args.len());
        for contract in execution.native_args {
            if let Some(literal) = contract.strip_prefix("raw:") {
                args.push(literal.to_string());
                continue;
            }

            let Some((position, projection)) = contract.split_once(':') else {
                bail!(
                    "NativeArgsContractUnsupported: malformed legacy native arg contract '{contract}'"
                );
            };
            let parsed_position = position.parse::<usize>().map_err(|_| {
                anyhow::anyhow!(
                    "NativeArgsContractUnsupported: malformed legacy native arg position '{position}'"
                )
            })?;
            if parsed_position == 0 {
                bail!("NativeArgsContractUnsupported: legacy native arg positions are one-based");
            }

            let Some((name, runtime_key)) = projection.split_once('=') else {
                bail!(
                    "NativeArgsContractUnsupported: malformed legacy native arg projection '{projection}'"
                );
            };
            if name != runtime_key {
                bail!(
                    "NativeArgsContractUnsupported: legacy native arg projection '{projection}' is not supported"
                );
            }

            let value = match runtime_key {
                "old-version" => runtime_old_version(runtime),
                "new-version" => Ok(runtime_new_version(self, runtime)),
                "package-instance-count" | "count" => runtime
                    .package_instance_count
                    .map(|count| count.to_string())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "NativeArgsContractUnsupported: package-instance-count is unavailable for legacy native args"
                        )
                    }),
                other => Err(anyhow::anyhow!(
                    "NativeArgsContractUnsupported: unsupported legacy native arg runtime value '{other}'"
                )),
            }?;
            args.push(value);
        }

        Ok(args)
    }

    fn legacy_environment(
        &self,
        execution: &LegacyScriptletExecution<'_>,
    ) -> AnyhowResult<Vec<(String, String)>> {
        let mut env = vec![
            ("CONARY_PACKAGE_NAME".to_string(), self.package_name.clone()),
            (
                "CONARY_PACKAGE_VERSION".to_string(),
                self.package_version.clone(),
            ),
            ("CONARY_ROOT".to_string(), "/".to_string()),
            ("CONARY_PHASE".to_string(), execution.phase.to_string()),
            ("PATH".to_string(), LEGACY_SAFE_PATH.to_string()),
        ];

        for item in execution.native_environment {
            let Some((key, value)) = item.split_once('=') else {
                bail!(
                    "NativeArgsContractUnsupported: bare native environment key '{}' requires an explicit runtime value",
                    item
                );
            };
            validate_legacy_environment_key(key)?;
            env.push((key.to_string(), value.to_string()));
        }

        Ok(env)
    }

    /// Execute scriptlet in sandbox on live root
    fn execute_sandbox_live(
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
    fn execute_in_target(
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
    fn execute_with_chroot(
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
                    // Use raw write of a static string — no heap allocation,
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
    fn execute_direct(
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
    fn execute_direct_with_options(
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

fn decode_legacy_body(execution: &LegacyScriptletExecution<'_>) -> AnyhowResult<String> {
    let body_bytes = match execution.body_encoding.unwrap_or("utf-8") {
        "utf-8" => execution.body.as_bytes().to_vec(),
        "base64" => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&execution.body)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "NativeArgsContractUnsupported: legacy entry '{}' body base64 decode failed: {error}",
                        execution.entry_id
                    )
                })?
        }
        other => bail!(
            "NativeArgsContractUnsupported: legacy entry '{}' body_encoding '{}' is unsupported",
            execution.entry_id,
            other
        ),
    };

    let actual = crate::hash::sha256_prefixed(&body_bytes);
    if !actual.eq_ignore_ascii_case(&execution.body_sha256) {
        bail!(
            "NativeArgsContractUnsupported: legacy entry '{}' body_sha256 mismatch: expected {}, got {}",
            execution.entry_id,
            execution.body_sha256,
            actual
        );
    }

    String::from_utf8(body_bytes).map_err(|error| {
        anyhow::anyhow!(
            "NativeArgsContractUnsupported: legacy entry '{}' body is not UTF-8 executable script text: {error}",
            execution.entry_id
        )
    })
}

fn runtime_old_version(runtime: &LegacyInvocationRuntime<'_>) -> AnyhowResult<String> {
    runtime
        .old_version
        .map(str::to_string)
        .or_else(|| match runtime.mode {
            ExecutionMode::Upgrade { old_version } => Some(old_version.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "NativeArgsContractUnsupported: old-version is unavailable for legacy native args"
            )
        })
}

fn runtime_new_version(
    executor: &ScriptletExecutor,
    runtime: &LegacyInvocationRuntime<'_>,
) -> String {
    runtime
        .new_version
        .map(str::to_string)
        .or_else(|| match runtime.mode {
            ExecutionMode::UpgradeRemoval { new_version } => Some(new_version.clone()),
            _ => None,
        })
        .unwrap_or_else(|| executor.package_version.clone())
}

fn validate_stdin_contract(execution: &LegacyScriptletExecution<'_>) -> AnyhowResult<()> {
    match execution.stdin_contract {
        None | Some("none") | Some("null") => Ok(()),
        Some(other) => bail!(
            "NativeArgsContractUnsupported: legacy entry '{}' stdin contract '{}' is unsupported in Goal 6",
            execution.entry_id,
            other
        ),
    }
}

fn validate_chroot_contract(execution: &LegacyScriptletExecution<'_>) -> AnyhowResult<()> {
    match execution.chroot_contract {
        None | Some("install-root") | Some("package-manager-default") => Ok(()),
        Some(other) => bail!(
            "SandboxRequirementUnsupported: legacy entry '{}' chroot contract '{}' is unsupported in Goal 6",
            execution.entry_id,
            other
        ),
    }
}

fn validate_legacy_environment_key(key: &str) -> AnyhowResult<()> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        || key.as_bytes()[0].is_ascii_digit()
    {
        bail!("NativeArgsContractUnsupported: malformed native environment key '{key}'");
    }

    if DANGEROUS_LEGACY_ENV_KEYS.contains(&key) {
        bail!("NativeArgsContractUnsupported: native environment key '{key}' is denied");
    }

    Ok(())
}

pub fn set_seccomp_warn_override(enabled: bool) {
    runtime::set_seccomp_warn_override(enabled);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::enforcement::EnforcementMode;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn legacy_execution_with_contracts(native_args: &[String]) -> LegacyScriptletExecution<'_> {
        let body = "echo legacy\n".to_string();
        LegacyScriptletExecution {
            entry_id: "legacy-entry",
            phase: "post-install",
            interpreter: "/bin/sh",
            interpreter_args: &[],
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            native_args,
            native_environment: &[],
            stdin_contract: None,
            chroot_contract: None,
            timeout_ms: 30_000,
        }
    }

    fn upgrade_runtime(mode: &ExecutionMode) -> LegacyInvocationRuntime<'_> {
        LegacyInvocationRuntime {
            mode,
            old_version: Some("0.9.0"),
            new_version: Some("1.0.0"),
            package_instance_count: Some(2),
        }
    }

    #[test]
    fn legacy_native_arg_contracts_use_runtime_versions_and_literals() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);
        let contracts = vec![
            "1:old-version=old-version".to_string(),
            "2:new-version=new-version".to_string(),
            "raw:literal".to_string(),
        ];
        let execution = legacy_execution_with_contracts(&contracts);
        let mode = ExecutionMode::Upgrade {
            old_version: "should-not-leak".to_string(),
        };

        let args = executor
            .derive_legacy_native_args(&execution, &upgrade_runtime(&mode))
            .expect("contracts derive");

        assert_eq!(args, vec!["0.9.0", "1.0.0", "literal"]);
    }

    #[test]
    fn legacy_native_arg_contracts_use_runtime_remove_count() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        let contracts = vec!["1:count=count".to_string()];
        let execution = legacy_execution_with_contracts(&contracts);
        let mode = ExecutionMode::Remove;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(0),
        };

        let args = executor
            .derive_legacy_native_args(&execution, &runtime)
            .expect("remove count contract derives");

        assert_eq!(args, vec!["0"]);
    }

    #[test]
    fn legacy_native_arg_contracts_refuse_malformed_or_missing_runtime_values() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

        for contracts in [
            vec!["old-version=old-version".to_string()],
            vec!["1:unknown=unsupported".to_string()],
            vec!["1:old-version=old-version".to_string()],
        ] {
            let execution = legacy_execution_with_contracts(&contracts);
            let mode = ExecutionMode::Install;
            let runtime = LegacyInvocationRuntime {
                mode: &mode,
                old_version: None,
                new_version: None,
                package_instance_count: None,
            };

            let error = executor
                .derive_legacy_native_args(&execution, &runtime)
                .expect_err("unsupported contract should refuse");
            assert!(
                error.to_string().contains("NativeArgsContractUnsupported"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn legacy_preflight_refuses_unsupported_invocation_fields() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);
        let mode = ExecutionMode::Remove;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(0),
        };

        let env = vec!["LD_PRELOAD=/tmp/libhack.so".to_string()];
        let path_env = vec!["PATH=/tmp/hijack".to_string()];
        let bare_env = vec!["RPM_INSTALL_PREFIX".to_string()];

        let cases = [
            LegacyScriptletExecution {
                stdin_contract: Some("debconf"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                stdin_contract: Some("paths"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                stdin_contract: Some("unknown"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                chroot_contract: Some("host-root"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                chroot_contract: Some("unknown"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                native_environment: &env,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                native_environment: &path_env,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                native_environment: &bare_env,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                timeout_ms: 999,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                timeout_ms: 300_001,
                ..legacy_execution_with_contracts(&[])
            },
        ];

        for execution in cases {
            let error = executor
                .preflight_legacy_entry(&execution, &runtime)
                .expect_err("unsupported invocation field should refuse");
            let message = error.to_string();
            assert!(
                message.contains("NativeArgsContractUnsupported")
                    || message.contains("SandboxRequirementUnsupported")
                    || message.contains("TimeoutOutOfRange"),
                "unexpected error: {message}"
            );
        }
    }

    #[test]
    fn legacy_preflight_rejects_body_hash_mismatch() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);
        let mode = ExecutionMode::Install;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: None,
        };
        let execution = LegacyScriptletExecution {
            body_sha256: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            ..legacy_execution_with_contracts(&[])
        };

        let error = executor
            .preflight_legacy_entry(&execution, &runtime)
            .expect_err("body hash mismatch should refuse");

        assert!(
            error.to_string().contains("body_sha256 mismatch"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn legacy_execution_uses_safe_path_and_derived_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb)
                .with_sandbox_mode(SandboxMode::None);
        let contracts = vec![
            "1:old-version=old-version".to_string(),
            "2:new-version=new-version".to_string(),
            "raw:literal".to_string(),
        ];
        let body = r#"
                test "$PATH" = "/usr/sbin:/usr/bin:/sbin:/bin"
                test "$1" = "0.9.0"
                test "$2" = "1.0.0"
                test "$3" = "literal"
            "#
        .to_string();
        let execution = LegacyScriptletExecution {
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            ..legacy_execution_with_contracts(&contracts)
        };
        let mode = ExecutionMode::Upgrade {
            old_version: "should-not-leak".to_string(),
        };

        let outcome =
            executor.execute_legacy_entry_with_outcome(&execution, &upgrade_runtime(&mode));

        assert!(
            matches!(outcome, ScriptletOutcome::Success { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn legacy_execution_skips_target_root_when_interpreter_is_absent() {
        let root = tempfile::tempdir().expect("target root");
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::None);
        let mode = ExecutionMode::Remove;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: Some("1.0.0"),
            new_version: None,
            package_instance_count: Some(0),
        };
        let execution = LegacyScriptletExecution {
            phase: "post-remove",
            ..legacy_execution_with_contracts(&[])
        };

        let outcome = executor.execute_legacy_entry_with_outcome(&execution, &runtime);

        assert!(
            matches!(outcome, ScriptletOutcome::Skipped { .. }),
            "{outcome:?}"
        );
    }

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

    // -- GAP 3: build_scriptlet_seccomp() ------------------------------------

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

    // -- GAP 4: execute_direct double-wait fix (stdout/stderr + timeout) ------

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

    // -- GAP 6: Native chroot + seccomp (root-gated) -------------------------

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
