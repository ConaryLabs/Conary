// apps/conary/src/commands/install/native_events.rs

//! Install-side execution of exact native package-manager transaction events.

use anyhow::{Context, Result};
use conary_core::ccs::native_lifecycle::NativeLifecycleBundle;
use conary_core::ccs::native_transaction::{
    DebPackageState, NativeBundleRole, NativeBundleView, NativeInstalledCapability,
    NativePackageIdentity, NativeTransactionChange, NativeTransactionOperation,
    NativeTransactionPathCapabilities, NativeTransactionPlan, NativeTransactionState,
    plan_native_transaction,
};
use conary_core::db::models::{
    ConfigFile, InstalledNativeLifecycleBundle, PackagePayloadOwnership, Trove,
};
use conary_core::repository::dependency_model::PackageRelationRemovalMode;
use conary_core::repository::versioning::VersionScheme;
use conary_core::scriptlet::{SandboxMode, ScriptletExecutor};
use conary_core::transaction::{
    PackageRelationDeconfiguration, PackageRelationDeconfigurationCause, PackageRelationRemoval,
    validate_package_relation_transitions,
};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::Path;

#[path = "native_events/deb_state.rs"]
mod deb_state;
#[path = "native_events/debian_runtime.rs"]
mod debian_runtime;
#[path = "native_events/execution.rs"]
mod execution;
#[path = "native_events/graph_execution.rs"]
mod graph_execution;
#[path = "native_events/preflight.rs"]
mod preflight;
#[path = "native_events/runtime.rs"]
mod runtime;
#[path = "native_events/transaction_state.rs"]
mod transaction_state;

use preflight::NativePathProjection;
use transaction_state::{
    declared_capabilities_excluding, declared_path_capabilities_for_trove,
    installed_instance_count, installed_packages_before, path_capabilities,
    sort_and_deduplicate_capabilities,
};

pub(super) struct NativeInstallInput<'a> {
    pub package_name: &'a str,
    pub package_version: &'a str,
    pub package_arch: Option<&'a str>,
    pub version_scheme: VersionScheme,
    pub provides: &'a [conary_core::repository::dependency_model::ProvidedCapability],
    pub new_bundle: Option<&'a NativeLifecycleBundle>,
    pub old_trove: Option<&'a Trove>,
    pub relation_removals: &'a [PackageRelationRemoval],
    pub relation_deconfigurations: &'a [PackageRelationDeconfiguration],
    pub paths: Vec<String>,
}

pub(crate) struct NativeRemoveInput<'a> {
    pub trove: &'a Trove,
    pub paths: Vec<String>,
    pub operation: NativeTransactionOperation,
}

#[derive(Debug)]
struct NativeBundleOwner {
    package_name: String,
    package_version: String,
    instances_after: u32,
    role: NativeBundleRole,
    initial_package_state: DebPackageState,
    initial_pending_triggers: Vec<String>,
    initial_awaited_packages: Vec<NativePackageIdentity>,
    bundle: NativeLifecycleBundle,
}

#[derive(Debug)]
pub(crate) struct DebLifecycleFailure {
    pub(crate) package_name: String,
    pub(crate) package_state: DebPackageState,
    primary_error: String,
}

impl fmt::Display for DebLifecycleFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Debian lifecycle for '{}' stopped in state '{}': {}",
            self.package_name,
            self.package_state.as_str(),
            self.primary_error
        )
    }
}

impl std::error::Error for DebLifecycleFailure {}

#[derive(Debug, Default)]
pub(crate) struct PreparedNativeTransaction {
    owners: Vec<NativeBundleOwner>,
    plan: NativeTransactionPlan,
    changes: Vec<NativeTransactionChange>,
    transaction_state: NativeTransactionState,
    debian_config_before: Vec<debian_runtime::DebianConfigSnapshot>,
    operations: Vec<NativeTransactionOperation>,
    requires_upgrade_payload_boundary: bool,
    arch_transaction: bool,
    arch_ldconfig_required_after: bool,
    host_capabilities: Option<conary_core::ccs::HostCapabilityInventory>,
    path_projection: NativePathProjection,
    activation_invocations: RefCell<Vec<CapturedNativeActivation>>,
    continued_lifecycle_failures: RefCell<Vec<ContinuedLifecycleFailure>>,
}

/// A lifecycle entry that failed in a warn-and-continue class.
///
/// The source format proceeds past this failure, so the transaction does too,
/// but the failure stays typed evidence rather than becoming a silent success.
#[derive(Debug, Clone)]
pub(crate) struct ContinuedLifecycleFailure {
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) entry: String,
    pub(crate) failure: conary_core::scriptlet::ScriptletFailureOutcome,
}

#[derive(Debug, Clone)]
pub(crate) struct CapturedNativeActivation {
    pub(crate) source_package: String,
    pub(crate) source_version: String,
    pub(crate) source_entry: String,
    pub(crate) invocation: conary_core::activation::RuntimeActivationInvocation,
}

impl PreparedNativeTransaction {
    pub(super) fn prepare_install(
        conn: &rusqlite::Connection,
        input: NativeInstallInput<'_>,
    ) -> Result<Self> {
        Self::prepare_batch(conn, &[input])
    }

    pub(super) fn prepare_batch(
        conn: &rusqlite::Connection,
        inputs: &[NativeInstallInput<'_>],
    ) -> Result<Self> {
        let relation_removals = inputs
            .iter()
            .flat_map(|input| input.relation_removals.iter().cloned())
            .collect::<Vec<_>>();
        let relation_deconfigurations = inputs
            .iter()
            .flat_map(|input| input.relation_deconfigurations.iter().cloned())
            .collect::<Vec<_>>();
        validate_package_relation_transitions(conn, &relation_removals, &relation_deconfigurations)
            .context("Native transaction relation preflight failed")?;
        let mut relation_removal_troves = Vec::new();
        for removal in &relation_removals {
            let trove = Trove::find_by_id(conn, removal.trove_id)?.with_context(|| {
                format!(
                    "relation removal target {} {} disappeared",
                    removal.package_name, removal.package_version
                )
            })?;
            relation_removal_troves.push((removal.clone(), trove));
        }
        let mut relation_deconfiguration_troves = Vec::new();
        for deconfiguration in &relation_deconfigurations {
            let trove =
                Trove::find_by_id(conn, deconfiguration.package.trove_id)?.with_context(|| {
                    format!(
                        "relation deconfiguration target {} {} disappeared",
                        deconfiguration.package.package_name,
                        deconfiguration.package.package_version
                    )
                })?;
            relation_deconfiguration_troves.push((deconfiguration.clone(), trove));
        }
        let relation_removal_has_arch = relation_removal_troves
            .iter()
            .any(|(_, trove)| trove.version_scheme == VersionScheme::Arch);
        let arch_transaction = inputs
            .iter()
            .any(|input| input.version_scheme == VersionScheme::Arch)
            || relation_removal_has_arch;
        let input_old_trove_ids = inputs
            .iter()
            .map(|input| {
                input
                    .old_trove
                    .map(|trove| {
                        trove.id.with_context(|| {
                            format!(
                                "installed package '{}-{}' selected for native replacement has no persisted trove ID",
                                trove.name, trove.version
                            )
                        })
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?;
        let mut replaced_trove_ids = input_old_trove_ids
            .iter()
            .flatten()
            .copied()
            .collect::<HashSet<_>>();
        replaced_trove_ids.extend(
            relation_removal_troves
                .iter()
                .map(|(removal, _)| removal.trove_id),
        );
        let installed_bundles = InstalledNativeLifecycleBundle::find_all(conn)?;
        let installed_bundle_trove_ids = installed_bundles
            .iter()
            .map(|installed| installed.trove_id)
            .collect::<HashSet<_>>();
        let requires_upgrade_payload_boundary =
            inputs
                .iter()
                .zip(&input_old_trove_ids)
                .any(|(input, old_trove_id)| {
                    old_trove_id.is_some()
                        && (input.new_bundle.is_some()
                            || old_trove_id
                                .is_some_and(|id| installed_bundle_trove_ids.contains(&id)))
                })
                || !relation_removal_troves.is_empty()
                || !relation_deconfiguration_troves.is_empty();
        let mut counts_after = HashMap::<String, u32>::new();
        let mut input_instance_counts = Vec::with_capacity(inputs.len());
        let mut changes = Vec::with_capacity(
            inputs.len() + relation_removal_troves.len() + relation_deconfiguration_troves.len(),
        );
        let mut path_capability_changes = Vec::with_capacity(
            inputs.len() + relation_removal_troves.len() + relation_deconfiguration_troves.len(),
        );
        let mut deb_change_indices = BTreeSet::new();
        for (transaction_index, input) in inputs.iter().enumerate() {
            let old_trove_id = input_old_trove_ids[transaction_index];
            if input.version_scheme == VersionScheme::Debian {
                deb_change_indices.insert(transaction_index);
            }
            let instances_before = counts_after
                .get(input.package_name)
                .copied()
                .unwrap_or(installed_instance_count(conn, input.package_name)?);
            let instances_after = if input.old_trove.is_some() {
                if instances_before == 0 {
                    anyhow::bail!(
                        "upgrade transaction for '{}' has no installed instance",
                        input.package_name
                    );
                }
                instances_before
            } else {
                instances_before
                    .checked_add(1)
                    .context("installed package instance count overflow")?
            };
            counts_after.insert(input.package_name.to_string(), instances_after);
            input_instance_counts.push((instances_before, instances_after));
            let old_paths = old_trove_id
                .map(|trove_id| {
                    Ok::<_, anyhow::Error>(
                        PackagePayloadOwnership::load(conn, trove_id)?
                            .lifecycle_paths()
                            .iter()
                            .cloned()
                            .collect::<BTreeSet<_>>(),
                    )
                })
                .transpose()?
                .unwrap_or_default();
            let installed_source_arch = old_trove_id
                .and_then(|trove_id| {
                    installed_bundles
                        .iter()
                        .find(|installed| installed.trove_id == trove_id)
                })
                .and_then(|installed| installed.source_arch.clone());
            let old_arch = installed_source_arch
                .or_else(|| input.old_trove.and_then(|trove| trove.architecture.clone()));
            let new_arch = input
                .new_bundle
                .and_then(|bundle| bundle.source_arch.clone())
                .or_else(|| input.package_arch.map(str::to_string));
            let old_path_capabilities = old_trove_id
                .map(|trove_id| declared_path_capabilities_for_trove(conn, trove_id))
                .transpose()?
                .unwrap_or_default();
            let new_path_capabilities =
                path_capabilities(input.provides.iter().map(|provide| provide.name.as_str()));
            changes.push(NativeTransactionChange {
                package_name: input.package_name.to_string(),
                old_arch,
                new_arch,
                old_version: input.old_trove.map(|trove| trove.version.clone()),
                new_version: Some(input.package_version.to_string()),
                operation: if input.old_trove.is_some() {
                    NativeTransactionOperation::Upgrade
                } else {
                    NativeTransactionOperation::Install
                },
                old_paths,
                new_paths: input.paths.iter().cloned().collect(),
                instances_before,
                instances_after,
                transaction_index,
            });
            path_capability_changes.push(NativeTransactionPathCapabilities {
                old_paths: old_path_capabilities,
                new_paths: new_path_capabilities,
            });
        }
        for (offset, (removal, trove)) in relation_removal_troves.iter().enumerate() {
            let trove_id = removal.trove_id;
            let transaction_index = inputs.len() + offset;
            if trove.version_scheme == VersionScheme::Debian {
                deb_change_indices.insert(transaction_index);
            }
            let instances_before = counts_after
                .get(trove.name.as_str())
                .copied()
                .unwrap_or(installed_instance_count(conn, &trove.name)?);
            let instances_after = instances_before.checked_sub(1).with_context(|| {
                format!(
                    "removal transaction for '{}' has no installed instance",
                    trove.name
                )
            })?;
            counts_after.insert(trove.name.clone(), instances_after);
            let old_paths = PackagePayloadOwnership::load(conn, trove_id)?
                .lifecycle_paths()
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let operation = if trove.version_scheme == VersionScheme::Debian {
                debian_relation_removal_operation(conn, removal, trove_id, inputs, &old_paths)?
            } else {
                NativeTransactionOperation::Remove
            };
            let old_path_capabilities = declared_path_capabilities_for_trove(conn, trove_id)?;
            changes.push(NativeTransactionChange {
                package_name: trove.name.clone(),
                old_arch: installed_bundles
                    .iter()
                    .find(|installed| installed.trove_id == trove_id)
                    .and_then(|installed| installed.source_arch.clone())
                    .or_else(|| trove.architecture.clone()),
                new_arch: None,
                old_version: Some(trove.version.clone()),
                new_version: None,
                operation,
                old_paths,
                new_paths: BTreeSet::new(),
                instances_before,
                instances_after,
                transaction_index,
            });
            path_capability_changes.push(NativeTransactionPathCapabilities {
                old_paths: old_path_capabilities,
                new_paths: BTreeSet::new(),
            });
        }
        let deconfigured_trove_ids = relation_deconfiguration_troves
            .iter()
            .map(|(deconfiguration, _)| deconfiguration.package.trove_id)
            .collect::<HashSet<_>>();
        for (offset, (deconfiguration, trove)) in relation_deconfiguration_troves.iter().enumerate()
        {
            let trove_id = deconfiguration.package.trove_id;
            let transaction_index = inputs.len() + relation_removal_troves.len() + offset;
            let triggering_index = deconfiguration.triggering_incoming.transaction_index;
            let triggering_input = inputs.get(triggering_index).with_context(|| {
                format!(
                    "relation deconfiguration for {} {} refers to missing incoming transaction element {triggering_index}",
                    trove.name, trove.version
                )
            })?;
            if triggering_input.package_name != deconfiguration.triggering_incoming.package_name
                || triggering_input.package_version
                    != deconfiguration.triggering_incoming.package_version
                || triggering_input.package_arch.map(str::to_string)
                    != deconfiguration.triggering_incoming.package_architecture
            {
                anyhow::bail!(
                    "relation deconfiguration for {} {} refers to a changed incoming transaction identity",
                    trove.name,
                    trove.version
                );
            }
            let installed_bundle = installed_bundles
                .iter()
                .find(|installed| installed.trove_id == trove_id)
                .with_context(|| {
                    format!(
                        "relation deconfiguration for {} {} has no installed native lifecycle contract",
                        trove.name, trove.version
                    )
                })?;
            let bundle = installed_bundle.bundle().with_context(|| {
                format!(
                    "installed native lifecycle bundle for {} {} is malformed",
                    trove.name, trove.version
                )
            })?;
            if bundle.source_format.as_str() != "deb" {
                anyhow::bail!(
                    "relation deconfiguration for {} {} requires Debian's deconfigure ABI, but its installed native lifecycle contract is '{}'",
                    trove.name,
                    trove.version,
                    bundle.source_format.as_str()
                );
            }
            let removing_conflict_transaction_index = match &deconfiguration.cause {
                PackageRelationDeconfigurationCause::Breaks { .. } => None,
                PackageRelationDeconfigurationCause::Dependency {
                    removing_conflict,
                    ..
                } => removing_conflict
                    .as_ref()
                    .map(|conflict| {
                        relation_removal_troves
                            .iter()
                            .position(|(removal, _)| removal.trove_id == conflict.trove_id)
                            .map(|index| inputs.len() + index)
                            .with_context(|| {
                                format!(
                                    "relation deconfiguration for {} {} refers to missing conflict removal {} {}",
                                    trove.name,
                                    trove.version,
                                    conflict.package_name,
                                    conflict.package_version
                                )
                            })
                    })
                    .transpose()?,
            };
            let instances = counts_after
                .get(trove.name.as_str())
                .copied()
                .unwrap_or(installed_instance_count(conn, &trove.name)?);
            let old_paths = PackagePayloadOwnership::load(conn, trove_id)?
                .lifecycle_paths()
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let source_arch = installed_bundle
                .source_arch
                .clone()
                .or_else(|| trove.architecture.clone());
            let retained_path_capabilities = declared_path_capabilities_for_trove(conn, trove_id)?;
            deb_change_indices.insert(transaction_index);
            changes.push(NativeTransactionChange {
                package_name: trove.name.clone(),
                old_arch: source_arch.clone(),
                new_arch: source_arch,
                old_version: Some(trove.version.clone()),
                new_version: Some(trove.version.clone()),
                operation: NativeTransactionOperation::DeconfigureInFavour {
                    installing_transaction_index: triggering_index,
                    removing_conflict_transaction_index,
                },
                old_paths: old_paths.clone(),
                new_paths: old_paths,
                instances_before: instances,
                instances_after: instances,
                transaction_index,
            });
            path_capability_changes.push(NativeTransactionPathCapabilities {
                old_paths: retained_path_capabilities.clone(),
                new_paths: retained_path_capabilities,
            });
        }

        let mut owners = Vec::new();
        for installed in installed_bundles {
            let bundle = installed.bundle().with_context(|| {
                format!(
                    "installed native lifecycle bundle for {} is malformed",
                    installed.source_package
                )
            })?;
            let instances_after = counts_after
                .get(installed.source_package.as_str())
                .copied()
                .unwrap_or(installed_instance_count(conn, &installed.source_package)?);
            owners.push(NativeBundleOwner {
                instances_after,
                package_name: installed.source_package,
                package_version: installed.source_version,
                role: if replaced_trove_ids.contains(&installed.trove_id) {
                    NativeBundleRole::Removing
                } else if deconfigured_trove_ids.contains(&installed.trove_id) {
                    NativeBundleRole::Deconfiguring
                } else {
                    NativeBundleRole::Installed
                },
                initial_package_state: installed.lifecycle_state,
                initial_pending_triggers: installed.pending_triggers,
                initial_awaited_packages: installed.awaited_packages,
                bundle,
            });
        }

        for (input, (_, instances_after)) in inputs.iter().zip(input_instance_counts) {
            if let Some(bundle) = input.new_bundle {
                owners.push(NativeBundleOwner {
                    package_name: input.package_name.to_string(),
                    package_version: input.package_version.to_string(),
                    instances_after,
                    role: NativeBundleRole::Installing,
                    initial_package_state: DebPackageState::NotInstalled,
                    initial_pending_triggers: Vec::new(),
                    initial_awaited_packages: Vec::new(),
                    bundle: bundle.clone(),
                });
            }
        }

        let views = owners
            .iter()
            .map(|owner| NativeBundleView {
                package_name: &owner.package_name,
                package_version: &owner.package_version,
                instances_after: owner.instances_after,
                role: owner.role,
                bundle: &owner.bundle,
            })
            .collect::<Vec<_>>();
        let mut unavailable_capability_trove_ids = replaced_trove_ids.clone();
        unavailable_capability_trove_ids.extend(deconfigured_trove_ids.iter().copied());
        let mut installed_capabilities_after =
            declared_capabilities_excluding(conn, &unavailable_capability_trove_ids)?;
        for input in inputs {
            installed_capabilities_after.push(NativeInstalledCapability {
                name: input.package_name.to_string(),
                version: Some(input.package_version.to_string()),
                version_scheme: input.version_scheme,
            });
            installed_capabilities_after.extend(input.provides.iter().map(|provide| {
                NativeInstalledCapability {
                    name: provide.name.clone(),
                    version: provide.version.clone(),
                    version_scheme: input.version_scheme,
                }
            }));
        }
        sort_and_deduplicate_capabilities(&mut installed_capabilities_after);
        let installed_path_capabilities_after = path_capabilities(
            installed_capabilities_after
                .iter()
                .map(|capability| capability.name.as_str()),
        );
        let mut installed_paths_after =
            PackagePayloadOwnership::installed_paths_excluding(conn, &replaced_trove_ids)?;
        for input in inputs {
            installed_paths_after.extend(input.paths.iter().cloned());
        }
        let arch_ldconfig_required_after =
            archive_paths_contain(&installed_paths_after, "etc/ld.so.conf");
        let transaction_state = NativeTransactionState {
            installed_packages_before: installed_packages_before(conn)?,
            installed_capabilities_after,
            installed_paths_after: installed_paths_after.clone(),
            deb_package_states: deb_state::transaction_package_states(conn, &owners)?,
            deb_change_indices,
        };
        let plan = plan_native_transaction(&views, &changes, &transaction_state)?;
        let host_capabilities = host_capabilities_for_plan(conn, arch_transaction, &plan)?;
        let path_projection = NativePathProjection::from_transaction(
            &plan,
            &changes,
            &installed_paths_after,
            &path_capability_changes,
            &installed_path_capabilities_after,
        )?;
        Ok(Self {
            owners,
            plan,
            changes: changes.clone(),
            transaction_state,
            debian_config_before: debian_runtime::config_snapshots(conn)?,
            operations: changes.iter().map(|change| change.operation).collect(),
            requires_upgrade_payload_boundary,
            arch_transaction,
            arch_ldconfig_required_after,
            host_capabilities,
            path_projection,
            activation_invocations: RefCell::new(Vec::new()),
            continued_lifecycle_failures: RefCell::new(Vec::new()),
        })
    }

    pub(crate) fn prepare_remove(
        conn: &rusqlite::Connection,
        trove_id: i64,
        package_name: &str,
        package_version: &str,
        paths: Vec<String>,
        purge_config_files: bool,
    ) -> Result<Self> {
        let removed_trove = Trove::find_by_id(conn, trove_id)?
            .with_context(|| format!("installed trove {trove_id} disappeared"))?;
        if removed_trove.name != package_name || removed_trove.version != package_version {
            anyhow::bail!(
                "installed trove {trove_id} identity changed during removal preparation: expected {package_name} {package_version}, found {} {}",
                removed_trove.name,
                removed_trove.version
            );
        }
        let version_scheme = removed_trove.version_scheme;
        let operation = if purge_config_files && version_scheme == VersionScheme::Debian {
            NativeTransactionOperation::Purge
        } else {
            NativeTransactionOperation::Remove
        };
        Self::prepare_removals(
            conn,
            &[NativeRemoveInput {
                trove: &removed_trove,
                paths,
                operation,
            }],
        )
    }

    pub(crate) fn prepare_removals(
        conn: &rusqlite::Connection,
        inputs: &[NativeRemoveInput<'_>],
    ) -> Result<Self> {
        if inputs.is_empty() {
            return Ok(Self::default());
        }

        let mut removed_trove_ids = HashSet::new();
        let mut counts_after = HashMap::<String, u32>::new();
        let mut changes = Vec::with_capacity(inputs.len());
        let mut path_capability_changes = Vec::with_capacity(inputs.len());
        let mut deb_change_indices = BTreeSet::new();
        let mut arch_transaction = false;
        for (transaction_index, input) in inputs.iter().enumerate() {
            let trove_id = input
                .trove
                .id
                .context("native removal input has no installed trove id")?;
            let current = Trove::find_by_id(conn, trove_id)?
                .with_context(|| format!("installed trove {trove_id} disappeared"))?;
            if current.name != input.trove.name
                || current.version != input.trove.version
                || current.architecture != input.trove.architecture
            {
                anyhow::bail!(
                    "installed trove {trove_id} identity changed during removal preparation"
                );
            }
            let version_scheme = current.version_scheme;
            if input.operation.counterpart_transaction_index().is_some() {
                anyhow::bail!(
                    "standalone native removal for '{}' cannot refer to an installing transaction element",
                    current.name
                );
            }
            if input.operation == NativeTransactionOperation::Purge
                && version_scheme != VersionScheme::Debian
            {
                anyhow::bail!(
                    "native purge operation is only valid for a Debian package, found '{}'",
                    current.name
                );
            }
            arch_transaction |= version_scheme == VersionScheme::Arch;
            if version_scheme == VersionScheme::Debian {
                deb_change_indices.insert(transaction_index);
            }
            if !removed_trove_ids.insert(trove_id) {
                anyhow::bail!("native removal input repeats installed trove {trove_id}");
            }
            let instances_before = counts_after
                .get(input.trove.name.as_str())
                .copied()
                .unwrap_or(installed_instance_count(conn, &input.trove.name)?);
            let instances_after = instances_before.checked_sub(1).with_context(|| {
                format!(
                    "removal transaction for '{}' has no installed instance",
                    input.trove.name
                )
            })?;
            counts_after.insert(input.trove.name.clone(), instances_after);
            let old_path_capabilities = declared_path_capabilities_for_trove(conn, trove_id)?;
            changes.push(NativeTransactionChange {
                package_name: input.trove.name.clone(),
                old_arch: InstalledNativeLifecycleBundle::find_by_trove(conn, trove_id)?
                    .and_then(|installed| installed.source_arch)
                    .or_else(|| input.trove.architecture.clone()),
                new_arch: None,
                old_version: Some(input.trove.version.clone()),
                new_version: None,
                operation: input.operation,
                old_paths: input.paths.iter().cloned().collect(),
                new_paths: BTreeSet::new(),
                instances_before,
                instances_after,
                transaction_index,
            });
            path_capability_changes.push(NativeTransactionPathCapabilities {
                old_paths: old_path_capabilities,
                new_paths: BTreeSet::new(),
            });
        }

        let mut owners = Vec::new();
        let installed_bundles = InstalledNativeLifecycleBundle::find_all(conn)?;
        for installed in installed_bundles {
            let bundle = installed.bundle().with_context(|| {
                format!(
                    "installed native lifecycle bundle for {} is malformed",
                    installed.source_package
                )
            })?;
            let role = if removed_trove_ids.contains(&installed.trove_id) {
                NativeBundleRole::Removing
            } else {
                NativeBundleRole::Installed
            };
            let instances_after = counts_after
                .get(installed.source_package.as_str())
                .copied()
                .unwrap_or(installed_instance_count(conn, &installed.source_package)?);
            owners.push(NativeBundleOwner {
                role,
                instances_after,
                package_name: installed.source_package,
                package_version: installed.source_version,
                initial_package_state: installed.lifecycle_state,
                initial_pending_triggers: installed.pending_triggers,
                initial_awaited_packages: installed.awaited_packages,
                bundle,
            });
        }
        let views = owners
            .iter()
            .map(|owner| NativeBundleView {
                package_name: &owner.package_name,
                package_version: &owner.package_version,
                instances_after: owner.instances_after,
                role: owner.role,
                bundle: &owner.bundle,
            })
            .collect::<Vec<_>>();
        let installed_capabilities_after =
            declared_capabilities_excluding(conn, &removed_trove_ids)?;
        let installed_path_capabilities_after = path_capabilities(
            installed_capabilities_after
                .iter()
                .map(|capability| capability.name.as_str()),
        );
        let installed_paths_after =
            PackagePayloadOwnership::installed_paths_excluding(conn, &removed_trove_ids)?;
        let arch_ldconfig_required_after =
            archive_paths_contain(&installed_paths_after, "etc/ld.so.conf");
        let transaction_state = NativeTransactionState {
            installed_packages_before: installed_packages_before(conn)?,
            installed_capabilities_after,
            installed_paths_after: installed_paths_after.clone(),
            deb_package_states: deb_state::transaction_package_states(conn, &owners)?,
            deb_change_indices,
        };
        let plan = plan_native_transaction(&views, &changes, &transaction_state)?;
        let host_capabilities = host_capabilities_for_plan(conn, arch_transaction, &plan)?;
        let path_projection = NativePathProjection::from_transaction(
            &plan,
            &changes,
            &installed_paths_after,
            &path_capability_changes,
            &installed_path_capabilities_after,
        )?;
        Ok(Self {
            owners,
            plan,
            changes: changes.clone(),
            transaction_state,
            debian_config_before: debian_runtime::config_snapshots(conn)?,
            operations: changes.iter().map(|change| change.operation).collect(),
            requires_upgrade_payload_boundary: false,
            arch_transaction,
            arch_ldconfig_required_after,
            host_capabilities,
            path_projection,
            activation_invocations: RefCell::new(Vec::new()),
            continued_lifecycle_failures: RefCell::new(Vec::new()),
        })
    }

    pub(crate) fn requires_upgrade_payload_boundary(&self) -> bool {
        self.requires_upgrade_payload_boundary
    }

    pub(super) fn rpm_sysusers_interface(&self) -> Result<&conary_core::ccs::ExecutableInterface> {
        self.host_capabilities
            .as_ref()
            .context("RPM sysusers event lost its required typed host capability inventory")?
            .sysusers_interface()
            .map_err(anyhow::Error::from)
    }
}

fn host_capabilities_for_plan(
    conn: &rusqlite::Connection,
    arch_transaction: bool,
    plan: &NativeTransactionPlan,
) -> Result<Option<conary_core::ccs::HostCapabilityInventory>> {
    (arch_transaction || plan.requires_sysusers_interface())
        .then(|| conary_core::ccs::HostCapabilityInventory::load_required(conn))
        .transpose()
        .context("native transaction typed host capability preflight failed")
}

fn debian_relation_removal_operation(
    conn: &rusqlite::Connection,
    removal: &PackageRelationRemoval,
    trove_id: i64,
    inputs: &[NativeInstallInput<'_>],
    old_paths: &BTreeSet<String>,
) -> Result<NativeTransactionOperation> {
    let authorized_packages = match removal.mode {
        PackageRelationRemovalMode::Constraint => &removal.incoming_packages,
        PackageRelationRemovalMode::OwnershipTransfer => &removal.ownership_transfer_packages,
    };
    let counterpart_index = removal.triggering_incoming.transaction_index;
    let counterpart = inputs.get(counterpart_index).with_context(|| {
        format!(
            "Debian relation removal for '{} {}' refers to missing installing transaction element {counterpart_index}",
            removal.package_name, removal.package_version
        )
    })?;
    if counterpart.package_name != removal.triggering_incoming.package_name
        || counterpart.package_version != removal.triggering_incoming.package_version
        || counterpart.package_arch.map(str::to_string)
            != removal.triggering_incoming.package_architecture
        || authorized_packages
            .binary_search(&removal.triggering_incoming.package_name)
            .is_err()
    {
        anyhow::bail!(
            "Debian relation removal for '{} {}' carries an inconsistent installing transaction identity",
            removal.package_name,
            removal.package_version
        );
    }

    if removal.mode == PackageRelationRemovalMode::OwnershipTransfer {
        let config_paths = ConfigFile::find_by_trove(conn, trove_id)?
            .into_iter()
            .map(|config| config.path)
            .collect::<BTreeSet<_>>();
        let non_config_paths = old_paths
            .difference(&config_paths)
            .cloned()
            .collect::<BTreeSet<_>>();
        if !non_config_paths.is_empty() {
            let incoming_paths = counterpart.paths.iter().cloned().collect::<BTreeSet<_>>();
            if non_config_paths.is_subset(&incoming_paths) {
                return Ok(NativeTransactionOperation::Disappear {
                    overwriter_transaction_index: counterpart_index,
                });
            }
        }
    }

    Ok(NativeTransactionOperation::RemoveInFavour {
        replacement_transaction_index: counterpart_index,
    })
}

fn archive_paths_contain(paths: &BTreeSet<String>, expected: &str) -> bool {
    paths
        .iter()
        .any(|path| path.trim_start_matches('/') == expected)
}

fn executor_for_owner(owner: &NativeBundleOwner, root: &Path) -> Result<ScriptletExecutor> {
    let format = match &owner.bundle.source_format {
        conary_core::ccs::native_lifecycle::SourceFormat::Rpm => {
            conary_core::scriptlet::PackageFormat::Rpm
        }
        conary_core::ccs::native_lifecycle::SourceFormat::Deb => {
            conary_core::scriptlet::PackageFormat::Deb
        }
        conary_core::ccs::native_lifecycle::SourceFormat::Arch => {
            conary_core::scriptlet::PackageFormat::Arch
        }
    };
    let executor =
        ScriptletExecutor::new(root, &owner.package_name, &owner.package_version, format)
            // Native package-manager lifecycle behavior must persist. The install
            // transaction owns failure handling and recovery.
            .with_sandbox_mode(SandboxMode::Always);
    Ok(executor)
}

#[cfg(test)]
#[path = "native_events/tests.rs"]
mod tests;
