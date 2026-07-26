// conary-core/src/scriptlet/sandbox.rs

//! Closed scriptlet execution-boundary types.

use super::ScriptletExecutor;
use crate::error::{Error, Result, ScriptletFailureKind};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Sandbox policy for scriptlet execution.
///
/// There is one policy and no bypass: lifecycle programs run inside a
/// materialized selected/target root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    #[default]
    Always,
}

impl SandboxMode {
    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
        }
    }
}

/// Execution boundary actually used for lifecycle programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveSandbox {
    /// Chrooted materialized selected root or explicit offline target root.
    TargetRoot,
}

impl EffectiveSandbox {
    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetRoot => "target-root",
        }
    }
}

impl ScriptletExecutor {
    pub(super) const fn effective_sandbox(&self) -> EffectiveSandbox {
        EffectiveSandbox::TargetRoot
    }

    /// Refuse the host root before resolving an interpreter or staging bytes.
    pub(super) fn require_target_root(&self) -> Result<()> {
        if self.root == Path::new("/") {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                "Lifecycle execution requires a materialized selected/target root; the host root '/' is not an execution target",
            ));
        }
        if !self.root.is_absolute() {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                format!(
                    "Lifecycle execution root must be absolute: {}",
                    self.root.display()
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{PackageFormat, ScriptletExecutor};
    use super::{EffectiveSandbox, SandboxMode};
    use std::path::Path;

    #[test]
    fn sandbox_mode_has_no_bypass_variant() {
        assert_eq!(SandboxMode::default(), SandboxMode::Always);
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"always\"").unwrap(),
            SandboxMode::Always
        );
        for removed in ["never", "auto", "none"] {
            assert!(serde_json::from_str::<SandboxMode>(&format!("\"{removed}\"")).is_err());
        }
    }

    #[test]
    fn host_root_is_not_a_lifecycle_execution_target() {
        let executor = ScriptletExecutor::new(Path::new("/"), "fixture", "1", PackageFormat::Rpm);
        let error = executor.require_target_root().unwrap_err().to_string();
        assert!(error.contains("host root '/' is not an execution target"));
    }

    #[test]
    fn materialized_root_uses_the_only_effective_boundary() {
        let root = tempfile::tempdir().unwrap();
        let executor = ScriptletExecutor::new(root.path(), "fixture", "1", PackageFormat::Rpm);
        executor.require_target_root().unwrap();
        assert_eq!(executor.effective_sandbox(), EffectiveSandbox::TargetRoot);
    }
}
