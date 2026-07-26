// conary-core/src/ccs/native_transaction/deb/refcount.rs

//! Event-time `DPKG_MAINTSCRIPT_PACKAGE_REFCOUNT` projection.

use super::super::{
    DebPackageState, NativeBundleView, NativeEventPlacement, NativeEventStage,
    NativeTransactionChange, NativeTransactionEvent, NativeTransactionOperation,
    NativeTransactionState,
};
use super::{DebRecoveryResult, DebTransactionPlan};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

type PackageInstances = BTreeMap<String, BTreeSet<Option<String>>>;

struct RefcountProjection {
    counts_after_element: BTreeMap<(usize, String), u32>,
    final_instances: PackageInstances,
}

pub(super) fn attach(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut [NativeTransactionEvent],
    plan: &mut DebTransactionPlan,
) -> Result<()> {
    validate_deb_change_indices(changes, state)?;
    let initial = initial_instances(state)?;
    let projection = simulate_instances(bundles, changes, state, initial.clone())?;

    for event in events.iter_mut() {
        attach_event_refcount(
            event,
            &initial,
            &projection.counts_after_element,
            &projection.final_instances,
        )?;
    }
    for recovery in plan.event_failure_recovery.values_mut() {
        attach_recovery_refcounts(
            recovery,
            &initial,
            &projection.counts_after_element,
            &projection.final_instances,
        )?;
    }
    for recovery in plan.payload_failure_recovery.values_mut() {
        attach_recovery_refcounts(
            recovery,
            &initial,
            &projection.counts_after_element,
            &projection.final_instances,
        )?;
    }
    Ok(())
}

fn attach_recovery_refcounts(
    recovery: &mut DebRecoveryResult,
    initial: &PackageInstances,
    counts_after_element: &BTreeMap<(usize, String), u32>,
    final_instances: &PackageInstances,
) -> Result<()> {
    match recovery {
        DebRecoveryResult::Disposition(_) => Ok(()),
        DebRecoveryResult::PersistState(transition) => attach_recovery_refcounts(
            &mut transition.next,
            initial,
            counts_after_element,
            final_instances,
        ),
        DebRecoveryResult::Run(node) => {
            attach_event_refcount(
                &mut node.event,
                initial,
                counts_after_element,
                final_instances,
            )?;
            attach_recovery_refcounts(
                &mut node.on_success,
                initial,
                counts_after_element,
                final_instances,
            )?;
            attach_recovery_refcounts(
                &mut node.on_failure,
                initial,
                counts_after_element,
                final_instances,
            )
        }
    }
}

fn attach_event_refcount(
    event: &mut NativeTransactionEvent,
    initial: &PackageInstances,
    counts_after_element: &BTreeMap<(usize, String), u32>,
    final_instances: &PackageInstances,
) -> Result<()> {
    if event.source_format != "deb" {
        return Ok(());
    }
    let count = if matches!(
        event.stage,
        NativeEventStage::DebPostInstall
            | NativeEventStage::DebAwaitedTriggerProcessing
            | NativeEventStage::DebTriggerProcessing
    ) {
        package_count(final_instances, &event.owner_package)?
    } else {
        match event.placement {
            NativeEventPlacement::TransactionBefore => {
                package_count(initial, &event.owner_package)?
            }
            NativeEventPlacement::TransactionElement { transaction_index } => counts_after_element
                .get(&(transaction_index, event.owner_package.clone()))
                .copied()
                .with_context(|| {
                    format!(
                        "Debian event for '{} {} {:?}' has no refcount projection at transaction element {}",
                        event.owner_package,
                        event.owner_version,
                        event.owner_arch,
                        transaction_index
                    )
                })?,
            NativeEventPlacement::TransactionAfter => {
                package_count(final_instances, &event.owner_package)?
            }
        }
    };
    if count == 0 {
        bail!(
            "Debian maintainer event for '{} {} {:?}' has zero package refcount",
            event.owner_package,
            event.owner_version,
            event.owner_arch
        );
    }
    event.deb_package_refcount = Some(count);
    Ok(())
}

fn validate_deb_change_indices(
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
) -> Result<()> {
    for index in &state.deb_change_indices {
        if !changes
            .iter()
            .any(|change| change.transaction_index == *index)
        {
            bail!("Debian transaction state references missing element index {index}");
        }
    }
    Ok(())
}

fn initial_instances(state: &NativeTransactionState) -> Result<PackageInstances> {
    let mut instances = PackageInstances::new();
    for package in &state.deb_package_states {
        if package.lifecycle_state == DebPackageState::NotInstalled {
            continue;
        }
        let arches = instances.entry(package.package_name.clone()).or_default();
        if !arches.insert(package.source_arch.clone()) {
            bail!(
                "Debian package state repeats package-set instance '{} {:?}'",
                package.package_name,
                package.source_arch
            );
        }
    }
    Ok(instances)
}

fn simulate_instances(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    mut instances: PackageInstances,
) -> Result<RefcountProjection> {
    let mut counts = BTreeMap::new();
    let mut ordered = changes.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|change| change.transaction_index);
    for change in ordered {
        if state.deb_change_indices.contains(&change.transaction_index) {
            apply_change(bundles, &mut instances, change)?;
        }
        for package_name in instances.keys() {
            counts.insert(
                (change.transaction_index, package_name.clone()),
                package_count(&instances, package_name)?,
            );
        }
        counts
            .entry((change.transaction_index, change.package_name.clone()))
            .or_insert(0);
    }
    Ok(RefcountProjection {
        counts_after_element: counts,
        final_instances: instances,
    })
}

fn apply_change(
    bundles: &[NativeBundleView<'_>],
    instances: &mut PackageInstances,
    change: &NativeTransactionChange,
) -> Result<()> {
    let arches = instances.entry(change.package_name.clone()).or_default();
    match change.operation {
        NativeTransactionOperation::Install => {
            arches.insert(change.new_arch.clone());
        }
        NativeTransactionOperation::Upgrade => {
            if !arches.remove(&change.old_arch) {
                bail!(
                    "Debian upgrade for '{} {:?}' has no counted old package instance",
                    change.package_name,
                    change.old_arch
                );
            }
            if change.old_arch != change.new_arch && arches.contains(&change.new_arch) {
                bail!(
                    "Debian crossgrade for '{}' targets already-counted architecture {:?}",
                    change.package_name,
                    change.new_arch
                );
            }
            arches.insert(change.new_arch.clone());
        }
        NativeTransactionOperation::DeconfigureInFavour { .. } => {
            if !arches.contains(&change.old_arch)
                && bundles.iter().any(|view| {
                    view.role == super::super::NativeBundleRole::Deconfiguring
                        && view.package_name == change.package_name
                        && view.bundle.source_arch == change.old_arch
                        && view.bundle.source_format.as_str() == "deb"
                })
            {
                bail!(
                    "Debian deconfiguration for '{} {:?}' has no counted package instance",
                    change.package_name,
                    change.old_arch
                );
            }
        }
        NativeTransactionOperation::Remove
        | NativeTransactionOperation::Purge
        | NativeTransactionOperation::RemoveInFavour { .. }
        | NativeTransactionOperation::Disappear { .. } => {
            // Ordinary removal ends in config-files, which is still a dpkg
            // package-set instance. Purge removes the instance only after its
            // postrm purge call succeeds.
            if !arches.contains(&change.old_arch)
                && bundles.iter().any(|view| {
                    view.package_name == change.package_name
                        && view.bundle.source_arch == change.old_arch
                        && view.bundle.source_format.as_str() == "deb"
                })
            {
                bail!(
                    "Debian removal for '{} {:?}' has no counted package instance",
                    change.package_name,
                    change.old_arch
                );
            }
        }
    }
    Ok(())
}

fn package_count(instances: &PackageInstances, package_name: &str) -> Result<u32> {
    instances
        .get(package_name)
        .map_or(0, BTreeSet::len)
        .try_into()
        .context("Debian package refcount exceeds u32")
}
