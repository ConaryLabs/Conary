// conary-core/src/ccs/native_transaction/deb.rs

//! Debian maintainer-script transaction planning and error recovery.
//!
//! Debian Policy defines a conditional call graph, not a flat list of hooks.
//! This module keeps that graph typed so callers can execute the documented
//! fallback and unwind calls without inspecting script bodies or delegating
//! transaction authority to `dpkg`.

use super::{
    BundleEventSpec, NativeBundleRole, NativeBundleView, NativeEventPlacement, NativeEventStage,
    NativePackageIdentity, NativeTransactionChange, NativeTransactionEvent,
    NativeTransactionOperation, NativeTransactionState, entry_event,
};
use crate::ccs::native_lifecycle::{
    DebControlMember, DebMaintainerArgumentValue, DebMaintainerMetadata, DebMaintainerMode,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;

mod recovery;
mod refcount;
mod triggers;

use recovery::{
    append_relation_abort_unwind, append_relation_abort_unwind_with_bounds, recovery_event,
    recovery_step, register_event_recovery, register_terminal_state,
};
#[cfg(test)]
pub(super) use triggers::deb_file_trigger_matches;
pub use triggers::{DebTriggerActivation, DebTriggerBatch, DebTriggerUnconfiguration};

/// Debian's persisted package states relevant to maintainer-script recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebPackageState {
    NotInstalled,
    ConfigFiles,
    HalfInstalled,
    Unpacked,
    HalfConfigured,
    TriggersAwaited,
    TriggersPending,
    Installed,
}

impl DebPackageState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not-installed",
            Self::ConfigFiles => "config-files",
            Self::HalfInstalled => "half-installed",
            Self::Unpacked => "unpacked",
            Self::HalfConfigured => "half-configured",
            Self::TriggersAwaited => "triggers-awaited",
            Self::TriggersPending => "triggers-pending",
            Self::Installed => "installed",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "not-installed" => Ok(Self::NotInstalled),
            "config-files" => Ok(Self::ConfigFiles),
            "half-installed" => Ok(Self::HalfInstalled),
            "unpacked" => Ok(Self::Unpacked),
            "half-configured" => Ok(Self::HalfConfigured),
            "triggers-awaited" => Ok(Self::TriggersAwaited),
            "triggers-pending" => Ok(Self::TriggersPending),
            "installed" => Ok(Self::Installed),
            other => bail!("unknown Debian package state '{other}'"),
        }
    }
}

/// What the caller must do after a recovery branch reaches a leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebRecoveryDisposition {
    /// The documented fallback repaired the failed primary action, so the
    /// forward transaction continues.
    Continue,
    /// The forward transaction stops in an exact Debian package state.
    Abort {
        package_state: DebPackageState,
        /// The new payload was already exposed but the transaction is still
        /// before Debian's old-postrm point of no return.
        restore_previous_payload: bool,
    },
}

/// One conditional recovery-script invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebRecoveryNode {
    pub event: NativeTransactionEvent,
    pub on_success: DebRecoveryResult,
    pub on_failure: DebRecoveryResult,
}

/// One exact package-state transition performed while unwinding a failed
/// Debian transaction element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebRecoveryStateTransition {
    pub package: NativePackageIdentity,
    pub role: NativeBundleRole,
    pub package_state: DebPackageState,
    pub next: DebRecoveryResult,
}

/// A recursive Debian fallback/error-unwind branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebRecoveryResult {
    Disposition(DebRecoveryDisposition),
    Run(Box<DebRecoveryNode>),
    PersistState(Box<DebRecoveryStateTransition>),
}

impl DebRecoveryResult {
    fn continue_transaction() -> Self {
        Self::Disposition(DebRecoveryDisposition::Continue)
    }

    fn abort(package_state: DebPackageState, restore_previous_payload: bool) -> Self {
        Self::Disposition(DebRecoveryDisposition::Abort {
            package_state,
            restore_previous_payload,
        })
    }
}

/// Debian-specific dynamic state attached to the otherwise flat native plan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebTransactionPlan {
    event_failure_recovery: BTreeMap<String, DebRecoveryResult>,
    payload_failure_recovery: BTreeMap<NativePackageIdentity, DebRecoveryResult>,
    terminal_failure_states: BTreeMap<String, DebPackageState>,
    pub trigger_activations: Vec<DebTriggerActivation>,
    pub trigger_unconfigurations: Vec<DebTriggerUnconfiguration>,
    pub trigger_batches: Vec<DebTriggerBatch>,
}

impl DebTransactionPlan {
    pub fn recovery_for_event(&self, event: &NativeTransactionEvent) -> Option<&DebRecoveryResult> {
        self.event_failure_recovery.get(&event_key(event))
    }

    pub fn recovery_for_payload_failure(
        &self,
        package: &NativePackageIdentity,
    ) -> Option<&DebRecoveryResult> {
        self.payload_failure_recovery.get(package)
    }

    pub fn terminal_failure_state(
        &self,
        event: &NativeTransactionEvent,
    ) -> Option<DebPackageState> {
        self.terminal_failure_states.get(&event_key(event)).copied()
    }
}

pub(super) fn regular_invocation(
    view: &NativeBundleView<'_>,
    change: &NativeTransactionChange,
    changes: &[NativeTransactionChange],
    metadata: &DebMaintainerMetadata,
) -> Result<Option<(NativeEventStage, Vec<String>)>> {
    let (stage, mode) = match (view.role, metadata.control_member, change.operation) {
        (
            NativeBundleRole::Installing,
            DebControlMember::Config,
            NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade,
        ) => (
            NativeEventStage::DebPreConfigure,
            DebMaintainerMode::Configure,
        ),
        (
            NativeBundleRole::Installing,
            DebControlMember::Preinst,
            NativeTransactionOperation::Install,
        ) => (
            NativeEventStage::PackagePreInstall,
            DebMaintainerMode::Install,
        ),
        (
            NativeBundleRole::Installing,
            DebControlMember::Preinst,
            NativeTransactionOperation::Upgrade,
        ) => (
            NativeEventStage::PackagePreInstall,
            DebMaintainerMode::Upgrade,
        ),
        (
            NativeBundleRole::Installing,
            DebControlMember::Postinst,
            NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade,
        ) => (
            NativeEventStage::DebPostInstall,
            DebMaintainerMode::Configure,
        ),
        (
            NativeBundleRole::Removing,
            DebControlMember::Prerm,
            NativeTransactionOperation::Upgrade,
        ) => (
            NativeEventStage::DebPreRemoveUpgrade,
            DebMaintainerMode::Upgrade,
        ),
        (
            NativeBundleRole::Removing,
            DebControlMember::Postrm,
            NativeTransactionOperation::Upgrade,
        ) => (
            NativeEventStage::DebPostRemoveUpgrade,
            DebMaintainerMode::Upgrade,
        ),
        (
            NativeBundleRole::Removing,
            DebControlMember::Prerm,
            NativeTransactionOperation::Remove | NativeTransactionOperation::Purge,
        ) => (
            NativeEventStage::PackagePreRemove,
            DebMaintainerMode::Remove,
        ),
        (
            NativeBundleRole::Removing,
            DebControlMember::Postrm,
            NativeTransactionOperation::Remove | NativeTransactionOperation::Purge,
        ) => (
            NativeEventStage::PackagePostRemove,
            DebMaintainerMode::Remove,
        ),
        (
            NativeBundleRole::Removing,
            DebControlMember::Prerm,
            NativeTransactionOperation::RemoveInFavour { .. },
        ) => (
            NativeEventStage::DebPreRemoveInFavour,
            DebMaintainerMode::Remove,
        ),
        (
            NativeBundleRole::Deconfiguring,
            DebControlMember::Prerm,
            NativeTransactionOperation::DeconfigureInFavour { .. },
        ) => (
            NativeEventStage::DebPreDeconfigure,
            DebMaintainerMode::Deconfigure,
        ),
        (
            NativeBundleRole::Removing,
            DebControlMember::Postrm,
            NativeTransactionOperation::RemoveInFavour { .. },
        ) => (
            NativeEventStage::PackagePostRemove,
            DebMaintainerMode::Remove,
        ),
        (
            NativeBundleRole::Removing,
            DebControlMember::Postrm,
            NativeTransactionOperation::Disappear { .. },
        ) => (
            NativeEventStage::DebPostRemoveDisappear,
            DebMaintainerMode::Disappear,
        ),
        _ => return Ok(None),
    };
    let args = materialize_invocation(metadata, &mode, change, changes)?;
    Ok(Some((stage, args)))
}

pub(super) const fn regular_event_transaction_index(
    change: &NativeTransactionChange,
    stage: NativeEventStage,
) -> usize {
    match (change.operation, stage) {
        (
            NativeTransactionOperation::RemoveInFavour {
                replacement_transaction_index,
            },
            NativeEventStage::DebPreRemoveInFavour,
        ) => replacement_transaction_index,
        (
            NativeTransactionOperation::DeconfigureInFavour {
                installing_transaction_index,
                ..
            },
            NativeEventStage::DebPreDeconfigure,
        ) => installing_transaction_index,
        (
            NativeTransactionOperation::Disappear {
                overwriter_transaction_index,
            },
            NativeEventStage::DebPostRemoveDisappear,
        ) => overwriter_transaction_index,
        _ => change.transaction_index,
    }
}

pub(super) fn materialize_invocation(
    metadata: &DebMaintainerMetadata,
    mode: &DebMaintainerMode,
    change: &NativeTransactionChange,
    changes: &[NativeTransactionChange],
) -> Result<Vec<String>> {
    let counterpart = change
        .operation
        .counterpart_transaction_index()
        .map(|transaction_index| {
            changes
                .iter()
                .find(|candidate| candidate.transaction_index == transaction_index)
                .with_context(|| {
                    format!(
                        "Debian '{}' invocation for '{}' refers to missing transaction element {transaction_index}",
                        mode.as_str(),
                        change.package_name
                    )
                })
        })
        .transpose()?;
    let conflicting_package = change
        .operation
        .removing_conflict_transaction_index()
        .map(|transaction_index| {
            changes
                .iter()
                .find(|candidate| candidate.transaction_index == transaction_index)
                .with_context(|| {
                    format!(
                        "Debian '{}' invocation for '{}' refers to missing conflicting transaction element {transaction_index}",
                        mode.as_str(),
                        change.package_name
                    )
                })
        })
        .transpose()?;
    for invocation in metadata
        .invocations
        .iter()
        .filter(|invocation| &invocation.mode == mode)
    {
        let mut args = Vec::with_capacity(invocation.arguments.len());
        let mut complete = true;
        for argument in &invocation.arguments {
            let value = match &argument.value {
                DebMaintainerArgumentValue::Action => Some(mode.as_str().to_string()),
                DebMaintainerArgumentValue::OldVersion
                | DebMaintainerArgumentValue::InstalledVersion => change.old_version.clone(),
                DebMaintainerArgumentValue::MostRecentlyConfiguredVersion => {
                    Some(change.old_version.clone().unwrap_or_default())
                }
                DebMaintainerArgumentValue::NewVersion => change
                    .new_version
                    .clone()
                    .or_else(|| counterpart.and_then(|candidate| candidate.new_version.clone())),
                DebMaintainerArgumentValue::Literal => argument.literal.clone(),
                DebMaintainerArgumentValue::PackageName => {
                    counterpart.map(|candidate| candidate.package_name.clone())
                }
                DebMaintainerArgumentValue::InstallingPackageName => {
                    counterpart.map(|candidate| candidate.package_name.clone())
                }
                DebMaintainerArgumentValue::InstallingPackageVersion => {
                    counterpart.and_then(|candidate| candidate.new_version.clone())
                }
                DebMaintainerArgumentValue::ConflictingPackageMarker => {
                    conflicting_package.map(|_| "removing".to_string())
                }
                DebMaintainerArgumentValue::ConflictingPackageName => {
                    conflicting_package.map(|candidate| candidate.package_name.clone())
                }
                DebMaintainerArgumentValue::ConflictingPackageVersion => {
                    conflicting_package.and_then(|candidate| candidate.old_version.clone())
                }
                DebMaintainerArgumentValue::PackageInstanceCount
                | DebMaintainerArgumentValue::TriggerName
                | DebMaintainerArgumentValue::TriggerNames
                | DebMaintainerArgumentValue::TriggerCount
                | DebMaintainerArgumentValue::FilePath => None,
            };
            match value {
                Some(value) => args.push(value),
                None if argument.required => {
                    complete = false;
                    break;
                }
                None => break,
            }
        }
        if complete {
            return Ok(args);
        }
    }

    bail!(
        "Debian maintainer script '{:?}' has no materializable '{}' invocation contract for package change '{}'",
        metadata.control_member,
        mode.as_str(),
        change.package_name
    )
}

pub(super) fn plan(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<DebTransactionPlan> {
    let mut plan = DebTransactionPlan::default();
    triggers::plan(bundles, changes, state, events, &mut plan)?;
    plan_purge_events(bundles, changes, events)?;
    plan_failure_graphs(bundles, changes, events, &mut plan)?;
    refcount::attach(bundles, changes, state, events, &mut plan)?;
    Ok(plan)
}

fn payload_identity(change: &NativeTransactionChange) -> Result<NativePackageIdentity> {
    let package_version = match change.operation {
        NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade => {
            change.new_version.as_ref()
        }
        NativeTransactionOperation::Remove
        | NativeTransactionOperation::Purge
        | NativeTransactionOperation::RemoveInFavour { .. }
        | NativeTransactionOperation::Disappear { .. } => change.old_version.as_ref(),
        NativeTransactionOperation::DeconfigureInFavour { .. } => {
            bail!(
                "state-only deconfiguration for '{}' has no payload identity",
                change.package_name
            )
        }
    }
    .with_context(|| {
        format!(
            "native transaction change for '{}' has no payload owner version",
            change.package_name
        )
    })?;
    Ok(NativePackageIdentity {
        package_name: change.package_name.clone(),
        package_version: package_version.clone(),
        package_arch: change.triggering_arch().map(str::to_string),
    })
}

fn plan_purge_events(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    for change in changes
        .iter()
        .filter(|change| change.operation == NativeTransactionOperation::Purge)
    {
        let Some(view) = bundles.iter().find(|view| {
            view.role == NativeBundleRole::Removing
                && view.package_name == change.package_name
                && view.bundle.source_arch == change.old_arch
                && view.bundle.source_format.as_str() == "deb"
        }) else {
            continue;
        };
        let Some(entry) = view.bundle.entries.iter().find(|entry| {
            entry.deb_maintainer.as_ref().is_some_and(|metadata| {
                metadata.control_member == DebControlMember::Postrm
                    && metadata
                        .invocations
                        .iter()
                        .any(|invocation| invocation.mode == DebMaintainerMode::Purge)
            })
        }) else {
            continue;
        };
        let metadata = entry
            .deb_maintainer
            .as_ref()
            .expect("entry was selected by Debian metadata");
        let args = materialize_invocation(metadata, &DebMaintainerMode::Purge, change, changes)?;
        events.push(entry_event(
            view,
            entry,
            BundleEventSpec {
                stage: NativeEventStage::DebPostRemovePurge,
                args,
                stdin: Vec::new(),
                matched_targets: Vec::new(),
                order_key: format!(
                    "transaction:{:020}:debian-purge:{}",
                    change.transaction_index, entry.id
                ),
                placement: NativeEventPlacement::TransactionElement {
                    transaction_index: change.transaction_index,
                },
            },
        ));
    }
    Ok(())
}

fn plan_failure_graphs(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    events: &[NativeTransactionEvent],
    plan: &mut DebTransactionPlan,
) -> Result<()> {
    for change in changes {
        let has_deb_view = bundles.iter().any(|view| {
            view.package_name == change.package_name
                && match view.role {
                    NativeBundleRole::Installing => view.bundle.source_arch == change.new_arch,
                    NativeBundleRole::Removing | NativeBundleRole::Deconfiguring => {
                        view.bundle.source_arch == change.old_arch
                    }
                    NativeBundleRole::Installed => false,
                }
                && view.bundle.source_format.as_str() == "deb"
        });
        if !has_deb_view {
            continue;
        }

        match change.operation {
            NativeTransactionOperation::Install => {
                let abort_install = append_relation_abort_unwind(
                    recovery_step(
                        recovery_event(
                            bundles,
                            changes,
                            change,
                            NativeBundleRole::Installing,
                            DebControlMember::Postrm,
                            DebMaintainerMode::AbortInstall,
                        )?,
                        DebRecoveryResult::abort(DebPackageState::NotInstalled, false),
                        DebRecoveryResult::abort(DebPackageState::HalfInstalled, false),
                    ),
                    bundles,
                    changes,
                    change.transaction_index,
                    None,
                )?;
                register_event_recovery(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Installing,
                    NativeEventStage::PackagePreInstall,
                    DebMaintainerMode::Install,
                    abort_install.clone(),
                    DebPackageState::HalfInstalled,
                );
                plan.payload_failure_recovery
                    .insert(payload_identity(change)?, abort_install);
                register_terminal_state(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Installing,
                    NativeEventStage::DebPostInstall,
                    DebMaintainerMode::Configure,
                    DebPackageState::HalfConfigured,
                );
            }
            NativeTransactionOperation::Upgrade => {
                let old_postinst_abort = || -> Result<DebRecoveryResult> {
                    Ok(recovery_step(
                        recovery_event(
                            bundles,
                            changes,
                            change,
                            NativeBundleRole::Removing,
                            DebControlMember::Postinst,
                            DebMaintainerMode::AbortUpgrade,
                        )?,
                        DebRecoveryResult::abort(DebPackageState::Installed, false),
                        DebRecoveryResult::abort(DebPackageState::Unpacked, false),
                    ))
                };

                let old_prerm_recovery = recovery_step(
                    recovery_event(
                        bundles,
                        changes,
                        change,
                        NativeBundleRole::Installing,
                        DebControlMember::Prerm,
                        DebMaintainerMode::FailedUpgrade,
                    )?,
                    DebRecoveryResult::continue_transaction(),
                    old_postinst_abort()?,
                );
                register_event_recovery(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Removing,
                    NativeEventStage::DebPreRemoveUpgrade,
                    DebMaintainerMode::Upgrade,
                    old_prerm_recovery,
                    DebPackageState::HalfConfigured,
                );

                let preinst_or_payload_recovery = append_relation_abort_unwind(
                    recovery_step(
                        recovery_event(
                            bundles,
                            changes,
                            change,
                            NativeBundleRole::Installing,
                            DebControlMember::Postrm,
                            DebMaintainerMode::AbortUpgrade,
                        )?,
                        old_postinst_abort()?,
                        DebRecoveryResult::abort(DebPackageState::HalfInstalled, false),
                    ),
                    bundles,
                    changes,
                    change.transaction_index,
                    None,
                )?;
                register_event_recovery(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Installing,
                    NativeEventStage::PackagePreInstall,
                    DebMaintainerMode::Upgrade,
                    preinst_or_payload_recovery.clone(),
                    DebPackageState::HalfInstalled,
                );
                plan.payload_failure_recovery
                    .insert(payload_identity(change)?, preinst_or_payload_recovery);

                let old_postinst_after_payload = recovery_step(
                    recovery_event(
                        bundles,
                        changes,
                        change,
                        NativeBundleRole::Removing,
                        DebControlMember::Postinst,
                        DebMaintainerMode::AbortUpgrade,
                    )?,
                    DebRecoveryResult::abort(DebPackageState::Installed, true),
                    DebRecoveryResult::abort(DebPackageState::Unpacked, true),
                );
                let new_postrm_abort = recovery_step(
                    recovery_event(
                        bundles,
                        changes,
                        change,
                        NativeBundleRole::Installing,
                        DebControlMember::Postrm,
                        DebMaintainerMode::AbortUpgrade,
                    )?,
                    old_postinst_after_payload,
                    DebRecoveryResult::abort(DebPackageState::HalfInstalled, true),
                );
                let old_preinst_abort = recovery_step(
                    recovery_event(
                        bundles,
                        changes,
                        change,
                        NativeBundleRole::Removing,
                        DebControlMember::Preinst,
                        DebMaintainerMode::AbortUpgrade,
                    )?,
                    new_postrm_abort,
                    DebRecoveryResult::abort(DebPackageState::HalfInstalled, true),
                );
                let old_postrm_recovery = append_relation_abort_unwind(
                    recovery_step(
                        recovery_event(
                            bundles,
                            changes,
                            change,
                            NativeBundleRole::Installing,
                            DebControlMember::Postrm,
                            DebMaintainerMode::FailedUpgrade,
                        )?,
                        DebRecoveryResult::continue_transaction(),
                        old_preinst_abort,
                    ),
                    bundles,
                    changes,
                    change.transaction_index,
                    None,
                )?;
                register_event_recovery(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Removing,
                    NativeEventStage::DebPostRemoveUpgrade,
                    DebMaintainerMode::Upgrade,
                    old_postrm_recovery,
                    DebPackageState::HalfInstalled,
                );
                register_terminal_state(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Installing,
                    NativeEventStage::DebPostInstall,
                    DebMaintainerMode::Configure,
                    DebPackageState::HalfConfigured,
                );
            }
            NativeTransactionOperation::Remove
            | NativeTransactionOperation::Purge
            | NativeTransactionOperation::RemoveInFavour { .. } => {
                let abort_remove = append_relation_abort_unwind(
                    recovery_step(
                        recovery_event(
                            bundles,
                            changes,
                            change,
                            NativeBundleRole::Removing,
                            DebControlMember::Postinst,
                            DebMaintainerMode::AbortRemove,
                        )?,
                        DebRecoveryResult::abort(DebPackageState::Installed, false),
                        DebRecoveryResult::abort(DebPackageState::HalfConfigured, false),
                    ),
                    bundles,
                    changes,
                    change
                        .operation
                        .counterpart_transaction_index()
                        .unwrap_or(change.transaction_index),
                    (change.operation.counterpart_transaction_index().is_some())
                        .then_some(change.transaction_index),
                )?;
                register_event_recovery(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Removing,
                    match change.operation {
                        NativeTransactionOperation::RemoveInFavour { .. } => {
                            NativeEventStage::DebPreRemoveInFavour
                        }
                        _ => NativeEventStage::PackagePreRemove,
                    },
                    DebMaintainerMode::Remove,
                    abort_remove.clone(),
                    DebPackageState::HalfConfigured,
                );
                plan.payload_failure_recovery
                    .insert(payload_identity(change)?, abort_remove);
                register_terminal_state(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Removing,
                    NativeEventStage::PackagePostRemove,
                    DebMaintainerMode::Remove,
                    DebPackageState::HalfInstalled,
                );
                if change.operation == NativeTransactionOperation::Purge {
                    register_terminal_state(
                        plan,
                        events,
                        change,
                        NativeBundleRole::Removing,
                        NativeEventStage::DebPostRemovePurge,
                        DebMaintainerMode::Purge,
                        DebPackageState::ConfigFiles,
                    );
                }
            }
            NativeTransactionOperation::Disappear { .. } => {
                register_terminal_state(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Removing,
                    NativeEventStage::DebPostRemoveDisappear,
                    DebMaintainerMode::Disappear,
                    DebPackageState::HalfInstalled,
                );
            }
            NativeTransactionOperation::DeconfigureInFavour {
                installing_transaction_index,
                ..
            } => {
                let abort_deconfigure = append_relation_abort_unwind_with_bounds(
                    recovery_step(
                        recovery_event(
                            bundles,
                            changes,
                            change,
                            NativeBundleRole::Deconfiguring,
                            DebControlMember::Postinst,
                            DebMaintainerMode::AbortDeconfigure,
                        )?,
                        DebRecoveryResult::abort(DebPackageState::Installed, false),
                        DebRecoveryResult::abort(DebPackageState::HalfConfigured, false),
                    ),
                    bundles,
                    changes,
                    installing_transaction_index,
                    None,
                    Some(change.transaction_index),
                    false,
                )?;
                register_event_recovery(
                    plan,
                    events,
                    change,
                    NativeBundleRole::Deconfiguring,
                    NativeEventStage::DebPreDeconfigure,
                    DebMaintainerMode::Deconfigure,
                    abort_deconfigure,
                    DebPackageState::HalfConfigured,
                );
            }
        }
    }
    Ok(())
}

pub(super) fn event_key(event: &NativeTransactionEvent) -> String {
    format!(
        "{}@{}@{}:{}",
        event.owner_package,
        event.owner_version,
        event.owner_arch.as_deref().unwrap_or(""),
        event.order_key
    )
}
