// apps/conary/src/commands/remove/types.rs

use conary_core::ccs::legacy_replay::{HostForeignReplayPolicy, LegacyReplayPlan};
use conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle;
use conary_core::db::models::{ScriptletEntry, Trove};
use conary_core::scriptlet::{
    PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletOutcome,
};

use crate::commands::{LegacyReplayCompatibilityAudit, LegacyReplayOptions, TroveSnapshot};

pub(crate) struct RemoveInnerResult {
    pub(super) changeset_id: i64,
    pub(crate) snapshot: TroveSnapshot,
    pub(super) trove: Trove,
    pub(super) stored_scriptlets: Vec<ScriptletEntry>,
    pub(super) scriptlet_format: ScriptletPackageFormat,
    pub(super) removed_count: usize,
    pub(super) dirs_removed: usize,
    #[allow(dead_code)]
    pub(super) planned_pre_remove: Option<LegacyReplayPlan>,
    pub(super) legacy_bundle: Option<LegacyScriptletBundle>,
    pub(super) legacy_pre_outcomes: Vec<ScriptletOutcome>,
    pub(super) legacy_audit_context: Option<LegacyRemoveReplayAuditContext>,
    pub(super) planned_post_remove: Option<LegacyReplayPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LegacyRemoveReplayAuditContext {
    pub(super) target_id: String,
    pub(super) source_target_id: String,
    pub(super) target_compatibility: String,
    pub(super) foreign_replay_policy: String,
    pub(super) host_policy: HostForeignReplayPolicy,
    pub(super) feature_gate_enabled: bool,
    pub(super) foreign_override: bool,
    pub(super) evidence_digest: Option<String>,
    pub(super) compatibility: LegacyReplayCompatibilityAudit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoveScriptletOptions {
    pub(super) no_scripts: bool,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) legacy_replay: LegacyReplayOptions,
}

impl RemoveScriptletOptions {
    pub(crate) fn new(
        no_scripts: bool,
        sandbox_mode: SandboxMode,
        legacy_replay: LegacyReplayOptions,
    ) -> Self {
        Self {
            no_scripts,
            sandbox_mode,
            legacy_replay,
        }
    }
}
