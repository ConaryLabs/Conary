// conary-core/src/ccs/native_transaction.rs

//! Exact transaction-event planning for preserved native package lifecycles.
//!
//! This planner consumes typed package-manager metadata. It never inspects
//! script bodies or command strings to decide whether or when an event runs.

use super::native_lifecycle::{
    LifecyclePath, NativeLifecycleBundle, NativeLifecycleEntry, RpmProgram,
};
use crate::repository::versioning::VersionScheme;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod arch;
mod deb;
mod graph;
mod rpm;

pub use deb::{
    DebPackageState, DebRecoveryDisposition, DebRecoveryNode, DebRecoveryResult,
    DebRecoveryStateTransition, DebTransactionPlan, DebTriggerActivation, DebTriggerBatch,
    DebTriggerUnconfiguration,
};
pub use graph::{
    DebTriggerActivationBoundary, NativeEventPathProjection, NativeTransactionGraph,
    NativeTransactionStep,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTransactionOperation {
    Install,
    Upgrade,
    Remove,
    /// Debian's two-phase complete removal. The ordinary payload is removed
    /// first, then conffiles are purged before `postrm purge`.
    Purge,
    /// A conflicting installed package removed in favour of one exact
    /// installing transaction element.
    RemoveInFavour {
        replacement_transaction_index: usize,
    },
    /// A package whose complete non-conffile payload is overwritten by one
    /// exact installing transaction element.
    Disappear {
        overwriter_transaction_index: usize,
    },
    /// A configured Debian package that must become unpacked before one exact
    /// incoming transaction element, optionally because another exact
    /// conflictor is being removed.
    DeconfigureInFavour {
        installing_transaction_index: usize,
        removing_conflict_transaction_index: Option<usize>,
    },
}

impl NativeTransactionOperation {
    pub const fn removes_package(self) -> bool {
        matches!(
            self,
            Self::Remove | Self::Purge | Self::RemoveInFavour { .. } | Self::Disappear { .. }
        )
    }

    pub const fn counterpart_transaction_index(self) -> Option<usize> {
        match self {
            Self::RemoveInFavour {
                replacement_transaction_index,
            } => Some(replacement_transaction_index),
            Self::Disappear {
                overwriter_transaction_index,
            } => Some(overwriter_transaction_index),
            Self::DeconfigureInFavour {
                installing_transaction_index,
                ..
            } => Some(installing_transaction_index),
            Self::Install | Self::Upgrade | Self::Remove | Self::Purge => None,
        }
    }

    pub const fn removing_conflict_transaction_index(self) -> Option<usize> {
        match self {
            Self::DeconfigureInFavour {
                removing_conflict_transaction_index,
                ..
            } => removing_conflict_transaction_index,
            _ => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Upgrade => "upgrade",
            Self::Remove => "remove",
            Self::Purge => "purge",
            Self::RemoveInFavour { .. } => "remove-in-favour",
            Self::Disappear { .. } => "disappear",
            Self::DeconfigureInFavour { .. } => "deconfigure-in-favour",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTransactionChange {
    pub package_name: String,
    /// Exact native package architecture before this transaction element.
    pub old_arch: Option<String>,
    /// Exact native package architecture after this transaction element.
    pub new_arch: Option<String>,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub operation: NativeTransactionOperation,
    /// Exact package-archive paths owned before this transaction element.
    pub old_paths: BTreeSet<String>,
    /// Exact package-archive paths owned after this transaction element.
    pub new_paths: BTreeSet<String>,
    /// Number of installed instances before the transaction.
    pub instances_before: u32,
    /// Number of installed instances after the transaction.
    pub instances_after: u32,
    /// Source package-manager transaction-element order.
    pub transaction_index: usize,
}

impl NativeTransactionChange {
    pub fn triggering_arch(&self) -> Option<&str> {
        match self.operation {
            NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade => {
                self.new_arch.as_deref()
            }
            NativeTransactionOperation::Remove
            | NativeTransactionOperation::Purge
            | NativeTransactionOperation::RemoveInFavour { .. }
            | NativeTransactionOperation::Disappear { .. }
            | NativeTransactionOperation::DeconfigureInFavour { .. } => self.old_arch.as_deref(),
        }
    }

    /// Paths newly owned by this transaction element.
    pub fn added_paths(&self) -> BTreeSet<String> {
        self.new_paths
            .difference(&self.old_paths)
            .cloned()
            .collect()
    }

    /// Paths whose ownership survives this transaction element.
    pub fn retained_paths(&self) -> BTreeSet<String> {
        self.new_paths
            .intersection(&self.old_paths)
            .cloned()
            .collect()
    }

    /// Paths whose ownership ends with this transaction element.
    pub fn removed_paths(&self) -> BTreeSet<String> {
        self.old_paths
            .difference(&self.new_paths)
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NativeBundleView<'a> {
    pub package_name: &'a str,
    pub package_version: &'a str,
    pub instances_after: u32,
    pub role: NativeBundleRole,
    pub bundle: &'a NativeLifecycleBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBundleRole {
    Installed,
    Installing,
    Removing,
    Deconfiguring,
}

/// One installed package instance in the pre-transaction package snapshot.
///
/// Trigger matching needs package versions and per-package path ownership, not
/// only the flattened final path set. Multiple rows with the same name model
/// multiple installed RPM instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeInstalledPackage {
    pub package_name: String,
    pub package_version: String,
    pub package_arch: Option<String>,
    pub paths: BTreeSet<String>,
}

/// Exact native package identity, including the architecture that
/// distinguishes co-installed multiarch instances.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NativePackageIdentity {
    pub package_name: String,
    pub package_version: String,
    pub package_arch: Option<String>,
}

impl NativePackageIdentity {
    pub fn new(package_name: &str, package_version: &str, package_arch: Option<&str>) -> Self {
        Self {
            package_name: package_name.to_string(),
            package_version: package_version.to_string(),
            package_arch: package_arch.map(str::to_string),
        }
    }
}

/// Exact persisted dpkg state for one native package instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDebPackageState {
    pub package_name: String,
    pub package_version: String,
    pub source_arch: Option<String>,
    pub lifecycle_state: DebPackageState,
    pub pending_triggers: Vec<String>,
    /// Exact package instances whose trigger processing this owner is awaiting.
    pub awaited_packages: Vec<NativePackageIdentity>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeTransactionState {
    /// Exact installed package instances before the transaction.
    pub installed_packages_before: Vec<NativeInstalledPackage>,
    /// Declared capabilities available after the transaction.
    pub installed_capabilities_after: Vec<NativeInstalledCapability>,
    /// Installed archive paths after applying the transaction.
    pub installed_paths_after: BTreeSet<String>,
    /// Persisted dpkg package states entering this transaction.
    pub deb_package_states: Vec<NativeDebPackageState>,
    /// Source transaction elements governed by Debian package-state semantics.
    pub deb_change_indices: BTreeSet<usize>,
}

/// A package or virtual capability available to native lifecycle contracts.
///
/// Versioned capabilities carry their native comparison scheme. A lifecycle
/// requirement is never compared through a different ecosystem's ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeInstalledCapability {
    pub name: String,
    pub version: Option<String>,
    pub version_scheme: VersionScheme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeEventStage {
    ArchPreTransaction,
    RpmPreTransaction,
    RpmPreUnTransaction,
    RpmTransactionFileTriggerUninstall,
    DebPreRemoveUpgrade,
    DebPreDeconfigure,
    DebPreRemoveInFavour,
    RpmSysusers,
    RpmTriggerPreInstall,
    /// Debian package preconfiguration (`config configure`) before `preinst`.
    DebPreConfigure,
    PackagePreInstall,
    RpmFileTriggerInstallHigh,
    DebPostRemoveUpgrade,
    DebPostInstall,
    DebAwaitedTriggerProcessing,
    DebErrorRecovery,
    PackagePostInstall,
    RpmTriggerInstall,
    RpmFileTriggerInstallLow,
    RpmFileTriggerUninstallHigh,
    RpmTriggerUninstall,
    PackagePreRemove,
    RpmFileTriggerUninstallLow,
    RpmFileTriggerPostUninstallHigh,
    PackagePostRemove,
    DebPostRemovePurge,
    DebPostRemoveDisappear,
    RpmTriggerPostUninstall,
    RpmFileTriggerPostUninstallLow,
    RpmPostTransaction,
    RpmPostUnTransaction,
    RpmDatabaseTransactionFileTriggerInstall,
    RpmTransactionFileTriggerPostUninstall,
    RpmTransactionFileTriggerInstall,
    DebTriggerProcessing,
    ArchPostTransaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEventProgram {
    BundleEntry { entry_id: String },
    Command { argv: Vec<String> },
}

/// Which RPM package set supplied a trigger owner.
///
/// RPM executes database-owned and transaction-owned triggers as distinct
/// passes even when they share a lifecycle stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmTriggerOwner {
    InstalledDatabase,
    Transaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTransactionEvent {
    pub owner_package: String,
    pub owner_version: String,
    pub owner_arch: Option<String>,
    pub source_format: String,
    pub stage: NativeEventStage,
    pub program: NativeEventProgram,
    pub args: Vec<String>,
    pub stdin: Vec<u8>,
    pub matched_targets: Vec<String>,
    pub rpm_trigger_owner: Option<RpmTriggerOwner>,
    /// Exact dpkg package reference count for Debian maintainer invocations.
    pub deb_package_refcount: Option<u32>,
    pub order_key: String,
    /// Exact transaction boundary that owns this event.
    pub placement: NativeEventPlacement,
}

struct BundleEventSpec {
    stage: NativeEventStage,
    args: Vec<String>,
    stdin: Vec<u8>,
    matched_targets: Vec<String>,
    order_key: String,
    placement: NativeEventPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEventPlacement {
    TransactionBefore,
    TransactionElement { transaction_index: usize },
    TransactionAfter,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeTransactionPlan {
    pub events: Vec<NativeTransactionEvent>,
    pub graph: NativeTransactionGraph,
    pub deb: DebTransactionPlan,
}

impl NativeTransactionPlan {
    pub fn events_at(
        &self,
        stage: NativeEventStage,
    ) -> impl Iterator<Item = &NativeTransactionEvent> {
        self.events.iter().filter(move |event| event.stage == stage)
    }
}

pub fn plan_native_transaction(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
) -> Result<NativeTransactionPlan> {
    validate_transaction_changes(changes)?;
    let mut events = Vec::new();
    for view in bundles {
        view.bundle.validate().with_context(|| {
            format!(
                "native lifecycle bundle for {} {} is invalid",
                view.package_name, view.package_version
            )
        })?;
        plan_regular_lifecycle_events(view, changes, &mut events)?;
        rpm::plan(view, changes, state, &mut events)?;
    }
    arch::plan(bundles, changes, state, &mut events)?;
    let deb = deb::plan(bundles, changes, state, &mut events)?;
    let graph = graph::build(&mut events, changes)?;
    Ok(NativeTransactionPlan { events, graph, deb })
}

fn plan_regular_lifecycle_events(
    view: &NativeBundleView<'_>,
    changes: &[NativeTransactionChange],
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    if view.role == NativeBundleRole::Installed {
        return Ok(());
    }
    let change = changes
        .iter()
        .find(|change| {
            change.package_name == view.package_name
                && match view.role {
                    NativeBundleRole::Installing => change.new_arch == view.bundle.source_arch,
                    NativeBundleRole::Removing | NativeBundleRole::Deconfiguring => {
                        change.old_arch == view.bundle.source_arch
                    }
                    NativeBundleRole::Installed => false,
                }
        })
        .with_context(|| {
            format!(
                "native lifecycle owner '{}' has no transaction change",
                view.package_name
            )
        })?;

    for (index, entry) in view.bundle.entries.iter().enumerate() {
        if entry.rpm_trigger.is_some() || entry.rpm_sysusers.is_some() || entry.arch_hook.is_some()
        {
            continue;
        }
        let Some((stage, args)) = regular_entry_invocation(view, change, changes, entry)? else {
            continue;
        };
        match view.bundle.source_format.as_str() {
            "rpm" => {
                ensure_rpm_program_available(entry)?;
                rpm_runtime(entry)?;
            }
            "arch" | "deb" => {}
            other => bail!(
                "native lifecycle bundle for '{}' has unsupported source format '{other}'",
                view.package_name
            ),
        };
        let placement = match stage {
            NativeEventStage::RpmPreTransaction | NativeEventStage::RpmPreUnTransaction => {
                NativeEventPlacement::TransactionBefore
            }
            NativeEventStage::RpmPostTransaction | NativeEventStage::RpmPostUnTransaction => {
                NativeEventPlacement::TransactionAfter
            }
            _ => NativeEventPlacement::TransactionElement {
                transaction_index: deb::regular_event_transaction_index(change, stage),
            },
        };
        events.push(entry_event(
            view,
            entry,
            BundleEventSpec {
                stage,
                args,
                stdin: Vec::new(),
                matched_targets: Vec::new(),
                order_key: format!(
                    "transaction:{:020}:lifecycle:{index:08}:{}",
                    change.transaction_index, entry.id
                ),
                placement,
            },
        ));
    }
    Ok(())
}

fn validate_transaction_changes(changes: &[NativeTransactionChange]) -> Result<()> {
    let mut transaction_indices = BTreeSet::new();
    for change in changes {
        if !transaction_indices.insert(change.transaction_index) {
            bail!(
                "native transaction repeats source element index {}",
                change.transaction_index
            );
        }
        match change.operation {
            NativeTransactionOperation::Install => {
                if change.old_version.is_some()
                    || change.old_arch.is_some()
                    || !change.old_paths.is_empty()
                {
                    bail!(
                        "install transaction change for '{}' carries pre-existing identity or paths",
                        change.package_name
                    );
                }
                if change.new_version.is_none() {
                    bail!(
                        "install transaction change for '{}' has no new version",
                        change.package_name
                    );
                }
                let expected_after = change
                    .instances_before
                    .checked_add(1)
                    .context("install transaction instance count overflow")?;
                if change.instances_after != expected_after {
                    bail!(
                        "install transaction change for '{}' has instance transition {} -> {}, expected {}",
                        change.package_name,
                        change.instances_before,
                        change.instances_after,
                        expected_after
                    );
                }
            }
            NativeTransactionOperation::Upgrade => {
                if change.old_version.is_none() || change.new_version.is_none() {
                    bail!(
                        "upgrade transaction change for '{}' lacks an old or new version",
                        change.package_name
                    );
                }
                if change.instances_after != change.instances_before {
                    bail!(
                        "upgrade transaction change for '{}' changes instance count {} -> {}",
                        change.package_name,
                        change.instances_before,
                        change.instances_after
                    );
                }
            }
            NativeTransactionOperation::DeconfigureInFavour {
                installing_transaction_index,
                removing_conflict_transaction_index,
            } => {
                if change.old_version.is_none()
                    || change.new_version != change.old_version
                    || change.new_arch != change.old_arch
                    || change.new_paths != change.old_paths
                    || change.instances_after != change.instances_before
                {
                    bail!(
                        "deconfigure transaction change for '{}' must preserve exact package identity, payload, and instance count",
                        change.package_name
                    );
                }
                let installing = changes
                    .iter()
                    .find(|candidate| {
                        candidate.transaction_index == installing_transaction_index
                    })
                    .with_context(|| {
                        format!(
                            "deconfigure transaction change for '{}' refers to missing installing transaction element {installing_transaction_index}",
                            change.package_name
                        )
                    })?;
                if !matches!(
                    installing.operation,
                    NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade
                ) {
                    bail!(
                        "deconfigure transaction change for '{}' refers to non-installing transaction element {installing_transaction_index}",
                        change.package_name
                    );
                }
                if let Some(conflict_index) = removing_conflict_transaction_index {
                    let conflict = changes
                        .iter()
                        .find(|candidate| candidate.transaction_index == conflict_index)
                        .with_context(|| {
                            format!(
                                "deconfigure transaction change for '{}' refers to missing conflict removal transaction element {conflict_index}",
                                change.package_name
                            )
                        })?;
                    if !matches!(
                        conflict.operation,
                        NativeTransactionOperation::RemoveInFavour {
                            replacement_transaction_index
                        } if replacement_transaction_index == installing_transaction_index
                    ) {
                        bail!(
                            "deconfigure transaction change for '{}' refers to a removal that is not in favour of transaction element {installing_transaction_index}",
                            change.package_name
                        );
                    }
                }
            }
            operation @ (NativeTransactionOperation::Remove
            | NativeTransactionOperation::Purge
            | NativeTransactionOperation::RemoveInFavour { .. }
            | NativeTransactionOperation::Disappear { .. }) => {
                if change.new_version.is_some()
                    || change.new_arch.is_some()
                    || !change.new_paths.is_empty()
                {
                    bail!(
                        "remove transaction change for '{}' carries a new identity or paths",
                        change.package_name
                    );
                }
                if change.old_version.is_none() {
                    bail!(
                        "remove transaction change for '{}' has no old version",
                        change.package_name
                    );
                }
                let expected_after = change.instances_before.checked_sub(1).with_context(|| {
                    format!(
                        "remove transaction change for '{}' starts with zero instances",
                        change.package_name
                    )
                })?;
                if change.instances_after != expected_after {
                    bail!(
                        "remove transaction change for '{}' has instance transition {} -> {}, expected {}",
                        change.package_name,
                        change.instances_before,
                        change.instances_after,
                        expected_after
                    );
                }
                if let Some(counterpart_index) = operation.counterpart_transaction_index() {
                    let counterpart = changes
                        .iter()
                        .find(|candidate| candidate.transaction_index == counterpart_index)
                        .with_context(|| {
                            format!(
                                "{} transaction change for '{}' refers to missing transaction element {counterpart_index}",
                                operation.label(),
                                change.package_name
                            )
                        })?;
                    if !matches!(
                        counterpart.operation,
                        NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade
                    ) {
                        bail!(
                            "{} transaction change for '{}' refers to non-installing transaction element {counterpart_index}",
                            operation.label(),
                            change.package_name
                        );
                    }
                    if counterpart.package_name == change.package_name {
                        bail!(
                            "{} transaction change for '{}' refers to itself as the replacement package",
                            operation.label(),
                            change.package_name
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn regular_entry_invocation(
    view: &NativeBundleView<'_>,
    change: &NativeTransactionChange,
    changes: &[NativeTransactionChange],
    entry: &NativeLifecycleEntry,
) -> Result<Option<(NativeEventStage, Vec<String>)>> {
    match view.bundle.source_format.as_str() {
        "rpm" => rpm_regular_invocation(view, change, entry),
        "deb" => entry.deb_maintainer.as_ref().map_or(Ok(None), |metadata| {
            deb::regular_invocation(view, change, changes, metadata)
        }),
        "arch" => arch::regular_invocation(view, change, entry),
        other => bail!(
            "native lifecycle bundle for '{}' has unsupported source format '{other}'",
            view.package_name
        ),
    }
}

fn rpm_regular_invocation(
    view: &NativeBundleView<'_>,
    change: &NativeTransactionChange,
    entry: &NativeLifecycleEntry,
) -> Result<Option<(NativeEventStage, Vec<String>)>> {
    let stage = match (view.role, &entry.phase) {
        (NativeBundleRole::Installing, LifecyclePath::PreTransaction) => {
            NativeEventStage::RpmPreTransaction
        }
        (NativeBundleRole::Removing, LifecyclePath::PreUntransaction) => {
            NativeEventStage::RpmPreUnTransaction
        }
        (NativeBundleRole::Installing, LifecyclePath::PreInstall | LifecyclePath::PreUpgrade) => {
            NativeEventStage::PackagePreInstall
        }
        (NativeBundleRole::Installing, LifecyclePath::PostInstall | LifecyclePath::PostUpgrade) => {
            NativeEventStage::PackagePostInstall
        }
        (NativeBundleRole::Removing, LifecyclePath::PreRemove) => {
            NativeEventStage::PackagePreRemove
        }
        (NativeBundleRole::Removing, LifecyclePath::PostRemove) => {
            NativeEventStage::PackagePostRemove
        }
        (NativeBundleRole::Installing, LifecyclePath::PostTransaction) => {
            NativeEventStage::RpmPostTransaction
        }
        (NativeBundleRole::Removing, LifecyclePath::PostUntransaction) => {
            NativeEventStage::RpmPostUnTransaction
        }
        _ => return Ok(None),
    };
    let instance_count = match view.role {
        NativeBundleRole::Installing => change
            .instances_before
            .checked_add(1)
            .context("RPM install-side instance count overflow")?,
        NativeBundleRole::Removing => change.instances_after,
        NativeBundleRole::Installed | NativeBundleRole::Deconfiguring => {
            bail!("installed RPM bundle reached regular lifecycle planning")
        }
    };
    Ok(Some((stage, vec![instance_count.to_string()])))
}

fn rpm_runtime(
    entry: &NativeLifecycleEntry,
) -> Result<&super::native_lifecycle::RpmRuntimeMetadata> {
    entry.rpm_runtime.as_ref().with_context(|| {
        format!(
            "RPM lifecycle entry '{}' has no typed runtime metadata",
            entry.id
        )
    })
}

fn ensure_rpm_program_available(entry: &NativeLifecycleEntry) -> Result<()> {
    let runtime = rpm_runtime(entry)?;
    match runtime.program {
        RpmProgram::External | RpmProgram::EmbeddedLua => Ok(()),
        RpmProgram::Sysusers => bail!(
            "RPM sysusers entry '{}' reached the regular scriptlet planner",
            entry.id
        ),
    }
}

fn required_change_version(
    version: Option<&str>,
    label: &str,
    change: &NativeTransactionChange,
) -> Result<String> {
    version.map(str::to_string).with_context(|| {
        format!(
            "{} transaction change for '{}' has no {label} version",
            match change.operation {
                NativeTransactionOperation::Install => "install",
                NativeTransactionOperation::Upgrade => "upgrade",
                NativeTransactionOperation::DeconfigureInFavour { .. } => "deconfigure",
                operation @ (NativeTransactionOperation::Remove
                | NativeTransactionOperation::Purge
                | NativeTransactionOperation::RemoveInFavour { .. }
                | NativeTransactionOperation::Disappear { .. }) => operation.label(),
            },
            change.package_name
        )
    })
}

fn entry_event(
    view: &NativeBundleView<'_>,
    entry: &NativeLifecycleEntry,
    spec: BundleEventSpec,
) -> NativeTransactionEvent {
    NativeTransactionEvent {
        owner_package: view.package_name.to_string(),
        owner_version: view.package_version.to_string(),
        owner_arch: view.bundle.source_arch.clone(),
        source_format: view.bundle.source_format.as_str().to_string(),
        stage: spec.stage,
        program: NativeEventProgram::BundleEntry {
            entry_id: entry.id.clone(),
        },
        args: spec.args,
        stdin: spec.stdin,
        matched_targets: spec.matched_targets,
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: spec.order_key,
        placement: spec.placement,
    }
}

fn lines_stdin(lines: &[String]) -> Vec<u8> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut value = lines.join("\n").into_bytes();
    value.push(b'\n');
    value
}

#[cfg(test)]
use rpm::rpm_stage;
#[cfg(test)]
mod rpm_tests;
#[cfg(test)]
mod tests;
