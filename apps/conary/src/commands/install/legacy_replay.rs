// apps/conary/src/commands/install/legacy_replay.rs
//! Install-side adapter for legacy scriptlet replay planning, execution, and audit metadata.

use super::CcsTransactionInstallOptions;
use super::scriptlets::scriptlet_warning_from_failure;
use anyhow::{Context, Result};
use conary_core::db::models::InstalledLegacyScriptletBundle;
use conary_core::scriptlet::{PackageFormat as ScriptletPackageFormat, SandboxMode};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LegacyReplayOptions {
    pub allow_legacy_replay: bool,
    pub allow_foreign_legacy_replay: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LegacyReplayInstallState {
    pub new_bundle_pre_plan: Option<conary_core::ccs::legacy_replay::LegacyReplayPlan>,
    pub new_bundle_post_plan: Option<conary_core::ccs::legacy_replay::LegacyReplayPlan>,
    pub old_bundle_pre_remove_plan: Option<conary_core::ccs::legacy_replay::LegacyReplayPlan>,
    pub old_bundle_post_remove_plan: Option<conary_core::ccs::legacy_replay::LegacyReplayPlan>,
    pub old_bundle_to_replay: Option<conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle>,
    pub accepted_bundle_to_persist: Option<AcceptedLegacyBundleInstall>,
    pub audit: Option<LegacyReplayAuditContext>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AcceptedLegacyBundleInstall {
    pub bundle: conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle,
    pub target_id: String,
    pub replay_policy: String,
    pub replay_enabled: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyReplayAuditContext {
    pub target_id: String,
    pub source_target_id: String,
    pub target_compatibility: String,
    pub foreign_replay_policy: String,
    pub host_policy: conary_core::ccs::legacy_replay::HostForeignReplayPolicy,
    pub feature_gate_enabled: bool,
    pub foreign_override: bool,
    pub evidence_digest: Option<String>,
    pub compatibility: crate::commands::LegacyReplayCompatibilityAudit,
}

const LEGACY_REPLAY_POLICY: &str = "goal6-safe-replay";

pub(in crate::commands) fn plan_ccs_fresh_install_legacy_replay(
    conn: &rusqlite::Connection,
    bundle: Option<&conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle>,
    opts: &CcsTransactionInstallOptions<'_>,
    is_upgrade: bool,
) -> Result<LegacyReplayInstallState> {
    use conary_core::ccs::legacy_replay::{LegacyReplayLifecycle, plan_legacy_replay};
    use conary_core::repository::distro::source_target_from_bundle;

    let Some(bundle) = bundle else {
        return Ok(LegacyReplayInstallState::default());
    };

    let host_context =
        crate::commands::legacy_replay_policy::resolve_legacy_replay_host_context(conn)?;
    let input = crate::commands::legacy_replay_policy::legacy_replay_policy_input(
        &host_context,
        crate::commands::legacy_replay_policy::LegacyReplayPolicyOptions {
            replay_enabled: opts.legacy_replay.allow_legacy_replay,
            foreign_replay_override: opts.legacy_replay.allow_foreign_legacy_replay,
            no_scripts: opts.no_scripts,
            requested_sandbox_mode: opts.sandbox_mode,
        },
    )?;

    let (pre_lifecycle, post_lifecycle) = if is_upgrade {
        (
            LegacyReplayLifecycle::UpgradeNewPre,
            LegacyReplayLifecycle::UpgradeNewPost,
        )
    } else {
        (
            LegacyReplayLifecycle::FreshInstallPre,
            LegacyReplayLifecycle::FreshInstallPost,
        )
    };

    let pre = plan_legacy_replay(Some(bundle), pre_lifecycle, &input)?;
    let post = plan_legacy_replay(Some(bundle), post_lifecycle, &input)?;

    let target_id = host_context.target.to_id();
    let source_target_id = source_target_from_bundle(bundle).to_id();
    let new_bundle_pre_plan = plan_from_preflight(pre)?;
    let new_bundle_post_plan = plan_from_preflight(post)?;
    let compatibility = compatibility_audit_from_plan(
        new_bundle_pre_plan
            .as_ref()
            .or(new_bundle_post_plan.as_ref()),
    );

    Ok(LegacyReplayInstallState {
        new_bundle_pre_plan,
        new_bundle_post_plan,
        accepted_bundle_to_persist: Some(AcceptedLegacyBundleInstall {
            bundle: bundle.clone(),
            target_id: target_id.clone(),
            replay_policy: LEGACY_REPLAY_POLICY.to_string(),
            replay_enabled: opts.legacy_replay.allow_legacy_replay,
        }),
        audit: Some(LegacyReplayAuditContext {
            target_id: target_id.clone(),
            source_target_id,
            target_compatibility: bundle.target_compatibility.as_str().to_string(),
            foreign_replay_policy: bundle.foreign_replay_policy.as_str().to_string(),
            host_policy: host_context.host_policy,
            feature_gate_enabled: opts.legacy_replay.allow_legacy_replay,
            foreign_override: opts.legacy_replay.allow_foreign_legacy_replay,
            evidence_digest: bundle.evidence_digest.clone(),
            compatibility,
        }),
        ..LegacyReplayInstallState::default()
    })
}

pub(in crate::commands) fn plan_ccs_old_installed_upgrade_legacy_replay(
    conn: &rusqlite::Connection,
    old_trove: Option<&conary_core::db::models::Trove>,
    opts: &CcsTransactionInstallOptions<'_>,
) -> Result<LegacyReplayInstallState> {
    use conary_core::ccs::legacy_replay::{LegacyReplayLifecycle, plan_legacy_replay};
    use conary_core::repository::distro::source_target_from_bundle;

    let Some(old_trove) = old_trove else {
        return Ok(LegacyReplayInstallState::default());
    };
    let Some(old_trove_id) = old_trove.id else {
        return Ok(LegacyReplayInstallState::default());
    };
    let Some(installed) = InstalledLegacyScriptletBundle::find_by_trove(conn, old_trove_id)? else {
        return Ok(LegacyReplayInstallState::default());
    };
    let bundle = installed
        .bundle()
        .context("installed legacy scriptlet bundle is malformed")?;

    let host_context =
        crate::commands::legacy_replay_policy::resolve_legacy_replay_host_context(conn)?;
    let input = crate::commands::legacy_replay_policy::legacy_replay_policy_input(
        &host_context,
        crate::commands::legacy_replay_policy::LegacyReplayPolicyOptions {
            replay_enabled: opts.legacy_replay.allow_legacy_replay,
            foreign_replay_override: opts.legacy_replay.allow_foreign_legacy_replay,
            no_scripts: opts.no_scripts,
            requested_sandbox_mode: opts.sandbox_mode,
        },
    )?;
    let pre = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::UpgradeOldPreRemove,
        &input,
    )?;
    let post = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::UpgradeOldPostRemove,
        &input,
    )?;
    let target_id = host_context.target.to_id();
    let source_target_id = source_target_from_bundle(&bundle).to_id();
    let old_bundle_pre_remove_plan = plan_from_preflight(pre)?;
    let old_bundle_post_remove_plan = plan_from_preflight(post)?;
    let compatibility = compatibility_audit_from_plan(
        old_bundle_pre_remove_plan
            .as_ref()
            .or(old_bundle_post_remove_plan.as_ref()),
    );

    Ok(LegacyReplayInstallState {
        old_bundle_pre_remove_plan,
        old_bundle_post_remove_plan,
        old_bundle_to_replay: Some(bundle.clone()),
        audit: Some(LegacyReplayAuditContext {
            target_id: target_id.clone(),
            source_target_id,
            target_compatibility: bundle.target_compatibility.as_str().to_string(),
            foreign_replay_policy: bundle.foreign_replay_policy.as_str().to_string(),
            host_policy: host_context.host_policy,
            feature_gate_enabled: opts.legacy_replay.allow_legacy_replay,
            foreign_override: opts.legacy_replay.allow_foreign_legacy_replay,
            evidence_digest: bundle.evidence_digest.clone(),
            compatibility,
        }),
        ..LegacyReplayInstallState::default()
    })
}

pub(in crate::commands) fn merge_old_upgrade_legacy_replay_state(
    state: &mut LegacyReplayInstallState,
    old_state: LegacyReplayInstallState,
) {
    state.old_bundle_pre_remove_plan = old_state.old_bundle_pre_remove_plan;
    state.old_bundle_post_remove_plan = old_state.old_bundle_post_remove_plan;
    state.old_bundle_to_replay = old_state.old_bundle_to_replay;
    if state.audit.is_none() {
        state.audit = old_state.audit;
    }
}

fn plan_from_preflight(
    preflight: conary_core::ccs::legacy_replay::LegacyReplayPreflight,
) -> Result<Option<conary_core::ccs::legacy_replay::LegacyReplayPlan>> {
    use conary_core::ccs::legacy_replay::LegacyReplayPreflight;

    match preflight {
        LegacyReplayPreflight::NativeFree => Ok(None),
        LegacyReplayPreflight::FullyReplaced(plan)
        | LegacyReplayPreflight::RequiresReplay(plan) => Ok(Some(plan)),
        LegacyReplayPreflight::Refused(refusal) => Err(legacy_replay_refusal_error(refusal)),
    }
}

fn compatibility_audit_from_plan(
    plan: Option<&conary_core::ccs::legacy_replay::LegacyReplayPlan>,
) -> crate::commands::LegacyReplayCompatibilityAudit {
    let Some(plan) = plan else {
        return crate::commands::LegacyReplayCompatibilityAudit::default();
    };
    let decision = &plan.compatibility_decision;
    crate::commands::LegacyReplayCompatibilityAudit {
        decision: decision.decision.clone(),
        reason_code: decision.reason_code.clone(),
        matrix_entry_id: decision.matrix_entry_id.clone(),
        matrix_digest: decision.matrix_digest.clone(),
        override_required: decision.override_required,
        override_used: decision.override_used,
        preflight_checks: decision
            .preflight_checks
            .iter()
            .map(|check| crate::commands::LegacyReplayPreflightCheckAudit {
                id: check.id.clone(),
                kind: check.kind.clone(),
                status: check.status.clone(),
                reason_code: check.reason_code.clone(),
            })
            .collect(),
    }
}

fn legacy_replay_refusal_error(
    refusal: conary_core::ccs::legacy_replay::LegacyReplayRefusal,
) -> anyhow::Error {
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

fn run_legacy_replay_plan_entries_with<F>(
    plan: Option<&conary_core::ccs::legacy_replay::LegacyReplayPlan>,
    bundle: Option<&conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle>,
    runtime: &conary_core::scriptlet::LegacyInvocationRuntime<'_>,
    mut execute: F,
) -> Result<Vec<conary_core::scriptlet::ScriptletOutcome>>
where
    F: FnMut(
        &conary_core::scriptlet::LegacyScriptletExecution<'_>,
        &conary_core::scriptlet::LegacyInvocationRuntime<'_>,
    ) -> conary_core::scriptlet::ScriptletOutcome,
{
    let Some(plan) = plan else {
        return Ok(Vec::new());
    };
    if plan.lifecycle_entries.is_empty() {
        return Ok(Vec::new());
    }
    let bundle = bundle.context("legacy replay plan exists without a legacy scriptlet bundle")?;

    let mut outcomes = Vec::with_capacity(plan.lifecycle_entries.len());
    for planned in &plan.lifecycle_entries {
        let entry = bundle
            .entries
            .iter()
            .find(|entry| entry.id == planned.entry_id)
            .with_context(|| {
                format!(
                    "legacy replay plan references missing bundle entry {}",
                    planned.entry_id
                )
            })?;
        let execution = conary_core::scriptlet::LegacyScriptletExecution {
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
        outcomes.push(execute(&execution, runtime));
    }

    Ok(outcomes)
}

pub(super) struct LegacyReplayExecutionScope<'a> {
    pub(super) root: &'a Path,
    pub(super) package_name: &'a str,
    pub(super) package_version: &'a str,
    pub(super) mode: &'a conary_core::scriptlet::ExecutionMode,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) old_version: Option<&'a str>,
    pub(super) new_version: Option<&'a str>,
}

pub(super) fn execute_legacy_replay_plan_entries(
    scope: LegacyReplayExecutionScope<'_>,
    bundle: Option<&conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle>,
    plan: Option<&conary_core::ccs::legacy_replay::LegacyReplayPlan>,
) -> Result<Vec<conary_core::scriptlet::ScriptletOutcome>> {
    let Some(bundle) = bundle else {
        return run_legacy_replay_plan_entries_with(
            plan,
            None,
            &conary_core::scriptlet::LegacyInvocationRuntime {
                mode: scope.mode,
                old_version: scope.old_version,
                new_version: scope.new_version.or(Some(scope.package_version)),
                package_instance_count: Some(1),
            },
            |_execution, _runtime| unreachable!("empty bundle cannot execute legacy replay"),
        );
    };

    let format = legacy_source_scriptlet_format(&bundle.source_format)?;
    let executor = conary_core::scriptlet::ScriptletExecutor::new(
        scope.root,
        scope.package_name,
        scope.package_version,
        format,
    )
    .with_sandbox_mode(scope.sandbox_mode);
    let runtime = conary_core::scriptlet::LegacyInvocationRuntime {
        mode: scope.mode,
        old_version: scope.old_version,
        new_version: scope.new_version.or(Some(scope.package_version)),
        package_instance_count: Some(1),
    };

    run_legacy_replay_plan_entries_with(plan, Some(bundle), &runtime, |execution, runtime| {
        executor.execute_legacy_entry_with_outcome(execution, runtime)
    })
}

pub(super) fn require_legacy_replay_success(
    outcomes: &[conary_core::scriptlet::ScriptletOutcome],
) -> Result<()> {
    for outcome in outcomes {
        outcome.clone().into_result()?;
    }
    Ok(())
}

pub(super) fn legacy_post_replay_warnings(
    package_name: &str,
    outcomes: &[conary_core::scriptlet::ScriptletOutcome],
) -> Result<Vec<crate::commands::ScriptletWarning>> {
    let mut warnings = Vec::new();

    for outcome in outcomes {
        match outcome {
            conary_core::scriptlet::ScriptletOutcome::Success { .. }
            | conary_core::scriptlet::ScriptletOutcome::Skipped { .. } => {}
            conary_core::scriptlet::ScriptletOutcome::Failure(failure)
                if failure.failure_kind
                    == conary_core::scriptlet::ScriptletFailureKind::ScriptExited =>
            {
                warnings.push(scriptlet_warning_from_failure(
                    package_name,
                    failure.clone(),
                    "legacy post-install scriptlet failed after package files were installed",
                ));
            }
            conary_core::scriptlet::ScriptletOutcome::Failure(failure) => {
                return Err(anyhow::anyhow!(
                    "legacy post-install scriptlet failed after commit with a non-degradable failure: {}",
                    failure.message
                ));
            }
        }
    }

    Ok(warnings)
}

pub(super) fn build_legacy_replay_audit_for_install(
    state: &LegacyReplayInstallState,
    old_pre_outcomes: &[conary_core::scriptlet::ScriptletOutcome],
    new_pre_outcomes: &[conary_core::scriptlet::ScriptletOutcome],
    old_post_outcomes: &[conary_core::scriptlet::ScriptletOutcome],
    new_post_outcomes: &[conary_core::scriptlet::ScriptletOutcome],
) -> Option<crate::commands::LegacyReplayAudit> {
    let context = state.audit.as_ref()?;
    let mut planned_entries = Vec::new();
    append_legacy_replay_plan_audit_entries(
        &mut planned_entries,
        state.old_bundle_pre_remove_plan.as_ref(),
        old_pre_outcomes,
    );
    append_legacy_replay_plan_audit_entries(
        &mut planned_entries,
        state.new_bundle_pre_plan.as_ref(),
        new_pre_outcomes,
    );
    append_legacy_replay_plan_audit_entries(
        &mut planned_entries,
        state.old_bundle_post_remove_plan.as_ref(),
        old_post_outcomes,
    );
    append_legacy_replay_plan_audit_entries(
        &mut planned_entries,
        state.new_bundle_post_plan.as_ref(),
        new_post_outcomes,
    );

    Some(crate::commands::LegacyReplayAudit {
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

fn append_legacy_replay_plan_audit_entries(
    entries: &mut Vec<crate::commands::LegacyReplayPlannedEntryAudit>,
    plan: Option<&conary_core::ccs::legacy_replay::LegacyReplayPlan>,
    outcomes: &[conary_core::scriptlet::ScriptletOutcome],
) {
    let Some(plan) = plan else {
        return;
    };

    for (index, entry) in plan.lifecycle_entries.iter().enumerate() {
        entries.push(crate::commands::LegacyReplayPlannedEntryAudit {
            entry_id: entry.entry_id.clone(),
            native_slot: entry.native_slot.clone(),
            phase: legacy_lifecycle_phase_name(&entry.phase).to_string(),
            timeout_ms: entry.timeout_ms,
            raw_replay_required: plan.raw_replay_required,
            outcome: outcomes.get(index).map(legacy_replay_outcome_audit),
        });
    }
}

fn legacy_replay_outcome_audit(
    outcome: &conary_core::scriptlet::ScriptletOutcome,
) -> crate::commands::LegacyReplayOutcomeAudit {
    match outcome {
        conary_core::scriptlet::ScriptletOutcome::Skipped {
            phase,
            requested_sandbox_mode,
            effective_sandbox,
        } => crate::commands::LegacyReplayOutcomeAudit {
            status: "skipped".to_string(),
            phase: phase.clone(),
            requested_sandbox_mode: requested_sandbox_mode.as_str().to_string(),
            effective_sandbox: effective_sandbox.as_str().to_string(),
            failure_kind: None,
            message: None,
        },
        conary_core::scriptlet::ScriptletOutcome::Success {
            phase,
            requested_sandbox_mode,
            effective_sandbox,
        } => crate::commands::LegacyReplayOutcomeAudit {
            status: "success".to_string(),
            phase: phase.clone(),
            requested_sandbox_mode: requested_sandbox_mode.as_str().to_string(),
            effective_sandbox: effective_sandbox.as_str().to_string(),
            failure_kind: None,
            message: None,
        },
        conary_core::scriptlet::ScriptletOutcome::Failure(failure) => {
            crate::commands::LegacyReplayOutcomeAudit {
                status: "failure".to_string(),
                phase: failure.phase.clone(),
                requested_sandbox_mode: failure.requested_sandbox_mode.as_str().to_string(),
                effective_sandbox: failure.effective_sandbox.as_str().to_string(),
                failure_kind: Some(failure.failure_kind.as_str().to_string()),
                message: Some(failure.message.clone()),
            }
        }
    }
}

fn host_foreign_replay_policy_name(
    policy: conary_core::ccs::legacy_replay::HostForeignReplayPolicy,
) -> &'static str {
    match policy {
        conary_core::ccs::legacy_replay::HostForeignReplayPolicy::Strict => "strict",
        conary_core::ccs::legacy_replay::HostForeignReplayPolicy::Guarded => "guarded",
        conary_core::ccs::legacy_replay::HostForeignReplayPolicy::Permissive => "permissive",
    }
}

fn legacy_source_scriptlet_format(
    source_format: &conary_core::ccs::legacy_scriptlets::SourceFormat,
) -> Result<ScriptletPackageFormat> {
    match source_format {
        conary_core::ccs::legacy_scriptlets::SourceFormat::Rpm => Ok(ScriptletPackageFormat::Rpm),
        conary_core::ccs::legacy_scriptlets::SourceFormat::Deb => Ok(ScriptletPackageFormat::Deb),
        conary_core::ccs::legacy_scriptlets::SourceFormat::Arch => Ok(ScriptletPackageFormat::Arch),
        conary_core::ccs::legacy_scriptlets::SourceFormat::Unknown(value) => {
            anyhow::bail!("legacy replay source format is unknown: {value}")
        }
    }
}

fn legacy_lifecycle_phase_name(
    phase: &conary_core::ccs::legacy_scriptlets::LifecyclePath,
) -> &'static str {
    use conary_core::ccs::legacy_scriptlets::LifecyclePath;

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

#[cfg(test)]
mod tests {
    use super::super::conversion;
    use super::super::{CcsTransactionInstallOptions, ComponentSelection, InstallOptions};
    use super::*;
    use conary_core::ccs::legacy_replay::{
        HostForeignReplayPolicy, LegacyReplayPlan, PlannedLegacyEntry,
    };
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
        LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
        PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
        TransactionOrder, VersionScheme,
    };
    use conary_core::scriptlet::{
        EffectiveSandbox, ExecutionMode, LegacyInvocationRuntime, SandboxMode, ScriptletOutcome,
    };
    use std::collections::BTreeMap;

    #[test]
    fn legacy_replay_options_default_disabled_for_install_surfaces() {
        let default_replay = LegacyReplayOptions::default();
        assert!(!default_replay.allow_legacy_replay);
        assert!(!default_replay.allow_foreign_legacy_replay);

        let install_opts = InstallOptions::default();
        assert_eq!(install_opts.legacy_replay, default_replay);

        let transaction_opts = CcsTransactionInstallOptions {
            db_path: "/tmp/conary.db",
            root: "/",
            dry_run: true,
            defer_generation: false,
            quiet: false,
            no_scripts: false,
            sandbox_mode: SandboxMode::None,
            allow_downgrade: false,
            reinstall: false,
            selection_reason: None,
            component_selection: ComponentSelection::Defaults,
            selected_manifest_components: None,
            repository_provenance: None,
            legacy_replay: default_replay,
        };
        assert_eq!(transaction_opts.legacy_replay, default_replay);

        let converted_opts = conversion::ConvertedCcsInstallOptions {
            ccs_path: "/tmp/pkg.ccs",
            db_path: "/tmp/conary.db",
            root: "/",
            dry_run: true,
            sandbox_mode: SandboxMode::None,
            no_deps: true,
            no_scripts: false,
            allow_downgrade: false,
            dep_mode: None,
            yes: true,
            dependency_passes_remaining: 0,
            repository_provenance: None,
            legacy_replay: default_replay,
        };
        assert_eq!(converted_opts.legacy_replay, default_replay);
    }

    #[test]
    fn legacy_replay_install_state_defaults_to_empty_carriers() {
        let state = LegacyReplayInstallState::default();

        assert!(state.new_bundle_pre_plan.is_none());
        assert!(state.new_bundle_post_plan.is_none());
        assert!(state.old_bundle_pre_remove_plan.is_none());
        assert!(state.old_bundle_post_remove_plan.is_none());
        assert!(state.accepted_bundle_to_persist.is_none());
        assert!(state.audit.is_none());
    }

    #[test]
    fn legacy_replay_plan_runner_invokes_selected_legacy_entry_once() {
        let bundle = test_legacy_bundle(vec![test_legacy_entry(
            "rpm:%post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
            "echo legacy-post\n",
        )]);
        let plan = test_legacy_plan(vec![("rpm:%post", "%post", LifecyclePath::PostInstall)]);
        let mode = ExecutionMode::Install;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(1),
        };
        let mut calls = Vec::new();

        let outcomes = run_legacy_replay_plan_entries_with(
            Some(&plan),
            Some(&bundle),
            &runtime,
            |execution, runtime| {
                calls.push((
                    execution.entry_id.to_string(),
                    execution.phase.to_string(),
                    execution.body.clone(),
                    matches!(runtime.mode, ExecutionMode::Install),
                ));
                ScriptletOutcome::Success {
                    phase: execution.phase.to_string(),
                    requested_sandbox_mode: SandboxMode::None,
                    effective_sandbox: EffectiveSandbox::TargetRoot,
                }
            },
        )
        .expect("run legacy replay plan");

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            calls,
            vec![(
                "rpm:%post".to_string(),
                "post-install".to_string(),
                "echo legacy-post\n".to_string(),
                true,
            )]
        );
    }

    #[test]
    fn legacy_replay_plan_runner_skips_fully_replaced_plan_entries() {
        let bundle = test_legacy_bundle(vec![test_legacy_entry(
            "rpm:%post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Replaced,
            "echo replaced\n",
        )]);
        let plan = test_legacy_plan(Vec::new());
        let mode = ExecutionMode::Install;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(1),
        };
        let mut calls = 0;

        let outcomes = run_legacy_replay_plan_entries_with(
            Some(&plan),
            Some(&bundle),
            &runtime,
            |_execution, _runtime| {
                calls += 1;
                ScriptletOutcome::Success {
                    phase: "post-install".to_string(),
                    requested_sandbox_mode: SandboxMode::None,
                    effective_sandbox: EffectiveSandbox::TargetRoot,
                }
            },
        )
        .expect("run legacy replay plan");

        assert!(outcomes.is_empty());
        assert_eq!(calls, 0);
    }

    #[test]
    fn legacy_replay_audit_records_planned_entry_outcome() {
        let state = LegacyReplayInstallState {
            new_bundle_post_plan: Some(test_legacy_plan(vec![(
                "rpm:%post",
                "%post",
                LifecyclePath::PostInstall,
            )])),
            audit: Some(LegacyReplayAuditContext {
                target_id: "rpm/fedora/44/x86_64".to_string(),
                source_target_id: "rpm/fedora/44/x86_64".to_string(),
                target_compatibility: "source-native".to_string(),
                foreign_replay_policy: "deny".to_string(),
                host_policy: HostForeignReplayPolicy::Strict,
                feature_gate_enabled: true,
                foreign_override: false,
                evidence_digest: Some(conary_core::hash::sha256_prefixed(b"bundle-evidence")),
                compatibility: crate::commands::LegacyReplayCompatibilityAudit::default(),
            }),
            ..LegacyReplayInstallState::default()
        };
        let post_outcomes = vec![ScriptletOutcome::Success {
            phase: "post-install".to_string(),
            requested_sandbox_mode: SandboxMode::None,
            effective_sandbox: EffectiveSandbox::Direct,
        }];

        let audit = build_legacy_replay_audit_for_install(&state, &[], &[], &[], &post_outcomes)
            .expect("legacy replay audit");

        assert!(audit.bundle_present);
        assert_eq!(audit.target_id, "rpm/fedora/44/x86_64");
        assert_eq!(audit.feature_gate, "enabled");
        assert_eq!(audit.planned_entries.len(), 1);
        let entry = &audit.planned_entries[0];
        assert_eq!(entry.entry_id, "rpm:%post");
        assert_eq!(entry.native_slot, "%post");
        assert_eq!(entry.phase, "post-install");
        assert_eq!(entry.timeout_ms, 30_000);
        assert!(entry.raw_replay_required);
        let outcome = entry.outcome.as_ref().expect("entry outcome");
        assert_eq!(outcome.status, "success");
        assert_eq!(outcome.phase, "post-install");
        assert_eq!(outcome.requested_sandbox_mode, "never");
        assert_eq!(outcome.effective_sandbox, "direct");
    }

    fn test_legacy_plan(entries: Vec<(&str, &str, LifecyclePath)>) -> LegacyReplayPlan {
        LegacyReplayPlan {
            target_id: "rpm/fedora/44/x86_64".to_string(),
            source_target_id: "rpm/fedora/44/x86_64".to_string(),
            bundle_evidence_digest: Some(conary_core::hash::sha256_prefixed(b"bundle-evidence")),
            lifecycle_entries: entries
                .into_iter()
                .map(|(entry_id, native_slot, phase)| PlannedLegacyEntry {
                    entry_id: entry_id.to_string(),
                    native_slot: native_slot.to_string(),
                    phase,
                    timeout_ms: 30_000,
                })
                .collect(),
            sandbox_floor: SandboxMode::None,
            ccs_hooks_allowed: true,
            raw_replay_required: true,
            compatibility_decision: accepted_compatibility_decision(),
        }
    }

    fn accepted_compatibility_decision()
    -> conary_core::ccs::legacy_replay::LegacyReplayCompatibilityDecision {
        conary_core::ccs::legacy_replay::LegacyReplayCompatibilityDecision {
            decision: "accepted".to_string(),
            reason_code: "compatibility-source-native".to_string(),
            matrix_entry_id: None,
            matrix_digest: None,
            preflight_checks: Vec::new(),
            override_required: false,
            override_used: false,
        }
    }

    fn test_legacy_bundle(entries: Vec<LegacyScriptletEntry>) -> LegacyScriptletBundle {
        let mut decision_counts = DecisionCounts::default();
        for entry in &entries {
            match &entry.decision {
                ScriptletDecision::Replaced => decision_counts.replaced += 1,
                ScriptletDecision::Legacy => decision_counts.legacy += 1,
                ScriptletDecision::Blocked => decision_counts.blocked += 1,
                ScriptletDecision::Review => decision_counts.review += 1,
                ScriptletDecision::Unknown(value) => {
                    *decision_counts.extra.entry(value.clone()).or_insert(0) += 1;
                }
            }
        }

        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "legacy-runner-fixture".to_string(),
            source_version: "1.0.0-1.fc44".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "test".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "goal6-test".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(conary_core::hash::sha256_prefixed(b"bundle-evidence")),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::PublicIfNoBlocked,
            publication_status: PublicationStatus::Public,
            scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
            decision_counts,
            unsupported_class_counts: BTreeMap::new(),
            entries,
            extra: BTreeMap::new(),
        }
    }

    fn test_legacy_entry(
        id: &str,
        phase: LifecyclePath,
        decision: ScriptletDecision,
        body: &str,
    ) -> LegacyScriptletEntry {
        LegacyScriptletEntry {
            id: id.to_string(),
            native_slot: id.split(':').nth(1).unwrap_or("%post").to_string(),
            phase,
            lifecycle_paths: vec!["install:post".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: vec!["-e".to_string()],
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation {
                args: vec!["1".to_string()],
                environment: vec![],
                stdin: None,
                chroot: None,
                extra: BTreeMap::new(),
            },
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                before: vec![],
                after: vec![],
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: vec![],
            decision,
            reason_code: "legacy-replay-required".to_string(),
            human_reason: Some("test fixture".to_string()),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                format!("{id}:{body}").as_bytes(),
            )),
            source_evidence_refs: vec![format!("capture:{id}")],
            effects: vec![],
            unknown_commands: vec![],
            blocked_classes: vec![],
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }
}
