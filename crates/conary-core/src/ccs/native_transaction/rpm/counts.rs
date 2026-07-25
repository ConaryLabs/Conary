// conary-core/src/ccs/native_transaction/rpm/counts.rs

//! RPM database and script-argument instance counts at exact lifecycle boundaries.

use super::super::{
    NativeBundleView, NativeInstalledPackage, NativeTransactionChange, NativeTransactionOperation,
    NativeTransactionState,
};
use super::changes_in_order;
use crate::ccs::native_lifecycle::RpmTriggerAction;
use anyhow::{Context, Result, bail};

pub(super) fn transaction_owner_count(
    change: &NativeTransactionChange,
    action: RpmTriggerAction,
) -> Result<u32> {
    match action {
        RpmTriggerAction::PreInstall => Ok(change.instances_before),
        RpmTriggerAction::Install => install_side_count(change),
        RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => Ok(change.instances_after),
    }
}

pub(super) fn triggering_package_count(
    change: &NativeTransactionChange,
    action: RpmTriggerAction,
) -> Result<u32> {
    transaction_owner_count(change, action)
}

pub(super) fn database_count_at_change(
    package_name: &str,
    fallback_final_count: u32,
    target: &NativeTransactionChange,
    action: RpmTriggerAction,
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
) -> Result<u32> {
    let mut count = initial_package_count(package_name, fallback_final_count, state, changes)?;
    for change in changes_in_order(changes) {
        if change.transaction_index > target.transaction_index {
            break;
        }
        if change.transaction_index < target.transaction_index {
            if change.package_name == package_name {
                count = change.instances_after;
            }
            continue;
        }
        if change.package_name == package_name {
            count = match action {
                RpmTriggerAction::PreInstall => change.instances_before,
                RpmTriggerAction::Install => install_side_count(change)?,
                RpmTriggerAction::Uninstall | RpmTriggerAction::PostUninstall => {
                    match change.operation {
                        NativeTransactionOperation::Upgrade => install_side_count(change)?,
                        NativeTransactionOperation::Install => change.instances_after,
                        NativeTransactionOperation::Remove
                        | NativeTransactionOperation::Purge
                        | NativeTransactionOperation::RemoveInFavour { .. }
                        | NativeTransactionOperation::Disappear { .. } => change.instances_before,
                        NativeTransactionOperation::DeconfigureInFavour { .. } => {
                            bail!(
                                "RPM trigger action cannot target Debian deconfiguration element '{}'",
                                change.package_name
                            )
                        }
                    }
                }
            };
        }
    }
    Ok(count)
}

pub(super) fn transaction_file_database_owner_count(
    view: &NativeBundleView<'_>,
    action: RpmTriggerAction,
    state: &NativeTransactionState,
    changes: &[NativeTransactionChange],
) -> Result<u32> {
    match action {
        RpmTriggerAction::Uninstall => {
            initial_package_count(view.package_name, view.instances_after, state, changes)
        }
        RpmTriggerAction::Install | RpmTriggerAction::PostUninstall => Ok(view.instances_after),
        RpmTriggerAction::PreInstall => Ok(view.instances_after),
    }
}

pub(super) fn package_count_in_snapshot(
    package_name: &str,
    snapshot: &[NativeInstalledPackage],
) -> Result<u32> {
    snapshot
        .iter()
        .filter(|package| package.package_name == package_name)
        .count()
        .try_into()
        .context("RPM package instance count exceeds u32")
}

fn install_side_count(change: &NativeTransactionChange) -> Result<u32> {
    change.instances_before.checked_add(1).with_context(|| {
        format!(
            "RPM install-side instance count overflow for '{}'",
            change.package_name
        )
    })
}

fn initial_package_count(
    package_name: &str,
    fallback_final_count: u32,
    state: &NativeTransactionState,
    changes: &[NativeTransactionChange],
) -> Result<u32> {
    let snapshot_count = state
        .installed_packages_before
        .iter()
        .filter(|package| package.package_name == package_name)
        .count();
    if snapshot_count > 0 {
        return snapshot_count
            .try_into()
            .context("RPM package instance count exceeds u32");
    }
    Ok(changes
        .iter()
        .filter(|change| change.package_name == package_name)
        .min_by_key(|change| change.transaction_index)
        .map(|change| change.instances_before)
        .unwrap_or(fallback_final_count))
}
