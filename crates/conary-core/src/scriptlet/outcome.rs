// conary-core/src/scriptlet/outcome.rs

use super::{EffectiveSandbox, SandboxMode, ScriptletExecutor};
pub use crate::error::ScriptletFailureKind;
use crate::error::{Error, Result};

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
    /// Convert the structured outcome into the command-facing result.
    pub fn into_result(self) -> Result<()> {
        match self {
            Self::Success { .. } => Ok(()),
            Self::Failure(failure) => Err(Error::scriptlet(failure.failure_kind, failure.message)),
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
        let (failure_kind, message) = match error {
            Error::ScriptletExecution { kind, message } => (kind, message),
            other => (ScriptletFailureKind::ProcessSetupFailed, other.to_string()),
        };
        self.failure_outcome(
            phase,
            failure_kind,
            requested_sandbox_mode,
            effective_sandbox,
            message,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scriptlet::PackageFormat;
    use std::path::Path;

    #[test]
    fn failure_class_is_independent_of_human_message_text() {
        let executor = ScriptletExecutor::new(Path::new("/"), "fixture", "1", PackageFormat::Rpm);
        let outcome = executor.failure_from_error(
            "post-install",
            SandboxMode::Always,
            EffectiveSandbox::TargetRoot,
            Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                "wording says timed out and failed with exit code, but is not authority",
            ),
        );

        let ScriptletOutcome::Failure(failure) = outcome else {
            panic!("typed execution error must produce a failure outcome");
        };
        assert_eq!(
            failure.failure_kind,
            ScriptletFailureKind::ContractViolation
        );
    }
}
