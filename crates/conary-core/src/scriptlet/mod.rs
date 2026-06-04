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

use crate::capability::SyscallCapabilities;
use crate::capability::enforcement::{EnforcementMode, EnforcementPolicy};
use crate::container::{
    BindMount, ContainerConfig, Sandbox, ScriptRisk, analyze_script, isolation_available,
};
use crate::db::models::ScriptletEntry;
use crate::error::{Error, Result};
use crate::packages::traits::{Scriptlet, ScriptletPhase};
use anyhow::{Result as AnyhowResult, bail};
use runtime::{
    apply_sanitized_command_env, build_scriptlet_seccomp, chroot_mount_private_flags,
    chroot_namespace_flags, current_seccomp_mode, log_script_output, wait_and_capture,
    write_executable_script,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};

mod runtime;

/// Sandbox mode for scriptlet execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// No sandboxing - direct execution
    #[serde(rename = "never", alias = "none")]
    None,
    /// Automatic - sandbox based on script risk analysis
    Auto,
    /// Always sandbox all scripts
    #[default]
    Always,
}

impl SandboxMode {
    /// Parse sandbox mode from string (auto, always, never)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "never" | "none" | "off" | "false" => Some(Self::None),
            "auto" => Some(Self::Auto),
            "always" | "on" | "true" => Some(Self::Always),
            _ => None,
        }
    }

    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "never",
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

/// Sandbox boundary actually used for a scriptlet execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveSandbox {
    /// Live-root protected mode with namespace isolation.
    ProtectedLiveRoot,
    /// Direct legacy execution on the live host.
    Direct,
    /// Alternate-root execution for bootstrap/offline targets.
    TargetRoot,
}

impl EffectiveSandbox {
    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtectedLiveRoot => "protected-live-root",
            Self::Direct => "direct",
            Self::TargetRoot => "target-root",
        }
    }
}

/// Typed failure classification for scriptlet execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptletFailureKind {
    /// The script process ran and returned a non-zero exit status.
    ScriptExited,
    /// The script process exceeded the configured timeout.
    ScriptTimedOut,
    /// Namespace, mount, interpreter, or other sandbox setup failed.
    SandboxSetupUnavailable,
    /// Landlock/seccomp/capability enforcement setup failed.
    EnforcementSetupFailed,
}

impl ScriptletFailureKind {
    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScriptExited => "ScriptExited",
            Self::ScriptTimedOut => "ScriptTimedOut",
            Self::SandboxSetupUnavailable => "SandboxSetupUnavailable",
            Self::EnforcementSetupFailed => "EnforcementSetupFailed",
        }
    }
}

/// Failure details for a scriptlet execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletFailureOutcome {
    pub phase: String,
    pub failure_kind: ScriptletFailureKind,
    pub requested_sandbox_mode: SandboxMode,
    pub effective_sandbox: EffectiveSandbox,
    pub message: String,
}

/// Structured result of a scriptlet attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptletOutcome {
    /// The scriptlet was intentionally skipped, usually because a target-root
    /// interpreter is not available during early bootstrap.
    Skipped {
        phase: String,
        requested_sandbox_mode: SandboxMode,
        effective_sandbox: EffectiveSandbox,
    },
    /// The scriptlet completed successfully.
    Success {
        phase: String,
        requested_sandbox_mode: SandboxMode,
        effective_sandbox: EffectiveSandbox,
    },
    /// The scriptlet failed with typed context.
    Failure(ScriptletFailureOutcome),
}

impl ScriptletOutcome {
    /// Convert an outcome back into the historical `Result<()>` API.
    pub fn into_result(self) -> Result<()> {
        match self {
            Self::Skipped { .. } | Self::Success { .. } => Ok(()),
            Self::Failure(failure) => Err(Error::ScriptletError(failure.message)),
        }
    }
}

/// Default timeout for scriptlet execution (60 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
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
const LIVE_SANDBOX_READONLY_ETC_FILES: [&str; 5] = [
    "/etc/passwd",
    "/etc/group",
    "/etc/hosts",
    "/etc/shadow",
    "/etc/sudoers",
];

/// Package format types for argument handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    Rpm,
    Deb,
    Arch,
}

impl PackageFormat {
    /// Parse from string representation
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rpm" => Some(Self::Rpm),
            "deb" => Some(Self::Deb),
            "arch" => Some(Self::Arch),
            _ => None,
        }
    }

    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
        }
    }
}

/// Execution mode determines arguments passed to scriptlets
#[derive(Debug, Clone)]
pub enum ExecutionMode {
    /// Fresh install
    Install,
    /// Package removal
    Remove,
    /// Upgrade from old version (for NEW package scriptlets)
    Upgrade { old_version: String },
    /// Upgrade removal (for OLD package scriptlets during upgrade)
    /// RPM: $1=1 (not 0, signaling "another version remains")
    /// DEB: "upgrade <new_version>" (not "remove")
    /// Arch: Should NOT be used - Arch skips old package scripts during upgrade
    UpgradeRemoval { new_version: String },
}

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

/// Scriptlet executor with cross-distro support
pub struct ScriptletExecutor {
    root: PathBuf,
    package_name: String,
    package_version: String,
    package_format: PackageFormat,
    timeout: Duration,
    sandbox_mode: SandboxMode,
}

impl ScriptletExecutor {
    /// Create a new executor
    pub fn new(root: &Path, name: &str, version: &str, format: PackageFormat) -> Self {
        Self {
            root: root.to_path_buf(),
            package_name: name.to_string(),
            package_version: version.to_string(),
            package_format: format,
            timeout: DEFAULT_TIMEOUT,
            sandbox_mode: SandboxMode::default(),
        }
    }

    /// Set custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set sandbox mode for scriptlet execution
    pub fn with_sandbox_mode(mut self, mode: SandboxMode) -> Self {
        self.sandbox_mode = mode;
        self
    }

    /// Execute a scriptlet from package parsing
    pub fn execute(&self, scriptlet: &Scriptlet, mode: &ExecutionMode) -> Result<()> {
        self.execute_with_outcome(scriptlet, mode).into_result()
    }

    /// Execute a scriptlet and return typed outcome metadata.
    pub fn execute_with_outcome(
        &self,
        scriptlet: &Scriptlet,
        mode: &ExecutionMode,
    ) -> ScriptletOutcome {
        self.execute_impl_with_outcome(
            &scriptlet.phase.to_string(),
            &scriptlet.interpreter,
            &scriptlet.content,
            scriptlet.flags.as_deref(),
            mode,
        )
    }

    /// Execute a scriptlet from database entry
    pub fn execute_entry(&self, entry: &ScriptletEntry, mode: &ExecutionMode) -> Result<()> {
        self.execute_entry_with_outcome(entry, mode).into_result()
    }

    /// Execute a database scriptlet entry and return typed outcome metadata.
    pub fn execute_entry_with_outcome(
        &self,
        entry: &ScriptletEntry,
        mode: &ExecutionMode,
    ) -> ScriptletOutcome {
        self.execute_impl_with_outcome(
            &entry.phase,
            &entry.interpreter,
            &entry.content,
            entry.flags.as_deref(),
            mode,
        )
    }

    /// Preflight a package-parsed scriptlet before any file/DB mutation.
    pub fn preflight(&self, scriptlet: &Scriptlet, mode: &ExecutionMode) -> Result<()> {
        self.preflight_impl(
            &scriptlet.phase.to_string(),
            &scriptlet.interpreter,
            &scriptlet.content,
            mode,
        )
    }

    /// Preflight a database scriptlet entry before any file/DB mutation.
    pub fn preflight_entry(&self, entry: &ScriptletEntry, mode: &ExecutionMode) -> Result<()> {
        self.preflight_impl(&entry.phase, &entry.interpreter, &entry.content, mode)
    }

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

    /// Check if we're operating on the live root
    fn is_live_root(&self) -> bool {
        self.root == Path::new("/")
    }

    fn clone_with_timeout(&self, timeout: Duration) -> Self {
        Self {
            root: self.root.clone(),
            package_name: self.package_name.clone(),
            package_version: self.package_version.clone(),
            package_format: self.package_format,
            timeout,
            sandbox_mode: self.sandbox_mode,
        }
    }

    /// Core execution implementation
    #[cfg(test)]
    fn execute_impl(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        _flags: Option<&str>,
        mode: &ExecutionMode,
    ) -> Result<()> {
        self.execute_impl_with_outcome(phase, interpreter, content, _flags, mode)
            .into_result()
    }

    fn execute_impl_with_outcome(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        _flags: Option<&str>,
        mode: &ExecutionMode,
    ) -> ScriptletOutcome {
        // Prepare script content (Arch needs wrapper generation)
        let script_content = if self.package_format == PackageFormat::Arch {
            self.prepare_arch_wrapper(content, phase)
        } else {
            content.to_string()
        };

        // Analyze script for dangerous patterns
        let analysis = analyze_script(&script_content);

        // Determine if we should sandbox based on mode and risk
        let use_sandbox = match self.sandbox_mode {
            SandboxMode::None => false,
            SandboxMode::Always => true,
            SandboxMode::Auto => {
                // Sandbox if risk is Medium or higher
                analysis.risk >= ScriptRisk::Medium
            }
        };
        let effective_sandbox = self.effective_sandbox(use_sandbox);
        let requested_sandbox_mode = self.sandbox_mode;

        if !analysis.patterns.is_empty() {
            info!(
                "{} scriptlet risk analysis: {} - {:?}",
                phase,
                analysis.risk.as_str(),
                analysis.patterns
            );
        }

        // Resolve interpreter (Arch always uses bash for wrapper)
        let interpreter_path = if self.package_format == PackageFormat::Arch {
            "/bin/bash".to_string()
        } else {
            interpreter.to_string()
        };

        // For target root installs, validate interpreter exists IN TARGET
        // For live root, validate it exists on the host
        let interpreter_check_path = if self.is_live_root() {
            PathBuf::from(&interpreter_path)
        } else {
            self.root.join(interpreter_path.trim_start_matches('/'))
        };

        if !interpreter_check_path.exists() {
            if self.is_live_root() {
                return self.failure_outcome(
                    phase,
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    requested_sandbox_mode,
                    effective_sandbox,
                    format!(
                        "Interpreter not found: {}. Cannot execute {} scriptlet.",
                        interpreter_path, phase
                    ),
                );
            } else {
                // For target root, warn but don't fail - the scriptlet might not be needed
                // or the target might be in early bootstrap (no shell yet)
                warn!(
                    "Interpreter {} not found in target root {}, skipping {} scriptlet",
                    interpreter_path,
                    self.root.display(),
                    phase
                );
                return ScriptletOutcome::Skipped {
                    phase: phase.to_string(),
                    requested_sandbox_mode,
                    effective_sandbox,
                };
            }
        }

        // Prepare arguments based on distro, mode, and phase
        let args = self.get_args(mode, phase);

        // Build environment variables
        let env = [
            ("CONARY_PACKAGE_NAME", self.package_name.as_str()),
            ("CONARY_PACKAGE_VERSION", self.package_version.as_str()),
            ("CONARY_ROOT", "/"), // Always "/" from script's perspective
            ("CONARY_PHASE", phase),
        ];

        info!(
            "Executing {} scriptlet for {} v{} (root: {}, sandbox: {})",
            phase,
            self.package_name,
            self.package_version,
            self.root.display(),
            use_sandbox
        );

        let result = if self.is_live_root() {
            // Live root execution
            if use_sandbox {
                if let Err(error) = self.preflight_protected_live_sandbox() {
                    return self.failure_from_error(
                        phase,
                        requested_sandbox_mode,
                        effective_sandbox,
                        error,
                    );
                }
                self.execute_sandbox_live(phase, &interpreter_path, &script_content, &args, &env)
            } else {
                self.execute_direct(phase, &interpreter_path, &script_content, &args, &env)
            }
        } else {
            // Target root execution - always use chroot/container
            self.execute_in_target(phase, &interpreter_path, &[], &script_content, &args, &env)
        };

        match result {
            Ok(()) => ScriptletOutcome::Success {
                phase: phase.to_string(),
                requested_sandbox_mode,
                effective_sandbox,
            },
            Err(error) => {
                self.failure_from_error(phase, requested_sandbox_mode, effective_sandbox, error)
            }
        }
    }

    fn preflight_impl(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        _mode: &ExecutionMode,
    ) -> Result<()> {
        let script_content = if self.package_format == PackageFormat::Arch {
            self.prepare_arch_wrapper(content, phase)
        } else {
            content.to_string()
        };
        let use_sandbox = self.should_use_sandbox(&script_content);
        let interpreter_path = if self.package_format == PackageFormat::Arch {
            "/bin/bash".to_string()
        } else {
            interpreter.to_string()
        };
        let interpreter_check_path = if self.is_live_root() {
            PathBuf::from(&interpreter_path)
        } else {
            self.root.join(interpreter_path.trim_start_matches('/'))
        };

        if !interpreter_check_path.exists() {
            if self.is_live_root() {
                return Err(Error::ScriptletError(format!(
                    "Interpreter not found: {}. Cannot execute {} scriptlet.",
                    interpreter_path, phase
                )));
            }
            return Ok(());
        }

        if self.is_live_root() && use_sandbox {
            self.preflight_protected_live_sandbox()?;
        }

        Ok(())
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

    fn should_use_sandbox(&self, script_content: &str) -> bool {
        match self.sandbox_mode {
            SandboxMode::None => false,
            SandboxMode::Always => true,
            SandboxMode::Auto => analyze_script(script_content).risk >= ScriptRisk::Medium,
        }
    }

    fn effective_sandbox(&self, use_sandbox: bool) -> EffectiveSandbox {
        if !self.is_live_root() {
            EffectiveSandbox::TargetRoot
        } else if use_sandbox {
            EffectiveSandbox::ProtectedLiveRoot
        } else {
            EffectiveSandbox::Direct
        }
    }

    fn failure_outcome(
        &self,
        phase: &str,
        failure_kind: ScriptletFailureKind,
        requested_sandbox_mode: SandboxMode,
        effective_sandbox: EffectiveSandbox,
        message: String,
    ) -> ScriptletOutcome {
        ScriptletOutcome::Failure(ScriptletFailureOutcome {
            phase: phase.to_string(),
            failure_kind,
            requested_sandbox_mode,
            effective_sandbox,
            message,
        })
    }

    fn failure_from_error(
        &self,
        phase: &str,
        requested_sandbox_mode: SandboxMode,
        effective_sandbox: EffectiveSandbox,
        error: Error,
    ) -> ScriptletOutcome {
        let message = match error {
            Error::ScriptletError(message) => message,
            other => other.to_string(),
        };
        self.failure_outcome(
            phase,
            classify_scriptlet_failure(&message),
            requested_sandbox_mode,
            effective_sandbox,
            message,
        )
    }

    fn preflight_protected_live_sandbox(&self) -> Result<()> {
        if std::env::var_os("CONARY_TEST_FORCE_SCRIPTLET_SANDBOX_PREFLIGHT_UNAVAILABLE").is_some() {
            return Err(protected_scriptlet_sandbox_unavailable(
                "test override forced namespace preflight failure",
            ));
        }

        let config = self.live_sandbox_config()?;
        if !isolation_available() {
            return Err(protected_scriptlet_sandbox_unavailable(
                "mount/user namespace isolation is unavailable",
            ));
        }

        if let Some(policy) = config.capability_policy.as_ref()
            && policy.mode == EnforcementMode::Enforce
            && policy.syscalls.is_some()
        {
            let support = crate::capability::enforcement::check_enforcement_support();
            if !support.seccomp {
                return Err(Error::ScriptletError(
                    "Protected scriptlet sandboxing requires seccomp enforcement support. \
                     Enable seccomp in the kernel/container runtime or run inside a VM. \
                     Dangerous legacy direct execution is available only with --sandbox=never plus \
                     the live-host mutation acknowledgement, and it records effective_sandbox=direct."
                        .to_string(),
                ));
            }
        }

        Ok(())
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

    fn live_sandbox_config(&self) -> Result<ContainerConfig> {
        let mut config = ContainerConfig::default().for_untrusted();
        config.timeout = self.timeout;
        config
            .bind_mounts
            .retain(|mount| !is_live_sandbox_private_target(&mount.target));

        config.add_private_writable_mount("/etc", 0o755)?;
        config.add_private_writable_mount("/var", 0o755)?;

        for protected in LIVE_SANDBOX_READONLY_ETC_FILES {
            config
                .bind_mounts
                .push(BindMount::readonly(protected, protected));
        }

        config.capability_policy = Some(EnforcementPolicy {
            mode: EnforcementMode::Enforce,
            filesystem: None,
            syscalls: Some(SyscallCapabilities {
                allow: Vec::new(),
                deny: Vec::new(),
                profile: Some("scriptlet".to_string()),
            }),
            network_isolation: config.isolate_network,
        });

        Ok(config)
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

    /// Get arguments based on distro and execution mode
    ///
    /// Each distro has different argument semantics:
    /// - RPM: Integer count of packages remaining after operation
    /// - DEB: Action word + optional version string (per Debian Policy)
    /// - Arch: Version string(s)
    fn get_args(&self, mode: &ExecutionMode, phase: &str) -> Vec<String> {
        match self.package_format {
            PackageFormat::Rpm => {
                // RPM uses integer arguments (count of packages remaining):
                // Install: $1 = 1
                // Upgrade (new pkg): $1 = 2
                // Upgrade (old pkg removal): $1 = 1 (NOT 0! another version remains)
                // Remove: $1 = 0
                match mode {
                    ExecutionMode::Install => vec!["1".to_string()],
                    ExecutionMode::Remove => vec!["0".to_string()],
                    ExecutionMode::Upgrade { .. } => vec!["2".to_string()],
                    ExecutionMode::UpgradeRemoval { .. } => vec!["1".to_string()],
                }
            }
            PackageFormat::Deb => {
                // DEB uses action words + version strings (per Debian Policy):
                // preinst: install | upgrade <old-version>
                // postinst: configure <most-recently-configured-version>
                // prerm: remove | upgrade <new-version>
                // postrm: remove | upgrade <new-version>
                match mode {
                    ExecutionMode::Install => match phase {
                        "pre-install" => vec!["install".to_string()],
                        "post-install" => vec!["configure".to_string()],
                        _ => vec!["install".to_string()],
                    },
                    ExecutionMode::Remove => {
                        vec!["remove".to_string()]
                    }
                    ExecutionMode::Upgrade { old_version } => {
                        // For NEW package scripts during upgrade
                        match phase {
                            "pre-install" => vec!["upgrade".to_string(), old_version.clone()],
                            "post-install" => vec!["configure".to_string(), old_version.clone()],
                            _ => vec!["upgrade".to_string(), old_version.clone()],
                        }
                    }
                    ExecutionMode::UpgradeRemoval { new_version } => {
                        // For OLD package scripts during upgrade
                        // prerm/postrm get "upgrade <new_version>"
                        vec!["upgrade".to_string(), new_version.clone()]
                    }
                }
            }
            PackageFormat::Arch => {
                // Arch uses version strings:
                // Install: $1 = new_version
                // Remove: $1 = old_version
                // Upgrade: $1 = new_version, $2 = old_version
                // UpgradeRemoval: Should NOT be called for Arch!
                match mode {
                    ExecutionMode::Install => vec![self.package_version.clone()],
                    ExecutionMode::Remove => vec![self.package_version.clone()],
                    ExecutionMode::Upgrade { old_version } => {
                        vec![self.package_version.clone(), old_version.clone()]
                    }
                    ExecutionMode::UpgradeRemoval { .. } => {
                        // This should never be called for Arch - log warning
                        // Arch does NOT run old package scripts during upgrade
                        warn!("UpgradeRemoval mode called for Arch package - this is a bug!");
                        vec![self.package_version.clone()]
                    }
                }
            }
        }
    }

    /// Generate wrapper script for Arch .INSTALL function libraries
    ///
    /// Arch .INSTALL files define functions like post_install(), pre_upgrade(), etc.
    /// but don't call them. We need to source the file and call the appropriate function.
    fn prepare_arch_wrapper(&self, content: &str, phase: &str) -> String {
        // Map phase to Arch function name
        let function_name = match phase {
            "pre-install" => "pre_install",
            "post-install" => "post_install",
            "pre-remove" => "pre_remove",
            "post-remove" => "post_remove",
            "pre-upgrade" => "pre_upgrade",
            "post-upgrade" => "post_upgrade",
            _ => "post_install", // Fallback
        };

        format!(
            "#!/bin/bash\nset -e\n\n# Arch .INSTALL content:\n{}\n\n# Call the function if it exists\nif declare -f {} > /dev/null; then\n    {} \"$@\"\nfi\n",
            content, function_name, function_name
        )
    }
}

fn is_live_sandbox_private_target(target: &Path) -> bool {
    target == Path::new("/etc")
        || target.starts_with("/etc/")
        || target == Path::new("/var")
        || target.starts_with("/var/")
}

fn protected_scriptlet_sandbox_unavailable(reason: &str) -> Error {
    Error::ScriptletError(format!(
        "Protected scriptlet sandboxing requires mount and user namespace support. \
         Enable the required kernel/container namespace support or run inside a VM. \
         Dangerous legacy direct execution is available only with --sandbox=never plus \
         the live-host mutation acknowledgement, and it records effective_sandbox=direct. \
         ({reason})"
    ))
}

fn classify_scriptlet_failure(message: &str) -> ScriptletFailureKind {
    if message.contains("failed with exit code") {
        ScriptletFailureKind::ScriptExited
    } else if message.contains("timed out") || message.contains("Timeout:") {
        ScriptletFailureKind::ScriptTimedOut
    } else if message.contains("Capability enforcement failed")
        || message.contains("seccomp filter application failed")
        || message.contains("requires seccomp enforcement support")
    {
        ScriptletFailureKind::EnforcementSetupFailed
    } else {
        ScriptletFailureKind::SandboxSetupUnavailable
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

/// Convert ScriptletPhase to string for database storage
pub fn phase_to_string(phase: ScriptletPhase) -> String {
    phase.to_string()
}

/// Parse phase string back to ScriptletPhase
pub fn phase_from_string(s: &str) -> Option<ScriptletPhase> {
    match s {
        "pre-install" => Some(ScriptletPhase::PreInstall),
        "post-install" => Some(ScriptletPhase::PostInstall),
        "pre-remove" => Some(ScriptletPhase::PreRemove),
        "post-remove" => Some(ScriptletPhase::PostRemove),
        "pre-upgrade" => Some(ScriptletPhase::PreUpgrade),
        "post-upgrade" => Some(ScriptletPhase::PostUpgrade),
        "pre-transaction" => Some(ScriptletPhase::PreTransaction),
        "post-transaction" => Some(ScriptletPhase::PostTransaction),
        "trigger" => Some(ScriptletPhase::Trigger),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::enforcement::EnforcementMode;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn test_package_format_from_str() {
        assert_eq!(PackageFormat::parse("rpm"), Some(PackageFormat::Rpm));
        assert_eq!(PackageFormat::parse("deb"), Some(PackageFormat::Deb));
        assert_eq!(PackageFormat::parse("arch"), Some(PackageFormat::Arch));
        assert_eq!(PackageFormat::parse("unknown"), None);
    }

    #[test]
    fn test_rpm_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "pre-install"),
            vec!["1"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "pre-install"
            ),
            vec!["2"]
        );
        // UpgradeRemoval: old package scripts get $1=1 (NOT 0!)
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "pre-remove"
            ),
            vec!["1"]
        );
    }

    #[test]
    fn test_deb_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

        // Fresh install
        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "pre-install"),
            vec!["install"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "post-install"),
            vec!["configure"]
        );

        // Remove
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["remove"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "post-remove"),
            vec!["remove"]
        );

        // Upgrade
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "pre-install"
            ),
            vec!["upgrade", "0.9.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "post-install"
            ),
            vec!["configure", "0.9.0"]
        );
        // UpgradeRemoval: OLD package scripts get "upgrade <new_version>"
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "pre-remove"
            ),
            vec!["upgrade", "1.0.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "post-remove"
            ),
            vec!["upgrade", "1.0.0"]
        );
    }

    #[test]
    fn test_arch_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);

        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "post-install"),
            vec!["1.0.0"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["1.0.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "post-upgrade"
            ),
            vec!["1.0.0", "0.9.0"]
        );
    }

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
    fn test_arch_wrapper_generation() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);

        let content = "post_install() {\n    echo \"Hello\"\n}";
        let wrapper = executor.prepare_arch_wrapper(content, "post-install");

        assert!(wrapper.contains("#!/bin/bash"));
        assert!(wrapper.contains("set -e"));
        assert!(wrapper.contains(content));
        assert!(wrapper.contains("post_install \"$@\""));
    }

    #[test]
    fn test_phase_conversion() {
        assert_eq!(phase_to_string(ScriptletPhase::PreInstall), "pre-install");
        assert_eq!(
            phase_from_string("pre-install"),
            Some(ScriptletPhase::PreInstall)
        );
        assert_eq!(phase_from_string("invalid"), None);
    }

    #[test]
    fn test_sandbox_mode_default_is_always() {
        assert_eq!(SandboxMode::default(), SandboxMode::Always);
    }

    #[test]
    fn test_sandbox_mode_parse() {
        // "none" variants
        assert_eq!(SandboxMode::parse("never"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("none"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("off"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("false"), Some(SandboxMode::None));

        // "auto"
        assert_eq!(SandboxMode::parse("auto"), Some(SandboxMode::Auto));

        // "always" variants
        assert_eq!(SandboxMode::parse("always"), Some(SandboxMode::Always));
        assert_eq!(SandboxMode::parse("on"), Some(SandboxMode::Always));
        assert_eq!(SandboxMode::parse("true"), Some(SandboxMode::Always));

        // Case insensitivity
        assert_eq!(SandboxMode::parse("AUTO"), Some(SandboxMode::Auto));
        assert_eq!(SandboxMode::parse("NEVER"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("Always"), Some(SandboxMode::Always));

        // Invalid
        assert_eq!(SandboxMode::parse("invalid"), None);
        assert_eq!(SandboxMode::parse(""), None);
    }

    #[test]
    fn sandbox_mode_serde_round_trips_goal7_matrix_spellings() {
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"never\"").expect("never deserializes"),
            SandboxMode::None
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"none\"").expect("none alias deserializes"),
            SandboxMode::None
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"auto\"").expect("auto deserializes"),
            SandboxMode::Auto
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"always\"").expect("always deserializes"),
            SandboxMode::Always
        );
        assert_eq!(
            serde_json::to_string(&SandboxMode::None).expect("serialize none"),
            "\"never\""
        );
    }

    #[test]
    fn test_executor_default_sandbox_is_always() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        assert_eq!(executor.sandbox_mode, SandboxMode::Always);
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

    #[test]
    fn test_live_sandbox_config_rebinds_critical_etc_files_readonly() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");
        let etc_index = config
            .bind_mounts
            .iter()
            .position(|mount| mount.target == Path::new("/etc") && mount.writable)
            .expect("writable /etc bind mount missing");

        for protected in ["/etc/passwd", "/etc/shadow", "/etc/sudoers"] {
            let mount_index = config
                .bind_mounts
                .iter()
                .position(|mount| mount.target == Path::new(protected))
                .unwrap_or_else(|| panic!("missing protected mount for {protected}"));
            let mount = &config.bind_mounts[mount_index];
            assert!(
                !mount.writable,
                "{protected} should be rebound read-only inside the live sandbox"
            );
            assert!(
                mount_index > etc_index,
                "{protected} should be mounted after writable /etc so it is not shadowed"
            );
        }
    }

    #[test]
    fn test_live_sandbox_config_uses_private_layers_for_writable_etc_and_var() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");

        for protected_dir in ["/etc", "/var"] {
            let mount = config
                .bind_mounts
                .iter()
                .find(|mount| mount.target == Path::new(protected_dir) && mount.writable)
                .unwrap_or_else(|| panic!("missing writable {protected_dir} sandbox layer"));
            assert_ne!(
                mount.source,
                PathBuf::from(protected_dir),
                "{protected_dir} must use a private writable layer, not the live host path"
            );
            assert!(
                mount.source.exists(),
                "private layer backing {protected_dir} should exist for the sandbox lifetime"
            );
        }
    }

    #[test]
    fn test_live_sandbox_config_fails_closed_on_protection_setup_failures() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");

        let policy = config
            .capability_policy
            .as_ref()
            .expect("protected live sandbox should carry enforce-mode metadata");
        assert_eq!(policy.mode, EnforcementMode::Enforce);
    }

    #[test]
    fn test_live_sandbox_config_installs_scriptlet_seccomp_profile() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");

        let syscalls = config
            .capability_policy
            .as_ref()
            .and_then(|policy| policy.syscalls.as_ref())
            .expect("protected live sandbox should install the scriptlet seccomp profile");
        assert_eq!(syscalls.profile.as_deref(), Some("scriptlet"));
        assert!(syscalls.allow.is_empty());
        assert!(syscalls.deny.is_empty());
    }

    #[test]
    fn test_protected_live_root_preflight_reports_operator_diagnostic() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(
                "CONARY_TEST_FORCE_SCRIPTLET_SANDBOX_PREFLIGHT_UNAVAILABLE",
                "1",
            );
        }

        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        let scriptlet = Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "echo ok".to_string(),
            flags: None,
        };

        let err = executor
            .preflight(&scriptlet, &ExecutionMode::Install)
            .expect_err("forced protected sandbox preflight failure should be fatal");

        unsafe {
            std::env::remove_var("CONARY_TEST_FORCE_SCRIPTLET_SANDBOX_PREFLIGHT_UNAVAILABLE");
        }

        let message = err.to_string();
        assert!(
            message.contains(
                "Protected scriptlet sandboxing requires mount and user namespace support"
            ),
            "unexpected error: {message}"
        );
        assert!(message.contains("--sandbox=never"));
        assert!(message.contains("effective_sandbox=direct"));
    }

    #[test]
    fn test_execute_with_outcome_records_requested_and_effective_sandbox() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);
        let scriptlet = Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "exit 42".to_string(),
            flags: None,
        };

        let outcome = executor.execute_with_outcome(&scriptlet, &ExecutionMode::Install);

        let ScriptletOutcome::Failure(failure) = outcome else {
            panic!("expected scriptlet failure outcome");
        };
        assert_eq!(failure.failure_kind, ScriptletFailureKind::ScriptExited);
        assert_eq!(failure.requested_sandbox_mode, SandboxMode::None);
        assert_eq!(failure.effective_sandbox, EffectiveSandbox::Direct);
        assert!(failure.message.contains("failed with exit code 42"));
    }

    #[test]
    fn test_execute_impl_missing_interpreter() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_impl(
            "post-install",
            "/nonexistent/interpreter",
            "echo hello",
            None,
            &ExecutionMode::Install,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Interpreter not found"),
            "unexpected error: {}",
            err
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
