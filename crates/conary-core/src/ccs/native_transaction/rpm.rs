// conary-core/src/ccs/native_transaction/rpm.rs

//! Exact RPM package, file, and transaction-file trigger planning.
//!
//! RPM has two trigger-owner directions: an installed database owner reacting
//! to a changed transaction element, and a transaction owner reacting to the
//! installed package set. They remain separate typed events here.

use super::{
    BundleEventSpec, NativeBundleRole, NativeBundleView, NativeEventPlacement, NativeEventProgram,
    NativeEventStage, NativeInstalledPackage, NativeTransactionChange, NativeTransactionEvent,
    NativeTransactionOperation, NativeTransactionState, RpmTriggerOwner,
    ensure_rpm_program_available, entry_event, lines_stdin, rpm_runtime,
};
use crate::ccs::native_lifecycle::{
    NativeLifecycleEntry, RpmProgram, RpmTriggerAction, RpmTriggerKind, RpmTriggerTargetConstraint,
};
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme, repo_version_satisfies};
use anyhow::{Result, bail};
use std::collections::BTreeSet;

mod counts;

use counts::{
    database_count_at_change, package_count_in_snapshot, transaction_file_database_owner_count,
    transaction_owner_count, triggering_package_count,
};

const FILE_TRIGGER_HIGH_PRIORITY_BOUND: i32 = 10_000;

pub(super) fn plan(
    view: &NativeBundleView<'_>,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    if view.bundle.source_format.as_str() != "rpm" {
        return Ok(());
    }

    for (entry_index, entry) in view.bundle.entries.iter().enumerate() {
        if let Some(sysusers) = &entry.rpm_sysusers {
            if view.role == NativeBundleRole::Installing {
                let runtime = rpm_runtime(entry)?;
                if runtime.program != RpmProgram::Sysusers {
                    bail!(
                        "RPM sysusers entry '{}' does not declare the sysusers runtime",
                        entry.id
                    );
                }
                if !runtime.body_transforms.is_empty() {
                    bail!(
                        "RPM sysusers entry '{}' unexpectedly declares script body transforms",
                        entry.id
                    );
                }
                let transaction_index = owner_change(view, changes)
                    .map(|change| change.transaction_index)
                    .unwrap_or(usize::MAX);
                events.push(NativeTransactionEvent {
                    owner_package: view.package_name.to_string(),
                    owner_version: view.package_version.to_string(),
                    owner_arch: view.bundle.source_arch.clone(),
                    source_format: view.bundle.source_format.as_str().to_string(),
                    stage: NativeEventStage::RpmSysusers,
                    program: NativeEventProgram::RpmSysusers {
                        source_path: sysusers.source_path.clone(),
                    },
                    args: Vec::new(),
                    stdin: lines_stdin(&sysusers.lines),
                    matched_targets: sysusers.source_path.iter().cloned().collect(),
                    rpm_trigger_owner: None,
                    deb_package_refcount: None,
                    order_key: format!(
                        "rpm:{transaction_index:020}:sysusers:{entry_index:08}:{}",
                        entry.id
                    ),
                    placement: NativeEventPlacement::TransactionElement { transaction_index },
                });
            }
            continue;
        }

        let Some(trigger) = &entry.rpm_trigger else {
            continue;
        };
        ensure_rpm_program_available(entry)?;
        let action = single_trigger_action(&trigger.target_constraints, &entry.id)?;

        match trigger.kind {
            RpmTriggerKind::Package => {
                plan_installed_package_owner(
                    view,
                    entry,
                    entry_index,
                    action,
                    changes,
                    state,
                    events,
                )?;
                plan_transaction_package_owner(
                    view,
                    entry,
                    entry_index,
                    action,
                    changes,
                    state,
                    events,
                )?;
            }
            RpmTriggerKind::File => {
                plan_installed_file_owner(
                    view,
                    entry,
                    entry_index,
                    action,
                    changes,
                    state,
                    events,
                )?;
                plan_transaction_file_owner(
                    view,
                    entry,
                    entry_index,
                    action,
                    changes,
                    state,
                    events,
                )?;
            }
            RpmTriggerKind::TransactionFile => {
                plan_installed_transaction_file_owner(
                    view,
                    entry,
                    entry_index,
                    action,
                    changes,
                    state,
                    events,
                )?;
                plan_transaction_transaction_file_owner(
                    view,
                    entry,
                    entry_index,
                    action,
                    changes,
                    state,
                    events,
                )?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_installed_package_owner(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    entry_index: usize,
    action: RpmTriggerAction,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    for change in changes_in_order(changes) {
        let mut target_matches = false;
        for constraint in &entry
            .rpm_trigger
            .as_ref()
            .expect("trigger entry")
            .target_constraints
        {
            if constraint.action == action
                && constraint.package == change.package_name
                && constraint_matches_version(constraint, trigger_change_version(change, action))?
            {
                target_matches = true;
                break;
            }
        }
        if !rpm_action_matches_change(action, change.operation)
            || !installed_owner_visible(view, change, action, RpmTriggerKind::Package, changes)
            || !target_matches
        {
            continue;
        }

        push_trigger_event(
            view,
            entry,
            rpm_stage(RpmTriggerKind::Package, action, None)?,
            vec![
                database_count_at_change(
                    view.package_name,
                    view.instances_after,
                    change,
                    action,
                    changes,
                    state,
                )?
                .to_string(),
                triggering_package_count(change, action)?.to_string(),
            ],
            Vec::new(),
            vec![change.package_name.clone()],
            RpmTriggerOwner::InstalledDatabase,
            NativeEventPlacement::TransactionElement {
                transaction_index: change.transaction_index,
            },
            trigger_order_key(
                change.transaction_index,
                action,
                RpmTriggerOwner::InstalledDatabase,
                None,
                entry_index,
                &entry.id,
            ),
            events,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_transaction_package_owner(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    entry_index: usize,
    action: RpmTriggerAction,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    let Some(owner_change) = transaction_owner_change(view, changes, action) else {
        return Ok(());
    };
    let snapshot = package_snapshot(state, changes, owner_change, action);
    let constraints = &entry
        .rpm_trigger
        .as_ref()
        .expect("trigger entry")
        .target_constraints;
    let Some(first_target) = first_matching_package(constraints, action, &snapshot)? else {
        return Ok(());
    };
    let matched_targets = matching_package_names(constraints, action, &snapshot)?;
    let target_count = package_count_in_snapshot(&first_target.package_name, &snapshot)?;

    push_trigger_event(
        view,
        entry,
        rpm_stage(RpmTriggerKind::Package, action, None)?,
        vec![
            transaction_owner_count(owner_change, action)?.to_string(),
            target_count.to_string(),
        ],
        Vec::new(),
        matched_targets,
        RpmTriggerOwner::Transaction,
        NativeEventPlacement::TransactionElement {
            transaction_index: owner_change.transaction_index,
        },
        trigger_order_key(
            owner_change.transaction_index,
            action,
            RpmTriggerOwner::Transaction,
            None,
            entry_index,
            &entry.id,
        ),
        events,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_installed_file_owner(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    entry_index: usize,
    action: RpmTriggerAction,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    let trigger = entry.rpm_trigger.as_ref().expect("trigger entry");
    for change in changes_in_order(changes) {
        if !rpm_action_matches_change(action, change.operation)
            || !installed_owner_visible(view, change, action, RpmTriggerKind::File, changes)
        {
            continue;
        }
        let candidates = trigger_change_paths(change, action);
        let paths = matching_paths(&trigger.path_prefixes, candidates.iter());
        if paths.is_empty() {
            continue;
        }
        push_trigger_event(
            view,
            entry,
            rpm_stage(RpmTriggerKind::File, action, trigger.priority)?,
            vec![
                database_count_at_change(
                    view.package_name,
                    view.instances_after,
                    change,
                    action,
                    changes,
                    state,
                )?
                .to_string(),
                triggering_package_count(change, action)?.to_string(),
            ],
            lines_stdin(&paths),
            paths,
            RpmTriggerOwner::InstalledDatabase,
            NativeEventPlacement::TransactionElement {
                transaction_index: change.transaction_index,
            },
            trigger_order_key(
                change.transaction_index,
                action,
                RpmTriggerOwner::InstalledDatabase,
                trigger.priority,
                entry_index,
                &entry.id,
            ),
            events,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_transaction_file_owner(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    entry_index: usize,
    action: RpmTriggerAction,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    let Some(owner_change) = transaction_owner_change(view, changes, action) else {
        return Ok(());
    };
    let snapshot = package_snapshot(state, changes, owner_change, action);
    let trigger = entry.rpm_trigger.as_ref().expect("trigger entry");
    let matching_packages = snapshot
        .iter()
        .filter_map(|package| {
            let paths = matching_paths(&trigger.path_prefixes, package.paths.iter());
            (!paths.is_empty()).then_some((package, paths))
        })
        .collect::<Vec<_>>();
    let Some((first_target, _)) = matching_packages.first() else {
        return Ok(());
    };
    let paths = matching_packages
        .iter()
        .flat_map(|(_, paths)| paths.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let target_count = package_count_in_snapshot(&first_target.package_name, &snapshot)?;

    push_trigger_event(
        view,
        entry,
        rpm_stage(RpmTriggerKind::File, action, trigger.priority)?,
        vec![
            transaction_owner_count(owner_change, action)?.to_string(),
            target_count.to_string(),
        ],
        lines_stdin(&paths),
        paths,
        RpmTriggerOwner::Transaction,
        NativeEventPlacement::TransactionElement {
            transaction_index: owner_change.transaction_index,
        },
        trigger_order_key(
            owner_change.transaction_index,
            action,
            RpmTriggerOwner::Transaction,
            trigger.priority,
            entry_index,
            &entry.id,
        ),
        events,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_installed_transaction_file_owner(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    entry_index: usize,
    action: RpmTriggerAction,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    if view.role != NativeBundleRole::Installed
        && !(view.role == NativeBundleRole::Installing && action == RpmTriggerAction::PostUninstall)
    {
        return Ok(());
    }
    let trigger = entry.rpm_trigger.as_ref().expect("trigger entry");
    let activation_paths = transaction_activation_paths(changes, action);
    let matching_activation = matching_paths(&trigger.path_prefixes, activation_paths.iter());
    if matching_activation.is_empty() {
        return Ok(());
    }
    // rpm's database-owner transaction trigger searches the files added by
    // this transaction. The final transaction file set is reserved for the
    // transaction-owned immediate pass below.
    let paths = matching_activation;
    let stdin = if action == RpmTriggerAction::PostUninstall {
        Vec::new()
    } else {
        lines_stdin(&paths)
    };
    let transaction_index = changes
        .iter()
        .filter(|change| rpm_action_matches_change(action, change.operation))
        .map(|change| change.transaction_index)
        .min()
        .unwrap_or(usize::MAX);
    push_trigger_event(
        view,
        entry,
        database_transaction_file_stage(action)?,
        vec![transaction_file_database_owner_count(view, action, state, changes)?.to_string()],
        stdin,
        paths,
        RpmTriggerOwner::InstalledDatabase,
        transaction_trigger_placement(action),
        trigger_order_key(
            transaction_index,
            action,
            RpmTriggerOwner::InstalledDatabase,
            None,
            entry_index,
            &entry.id,
        ),
        events,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_transaction_transaction_file_owner(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    entry_index: usize,
    action: RpmTriggerAction,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    let Some(owner_change) = transaction_owner_change(view, changes, action) else {
        return Ok(());
    };
    let trigger = entry.rpm_trigger.as_ref().expect("trigger entry");
    let paths = match action {
        RpmTriggerAction::Install => {
            matching_paths(&trigger.path_prefixes, state.installed_paths_after.iter())
        }
        RpmTriggerAction::Uninstall => {
            let removed = transaction_activation_paths(changes, action);
            matching_paths(&trigger.path_prefixes, removed.iter())
        }
        RpmTriggerAction::PostUninstall => return Ok(()),
        RpmTriggerAction::PreInstall => {
            bail!("RPM transaction file triggers do not define a pre-install action")
        }
    };
    if paths.is_empty() {
        return Ok(());
    }
    push_trigger_event(
        view,
        entry,
        rpm_stage(RpmTriggerKind::TransactionFile, action, trigger.priority)?,
        vec![transaction_owner_count(owner_change, action)?.to_string()],
        lines_stdin(&paths),
        paths,
        RpmTriggerOwner::Transaction,
        transaction_trigger_placement(action),
        trigger_order_key(
            owner_change.transaction_index,
            action,
            RpmTriggerOwner::Transaction,
            None,
            entry_index,
            &entry.id,
        ),
        events,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_trigger_event(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    stage: NativeEventStage,
    args: Vec<String>,
    stdin: Vec<u8>,
    matched_targets: Vec<String>,
    owner: RpmTriggerOwner,
    placement: NativeEventPlacement,
    order_key: String,
    events: &mut Vec<NativeTransactionEvent>,
) {
    let mut event = entry_event(
        view,
        entry,
        BundleEventSpec {
            stage,
            args,
            stdin,
            matched_targets,
            order_key,
            placement,
        },
    );
    event.rpm_trigger_owner = Some(owner);
    events.push(event);
}

fn transaction_trigger_placement(action: RpmTriggerAction) -> NativeEventPlacement {
    match action {
        RpmTriggerAction::Uninstall => NativeEventPlacement::TransactionBefore,
        RpmTriggerAction::Install | RpmTriggerAction::PostUninstall => {
            NativeEventPlacement::TransactionAfter
        }
        RpmTriggerAction::PreInstall => {
            unreachable!("RPM transaction file triggers have no pre-install action")
        }
    }
}

fn transaction_owner_change<'a>(
    view: &NativeBundleView<'_>,
    changes: &'a [NativeTransactionChange],
    action: RpmTriggerAction,
) -> Option<&'a NativeTransactionChange> {
    let role_matches = matches!(
        (view.role, action),
        (
            NativeBundleRole::Installing,
            RpmTriggerAction::PreInstall | RpmTriggerAction::Install
        ) | (NativeBundleRole::Removing, RpmTriggerAction::Uninstall)
    );
    role_matches.then(|| owner_change(view, changes)).flatten()
}

fn owner_change<'a>(
    view: &NativeBundleView<'_>,
    changes: &'a [NativeTransactionChange],
) -> Option<&'a NativeTransactionChange> {
    changes.iter().find(|change| {
        change.package_name == view.package_name
            && match view.role {
                NativeBundleRole::Installing => change.new_arch == view.bundle.source_arch,
                NativeBundleRole::Removing => change.old_arch == view.bundle.source_arch,
                NativeBundleRole::Installed | NativeBundleRole::Deconfiguring => false,
            }
            && match view.role {
                NativeBundleRole::Installing => {
                    change.new_version.as_deref() == Some(view.package_version)
                }
                NativeBundleRole::Removing => {
                    change.old_version.as_deref() == Some(view.package_version)
                }
                NativeBundleRole::Installed | NativeBundleRole::Deconfiguring => false,
            }
    })
}

fn installed_owner_visible(
    view: &NativeBundleView<'_>,
    target: &NativeTransactionChange,
    action: RpmTriggerAction,
    kind: RpmTriggerKind,
    changes: &[NativeTransactionChange],
) -> bool {
    match view.role {
        NativeBundleRole::Installed => true,
        NativeBundleRole::Deconfiguring => false,
        NativeBundleRole::Installing => {
            let Some(owner) = owner_change(view, changes) else {
                return false;
            };
            if kind == RpmTriggerKind::File && action == RpmTriggerAction::Uninstall {
                // RPM skips database file-trigger owners that are in the
                // transaction's installed-package set for `%filetriggerun`.
                return false;
            }
            match action {
                RpmTriggerAction::PreInstall => owner.transaction_index < target.transaction_index,
                RpmTriggerAction::Install => owner.transaction_index <= target.transaction_index,
                RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => {
                    owner.transaction_index <= target.transaction_index
                }
            }
        }
        NativeBundleRole::Removing => {
            let Some(owner) = owner_change(view, changes) else {
                return false;
            };
            if kind == RpmTriggerKind::File
                && matches!(
                    action,
                    RpmTriggerAction::PreInstall
                        | RpmTriggerAction::Install
                        | RpmTriggerAction::PostUninstall
                )
            {
                return false;
            }
            match action {
                RpmTriggerAction::PreInstall | RpmTriggerAction::Install => {
                    owner.transaction_index >= target.transaction_index
                }
                RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => {
                    owner.transaction_index >= target.transaction_index
                }
            }
        }
    }
}

fn package_snapshot(
    state: &NativeTransactionState,
    changes: &[NativeTransactionChange],
    owner: &NativeTransactionChange,
    action: RpmTriggerAction,
) -> Vec<NativeInstalledPackage> {
    let mut packages = state.installed_packages_before.clone();
    for change in changes_in_order(changes) {
        if change.transaction_index > owner.transaction_index {
            break;
        }
        if change.transaction_index < owner.transaction_index {
            apply_final_change(&mut packages, change);
            continue;
        }
        match action {
            RpmTriggerAction::PreInstall => {}
            RpmTriggerAction::Install => add_new_package(&mut packages, change),
            RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => {
                if change.operation == NativeTransactionOperation::Upgrade {
                    add_new_package(&mut packages, change);
                }
            }
        }
    }
    packages
}

fn apply_final_change(
    packages: &mut Vec<NativeInstalledPackage>,
    change: &NativeTransactionChange,
) {
    if let Some(old_version) = &change.old_version
        && let Some(index) = packages.iter().position(|package| {
            package.package_name == change.package_name
                && package.package_version == *old_version
                && package.package_arch == change.old_arch
        })
    {
        packages.remove(index);
    }
    add_new_package(packages, change);
}

fn add_new_package(packages: &mut Vec<NativeInstalledPackage>, change: &NativeTransactionChange) {
    let Some(new_version) = &change.new_version else {
        return;
    };
    if packages.iter().any(|package| {
        package.package_name == change.package_name
            && package.package_version == *new_version
            && package.package_arch == change.new_arch
            && package.paths == change.new_paths
    }) {
        return;
    }
    packages.push(NativeInstalledPackage {
        package_name: change.package_name.clone(),
        package_version: new_version.clone(),
        package_arch: change.new_arch.clone(),
        paths: change.new_paths.clone(),
    });
}

fn first_matching_package<'a>(
    constraints: &[RpmTriggerTargetConstraint],
    action: RpmTriggerAction,
    packages: &'a [NativeInstalledPackage],
) -> Result<Option<&'a NativeInstalledPackage>> {
    for constraint in constraints
        .iter()
        .filter(|constraint| constraint.action == action)
    {
        for package in packages {
            if package.package_name == constraint.package
                && constraint_matches_version(constraint, Some(package.package_version.as_str()))?
            {
                return Ok(Some(package));
            }
        }
    }
    Ok(None)
}

fn matching_package_names(
    constraints: &[RpmTriggerTargetConstraint],
    action: RpmTriggerAction,
    packages: &[NativeInstalledPackage],
) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for constraint in constraints
        .iter()
        .filter(|constraint| constraint.action == action)
    {
        for package in packages {
            if package.package_name == constraint.package
                && constraint_matches_version(constraint, Some(package.package_version.as_str()))?
            {
                names.insert(constraint.package.clone());
                break;
            }
        }
    }
    Ok(names.into_iter().collect())
}

fn changes_in_order(changes: &[NativeTransactionChange]) -> Vec<&NativeTransactionChange> {
    let mut ordered = changes.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|change| change.transaction_index);
    ordered
}

fn trigger_change_paths(
    change: &NativeTransactionChange,
    action: RpmTriggerAction,
) -> BTreeSet<String> {
    match action {
        RpmTriggerAction::PreInstall | RpmTriggerAction::Install => change.new_paths.clone(),
        RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => change.removed_paths(),
    }
}

fn transaction_activation_paths(
    changes: &[NativeTransactionChange],
    action: RpmTriggerAction,
) -> BTreeSet<String> {
    changes
        .iter()
        .filter(|change| rpm_action_matches_change(action, change.operation))
        .flat_map(|change| trigger_change_paths(change, action))
        .collect()
}

fn trigger_change_version(
    change: &NativeTransactionChange,
    action: RpmTriggerAction,
) -> Option<&str> {
    match action {
        RpmTriggerAction::PreInstall | RpmTriggerAction::Install => change.new_version.as_deref(),
        RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => {
            change.old_version.as_deref()
        }
    }
}

fn constraint_matches_version(
    constraint: &RpmTriggerTargetConstraint,
    version: Option<&str>,
) -> Result<bool> {
    let (Some(expected), Some(version)) = (constraint.version.as_deref(), version) else {
        return Ok(constraint.version.is_none());
    };
    let parsed = match constraint.operator.as_deref() {
        None => RepoVersionConstraint::Any,
        Some("=") => RepoVersionConstraint::Exact(expected.to_string()),
        Some(">") => RepoVersionConstraint::GreaterThan(expected.to_string()),
        Some(">=") => RepoVersionConstraint::GreaterOrEqual(expected.to_string()),
        Some("<") => RepoVersionConstraint::LessThan(expected.to_string()),
        Some("<=") => RepoVersionConstraint::LessOrEqual(expected.to_string()),
        Some(_) => return Ok(false),
    };
    Ok(repo_version_satisfies(
        VersionScheme::Rpm,
        version,
        &parsed,
    )?)
}

fn single_trigger_action(
    constraints: &[RpmTriggerTargetConstraint],
    entry_id: &str,
) -> Result<RpmTriggerAction> {
    let Some(first) = constraints.first() else {
        bail!("RPM trigger entry '{entry_id}' has no target constraints");
    };
    if constraints
        .iter()
        .any(|constraint| constraint.action != first.action)
    {
        bail!("RPM trigger entry '{entry_id}' mixes trigger actions");
    }
    Ok(first.action)
}

fn rpm_action_matches_change(
    action: RpmTriggerAction,
    operation: NativeTransactionOperation,
) -> bool {
    match action {
        RpmTriggerAction::PreInstall | RpmTriggerAction::Install => matches!(
            operation,
            NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade
        ),
        RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => {
            matches!(operation, NativeTransactionOperation::Upgrade) || operation.removes_package()
        }
    }
}

fn matching_paths<'a>(prefixes: &[String], paths: impl Iterator<Item = &'a String>) -> Vec<String> {
    paths
        .filter_map(|path| {
            let normalized = if path.starts_with('/') {
                path.clone()
            } else {
                format!("/{path}")
            };
            prefixes
                .iter()
                .any(|prefix| path_has_prefix(&normalized, prefix))
                .then_some(normalized)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn path_has_prefix(path: &str, prefix: &str) -> bool {
    // rpmfilesFindPrefix() compares exactly prefix.len() bytes with strncmp.
    // A trailing slash is significant and `/usr/lib` intentionally matches
    // `/usr/libfoo`.
    path.starts_with(prefix)
}

fn database_transaction_file_stage(action: RpmTriggerAction) -> Result<NativeEventStage> {
    match action {
        RpmTriggerAction::Install => Ok(NativeEventStage::RpmDatabaseTransactionFileTriggerInstall),
        RpmTriggerAction::Uninstall => Ok(NativeEventStage::RpmTransactionFileTriggerUninstall),
        RpmTriggerAction::PostUninstall => {
            Ok(NativeEventStage::RpmTransactionFileTriggerPostUninstall)
        }
        RpmTriggerAction::PreInstall => {
            bail!("RPM transaction file triggers do not define a pre-install action")
        }
    }
}

pub(super) fn rpm_stage(
    kind: RpmTriggerKind,
    action: RpmTriggerAction,
    priority: Option<i32>,
) -> Result<NativeEventStage> {
    let high_priority = priority.unwrap_or(100_000) >= FILE_TRIGGER_HIGH_PRIORITY_BOUND;
    let stage = match (kind, action) {
        (RpmTriggerKind::Package, RpmTriggerAction::PreInstall) => {
            NativeEventStage::RpmTriggerPreInstall
        }
        (RpmTriggerKind::Package, RpmTriggerAction::Install) => NativeEventStage::RpmTriggerInstall,
        (RpmTriggerKind::Package, RpmTriggerAction::Uninstall) => {
            NativeEventStage::RpmTriggerUninstall
        }
        (RpmTriggerKind::Package, RpmTriggerAction::PostUninstall) => {
            NativeEventStage::RpmTriggerPostUninstall
        }
        (RpmTriggerKind::File, RpmTriggerAction::Install) if high_priority => {
            NativeEventStage::RpmFileTriggerInstallHigh
        }
        (RpmTriggerKind::File, RpmTriggerAction::Install) => {
            NativeEventStage::RpmFileTriggerInstallLow
        }
        (RpmTriggerKind::File, RpmTriggerAction::Uninstall) if high_priority => {
            NativeEventStage::RpmFileTriggerUninstallHigh
        }
        (RpmTriggerKind::File, RpmTriggerAction::Uninstall) => {
            NativeEventStage::RpmFileTriggerUninstallLow
        }
        (RpmTriggerKind::File, RpmTriggerAction::PostUninstall) if high_priority => {
            NativeEventStage::RpmFileTriggerPostUninstallHigh
        }
        (RpmTriggerKind::File, RpmTriggerAction::PostUninstall) => {
            NativeEventStage::RpmFileTriggerPostUninstallLow
        }
        (RpmTriggerKind::TransactionFile, RpmTriggerAction::Uninstall) => {
            NativeEventStage::RpmTransactionFileTriggerUninstall
        }
        (RpmTriggerKind::TransactionFile, RpmTriggerAction::PostUninstall) => {
            NativeEventStage::RpmTransactionFileTriggerPostUninstall
        }
        (RpmTriggerKind::TransactionFile, RpmTriggerAction::Install) => {
            NativeEventStage::RpmTransactionFileTriggerInstall
        }
        (RpmTriggerKind::File | RpmTriggerKind::TransactionFile, RpmTriggerAction::PreInstall) => {
            bail!("RPM file triggers do not define a pre-install action")
        }
    };
    Ok(stage)
}

fn trigger_order_key(
    transaction_index: usize,
    action: RpmTriggerAction,
    owner: RpmTriggerOwner,
    priority: Option<i32>,
    entry_index: usize,
    entry_id: &str,
) -> String {
    let direction = match (action, owner) {
        (
            RpmTriggerAction::PreInstall | RpmTriggerAction::Install,
            RpmTriggerOwner::InstalledDatabase,
        ) => 0,
        (
            RpmTriggerAction::PreInstall | RpmTriggerAction::Install,
            RpmTriggerOwner::Transaction,
        ) => 1,
        (
            RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall,
            RpmTriggerOwner::Transaction,
        ) => 0,
        (
            RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall,
            RpmTriggerOwner::InstalledDatabase,
        ) => 1,
    };
    let priority = priority.unwrap_or(100_000);
    let descending_priority = i64::from(i32::MAX) - i64::from(priority);
    format!(
        "rpm:{transaction_index:020}:{direction}:{descending_priority:010}:{entry_index:08}:{entry_id}"
    )
}
