// conary-core/src/ccs/native_transaction/arch.rs

//! Exact libalpm install-script and transaction-hook planning.

use super::{
    NativeBundleRole, NativeBundleView, NativeEventPlacement, NativeEventProgram, NativeEventStage,
    NativeInstalledCapability, NativeTransactionChange, NativeTransactionEvent,
    NativeTransactionOperation, NativeTransactionState, lines_stdin, required_change_version,
};
use crate::ccs::native_lifecycle::{
    ArchHookOperation, ArchHookTriggerType, ArchHookWhen, LifecyclePath, NativeLifecycleEntry,
};
use crate::packages::arch::{alpm_hook_targets_match, package_hook_basename};
use crate::repository::package_relation::parse_arch_atom;
use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, parse_repo_constraint, repo_version_satisfies,
};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn regular_invocation(
    view: &NativeBundleView<'_>,
    change: &NativeTransactionChange,
    entry: &NativeLifecycleEntry,
) -> Result<Option<(NativeEventStage, Vec<String>)>> {
    if entry.arch_install.is_none() {
        return Ok(None);
    }
    let new_version = || required_change_version(change.new_version.as_deref(), "new", change);
    let old_version = || required_change_version(change.old_version.as_deref(), "old", change);
    let invocation = match (view.role, &entry.phase, change.operation) {
        (
            NativeBundleRole::Installing,
            LifecyclePath::PreInstall,
            NativeTransactionOperation::Install,
        ) => (NativeEventStage::PackagePreInstall, vec![new_version()?]),
        (
            NativeBundleRole::Installing,
            LifecyclePath::PostInstall,
            NativeTransactionOperation::Install,
        ) => (NativeEventStage::PackagePostInstall, vec![new_version()?]),
        (
            NativeBundleRole::Installing,
            LifecyclePath::PreUpgrade,
            NativeTransactionOperation::Upgrade,
        ) => (
            NativeEventStage::PackagePreInstall,
            vec![new_version()?, old_version()?],
        ),
        (
            NativeBundleRole::Installing,
            LifecyclePath::PostUpgrade,
            NativeTransactionOperation::Upgrade,
        ) => (
            NativeEventStage::PackagePostInstall,
            vec![new_version()?, old_version()?],
        ),
        (NativeBundleRole::Removing, LifecyclePath::PreRemove, operation)
            if operation.removes_package() =>
        {
            (NativeEventStage::PackagePreRemove, vec![old_version()?])
        }
        (NativeBundleRole::Removing, LifecyclePath::PostRemove, operation)
            if operation.removes_package() =>
        {
            (NativeEventStage::PackagePostRemove, vec![old_version()?])
        }
        _ => return Ok(None),
    };
    Ok(Some(invocation))
}

pub(super) fn plan(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    plan_hooks_for_snapshot(
        ArchHookWhen::PreTransaction,
        bundles,
        changes,
        state,
        events,
    )?;
    plan_hooks_for_snapshot(
        ArchHookWhen::PostTransaction,
        bundles,
        changes,
        state,
        events,
    )
}

#[derive(Clone, Copy)]
struct ActiveHook<'a> {
    view: &'a NativeBundleView<'a>,
    entry: &'a NativeLifecycleEntry,
}

fn plan_hooks_for_snapshot(
    when: ArchHookWhen,
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    let mut hooks_by_name = BTreeMap::<String, ActiveHook<'_>>::new();
    for view in bundles {
        if view.bundle.source_format.as_str() != "arch" || !visible_in_snapshot(view.role, when) {
            continue;
        }
        for entry in &view.bundle.entries {
            let Some(hook) = &entry.arch_hook else {
                continue;
            };
            if hook.action.when != when {
                continue;
            }
            let hook_name = hook_name(&hook.hook_path)?;
            let candidate = ActiveHook { view, entry };
            match hooks_by_name.get(&hook_name) {
                None => {
                    hooks_by_name.insert(hook_name, candidate);
                }
                Some(existing) => {
                    let existing_path = existing
                        .entry
                        .arch_hook
                        .as_ref()
                        .expect("active hook has metadata")
                        .hook_path
                        .as_str();
                    bail!(
                        "package-owned ALPM hook basename '{}' has two active definitions: '{}' from {} {} and '{}' from {} {}",
                        hook_name,
                        existing_path,
                        existing.view.package_name,
                        existing.view.package_version,
                        hook.hook_path,
                        view.package_name,
                        view.package_version
                    );
                }
            }
        }
    }

    for (hook_name, active) in hooks_by_name {
        plan_hook(hook_name, active, changes, state, events)?;
    }
    Ok(())
}

fn visible_in_snapshot(role: NativeBundleRole, when: ArchHookWhen) -> bool {
    match when {
        // libalpm scans pre-transaction hooks before payload mutation.
        ArchHookWhen::PreTransaction => {
            matches!(
                role,
                NativeBundleRole::Installed | NativeBundleRole::Removing
            )
        }
        // Post-transaction hooks are scanned after the new payload is in place.
        ArchHookWhen::PostTransaction => {
            matches!(
                role,
                NativeBundleRole::Installed | NativeBundleRole::Installing
            )
        }
    }
}

fn plan_hook(
    hook_name: String,
    active: ActiveHook<'_>,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    let hook = active
        .entry
        .arch_hook
        .as_ref()
        .expect("active hook has metadata");
    let action = &hook.action;
    if !action
        .depends
        .iter()
        .map(|dependency| {
            arch_dependency_satisfied(dependency, &state.installed_capabilities_after)
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .all(|satisfied| satisfied)
    {
        return Ok(());
    }

    let mut matched_targets = BTreeSet::new();
    for trigger in &hook.triggers {
        for change in changes {
            let Some(package_operation) = arch_operation(change.operation) else {
                // Debian deconfiguration preserves payload identity and is not
                // an ALPM package or path operation.
                continue;
            };
            let candidates = match trigger.trigger_type {
                ArchHookTriggerType::Package => {
                    if trigger.operations.contains(&package_operation) {
                        vec![change.package_name.clone()]
                    } else {
                        Vec::new()
                    }
                }
                ArchHookTriggerType::Path => trigger
                    .operations
                    .iter()
                    .flat_map(|operation| arch_path_candidates(change, *operation))
                    .map(|path| path.trim_start_matches('/').to_string())
                    .collect(),
            };
            for candidate in candidates {
                if alpm_hook_targets_match(&trigger.targets, &candidate).with_context(|| {
                    format!(
                        "invalid persisted ALPM Target contract in '{}'",
                        hook.hook_path
                    )
                })? {
                    matched_targets.insert(candidate);
                }
            }
        }
    }
    if matched_targets.is_empty() {
        return Ok(());
    }

    let matched_targets = matched_targets.into_iter().collect::<Vec<_>>();
    events.push(NativeTransactionEvent {
        owner_package: active.view.package_name.to_string(),
        owner_version: active.view.package_version.to_string(),
        owner_arch: active.view.bundle.source_arch.clone(),
        source_format: "arch".to_string(),
        stage: match action.when {
            ArchHookWhen::PreTransaction => NativeEventStage::ArchPreTransaction,
            ArchHookWhen::PostTransaction => NativeEventStage::ArchPostTransaction,
        },
        program: NativeEventProgram::Command {
            argv: action.argv.clone(),
        },
        args: Vec::new(),
        stdin: if action.needs_targets {
            lines_stdin(&matched_targets)
        } else {
            Vec::new()
        },
        matched_targets,
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: hook_name,
        placement: match action.when {
            ArchHookWhen::PreTransaction => NativeEventPlacement::TransactionBefore,
            ArchHookWhen::PostTransaction => NativeEventPlacement::TransactionAfter,
        },
    });
    Ok(())
}

fn arch_path_candidates(
    change: &NativeTransactionChange,
    operation: ArchHookOperation,
) -> BTreeSet<String> {
    match operation {
        ArchHookOperation::Install => change.added_paths(),
        ArchHookOperation::Upgrade => change.retained_paths(),
        ArchHookOperation::Remove => change.removed_paths(),
    }
}

fn arch_dependency_satisfied(
    raw: &str,
    capabilities: &[NativeInstalledCapability],
) -> Result<bool> {
    let clause = parse_arch_atom(raw)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("invalid persisted ALPM hook dependency '{raw}'"))?;
    let constraint = parse_repo_constraint(
        VersionScheme::Arch,
        clause.version_constraint.as_deref().unwrap_or(""),
    )
    .with_context(|| format!("invalid persisted ALPM hook dependency '{raw}'"))?;
    for capability in capabilities
        .iter()
        .filter(|capability| capability.name == clause.name)
    {
        if capability_satisfies(capability, &constraint)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn capability_satisfies(
    capability: &NativeInstalledCapability,
    constraint: &RepoVersionConstraint,
) -> Result<bool> {
    if matches!(constraint, RepoVersionConstraint::Any) {
        return Ok(true);
    }
    if capability.version_scheme != VersionScheme::Arch {
        return Ok(false);
    }
    let Some(version) = capability.version.as_deref() else {
        return Ok(false);
    };
    Ok(repo_version_satisfies(
        VersionScheme::Arch,
        version,
        constraint,
    )?)
}

fn arch_operation(operation: NativeTransactionOperation) -> Option<ArchHookOperation> {
    match operation {
        NativeTransactionOperation::Install => Some(ArchHookOperation::Install),
        NativeTransactionOperation::Upgrade => Some(ArchHookOperation::Upgrade),
        NativeTransactionOperation::Remove
        | NativeTransactionOperation::Purge
        | NativeTransactionOperation::RemoveInFavour { .. }
        | NativeTransactionOperation::Disappear { .. } => Some(ArchHookOperation::Remove),
        NativeTransactionOperation::DeconfigureInFavour { .. } => None,
    }
}

fn hook_name(path: &str) -> Result<String> {
    package_hook_basename(path)
        .map(str::to_string)
        .map_err(anyhow::Error::from)
}
