// conary-core/src/scriptlet/outcome.rs

use super::{EffectiveSandbox, SandboxMode, ScriptletExecutor};
use crate::error::{Error, Result};

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

impl ScriptletExecutor {
    pub(super) fn failure_outcome(
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

    pub(super) fn failure_from_error(
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
