// conary-core/src/ccs/legacy_replay.rs
//! Safe replay planning for legacy scriptlet bundles.

use crate::ccs::legacy_scriptlets::{
    ForeignReplayPolicy, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
    ScriptletDecision, TargetCompatibility,
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

    if let Some(refusal) = target_compatibility_refusal(bundle, &target_id, &source_target_id) {
        return Ok(refusal);
    }

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
    }
    None
}

fn target_compatibility_refusal(
    bundle: &LegacyScriptletBundle,
    target_id: &str,
    source_target_id: &str,
) -> Option<LegacyReplayPreflight> {
    match &bundle.target_compatibility {
        TargetCompatibility::ReviewRequired | TargetCompatibility::Unknown(_) => Some(refused(
            LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
            None,
            "target compatibility requires review before raw replay",
        )),
        TargetCompatibility::Blocked => Some(refused(
            LegacyReplayRefusalKind::TargetCompatibilityBlocked,
            None,
            "target compatibility blocks raw replay",
        )),
        TargetCompatibility::SourceNative => {
            if target_id == source_target_id
                || bundle
                    .allowed_targets
                    .iter()
                    .any(|allowed| allowed == target_id)
            {
                None
            } else {
                Some(refused(
                    LegacyReplayRefusalKind::TargetMismatch,
                    None,
                    "source-native legacy replay target does not match this host",
                ))
            }
        }
        TargetCompatibility::FamilyCompatible | TargetCompatibility::ConaryPortable => None,
    }
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
    }
}

fn refused(
    kind: LegacyReplayRefusalKind,
    entry_id: Option<&str>,
    message: impl Into<String>,
) -> LegacyReplayPreflight {
    LegacyReplayPreflight::Refused(LegacyReplayRefusal {
        kind,
        entry_id: entry_id.map(str::to_string),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
        LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
        PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
        TransactionOrder, VersionScheme,
    };
    use crate::hash;
    use crate::repository::distro::{ReplayTarget, source_target_from_bundle};
    use crate::scriptlet::SandboxMode;
    use std::collections::BTreeMap;

    fn target() -> ReplayTarget<'static> {
        ReplayTarget {
            format: "rpm",
            distro: "fedora",
            release: "44",
            arch: "x86_64",
        }
    }

    fn policy_input() -> LegacyReplayPolicyInput<'static> {
        LegacyReplayPolicyInput {
            replay_enabled: false,
            foreign_replay_override: false,
            no_scripts: false,
            requested_sandbox_mode: SandboxMode::Always,
            host_policy: HostForeignReplayPolicy::Strict,
            target: target(),
        }
    }

    fn entry(id: &str, phase: LifecyclePath, decision: ScriptletDecision) -> LegacyScriptletEntry {
        let body = format!("echo {id}\n");
        LegacyScriptletEntry {
            id: id.to_string(),
            native_slot: id.to_string(),
            phase,
            lifecycle_paths: vec!["fixture".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "default".to_string(),
                before: Vec::new(),
                after: Vec::new(),
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision,
            reason_code: "fixture".to_string(),
            human_reason: None,
            evidence_digest: None,
            source_evidence_refs: Vec::new(),
            effects: Vec::new(),
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn bundle_with_entries(entries: Vec<LegacyScriptletEntry>) -> LegacyScriptletBundle {
        let mut decision_counts = DecisionCounts::default();
        for entry in &entries {
            match &entry.decision {
                ScriptletDecision::Replaced => decision_counts.replaced += 1,
                ScriptletDecision::Legacy => decision_counts.legacy += 1,
                ScriptletDecision::Blocked => decision_counts.blocked += 1,
                ScriptletDecision::Review => decision_counts.review += 1,
                ScriptletDecision::Unknown(value) => {
                    *decision_counts.extra.entry(value.clone()).or_default() += 1;
                }
            }
        }

        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "fixture".to_string(),
            source_version: "1.0-1".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "test".to_string(),
            conversion_tool_version: "0.0.0".to_string(),
            conversion_policy: "fixture".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            ),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: Vec::new(),
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::PublicIfNoBlocked,
            publication_status: PublicationStatus::Public,
            scriptlet_fidelity: ScriptletFidelity::Mixed,
            decision_counts,
            unsupported_class_counts: BTreeMap::new(),
            entries,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn arch_source_release_none_normalizes_to_rolling() {
        let mut bundle = bundle_with_entries(Vec::new());
        bundle.source_format = SourceFormat::Arch;
        bundle.source_family = "arch".to_string();
        bundle.source_distro = Some("arch".to_string());
        bundle.source_release = None;
        bundle.source_arch = Some("x86_64".to_string());

        assert_eq!(
            source_target_from_bundle(&bundle).to_id(),
            "arch/arch/rolling/x86_64"
        );
    }

    #[test]
    fn review_blocked_and_unknown_entries_refuse_admission_anywhere_in_bundle() {
        for (decision, expected) in [
            (
                ScriptletDecision::Review,
                LegacyReplayRefusalKind::ReviewEntry,
            ),
            (
                ScriptletDecision::Blocked,
                LegacyReplayRefusalKind::BlockedEntry,
            ),
            (
                ScriptletDecision::Unknown("mystery".to_string()),
                LegacyReplayRefusalKind::UnknownDecision,
            ),
        ] {
            let bundle =
                bundle_with_entries(vec![entry("future", LifecyclePath::PostRemove, decision)]);

            assert_refused(
                plan_legacy_replay(
                    Some(&bundle),
                    LegacyReplayLifecycle::FreshInstallPost,
                    &policy_input(),
                )
                .expect("plan"),
                expected,
            );
        }
    }

    #[test]
    fn future_lifecycle_legacy_entry_is_not_selected_for_current_install() {
        let bundle = bundle_with_entries(vec![entry(
            "future-remove",
            LifecyclePath::PostRemove,
            ScriptletDecision::Legacy,
        )]);

        let preflight = plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &policy_input(),
        )
        .expect("plan");

        assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
    }

    #[test]
    fn no_bundle_keeps_no_scripts_native_free() {
        let mut input = policy_input();
        input.no_scripts = true;

        let preflight = plan_legacy_replay(None, LegacyReplayLifecycle::FreshInstallPost, &input)
            .expect("plan");

        assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
    }

    #[test]
    fn native_free_bundle_is_allowed_with_no_scripts() {
        let bundle = bundle_with_entries(Vec::new());
        let mut input = policy_input();
        input.no_scripts = true;

        let preflight = plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan");

        assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
    }

    #[test]
    fn no_scripts_future_lifecycle_legacy_entry_is_not_selected_for_current_install() {
        let bundle = bundle_with_entries(vec![entry(
            "future-remove",
            LifecyclePath::PostRemove,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.no_scripts = true;

        let preflight = plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan");

        assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
    }

    #[test]
    fn selected_legacy_entry_requires_feature_gate() {
        let bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &policy_input(),
            )
            .expect("plan"),
            LegacyReplayRefusalKind::LegacyReplayFeatureDisabled,
        );
    }

    #[test]
    fn no_scripts_refuses_selected_required_legacy_replay() {
        let bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.replay_enabled = true;
        input.no_scripts = true;

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::NoScriptsWouldSkipRequiredReplay,
        );
    }

    #[test]
    fn replaced_entries_never_schedule_raw_replay() {
        let bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Replaced,
        )]);

        let preflight = plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &policy_input(),
        )
        .expect("plan");

        let LegacyReplayPreflight::FullyReplaced(plan) = preflight else {
            panic!("expected fully replaced plan");
        };
        assert!(!plan.raw_replay_required);
        assert!(plan.lifecycle_entries.is_empty());
    }

    #[test]
    fn no_scripts_replaced_only_bundle_suppresses_ccs_hooks_in_plan() {
        let bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Replaced,
        )]);
        let mut input = policy_input();
        input.no_scripts = true;

        let preflight = plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan");

        let LegacyReplayPreflight::FullyReplaced(plan) = preflight else {
            panic!("expected fully replaced plan");
        };
        assert!(!plan.ccs_hooks_allowed);
        assert!(!plan.raw_replay_required);
        assert!(plan.lifecycle_entries.is_empty());
    }

    #[test]
    fn review_and_blocked_entries_refuse_even_with_no_scripts() {
        for (decision, expected) in [
            (
                ScriptletDecision::Review,
                LegacyReplayRefusalKind::ReviewEntry,
            ),
            (
                ScriptletDecision::Blocked,
                LegacyReplayRefusalKind::BlockedEntry,
            ),
        ] {
            let bundle =
                bundle_with_entries(vec![entry("future", LifecyclePath::PostRemove, decision)]);
            let mut input = policy_input();
            input.no_scripts = true;

            assert_refused(
                plan_legacy_replay(
                    Some(&bundle),
                    LegacyReplayLifecycle::FreshInstallPost,
                    &input,
                )
                .expect("plan"),
                expected,
            );
        }
    }

    #[test]
    fn scriptlet_fidelity_legacy_replay_does_not_override_entry_decisions() {
        let mut bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Replaced,
        )]);
        bundle.scriptlet_fidelity = ScriptletFidelity::LegacyReplay;

        let preflight = plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &policy_input(),
        )
        .expect("plan");

        assert!(matches!(preflight, LegacyReplayPreflight::FullyReplaced(_)));
    }

    #[test]
    fn upgrade_lifecycle_selection_uses_upgrade_slots_and_fallbacks() {
        let direct = bundle_with_entries(vec![entry(
            "pre-upgrade",
            LifecyclePath::PreUpgrade,
            ScriptletDecision::Legacy,
        )]);
        let fallback = bundle_with_entries(vec![entry(
            "pre-install",
            LifecyclePath::PreInstall,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.replay_enabled = true;

        assert_plan_entry_ids(
            plan_legacy_replay(Some(&direct), LegacyReplayLifecycle::UpgradeNewPre, &input)
                .expect("plan"),
            &["pre-upgrade"],
        );
        assert_plan_entry_ids(
            plan_legacy_replay(
                Some(&fallback),
                LegacyReplayLifecycle::UpgradeNewPre,
                &input,
            )
            .expect("plan"),
            &["pre-install"],
        );
    }

    #[test]
    fn raw_trigger_replay_is_refused() {
        let bundle = bundle_with_entries(vec![entry(
            "trigger",
            LifecyclePath::Trigger,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.replay_enabled = true;

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::TriggerReplayUnsupported,
        );
    }

    #[test]
    fn target_mismatch_refuses_source_native_raw_replay() {
        let bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.replay_enabled = true;
        input.target = ReplayTarget {
            format: "rpm",
            distro: "fedora",
            release: "45",
            arch: "x86_64",
        };

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::TargetMismatch,
        );
    }

    #[test]
    fn unknown_target_release_refuses_source_native_raw_replay() {
        let bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.replay_enabled = true;
        input.target = ReplayTarget {
            format: "rpm",
            distro: "fedora",
            release: "unknown",
            arch: "x86_64",
        };

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::TargetMismatch,
        );
    }

    #[test]
    fn same_source_raw_replay_does_not_need_foreign_override() {
        let bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.replay_enabled = true;
        input.host_policy = HostForeignReplayPolicy::Strict;
        input.foreign_replay_override = false;

        assert_plan_entry_ids(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            &["post"],
        );
    }

    #[test]
    fn old_upgrade_remove_lifecycle_selects_installed_bundle_remove_entries() {
        let bundle = bundle_with_entries(vec![
            entry(
                "old-pre-remove",
                LifecyclePath::PreRemove,
                ScriptletDecision::Legacy,
            ),
            entry(
                "old-post-remove",
                LifecyclePath::PostRemove,
                ScriptletDecision::Legacy,
            ),
        ]);
        let mut input = policy_input();
        input.replay_enabled = true;

        assert_plan_entry_ids(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::UpgradeOldPreRemove,
                &input,
            )
            .expect("plan"),
            &["old-pre-remove"],
        );
        assert_plan_entry_ids(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::UpgradeOldPostRemove,
                &input,
            )
            .expect("plan"),
            &["old-post-remove"],
        );
    }

    #[test]
    fn rollback_lifecycle_refuses_when_replay_is_unavailable() {
        let bundle = bundle_with_entries(vec![entry(
            "rollback-post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        let mut input = policy_input();
        input.replay_enabled = true;

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::RollbackRestore,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::RollbackReplayUnavailable,
        );
    }

    #[test]
    fn target_compatibility_review_blocked_and_unknown_refuse_replay() {
        for (compatibility, expected) in [
            (
                TargetCompatibility::ReviewRequired,
                LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
            ),
            (
                TargetCompatibility::Blocked,
                LegacyReplayRefusalKind::TargetCompatibilityBlocked,
            ),
            (
                TargetCompatibility::Unknown("future".to_string()),
                LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
            ),
        ] {
            let mut bundle = bundle_with_entries(vec![entry(
                "post",
                LifecyclePath::PostInstall,
                ScriptletDecision::Legacy,
            )]);
            bundle.target_compatibility = compatibility;
            let mut input = policy_input();
            input.replay_enabled = true;

            assert_refused(
                plan_legacy_replay(
                    Some(&bundle),
                    LegacyReplayLifecycle::FreshInstallPost,
                    &input,
                )
                .expect("plan"),
                expected,
            );
        }
    }

    #[test]
    fn foreign_replay_policy_and_host_policy_fail_closed() {
        let mut bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
        bundle.foreign_replay_policy = ForeignReplayPolicy::Deny;
        let mut input = policy_input();
        input.replay_enabled = true;
        input.target = ReplayTarget {
            format: "rpm",
            distro: "centos",
            release: "10",
            arch: "x86_64",
        };
        input.foreign_replay_override = true;
        input.host_policy = HostForeignReplayPolicy::Permissive;

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::ForeignReplayDeniedByBundle,
        );

        bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;
        input.host_policy = HostForeignReplayPolicy::Strict;
        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::ForeignReplayDeniedByHostPolicy,
        );

        input.host_policy = HostForeignReplayPolicy::Guarded;
        input.foreign_replay_override = false;
        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::ForeignReplayOverrideRequired,
        );

        input.foreign_replay_override = true;
        assert_plan_entry_ids(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            &["post"],
        );

        input.host_policy = HostForeignReplayPolicy::Permissive;
        input.foreign_replay_override = false;
        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::ForeignReplayOverrideRequired,
        );

        input.foreign_replay_override = true;
        assert_plan_entry_ids(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            &["post"],
        );
    }

    #[test]
    fn foreign_replay_override_without_replay_enabled_is_insufficient() {
        let mut bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
        bundle.foreign_replay_policy = ForeignReplayPolicy::Permissive;
        let mut input = policy_input();
        input.foreign_replay_override = true;
        input.host_policy = HostForeignReplayPolicy::Permissive;
        input.target = ReplayTarget {
            format: "rpm",
            distro: "centos",
            release: "10",
            arch: "x86_64",
        };

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::LegacyReplayFeatureDisabled,
        );
    }

    #[test]
    fn guarded_host_requires_guarded_compatible_bundle_policy() {
        let mut bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
        bundle.foreign_replay_policy = ForeignReplayPolicy::Permissive;
        let mut input = policy_input();
        input.replay_enabled = true;
        input.foreign_replay_override = true;
        input.host_policy = HostForeignReplayPolicy::Guarded;
        input.target = ReplayTarget {
            format: "rpm",
            distro: "centos",
            release: "10",
            arch: "x86_64",
        };

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::ForeignReplayDeniedByHostPolicy,
        );

        bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;
        assert_plan_entry_ids(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            &["post"],
        );
    }

    #[test]
    fn unknown_foreign_replay_policy_fails_closed() {
        let mut bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
        bundle.foreign_replay_policy = ForeignReplayPolicy::Unknown("future".to_string());
        let mut input = policy_input();
        input.replay_enabled = true;
        input.foreign_replay_override = true;
        input.host_policy = HostForeignReplayPolicy::Permissive;
        input.target = ReplayTarget {
            format: "rpm",
            distro: "centos",
            release: "10",
            arch: "x86_64",
        };

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::ForeignReplayDeniedByBundle,
        );
    }

    #[test]
    fn replay_timeout_bounds_are_enforced() {
        for timeout_ms in [999, 300_001] {
            let mut legacy = entry(
                "post",
                LifecyclePath::PostInstall,
                ScriptletDecision::Legacy,
            );
            legacy.timeout_ms = timeout_ms;
            let bundle = bundle_with_entries(vec![legacy]);
            let mut input = policy_input();
            input.replay_enabled = true;

            assert_refused(
                plan_legacy_replay(
                    Some(&bundle),
                    LegacyReplayLifecycle::FreshInstallPost,
                    &input,
                )
                .expect("plan"),
                LegacyReplayRefusalKind::TimeoutOutOfRange,
            );
        }
    }

    #[test]
    fn pre_mutation_ordering_conflicts_refuse_mixed_raw_and_replaced_entries() {
        let mut raw = entry("raw", LifecyclePath::PreInstall, ScriptletDecision::Legacy);
        raw.transaction_order.after = vec!["replaced".to_string()];
        let replaced = entry(
            "replaced",
            LifecyclePath::PreInstall,
            ScriptletDecision::Replaced,
        );
        let bundle = bundle_with_entries(vec![raw, replaced]);
        let mut input = policy_input();
        input.replay_enabled = true;

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPre,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::UnsatisfiedTransactionOrder,
        );
    }

    fn assert_refused(preflight: LegacyReplayPreflight, expected: LegacyReplayRefusalKind) {
        let LegacyReplayPreflight::Refused(refusal) = preflight else {
            panic!("expected refusal");
        };
        assert_eq!(refusal.kind, expected);
    }

    fn assert_plan_entry_ids(preflight: LegacyReplayPreflight, expected: &[&str]) {
        let LegacyReplayPreflight::RequiresReplay(plan) = preflight else {
            panic!("expected replay plan");
        };
        let actual: Vec<&str> = plan
            .lifecycle_entries
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect();
        assert_eq!(actual, expected);
    }
}
