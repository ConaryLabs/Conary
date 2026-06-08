// conary-core/src/scriptlet/outcome.rs

use super::{EffectiveSandbox, SandboxMode};
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
