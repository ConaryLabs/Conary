// apps/conary/src/commands/remove/legacy_replay.rs

use std::path::Path;

use anyhow::{Context, Result};
use conary_core::ccs::legacy_replay::{
    HostForeignReplayPolicy, LegacyReplayLifecycle, LegacyReplayPlan, LegacyReplayPreflight,
    LegacyReplayRefusal, plan_legacy_replay,
};
use conary_core::ccs::legacy_scriptlets::{LegacyScriptletBundle, LifecyclePath, SourceFormat};
use conary_core::db::models::InstalledLegacyScriptletBundle;
use conary_core::repository::distro::source_target_from_bundle;
use conary_core::scriptlet::{
    ExecutionMode, LegacyInvocationRuntime, LegacyScriptletExecution,
    PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletExecutor, ScriptletOutcome,
};

use super::types::{LegacyRemoveReplayAuditContext, RemoveInnerResult, RemoveScriptletOptions};
use crate::commands::{
    LegacyReplayAudit, LegacyReplayCompatibilityAudit, LegacyReplayOutcomeAudit,
    LegacyReplayPlannedEntryAudit, LegacyReplayPreflightCheckAudit,
};

#[derive(Debug, Default)]
pub(super) struct PreparedLegacyRemoveReplay {
    pub(super) bundle: Option<LegacyScriptletBundle>,
    pub(super) planned_pre_remove: Option<LegacyReplayPlan>,
    pub(super) planned_post_remove: Option<LegacyReplayPlan>,
    pub(super) audit_context: Option<LegacyRemoveReplayAuditContext>,
}

pub(super) fn load_installed_legacy_remove_plan(
    conn: &rusqlite::Connection,
    trove_id: i64,
    scriptlet_options: RemoveScriptletOptions,
) -> Result<PreparedLegacyRemoveReplay> {
    let Some(installed) = InstalledLegacyScriptletBundle::find_by_trove(conn, trove_id)? else {
        return Ok(PreparedLegacyRemoveReplay::default());
    };
    let bundle = installed
        .bundle()
        .context("installed legacy scriptlet bundle is malformed")?;
    plan_installed_legacy_remove_replay(conn, &bundle, scriptlet_options)
}

fn plan_installed_legacy_remove_replay(
    conn: &rusqlite::Connection,
    bundle: &LegacyScriptletBundle,
    scriptlet_options: RemoveScriptletOptions,
) -> Result<PreparedLegacyRemoveReplay> {
    let host_context =
        crate::commands::legacy_replay_policy::resolve_legacy_replay_host_context(conn)?;
    let input = crate::commands::legacy_replay_policy::legacy_replay_policy_input(
        &host_context,
        crate::commands::legacy_replay_policy::LegacyReplayPolicyOptions {
            replay_enabled: scriptlet_options.legacy_replay.allow_legacy_replay,
            foreign_replay_override: scriptlet_options.legacy_replay.allow_foreign_legacy_replay,
            no_scripts: scriptlet_options.no_scripts,
            requested_sandbox_mode: scriptlet_options.sandbox_mode,
        },
    )?;
    let pre = plan_legacy_replay(Some(bundle), LegacyReplayLifecycle::RemovePre, &input)?;
    let post = plan_legacy_replay(Some(bundle), LegacyReplayLifecycle::RemovePost, &input)?;
    let target_id = host_context.target.to_id();
    let source_target_id = source_target_from_bundle(bundle).to_id();
    let planned_pre_remove = remove_plan_from_preflight(pre)?;
    let planned_post_remove = remove_plan_from_preflight(post)?;
    let compatibility =
        compatibility_audit_from_plan(planned_pre_remove.as_ref().or(planned_post_remove.as_ref()));

    Ok(PreparedLegacyRemoveReplay {
        bundle: Some(bundle.clone()),
        planned_pre_remove,
        planned_post_remove,
        audit_context: Some(LegacyRemoveReplayAuditContext {
            target_id: target_id.clone(),
            source_target_id,
            target_compatibility: bundle.target_compatibility.as_str().to_string(),
            foreign_replay_policy: bundle.foreign_replay_policy.as_str().to_string(),
            host_policy: host_context.host_policy,
            feature_gate_enabled: scriptlet_options.legacy_replay.allow_legacy_replay,
            foreign_override: scriptlet_options.legacy_replay.allow_foreign_legacy_replay,
            evidence_digest: bundle.evidence_digest.clone(),
            compatibility,
        }),
    })
}

fn compatibility_audit_from_plan(
    plan: Option<&LegacyReplayPlan>,
) -> LegacyReplayCompatibilityAudit {
    let Some(plan) = plan else {
        return LegacyReplayCompatibilityAudit::default();
    };
    let decision = &plan.compatibility_decision;
    LegacyReplayCompatibilityAudit {
        decision: decision.decision.clone(),
        reason_code: decision.reason_code.clone(),
        matrix_entry_id: decision.matrix_entry_id.clone(),
        matrix_digest: decision.matrix_digest.clone(),
        override_required: decision.override_required,
        override_used: decision.override_used,
        preflight_checks: decision
            .preflight_checks
            .iter()
            .map(|check| LegacyReplayPreflightCheckAudit {
                id: check.id.clone(),
                kind: check.kind.clone(),
                status: check.status.clone(),
                reason_code: check.reason_code.clone(),
            })
            .collect(),
    }
}

fn remove_plan_from_preflight(
    preflight: LegacyReplayPreflight,
) -> Result<Option<LegacyReplayPlan>> {
    match preflight {
        LegacyReplayPreflight::NativeFree => Ok(None),
        LegacyReplayPreflight::FullyReplaced(plan)
        | LegacyReplayPreflight::RequiresReplay(plan) => Ok(Some(plan)),
        LegacyReplayPreflight::Refused(refusal) => Err(legacy_replay_refusal_error(refusal)),
    }
}

fn legacy_replay_refusal_error(refusal: LegacyReplayRefusal) -> anyhow::Error {
    let entry = refusal
        .entry_id
        .as_deref()
        .map(|entry_id| format!(" entry={entry_id}"))
        .unwrap_or_default();
    anyhow::anyhow!(
        "legacy scriptlet replay refused ({:?}{entry}): {}",
        refusal.kind,
        refusal.message
    )
}

pub(super) fn execute_legacy_remove_replay_plan_entries(
    root: &Path,
    package_name: &str,
    package_version: &str,
    bundle: Option<&LegacyScriptletBundle>,
    plan: Option<&LegacyReplayPlan>,
    sandbox_mode: SandboxMode,
) -> Result<Vec<ScriptletOutcome>> {
    let Some(plan) = plan else {
        return Ok(Vec::new());
    };
    if plan.lifecycle_entries.is_empty() {
        return Ok(Vec::new());
    }
    let bundle = bundle.context("legacy remove replay plan exists without an installed bundle")?;
    let format = legacy_source_scriptlet_format(&bundle.source_format)?;
    let executor = ScriptletExecutor::new(root, package_name, package_version, format)
        .with_sandbox_mode(sandbox_mode);
    let mode = ExecutionMode::Remove;
    let runtime = LegacyInvocationRuntime {
        mode: &mode,
        old_version: Some(package_version),
        new_version: None,
        package_instance_count: Some(0),
    };

    let mut outcomes = Vec::with_capacity(plan.lifecycle_entries.len());
    for planned in &plan.lifecycle_entries {
        let entry = bundle
            .entries
            .iter()
            .find(|entry| entry.id == planned.entry_id)
            .with_context(|| {
                format!(
                    "legacy remove replay plan references missing bundle entry {}",
                    planned.entry_id
                )
            })?;
        let execution = LegacyScriptletExecution {
            entry_id: &entry.id,
            phase: legacy_lifecycle_phase_name(&entry.phase),
            interpreter: &entry.interpreter,
            interpreter_args: &entry.interpreter_args,
            body: entry.body.clone(),
            body_sha256: entry.body_sha256.clone(),
            body_encoding: entry.body_encoding.as_deref(),
            native_args: &entry.native_invocation.args,
            native_environment: &entry.native_invocation.environment,
            stdin_contract: entry.native_invocation.stdin.as_deref(),
            chroot_contract: entry.native_invocation.chroot.as_deref(),
            timeout_ms: entry.timeout_ms,
        };
        outcomes.push(executor.execute_legacy_entry_with_outcome(&execution, &runtime));
    }

    Ok(outcomes)
}

pub(super) fn require_legacy_replay_success(outcomes: &[ScriptletOutcome]) -> Result<()> {
    for outcome in outcomes {
        outcome.clone().into_result()?;
    }
    Ok(())
}

pub(super) fn build_legacy_replay_audit_for_remove(
    remove_result: &RemoveInnerResult,
    pre_outcomes: &[ScriptletOutcome],
    post_outcomes: &[ScriptletOutcome],
) -> Option<LegacyReplayAudit> {
    let context = remove_result.legacy_audit_context.as_ref()?;
    let mut planned_entries = Vec::new();
    planned_entries.extend(legacy_replay_planned_entries_for_audit(
        remove_result.planned_pre_remove.as_ref(),
        pre_outcomes,
    ));
    planned_entries.extend(legacy_replay_planned_entries_for_audit(
        remove_result.planned_post_remove.as_ref(),
        post_outcomes,
    ));

    Some(LegacyReplayAudit {
        bundle_present: true,
        target_id: context.target_id.clone(),
        source_target_id: context.source_target_id.clone(),
        target_compatibility: context.target_compatibility.clone(),
        foreign_replay_policy: context.foreign_replay_policy.clone(),
        host_policy: host_foreign_replay_policy_name(context.host_policy).to_string(),
        feature_gate: if context.feature_gate_enabled {
            "enabled".to_string()
        } else {
            "disabled".to_string()
        },
        foreign_override: context.foreign_override,
        evidence_digest: context.evidence_digest.clone(),
        compatibility: context.compatibility.clone(),
        planned_entries,
    })
}

fn legacy_replay_planned_entries_for_audit(
    plan: Option<&LegacyReplayPlan>,
    outcomes: &[ScriptletOutcome],
) -> Vec<LegacyReplayPlannedEntryAudit> {
    let Some(plan) = plan else {
        return Vec::new();
    };

    plan.lifecycle_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| LegacyReplayPlannedEntryAudit {
            entry_id: entry.entry_id.clone(),
            native_slot: entry.native_slot.clone(),
            phase: legacy_lifecycle_phase_name(&entry.phase).to_string(),
            timeout_ms: entry.timeout_ms,
            raw_replay_required: plan.raw_replay_required,
            outcome: outcomes.get(index).map(legacy_replay_outcome_audit),
        })
        .collect()
}

fn legacy_replay_outcome_audit(outcome: &ScriptletOutcome) -> LegacyReplayOutcomeAudit {
    match outcome {
        ScriptletOutcome::Skipped {
            phase,
            requested_sandbox_mode,
            effective_sandbox,
        } => LegacyReplayOutcomeAudit {
            status: "skipped".to_string(),
            phase: phase.clone(),
            requested_sandbox_mode: requested_sandbox_mode.as_str().to_string(),
            effective_sandbox: effective_sandbox.as_str().to_string(),
            failure_kind: None,
            message: None,
        },
        ScriptletOutcome::Success {
            phase,
            requested_sandbox_mode,
            effective_sandbox,
        } => LegacyReplayOutcomeAudit {
            status: "success".to_string(),
            phase: phase.clone(),
            requested_sandbox_mode: requested_sandbox_mode.as_str().to_string(),
            effective_sandbox: effective_sandbox.as_str().to_string(),
            failure_kind: None,
            message: None,
        },
        ScriptletOutcome::Failure(failure) => LegacyReplayOutcomeAudit {
            status: "failure".to_string(),
            phase: failure.phase.clone(),
            requested_sandbox_mode: failure.requested_sandbox_mode.as_str().to_string(),
            effective_sandbox: failure.effective_sandbox.as_str().to_string(),
            failure_kind: Some(failure.failure_kind.as_str().to_string()),
            message: Some(failure.message.clone()),
        },
    }
}

fn legacy_source_scriptlet_format(source_format: &SourceFormat) -> Result<ScriptletPackageFormat> {
    match source_format {
        SourceFormat::Rpm => Ok(ScriptletPackageFormat::Rpm),
        SourceFormat::Deb => Ok(ScriptletPackageFormat::Deb),
        SourceFormat::Arch => Ok(ScriptletPackageFormat::Arch),
        SourceFormat::Unknown(value) => {
            anyhow::bail!("legacy replay source format is unknown: {value}")
        }
    }
}

fn legacy_lifecycle_phase_name(phase: &LifecyclePath) -> &'static str {
    match phase {
        LifecyclePath::PreInstall => "pre-install",
        LifecyclePath::PostInstall => "post-install",
        LifecyclePath::PreUpgrade => "pre-upgrade",
        LifecyclePath::PostUpgrade => "post-upgrade",
        LifecyclePath::PreRemove => "pre-remove",
        LifecyclePath::PostRemove => "post-remove",
        LifecyclePath::PreTransaction => "pre-transaction",
        LifecyclePath::PostTransaction => "post-transaction",
        LifecyclePath::Trigger => "trigger",
        LifecyclePath::FileTrigger => "file-trigger",
        LifecyclePath::Unknown(_) => "unknown",
    }
}

fn host_foreign_replay_policy_name(policy: HostForeignReplayPolicy) -> &'static str {
    match policy {
        HostForeignReplayPolicy::Strict => "strict",
        HostForeignReplayPolicy::Guarded => "guarded",
        HostForeignReplayPolicy::Permissive => "permissive",
    }
}
