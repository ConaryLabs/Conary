// conary-core/src/ccs/native_transaction/deb/recovery.rs

//! Debian failure-event materialization and reverse lifecycle unwind.

use super::*;

pub(super) fn recovery_event(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    change: &NativeTransactionChange,
    role: NativeBundleRole,
    control_member: DebControlMember,
    mode: DebMaintainerMode,
) -> Result<Option<NativeTransactionEvent>> {
    let Some(view) = bundles.iter().find(|view| {
        view.package_name == change.package_name
            && match role {
                NativeBundleRole::Installing => view.bundle.source_arch == change.new_arch,
                NativeBundleRole::Removing | NativeBundleRole::Deconfiguring => {
                    view.bundle.source_arch == change.old_arch
                }
                NativeBundleRole::Installed => false,
            }
            && view.role == role
            && view.bundle.source_format.as_str() == "deb"
    }) else {
        return Ok(None);
    };
    let Some(entry) = view.bundle.entries.iter().find(|entry| {
        entry.deb_maintainer.as_ref().is_some_and(|metadata| {
            metadata.control_member == control_member
                && metadata
                    .invocations
                    .iter()
                    .any(|invocation| invocation.mode == mode)
        })
    }) else {
        return Ok(None);
    };
    let metadata = entry
        .deb_maintainer
        .as_ref()
        .expect("entry was selected by Debian metadata");
    let args = materialize_invocation(metadata, &mode, change, changes)?;
    Ok(Some(entry_event(
        view,
        entry,
        BundleEventSpec {
            stage: NativeEventStage::DebErrorRecovery,
            args,
            stdin: Vec::new(),
            matched_targets: Vec::new(),
            order_key: format!(
                "deb-recovery:{}:{:?}:{}",
                change.package_name,
                control_member,
                mode.as_str()
            ),
            placement: NativeEventPlacement::TransactionElement {
                transaction_index: change.transaction_index,
            },
        },
    )))
}

pub(super) fn recovery_step(
    event: Option<NativeTransactionEvent>,
    on_success: DebRecoveryResult,
    on_failure: DebRecoveryResult,
) -> DebRecoveryResult {
    match event {
        Some(event) => DebRecoveryResult::Run(Box::new(DebRecoveryNode {
            event,
            on_success,
            on_failure,
        })),
        None => on_success,
    }
}

/// Append the documented reverse-order `abort-remove in-favour` and
/// `abort-deconfigure` cleanup for packages whose `prerm` already ran before
/// an incoming package failed.
pub(super) fn append_relation_abort_unwind(
    recovery: DebRecoveryResult,
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    incoming_transaction_index: usize,
    before_removal_transaction_index: Option<usize>,
) -> Result<DebRecoveryResult> {
    append_relation_abort_unwind_with_bounds(
        recovery,
        bundles,
        changes,
        incoming_transaction_index,
        before_removal_transaction_index,
        None,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_relation_abort_unwind_with_bounds(
    recovery: DebRecoveryResult,
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    incoming_transaction_index: usize,
    before_removal_transaction_index: Option<usize>,
    before_deconfiguration_transaction_index: Option<usize>,
    include_conflictors: bool,
) -> Result<DebRecoveryResult> {
    fn append_to_aborts(
        recovery: DebRecoveryResult,
        cleanup: impl Fn(DebRecoveryDisposition) -> DebRecoveryResult + Copy,
    ) -> DebRecoveryResult {
        match recovery {
            DebRecoveryResult::Disposition(disposition @ DebRecoveryDisposition::Continue) => {
                DebRecoveryResult::Disposition(disposition)
            }
            DebRecoveryResult::Disposition(disposition @ DebRecoveryDisposition::Abort { .. }) => {
                cleanup(disposition)
            }
            DebRecoveryResult::Run(node) => {
                let node = *node;
                DebRecoveryResult::Run(Box::new(DebRecoveryNode {
                    event: node.event,
                    on_success: append_to_aborts(node.on_success, cleanup),
                    on_failure: append_to_aborts(node.on_failure, cleanup),
                }))
            }
            DebRecoveryResult::PersistState(transition) => {
                let transition = *transition;
                DebRecoveryResult::PersistState(Box::new(DebRecoveryStateTransition {
                    package: transition.package,
                    role: transition.role,
                    package_state: transition.package_state,
                    next: append_to_aborts(transition.next, cleanup),
                }))
            }
        }
    }

    let mut deconfigured = changes
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.operation,
                NativeTransactionOperation::DeconfigureInFavour {
                    installing_transaction_index,
                    ..
                } if installing_transaction_index == incoming_transaction_index
            ) && before_deconfiguration_transaction_index
                .is_none_or(|cutoff| candidate.transaction_index < cutoff)
                && bundles.iter().any(|view| {
                    view.role == NativeBundleRole::Deconfiguring
                        && view.package_name == candidate.package_name
                        && candidate
                            .old_version
                            .as_deref()
                            .is_some_and(|version| view.package_version == version)
                        && view.bundle.source_arch == candidate.old_arch
                        && view.bundle.source_format.as_str() == "deb"
                })
        })
        .collect::<Vec<_>>();
    deconfigured.sort_by_key(|change| change.transaction_index);
    let mut conflictors = changes
        .iter()
        .filter(|candidate| {
            include_conflictors
                && matches!(
                    candidate.operation,
                    NativeTransactionOperation::RemoveInFavour {
                        replacement_transaction_index
                    } if replacement_transaction_index == incoming_transaction_index
                )
                && before_removal_transaction_index
                    .is_none_or(|cutoff| candidate.transaction_index < cutoff)
                && bundles.iter().any(|view| {
                    view.role == NativeBundleRole::Removing
                        && view.package_name == candidate.package_name
                        && candidate
                            .old_version
                            .as_deref()
                            .is_some_and(|version| view.package_version == version)
                        && view.bundle.source_arch == candidate.old_arch
                        && view.bundle.source_format.as_str() == "deb"
                })
        })
        .collect::<Vec<_>>();
    conflictors.sort_by_key(|change| change.transaction_index);
    if deconfigured.is_empty() && conflictors.is_empty() {
        return Ok(recovery);
    }

    let mut cleanup_steps = deconfigured
        .into_iter()
        .map(|package| {
            let identity = NativePackageIdentity {
                package_name: package.package_name.clone(),
                package_version: package.old_version.clone().with_context(|| {
                    format!(
                        "deconfigure-in-favour change for '{}' has no old version",
                        package.package_name
                    )
                })?,
                package_arch: package.old_arch.clone(),
            };
            let event = recovery_event(
                bundles,
                changes,
                package,
                NativeBundleRole::Deconfiguring,
                DebControlMember::Postinst,
                DebMaintainerMode::AbortDeconfigure,
            )?;
            Ok((identity, NativeBundleRole::Deconfiguring, event))
        })
        .collect::<Result<Vec<_>>>()?;
    cleanup_steps.extend(
        conflictors
            .into_iter()
            .map(|conflictor| {
                let identity = NativePackageIdentity {
                    package_name: conflictor.package_name.clone(),
                    package_version: conflictor.old_version.clone().with_context(|| {
                        format!(
                            "remove-in-favour change for '{}' has no old version",
                            conflictor.package_name
                        )
                    })?,
                    package_arch: conflictor.old_arch.clone(),
                };
                let event = recovery_event(
                    bundles,
                    changes,
                    conflictor,
                    NativeBundleRole::Removing,
                    DebControlMember::Postinst,
                    DebMaintainerMode::AbortRemove,
                )?;
                Ok((identity, NativeBundleRole::Removing, event))
            })
            .collect::<Result<Vec<_>>>()?,
    );

    let cleanup = |disposition| {
        let mut tail = DebRecoveryResult::Disposition(disposition);
        for (identity, role, event) in &cleanup_steps {
            let success = DebRecoveryResult::PersistState(Box::new(DebRecoveryStateTransition {
                package: identity.clone(),
                role: *role,
                package_state: DebPackageState::Installed,
                next: tail.clone(),
            }));
            let failure = DebRecoveryResult::PersistState(Box::new(DebRecoveryStateTransition {
                package: identity.clone(),
                role: *role,
                package_state: DebPackageState::HalfConfigured,
                next: tail,
            }));
            tail = recovery_step(event.clone(), success, failure);
        }
        tail
    };

    Ok(append_to_aborts(recovery, cleanup))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn register_event_recovery(
    plan: &mut DebTransactionPlan,
    events: &[NativeTransactionEvent],
    change: &NativeTransactionChange,
    role: NativeBundleRole,
    stage: NativeEventStage,
    mode: DebMaintainerMode,
    recovery: DebRecoveryResult,
    terminal_state: DebPackageState,
) {
    if let Some(event) = find_primary_event(events, change, role, stage, mode) {
        let key = event_key(event);
        plan.event_failure_recovery.insert(key.clone(), recovery);
        plan.terminal_failure_states.insert(key, terminal_state);
    }
}

pub(super) fn register_terminal_state(
    plan: &mut DebTransactionPlan,
    events: &[NativeTransactionEvent],
    change: &NativeTransactionChange,
    role: NativeBundleRole,
    stage: NativeEventStage,
    mode: DebMaintainerMode,
    terminal_state: DebPackageState,
) {
    if let Some(event) = find_primary_event(events, change, role, stage, mode) {
        plan.terminal_failure_states
            .insert(event_key(event), terminal_state);
    }
}

fn find_primary_event<'a>(
    events: &'a [NativeTransactionEvent],
    change: &NativeTransactionChange,
    role: NativeBundleRole,
    stage: NativeEventStage,
    mode: DebMaintainerMode,
) -> Option<&'a NativeTransactionEvent> {
    let owner_version = match role {
        NativeBundleRole::Installing => change.new_version.as_deref(),
        NativeBundleRole::Removing | NativeBundleRole::Deconfiguring => {
            change.old_version.as_deref()
        }
        NativeBundleRole::Installed => None,
    };
    events.iter().find(|event| {
        event.owner_package == change.package_name
            && owner_version.is_some_and(|version| event.owner_version == version)
            && event.owner_arch
                == match role {
                    NativeBundleRole::Installing => change.new_arch.clone(),
                    NativeBundleRole::Removing | NativeBundleRole::Deconfiguring => {
                        change.old_arch.clone()
                    }
                    NativeBundleRole::Installed => None,
                }
            && event.stage == stage
            && event
                .args
                .first()
                .is_some_and(|action| action == mode.as_str())
    })
}
