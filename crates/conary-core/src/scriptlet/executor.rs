// conary-core/src/scriptlet/executor.rs

use super::ScriptletFailureKind;
use super::{ExecutionMode, PackageFormat, SandboxMode, ScriptletOutcome};
use crate::container::{ScriptRisk, analyze_script};
use crate::db::models::ScriptletEntry;
use crate::error::{Error, Result};
use crate::packages::traits::Scriptlet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

/// Default timeout for scriptlet execution (60 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Scriptlet executor with cross-distro support
pub struct ScriptletExecutor {
    pub(super) root: PathBuf,
    pub(super) package_name: String,
    pub(super) package_version: String,
    pub(super) package_format: PackageFormat,
    pub(super) timeout: Duration,
    pub(super) sandbox_mode: SandboxMode,
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

    /// Check if we're operating on the live root
    pub(super) fn is_live_root(&self) -> bool {
        self.root == Path::new("/")
    }

    pub(super) fn clone_with_timeout(&self, timeout: Duration) -> Self {
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
}

#[cfg(test)]
mod tests {
    use super::super::{
        EffectiveSandbox, ExecutionMode, PackageFormat, SandboxMode, ScriptletFailureKind,
        ScriptletOutcome,
    };
    use super::ScriptletExecutor;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
    use std::path::Path;

    #[test]
    fn test_executor_default_sandbox_is_always() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        assert_eq!(executor.sandbox_mode, SandboxMode::Always);
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
}
