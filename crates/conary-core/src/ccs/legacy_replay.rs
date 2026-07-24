// conary-core/src/ccs/legacy_replay.rs
//! Safe replay planning for legacy scriptlet bundles.

use crate::ccs::legacy_scriptlets::{
    ForeignReplayPolicy, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
    ScriptletDecision, TargetCompatibility,
};
use crate::ccs::target_compatibility::{
    CompatibilityDecisionStatus, CompatibilityPreflightCheck, CompatibilityPreflightEnvironment,
    TargetCompatibilityDecision, TargetCompatibilityMatrix,
};
use crate::repository::distro::{ReplayTarget, replay_target_id, source_target_from_bundle};
use crate::scriptlet::SandboxMode;

const MIN_REPLAY_TIMEOUT_MS: u64 = 1_000;
const MAX_REPLAY_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyReplayPolicyInput<'a> {
    pub replay_enabled: bool,
    pub foreign_replay_override: bool,
    pub no_scripts: bool,
    pub requested_sandbox_mode: SandboxMode,
    pub host_policy: HostForeignReplayPolicy,
    pub target: ReplayTarget<'a>,
    pub compatibility_matrix: TargetCompatibilityMatrix,
    pub compatibility_environment: CompatibilityPreflightEnvironment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostForeignReplayPolicy {
    Strict,
    Guarded,
    Permissive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyReplayLifecycle {
    FreshInstallPre,
    FreshInstallPost,
    UpgradeNewPre,
    UpgradeNewPost,
    UpgradeOldPreRemove,
    UpgradeOldPostRemove,
    RemovePre,
    RemovePost,
    RollbackRestore,
    RollbackRemove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyReplayPreflight {
    NativeFree,
    FullyReplaced(LegacyReplayPlan),
    RequiresReplay(LegacyReplayPlan),
    Refused(LegacyReplayRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyReplayPlan {
    pub target_id: String,
    pub source_target_id: String,
    pub bundle_evidence_digest: Option<String>,
    pub lifecycle_entries: Vec<PlannedLegacyEntry>,
    pub sandbox_floor: SandboxMode,
    pub ccs_hooks_allowed: bool,
    pub raw_replay_required: bool,
    pub compatibility_decision: LegacyReplayCompatibilityDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyReplayCompatibilityDecision {
    pub decision: String,
    pub reason_code: String,
    pub matrix_entry_id: Option<String>,
    pub matrix_digest: Option<String>,
    pub preflight_checks: Vec<CompatibilityPreflightCheck>,
    pub override_required: bool,
    pub override_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedLegacyEntry {
    pub entry_id: String,
    pub native_slot: String,
    pub phase: LifecyclePath,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyReplayRefusalKind {
    ReviewEntry,
    BlockedEntry,
    UnknownDecision,
    LegacyReplayFeatureDisabled,
    NoScriptsWouldSkipRequiredReplay,
    TargetCompatibilityReviewRequired,
    TargetCompatibilityBlocked,
    TargetMismatch,
    ForeignReplayDeniedByBundle,
    ForeignReplayDeniedByHostPolicy,
    ForeignReplayOverrideRequired,
    SandboxRequirementUnsupported,
    TriggerReplayUnsupported,
    NativeArgsContractUnsupported,
    UnsatisfiedTransactionOrder,
    RollbackReplayUnavailable,
    ReplayExecutionUnavailable,
    TimeoutOutOfRange,
    MalformedBundle,
    CompatibilityMatrixEntryMissing,
    CompatibilityMatrixEntryAmbiguous,
    CompatibilityHelperMissing,
    CompatibilityHelperVersionMissing,
    CompatibilityHelperVersionUnsupported,
    CompatibilityPathMissing,
    CompatibilityServiceManagerMismatch,
    CompatibilitySecurityPolicyUnsupported,
    CompatibilitySandboxFloorUnsupported,
}

impl LegacyReplayRefusalKind {
    #[must_use]
    pub fn reason_code(self) -> &'static str {
        match self {
            Self::ReviewEntry => "legacy-review-entry",
            Self::BlockedEntry => "legacy-blocked-entry",
            Self::UnknownDecision => "legacy-unknown-decision",
            Self::LegacyReplayFeatureDisabled => "legacy-replay-feature-disabled",
            Self::NoScriptsWouldSkipRequiredReplay => "no-scripts-would-skip-required-replay",
            Self::TargetCompatibilityReviewRequired => "target-compatibility-review-required",
            Self::TargetCompatibilityBlocked => "target-compatibility-blocked",
            Self::TargetMismatch => "target-mismatch",
            Self::ForeignReplayDeniedByBundle => "foreign-replay-denied-by-bundle",
            Self::ForeignReplayDeniedByHostPolicy => "foreign-replay-denied-by-host-policy",
            Self::ForeignReplayOverrideRequired => "foreign-replay-override-required",
            Self::SandboxRequirementUnsupported => "sandbox-requirement-unsupported",
            Self::TriggerReplayUnsupported => "trigger-replay-unsupported",
            Self::NativeArgsContractUnsupported => "native-args-contract-unsupported",
            Self::UnsatisfiedTransactionOrder => "unsatisfied-transaction-order",
            Self::RollbackReplayUnavailable => "rollback-replay-unavailable",
            Self::ReplayExecutionUnavailable => "replay-execution-unavailable",
            Self::TimeoutOutOfRange => "timeout-out-of-range",
            Self::MalformedBundle => "malformed-bundle",
            Self::CompatibilityMatrixEntryMissing => "compatibility-matrix-entry-missing",
            Self::CompatibilityMatrixEntryAmbiguous => "compatibility-matrix-entry-ambiguous",
            Self::CompatibilityHelperMissing => "compatibility-helper-missing",
            Self::CompatibilityHelperVersionMissing => "compatibility-helper-version-missing",
            Self::CompatibilityHelperVersionUnsupported => {
                "compatibility-helper-version-unsupported"
            }
            Self::CompatibilityPathMissing => "compatibility-path-missing",
            Self::CompatibilityServiceManagerMismatch => "compatibility-service-manager-mismatch",
            Self::CompatibilitySecurityPolicyUnsupported => {
                "compatibility-security-policy-unsupported"
            }
            Self::CompatibilitySandboxFloorUnsupported => "compatibility-sandbox-floor-unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyReplayRefusal {
    pub kind: LegacyReplayRefusalKind,
    pub entry_id: Option<String>,
    pub message: String,
}

pub fn plan_legacy_replay(
    bundle: Option<&LegacyScriptletBundle>,
    lifecycle: LegacyReplayLifecycle,
    input: &LegacyReplayPolicyInput<'_>,
) -> anyhow::Result<LegacyReplayPreflight> {
    let Some(bundle) = bundle else {
        return Ok(LegacyReplayPreflight::NativeFree);
    };

    if let Err(error) = bundle.validate() {
        return Ok(refused(
            LegacyReplayRefusalKind::MalformedBundle,
            None,
            error.to_string(),
        ));
    }

    if let Some(refusal) = admission_refusal(bundle) {
        return Ok(refusal);
    }

    if matches!(
        lifecycle,
        LegacyReplayLifecycle::RollbackRestore | LegacyReplayLifecycle::RollbackRemove
    ) && bundle
        .entries
        .iter()
        .any(|entry| entry.decision == ScriptletDecision::Legacy)
    {
        return Ok(refused(
            LegacyReplayRefusalKind::RollbackReplayUnavailable,
            None,
            "rollback cannot execute raw legacy replay in Goal 6",
        ));
    }

    let selected = select_lifecycle_entries(bundle, lifecycle);
    if selected.is_empty() {
        return Ok(LegacyReplayPreflight::NativeFree);
    }

    let target_id = replay_target_id(&input.target);
    let source_target = source_target_from_bundle(bundle);
    let source_target_id = source_target.to_id();
    let selected_legacy: Vec<&LegacyScriptletEntry> = selected
        .iter()
        .copied()
        .filter(|entry| entry.decision == ScriptletDecision::Legacy)
        .collect();

    if selected_legacy.is_empty() {
        return Ok(LegacyReplayPreflight::FullyReplaced(build_plan(
            bundle,
            input,
            target_id,
            source_target_id,
            Vec::new(),
            false,
            compatibility_decision_for_no_raw_replay(),
        )));
    }

    if pre_mutation_order_conflict(lifecycle, &selected) {
        return Ok(refused(
            LegacyReplayRefusalKind::UnsatisfiedTransactionOrder,
            None,
            "raw legacy replay cannot be safely interleaved with generated hooks",
        ));
    }

    for entry in &selected_legacy {
        if entry.timeout_ms < MIN_REPLAY_TIMEOUT_MS || entry.timeout_ms > MAX_REPLAY_TIMEOUT_MS {
            return Ok(refused(
                LegacyReplayRefusalKind::TimeoutOutOfRange,
                Some(&entry.id),
                "legacy replay timeout is outside the Goal 6 allowed range",
            ));
        }
    }

    if input.no_scripts {
        return Ok(refused(
            LegacyReplayRefusalKind::NoScriptsWouldSkipRequiredReplay,
            selected_legacy.first().map(|entry| entry.id.as_str()),
            "--no-scripts would skip required raw legacy replay",
        ));
    }
    if !input.replay_enabled {
        return Ok(refused(
            LegacyReplayRefusalKind::LegacyReplayFeatureDisabled,
            selected_legacy.first().map(|entry| entry.id.as_str()),
            "raw legacy replay requires an explicit operator opt-in",
        ));
    }

    let compatibility_decision =
        match compatibility_decision_from_target(bundle, input, &target_id, &source_target_id) {
            Ok(decision) => decision,
            Err(refusal) => return Ok(LegacyReplayPreflight::Refused(refusal)),
        };

    if let Some(refusal) = foreign_replay_refusal(bundle, input, &target_id, &source_target_id) {
        return Ok(refusal);
    }

    Ok(LegacyReplayPreflight::RequiresReplay(build_plan(
        bundle,
        input,
        target_id,
        source_target_id,
        selected_legacy,
        true,
        compatibility_decision,
    )))
}

fn admission_refusal(bundle: &LegacyScriptletBundle) -> Option<LegacyReplayPreflight> {
    for entry in &bundle.entries {
        let kind = match &entry.decision {
            ScriptletDecision::Review => Some(LegacyReplayRefusalKind::ReviewEntry),
            ScriptletDecision::Blocked => Some(LegacyReplayRefusalKind::BlockedEntry),
            ScriptletDecision::Unknown(_) => Some(LegacyReplayRefusalKind::UnknownDecision),
            _ => None,
        };
        if let Some(kind) = kind {
            return Some(refused(
                kind,
                Some(&entry.id),
                "legacy scriptlet bundle contains a non-actionable entry decision",
            ));
        }
        if matches!(
            entry.phase,
            LifecyclePath::Trigger | LifecyclePath::FileTrigger
        ) && entry.decision != ScriptletDecision::Replaced
        {
            return Some(refused(
                LegacyReplayRefusalKind::TriggerReplayUnsupported,
                Some(&entry.id),
                "raw trigger and file-trigger replay is unsupported in Goal 6",
            ));
        }
        if entry.arch_install.is_some() && entry.decision == ScriptletDecision::Legacy {
            return Some(refused(
                LegacyReplayRefusalKind::ReplayExecutionUnavailable,
                Some(&entry.id),
                "raw Arch .INSTALL wrapper replay is unsupported",
            ));
        }
    }
    None
}

fn compatibility_decision_for_no_raw_replay() -> LegacyReplayCompatibilityDecision {
    LegacyReplayCompatibilityDecision {
        decision: "native-free".to_string(),
        reason_code: "compatibility-native-free".to_string(),
        matrix_entry_id: None,
        matrix_digest: None,
        preflight_checks: Vec::new(),
        override_required: false,
        override_used: false,
    }
}

fn compatibility_decision_from_target(
    bundle: &LegacyScriptletBundle,
    input: &LegacyReplayPolicyInput<'_>,
    target_id: &str,
    source_target_id: &str,
) -> Result<LegacyReplayCompatibilityDecision, LegacyReplayRefusal> {
    match &bundle.target_compatibility {
        TargetCompatibility::SourceNative => {
            if target_id == source_target_id
                || bundle
                    .allowed_targets
                    .iter()
                    .any(|allowed| allowed == target_id)
            {
                Ok(LegacyReplayCompatibilityDecision {
                    decision: "accepted".to_string(),
                    reason_code: "compatibility-source-native".to_string(),
                    matrix_entry_id: None,
                    matrix_digest: None,
                    preflight_checks: Vec::new(),
                    override_required: target_id != source_target_id,
                    override_used: input.foreign_replay_override,
                })
            } else {
                Err(refusal(
                    LegacyReplayRefusalKind::TargetMismatch,
                    None,
                    format!("target {target_id} does not match source {source_target_id}"),
                ))
            }
        }
        TargetCompatibility::ConaryPortable => Ok(LegacyReplayCompatibilityDecision {
            decision: "accepted".to_string(),
            reason_code: "compatibility-conary-portable".to_string(),
            matrix_entry_id: None,
            matrix_digest: None,
            preflight_checks: Vec::new(),
            override_required: target_id != source_target_id,
            override_used: input.foreign_replay_override,
        }),
        TargetCompatibility::FamilyCompatible => {
            let source_target = source_target_from_bundle(bundle);
            let matched = input
                .compatibility_matrix
                .match_entry(&source_target.as_target(), &input.target)
                .map_err(|error| {
                    refusal(
                        LegacyReplayRefusalKind::CompatibilityMatrixEntryAmbiguous,
                        None,
                        error.to_string(),
                    )
                })?;
            let Some(matched) = matched else {
                return Err(refusal(
                    LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
                    None,
                    format!(
                        "no compatibility matrix entry authorizes {source_target_id} on {target_id}"
                    ),
                ));
            };
            let decision = input
                .compatibility_matrix
                .preflight_entry(&matched, &input.compatibility_environment);
            if decision.decision == CompatibilityDecisionStatus::Accepted {
                Ok(LegacyReplayCompatibilityDecision {
                    decision: "accepted".to_string(),
                    reason_code: decision.reason_code,
                    matrix_entry_id: decision.matrix_entry_id,
                    matrix_digest: decision.matrix_digest,
                    preflight_checks: decision.preflight_checks,
                    override_required: target_id != source_target_id,
                    override_used: input.foreign_replay_override,
                })
            } else {
                Err(refusal_from_compatibility_decision(decision))
            }
        }
        TargetCompatibility::ReviewRequired => Err(refusal(
            LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
            None,
            "target compatibility requires review",
        )),
        TargetCompatibility::Blocked => Err(refusal(
            LegacyReplayRefusalKind::TargetCompatibilityBlocked,
            None,
            "target compatibility is blocked",
        )),
        TargetCompatibility::Unknown(value) => Err(refusal(
            LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
            None,
            format!("unknown target compatibility {value}"),
        )),
    }
}

fn refusal_from_compatibility_decision(
    decision: TargetCompatibilityDecision,
) -> LegacyReplayRefusal {
    let kind = match decision.reason_code.as_str() {
        "compatibility-helper-missing" => LegacyReplayRefusalKind::CompatibilityHelperMissing,
        "compatibility-helper-version-missing" => {
            LegacyReplayRefusalKind::CompatibilityHelperVersionMissing
        }
        "compatibility-helper-version-unsupported" => {
            LegacyReplayRefusalKind::CompatibilityHelperVersionUnsupported
        }
        "compatibility-path-missing" => LegacyReplayRefusalKind::CompatibilityPathMissing,
        "compatibility-service-manager-mismatch" => {
            LegacyReplayRefusalKind::CompatibilityServiceManagerMismatch
        }
        "compatibility-security-policy-unsupported" => {
            LegacyReplayRefusalKind::CompatibilitySecurityPolicyUnsupported
        }
        "compatibility-sandbox-floor-unsupported" => {
            LegacyReplayRefusalKind::CompatibilitySandboxFloorUnsupported
        }
        _ => LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    };

    refusal(
        kind,
        None,
        format!(
            "{} for matrix entry {}",
            decision.reason_code,
            decision.matrix_entry_id.as_deref().unwrap_or("unknown")
        ),
    )
}

fn foreign_replay_refusal(
    bundle: &LegacyScriptletBundle,
    input: &LegacyReplayPolicyInput<'_>,
    target_id: &str,
    source_target_id: &str,
) -> Option<LegacyReplayPreflight> {
    if target_id == source_target_id
        || bundle
            .allowed_targets
            .iter()
            .any(|allowed| allowed == target_id)
    {
        return None;
    }

    if matches!(
        &bundle.foreign_replay_policy,
        ForeignReplayPolicy::Deny | ForeignReplayPolicy::Unknown(_)
    ) {
        return Some(refused(
            LegacyReplayRefusalKind::ForeignReplayDeniedByBundle,
            None,
            "bundle policy denies foreign legacy replay",
        ));
    }
    if input.host_policy == HostForeignReplayPolicy::Strict {
        return Some(refused(
            LegacyReplayRefusalKind::ForeignReplayDeniedByHostPolicy,
            None,
            "host policy denies foreign legacy replay",
        ));
    }
    if !input.foreign_replay_override {
        return Some(refused(
            LegacyReplayRefusalKind::ForeignReplayOverrideRequired,
            None,
            "foreign legacy replay requires an explicit operator override",
        ));
    }

    match (&bundle.foreign_replay_policy, input.host_policy) {
        (
            ForeignReplayPolicy::Guarded,
            HostForeignReplayPolicy::Guarded | HostForeignReplayPolicy::Permissive,
        )
        | (ForeignReplayPolicy::Permissive, HostForeignReplayPolicy::Permissive) => None,
        _ => Some(refused(
            LegacyReplayRefusalKind::ForeignReplayDeniedByHostPolicy,
            None,
            "host policy is not compatible with the bundle foreign replay policy",
        )),
    }
}

fn select_lifecycle_entries(
    bundle: &LegacyScriptletBundle,
    lifecycle: LegacyReplayLifecycle,
) -> Vec<&LegacyScriptletEntry> {
    match lifecycle {
        LegacyReplayLifecycle::FreshInstallPre => entries_for_phases(
            bundle,
            &[LifecyclePath::PreTransaction, LifecyclePath::PreInstall],
        ),
        LegacyReplayLifecycle::FreshInstallPost => entries_for_phases(
            bundle,
            &[LifecyclePath::PostInstall, LifecyclePath::PostTransaction],
        ),
        LegacyReplayLifecycle::UpgradeNewPre => entries_for_upgrade_fallback(
            bundle,
            LifecyclePath::PreUpgrade,
            LifecyclePath::PreInstall,
        ),
        LegacyReplayLifecycle::UpgradeNewPost => entries_for_upgrade_fallback(
            bundle,
            LifecyclePath::PostUpgrade,
            LifecyclePath::PostInstall,
        ),
        LegacyReplayLifecycle::UpgradeOldPreRemove | LegacyReplayLifecycle::RemovePre => {
            entries_for_phases(bundle, &[LifecyclePath::PreRemove])
        }
        LegacyReplayLifecycle::UpgradeOldPostRemove | LegacyReplayLifecycle::RemovePost => {
            entries_for_phases(bundle, &[LifecyclePath::PostRemove])
        }
        LegacyReplayLifecycle::RollbackRestore | LegacyReplayLifecycle::RollbackRemove => {
            bundle.entries.iter().collect()
        }
    }
}

fn entries_for_phases<'a>(
    bundle: &'a LegacyScriptletBundle,
    phases: &[LifecyclePath],
) -> Vec<&'a LegacyScriptletEntry> {
    bundle
        .entries
        .iter()
        .filter(|entry| phases.iter().any(|phase| &entry.phase == phase))
        .collect()
}

fn entries_for_upgrade_fallback(
    bundle: &LegacyScriptletBundle,
    direct: LifecyclePath,
    fallback: LifecyclePath,
) -> Vec<&LegacyScriptletEntry> {
    let direct_entries = entries_for_phases(bundle, &[direct]);
    if direct_entries.is_empty() {
        entries_for_phases(bundle, &[fallback])
    } else {
        direct_entries
    }
}

fn pre_mutation_order_conflict(
    lifecycle: LegacyReplayLifecycle,
    selected: &[&LegacyScriptletEntry],
) -> bool {
    if !matches!(
        lifecycle,
        LegacyReplayLifecycle::FreshInstallPre
            | LegacyReplayLifecycle::UpgradeNewPre
            | LegacyReplayLifecycle::UpgradeOldPreRemove
            | LegacyReplayLifecycle::RemovePre
            | LegacyReplayLifecycle::RollbackRestore
            | LegacyReplayLifecycle::RollbackRemove
    ) {
        return false;
    }

    let legacy_entries: Vec<&LegacyScriptletEntry> = selected
        .iter()
        .copied()
        .filter(|entry| entry.decision == ScriptletDecision::Legacy)
        .collect();
    let replaced_entries: Vec<&LegacyScriptletEntry> = selected
        .iter()
        .copied()
        .filter(|entry| entry.decision == ScriptletDecision::Replaced)
        .collect();
    if legacy_entries.is_empty() || replaced_entries.is_empty() {
        return false;
    }

    legacy_entries.iter().any(|legacy| {
        replaced_entries.iter().any(|replaced| {
            references_entry(&legacy.transaction_order.after, replaced)
                || references_entry(&replaced.transaction_order.after, legacy)
        })
    })
}

fn references_entry(references: &[String], entry: &LegacyScriptletEntry) -> bool {
    references
        .iter()
        .any(|reference| reference == &entry.id || reference == &entry.native_slot)
}

fn build_plan(
    bundle: &LegacyScriptletBundle,
    input: &LegacyReplayPolicyInput<'_>,
    target_id: String,
    source_target_id: String,
    entries: Vec<&LegacyScriptletEntry>,
    raw_replay_required: bool,
    compatibility_decision: LegacyReplayCompatibilityDecision,
) -> LegacyReplayPlan {
    LegacyReplayPlan {
        target_id,
        source_target_id,
        bundle_evidence_digest: bundle.evidence_digest.clone(),
        lifecycle_entries: entries
            .into_iter()
            .map(|entry| PlannedLegacyEntry {
                entry_id: entry.id.clone(),
                native_slot: entry.native_slot.clone(),
                phase: entry.phase.clone(),
                timeout_ms: entry.timeout_ms,
            })
            .collect(),
        sandbox_floor: input.requested_sandbox_mode,
        ccs_hooks_allowed: !input.no_scripts,
        raw_replay_required,
        compatibility_decision,
    }
}

fn refused(
    kind: LegacyReplayRefusalKind,
    entry_id: Option<&str>,
    message: impl Into<String>,
) -> LegacyReplayPreflight {
    LegacyReplayPreflight::Refused(refusal(kind, entry_id, message))
}

fn refusal(
    kind: LegacyReplayRefusalKind,
    entry_id: Option<&str>,
    message: impl Into<String>,
) -> LegacyReplayRefusal {
    LegacyReplayRefusal {
        kind,
        entry_id: entry_id.map(str::to_string),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
