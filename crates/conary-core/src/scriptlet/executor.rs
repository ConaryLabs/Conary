// conary-core/src/scriptlet/executor.rs

use super::ScriptletFailureKind;
use super::lifecycle_bridge::LifecycleBridgeConfig;
use super::process::ScriptletProcess;
use super::{ExecutionMode, PackageFormat, SandboxMode, ScriptletOutcome};
use crate::container::analyze_script;
use crate::db::models::InstalledCcsRemoveHook;
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

/// Default timeout for scriptlet execution (60 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Scriptlet executor with cross-distro support
#[derive(Clone)]
pub struct ScriptletExecutor {
    pub(super) root: PathBuf,
    pub(super) package_name: String,
    pub(super) package_version: String,
    pub(super) package_format: PackageFormat,
    pub(super) timeout: Duration,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) activation_invocations:
        Arc<Mutex<Vec<crate::activation::RuntimeActivationInvocation>>>,
    pub(super) boot_runtime_invocations:
        Arc<Mutex<Vec<crate::activation::BootRuntimeActivationInvocation>>>,
    pub(super) lifecycle_bridge: Option<LifecycleBridgeConfig>,
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
            activation_invocations: Arc::new(Mutex::new(Vec::new())),
            boot_runtime_invocations: Arc::new(Mutex::new(Vec::new())),
            lifecycle_bridge: None,
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

    /// Install a private lifecycle command transport for this executor.
    pub fn with_lifecycle_bridge(mut self, bridge: LifecycleBridgeConfig) -> Self {
        self.lifecycle_bridge = Some(bridge);
        self
    }

    /// Execute the exact CCS pre-remove hook persisted for an installed trove.
    pub fn execute_ccs_remove_hook(&self, hook: &InstalledCcsRemoveHook) -> Result<()> {
        self.execute_ccs_remove_hook_with_outcome(hook)
            .into_result()
    }

    /// Execute a CCS pre-remove hook and return typed outcome metadata.
    pub fn execute_ccs_remove_hook_with_outcome(
        &self,
        hook: &InstalledCcsRemoveHook,
    ) -> ScriptletOutcome {
        self.execute_impl_with_outcome(
            "pre-remove",
            "/bin/sh",
            &hook.script,
            None,
            &ExecutionMode::Remove,
        )
    }

    /// Execute an exact signed CCS post-install hook in the selected root.
    pub(crate) fn execute_ccs_install_hook(&self, script: &str) -> Result<()> {
        self.execute_impl_with_outcome(
            "post-install",
            "/bin/sh",
            script,
            None,
            &ExecutionMode::Install,
        )
        .into_result()
    }

    /// Preflight a CCS pre-remove hook before any file/DB mutation.
    pub fn preflight_ccs_remove_hook(&self, hook: &InstalledCcsRemoveHook) -> Result<()> {
        self.preflight_impl(
            "pre-remove",
            "/bin/sh",
            &hook.script,
            &ExecutionMode::Remove,
        )
    }

    pub(super) fn clone_with_timeout(&self, timeout: Duration) -> Self {
        Self {
            root: self.root.clone(),
            package_name: self.package_name.clone(),
            package_version: self.package_version.clone(),
            package_format: self.package_format,
            timeout,
            sandbox_mode: self.sandbox_mode,
            activation_invocations: Arc::clone(&self.activation_invocations),
            boot_runtime_invocations: Arc::clone(&self.boot_runtime_invocations),
            lifecycle_bridge: self.lifecycle_bridge.clone(),
        }
    }

    /// Drain exact runtime service operations captured by successful
    /// selected-root lifecycle programs.
    pub fn take_activation_invocations(
        &self,
    ) -> Vec<crate::activation::RuntimeActivationInvocation> {
        std::mem::take(
            &mut *self
                .activation_invocations
                .lock()
                .expect("activation invocation collector poisoned"),
        )
    }

    /// Drain exact boot and kernel maintenance invocations captured at the
    /// selected-root execution boundary.
    pub fn take_boot_runtime_invocations(
        &self,
    ) -> Vec<crate::activation::BootRuntimeActivationInvocation> {
        std::mem::take(
            &mut *self
                .boot_runtime_invocations
                .lock()
                .expect("boot runtime invocation collector poisoned"),
        )
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
        // Resolve the typed execution boundary before preparing format-specific
        // execution, because malformed lifecycle contracts fail here too.
        let effective_sandbox = self.effective_sandbox();
        let requested_sandbox_mode = self.sandbox_mode;
        if let Err(error) = self.require_target_root() {
            return self.failure_from_error(
                phase,
                requested_sandbox_mode,
                effective_sandbox,
                error,
            );
        }

        // Analyze the formal shell command evidence.
        let analysis = analyze_script(content);

        if !analysis.patterns.is_empty() {
            info!(
                "{} scriptlet risk analysis: {} - {:?}",
                phase,
                analysis.risk.as_str(),
                analysis.patterns
            );
        }

        let interpreter_path = interpreter.to_string();

        let interpreter_check_path = self.root.join(interpreter_path.trim_start_matches('/'));

        if !interpreter_check_path.exists() {
            return self.failure_outcome(
                phase,
                ScriptletFailureKind::ProgramUnavailable,
                requested_sandbox_mode,
                effective_sandbox,
                format!(
                    "Interpreter {} not found in target root {}; cannot execute {} scriptlet",
                    interpreter_path,
                    self.root.display(),
                    phase
                ),
            );
        }

        // Prepare arguments based on distro, mode, and phase
        let args = match self.get_args(mode, phase) {
            Ok(args) => args,
            Err(error) => {
                return self.failure_from_error(
                    phase,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error,
                );
            }
        };

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
            effective_sandbox.as_str()
        );

        let result = self.execute_in_target(
            ScriptletProcess {
                phase,
                interpreter: &interpreter_path,
                interpreter_args: &[],
                args: &args,
                env: &env,
                stdin: &[],
            },
            content.as_bytes(),
        );

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
        _content: &str,
        _mode: &ExecutionMode,
    ) -> Result<()> {
        self.require_target_root()?;
        let interpreter_path = interpreter.to_string();
        let interpreter_check_path = self.root.join(interpreter_path.trim_start_matches('/'));

        if !interpreter_check_path.exists() {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ProgramUnavailable,
                format!(
                    "Interpreter {} not found in target root {}; cannot execute {} scriptlet",
                    interpreter_path,
                    self.root.display(),
                    phase
                ),
            ));
        }
        if !nix::unistd::geteuid().is_root() {
            return Err(Error::scriptlet(
                ScriptletFailureKind::SandboxSetupUnavailable,
                format!(
                    "Target-root lifecycle execution in {} requires root privileges",
                    self.root.display()
                ),
            ));
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
    use std::path::Path;

    #[test]
    fn test_executor_default_sandbox_is_always() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        assert_eq!(executor.sandbox_mode, SandboxMode::Always);
    }

    #[test]
    fn test_execute_with_outcome_rejects_the_host_root() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Conary);
        let outcome = executor.execute_impl_with_outcome(
            "post-install",
            "/definitely/missing/conary-interpreter",
            "exit 42",
            None,
            &ExecutionMode::Install,
        );

        let ScriptletOutcome::Failure(failure) = outcome else {
            panic!("expected scriptlet failure outcome");
        };
        assert_eq!(
            failure.failure_kind,
            ScriptletFailureKind::ContractViolation
        );
        assert_eq!(failure.requested_sandbox_mode, SandboxMode::Always);
        assert_eq!(failure.effective_sandbox, EffectiveSandbox::TargetRoot);
        assert!(failure.message.contains("host root '/'"));
    }

    #[test]
    fn test_execute_impl_missing_interpreter() {
        let root = tempfile::tempdir().unwrap();
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Rpm);

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
            err.contains("not found in target root"),
            "unexpected error: {}",
            err
        );
    }
}
