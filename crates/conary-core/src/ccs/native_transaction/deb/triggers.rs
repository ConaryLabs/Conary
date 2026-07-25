// conary-core/src/ccs/native_transaction/deb/triggers.rs

//! Exact Debian trigger activation and pending-batch planning.

use super::{DebPackageState, DebTransactionPlan, event_key};
use crate::ccs::native_lifecycle::{
    DebControlMember, DebMaintainerMode, DebTriggerAwaitMode, DebTriggerDirective,
};
use crate::ccs::native_transaction::{
    BundleEventSpec, DebTriggerActivationBoundary, NativeBundleRole, NativeBundleView,
    NativeEventPlacement, NativeEventStage, NativePackageIdentity, NativeTransactionChange,
    NativeTransactionEvent, NativeTransactionOperation, NativeTransactionState, entry_event,
};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

/// One exact trigger activation relationship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebTriggerActivation {
    pub transaction_index: usize,
    pub boundary: DebTriggerActivationBoundary,
    pub trigger_name: String,
    pub triggering: NativePackageIdentity,
    pub interested: NativePackageIdentity,
    /// `true` means the triggering package enters `triggers-awaited` until
    /// the interested package processes this trigger or becomes unconfigured.
    pub awaited: bool,
}

/// The pending-trigger state represented by one `postinst triggered` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebTriggerBatch {
    pub event_key: String,
    pub interested: NativePackageIdentity,
    pub pending_triggers: Vec<String>,
    pub awaited_by: Vec<NativePackageIdentity>,
}

/// One exact point where dpkg lowers a package below the configured trigger
/// states and therefore clears its pending triggers and reverse await edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebTriggerUnconfiguration {
    pub transaction_index: usize,
    pub boundary: DebTriggerActivationBoundary,
    pub owner: NativePackageIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TriggerInterest {
    owner: NativePackageIdentity,
    await_mode: DebTriggerAwaitMode,
    configured: bool,
}

#[derive(Debug, Default)]
struct PendingTriggerState {
    triggers: BTreeSet<String>,
    awaited_by: BTreeSet<NativePackageIdentity>,
}

#[derive(Clone, Copy)]
struct ActivationContext<'a> {
    triggering: &'a NativePackageIdentity,
    transaction_index: usize,
    boundary: DebTriggerActivationBoundary,
    triggering_can_await: bool,
}

pub(super) fn plan(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    state: &NativeTransactionState,
    events: &mut Vec<NativeTransactionEvent>,
    plan: &mut DebTransactionPlan,
) -> Result<()> {
    validate_persisted_trigger_states(bundles, state)?;
    let mut registry = initial_interest_registry(bundles, state);
    let mut pending = initial_pending_state(state);
    let mut ordered_changes = changes.iter().collect::<Vec<_>>();
    ordered_changes.sort_by_key(|change| change.transaction_index);

    for change in ordered_changes {
        if matches!(
            change.operation,
            NativeTransactionOperation::DeconfigureInFavour { .. }
        ) {
            continue;
        }
        let old_view = changed_view(bundles, change, NativeBundleRole::Removing);
        let new_view = changed_view(bundles, change, NativeBundleRole::Installing);
        let triggering = triggering_identity(change)?;
        let triggering_can_await = state.deb_change_indices.contains(&change.transaction_index);

        for view in old_view.into_iter().chain(new_view) {
            record_explicit_activations(
                view,
                ActivationContext {
                    triggering: &triggering,
                    transaction_index: change.transaction_index,
                    boundary: DebTriggerActivationBoundary::BeforePayloadEvents,
                    triggering_can_await,
                },
                &registry,
                &mut pending,
                plan,
            );
        }

        if let Some(view) = old_view {
            let old_identity = view_identity(view);
            let unconfiguration = DebTriggerUnconfiguration {
                transaction_index: change.transaction_index,
                boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
                owner: old_identity.clone(),
            };
            if !plan.trigger_unconfigurations.contains(&unconfiguration) {
                plan.trigger_unconfigurations.push(unconfiguration);
            }
            set_owner_configured(&mut registry, &old_identity, false);
            pending.remove(&old_identity);
        }

        let new_paths = normalized_trigger_paths(&change.new_paths);
        record_file_activations(
            &new_paths,
            ActivationContext {
                triggering: &triggering,
                transaction_index: change.transaction_index,
                boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
                triggering_can_await,
            },
            &registry,
            &mut pending,
            plan,
        );
        let removed_paths = normalized_trigger_paths(&change.old_paths)
            .difference(&new_paths)
            .cloned()
            .collect::<BTreeSet<_>>();
        record_file_activations(
            &removed_paths,
            ActivationContext {
                triggering: &triggering,
                transaction_index: change.transaction_index,
                boundary: DebTriggerActivationBoundary::BeforeOldPayloadFinalization,
                triggering_can_await,
            },
            &registry,
            &mut pending,
            plan,
        );

        if let Some(view) = old_view {
            remove_owner_interests(&mut registry, &view_identity(view));
        }
        if let Some(view) = new_view {
            add_owner_interests(&mut registry, view, false);
            record_explicit_activations(
                view,
                ActivationContext {
                    triggering: &triggering,
                    transaction_index: change.transaction_index,
                    boundary: DebTriggerActivationBoundary::BeforeConfigure,
                    triggering_can_await,
                },
                &registry,
                &mut pending,
                plan,
            );
            set_owner_configured(&mut registry, &view_identity(view), true);
        }
    }

    plan_final_batches(bundles, &registry, pending, events, plan)
}

fn initial_interest_registry(
    bundles: &[NativeBundleView<'_>],
    state: &NativeTransactionState,
) -> BTreeMap<String, Vec<TriggerInterest>> {
    let mut registry = BTreeMap::new();
    for view in bundles.iter().filter(|view| {
        view.bundle.source_format.as_str() == "deb"
            && view.role != NativeBundleRole::Installing
            && persisted_trigger_state(view, state)
                .is_some_and(|state| package_accepts_trigger_activations(state.lifecycle_state))
    }) {
        add_owner_interests(&mut registry, view, true);
    }
    registry
}

fn initial_pending_state(
    state: &NativeTransactionState,
) -> BTreeMap<NativePackageIdentity, PendingTriggerState> {
    let mut pending = BTreeMap::new();
    for package in &state.deb_package_states {
        if package.pending_triggers.is_empty() {
            continue;
        }
        let identity = persisted_identity(package);
        pending.insert(
            identity.clone(),
            PendingTriggerState {
                triggers: package.pending_triggers.iter().cloned().collect(),
                awaited_by: state
                    .deb_package_states
                    .iter()
                    .filter(|awaiter| awaiter.awaited_packages.contains(&identity))
                    .map(persisted_identity)
                    .collect(),
            },
        );
    }
    pending
}

fn package_accepts_trigger_activations(state: DebPackageState) -> bool {
    matches!(
        state,
        DebPackageState::TriggersAwaited
            | DebPackageState::TriggersPending
            | DebPackageState::Installed
    )
}

fn changed_view<'a>(
    bundles: &'a [NativeBundleView<'a>],
    change: &NativeTransactionChange,
    role: NativeBundleRole,
) -> Option<&'a NativeBundleView<'a>> {
    let architecture = match role {
        NativeBundleRole::Installing => &change.new_arch,
        NativeBundleRole::Removing => &change.old_arch,
        NativeBundleRole::Installed | NativeBundleRole::Deconfiguring => return None,
    };
    bundles.iter().find(|view| {
        view.role == role
            && view.package_name == change.package_name
            && &view.bundle.source_arch == architecture
            && view.bundle.source_format.as_str() == "deb"
    })
}

fn triggering_identity(change: &NativeTransactionChange) -> Result<NativePackageIdentity> {
    let (version, architecture) = match change.operation {
        NativeTransactionOperation::Install | NativeTransactionOperation::Upgrade => {
            (change.new_version.as_ref(), change.new_arch.as_deref())
        }
        NativeTransactionOperation::Remove
        | NativeTransactionOperation::Purge
        | NativeTransactionOperation::RemoveInFavour { .. }
        | NativeTransactionOperation::Disappear { .. }
        | NativeTransactionOperation::DeconfigureInFavour { .. } => {
            (change.old_version.as_ref(), change.old_arch.as_deref())
        }
    };
    let version = version.with_context(|| {
        format!(
            "native transaction trigger source '{}' has no operation version",
            change.package_name
        )
    })?;
    Ok(NativePackageIdentity::new(
        &change.package_name,
        version,
        architecture,
    ))
}

fn trigger_declarations(
    view: &NativeBundleView<'_>,
    directive: DebTriggerDirective,
) -> Vec<(String, DebTriggerAwaitMode)> {
    view.bundle
        .entries
        .iter()
        .filter_map(|entry| entry.deb_maintainer.as_ref())
        .flat_map(|metadata| metadata.trigger_declarations.iter())
        .filter(|declaration| declaration.directive == directive)
        .map(|declaration| (declaration.trigger_name.clone(), declaration.await_mode))
        .collect()
}

fn add_owner_interests(
    registry: &mut BTreeMap<String, Vec<TriggerInterest>>,
    view: &NativeBundleView<'_>,
    configured: bool,
) {
    let owner = view_identity(view);
    for (trigger_name, await_mode) in trigger_declarations(view, DebTriggerDirective::Interest) {
        let interests = registry.entry(trigger_name).or_default();
        let interest = TriggerInterest {
            owner: owner.clone(),
            await_mode,
            configured,
        };
        if !interests.contains(&interest) {
            interests.push(interest);
        }
    }
}

fn set_owner_configured(
    registry: &mut BTreeMap<String, Vec<TriggerInterest>>,
    owner: &NativePackageIdentity,
    configured: bool,
) {
    for interest in registry.values_mut().flatten() {
        if interest.owner == *owner {
            interest.configured = configured;
        }
    }
}

fn remove_owner_interests(
    registry: &mut BTreeMap<String, Vec<TriggerInterest>>,
    owner: &NativePackageIdentity,
) {
    registry.retain(|_, interests| {
        interests.retain(|interest| interest.owner != *owner);
        !interests.is_empty()
    });
}

fn record_explicit_activations(
    view: &NativeBundleView<'_>,
    context: ActivationContext<'_>,
    registry: &BTreeMap<String, Vec<TriggerInterest>>,
    pending: &mut BTreeMap<NativePackageIdentity, PendingTriggerState>,
    plan: &mut DebTransactionPlan,
) {
    for (trigger_name, await_mode) in trigger_declarations(view, DebTriggerDirective::Activate) {
        record_named_activation(&trigger_name, await_mode, context, registry, pending, plan);
    }
}

fn record_file_activations(
    paths: &BTreeSet<String>,
    context: ActivationContext<'_>,
    registry: &BTreeMap<String, Vec<TriggerInterest>>,
    pending: &mut BTreeMap<NativePackageIdentity, PendingTriggerState>,
    plan: &mut DebTransactionPlan,
) {
    let trigger_names = registry
        .keys()
        .filter(|trigger_name| {
            trigger_name.starts_with('/')
                && paths
                    .iter()
                    .any(|path| deb_file_trigger_matches(trigger_name, path))
        })
        .cloned()
        .collect::<Vec<_>>();
    for trigger_name in trigger_names {
        record_named_activation(
            &trigger_name,
            DebTriggerAwaitMode::Default,
            context,
            registry,
            pending,
            plan,
        );
    }
}

fn normalized_trigger_paths(paths: &BTreeSet<String>) -> BTreeSet<String> {
    paths
        .iter()
        .map(|path| path.trim_start_matches('/').to_string())
        .collect()
}

fn record_named_activation(
    trigger_name: &str,
    activation_await_mode: DebTriggerAwaitMode,
    context: ActivationContext<'_>,
    registry: &BTreeMap<String, Vec<TriggerInterest>>,
    pending: &mut BTreeMap<NativePackageIdentity, PendingTriggerState>,
    plan: &mut DebTransactionPlan,
) {
    let Some(interests) = registry.get(trigger_name) else {
        return;
    };
    for interest in interests.iter().filter(|interest| interest.configured) {
        let awaited = context.triggering_can_await
            && interest.await_mode != DebTriggerAwaitMode::NoAwait
            && activation_await_mode != DebTriggerAwaitMode::NoAwait;
        let pending_state = pending.entry(interest.owner.clone()).or_default();
        pending_state.triggers.insert(trigger_name.to_string());
        if awaited {
            pending_state.awaited_by.insert(context.triggering.clone());
        }
        let activation = DebTriggerActivation {
            transaction_index: context.transaction_index,
            boundary: context.boundary,
            trigger_name: trigger_name.to_string(),
            triggering: context.triggering.clone(),
            interested: interest.owner.clone(),
            awaited,
        };
        if !plan.trigger_activations.contains(&activation) {
            plan.trigger_activations.push(activation);
        }
    }
}

fn plan_final_batches(
    bundles: &[NativeBundleView<'_>],
    registry: &BTreeMap<String, Vec<TriggerInterest>>,
    pending: BTreeMap<NativePackageIdentity, PendingTriggerState>,
    events: &mut Vec<NativeTransactionEvent>,
    plan: &mut DebTransactionPlan,
) -> Result<()> {
    let final_interest_owners = registry
        .values()
        .flatten()
        .filter(|interest| interest.configured)
        .map(|interest| interest.owner.clone())
        .collect::<BTreeSet<_>>();
    for (interested, pending_state) in pending {
        if pending_state.triggers.is_empty() || !final_interest_owners.contains(&interested) {
            continue;
        }
        let view = final_owner_view(bundles, &interested).with_context(|| {
            format!(
                "final Debian trigger owner '{} {} {:?}' has no lifecycle bundle",
                interested.package_name, interested.package_version, interested.package_arch
            )
        })?;
        let postinst =
            view.bundle
                .entries
                .iter()
                .find(|entry| {
                    entry.deb_maintainer.as_ref().is_some_and(|metadata| {
                        metadata.control_member == DebControlMember::Postinst
                    })
                })
                .with_context(|| {
                    format!(
                        "Debian package '{}' declares trigger interests without a postinst entry",
                        view.package_name
                    )
                })?;
        let pending_triggers = pending_state.triggers.into_iter().collect::<Vec<_>>();
        let awaited_by = pending_state.awaited_by.into_iter().collect::<Vec<_>>();
        let stage = if awaited_by.is_empty() {
            NativeEventStage::DebTriggerProcessing
        } else {
            NativeEventStage::DebAwaitedTriggerProcessing
        };
        let event = entry_event(
            view,
            postinst,
            BundleEventSpec {
                stage,
                args: vec![
                    DebMaintainerMode::Triggered.as_str().to_string(),
                    pending_triggers.join(" "),
                ],
                stdin: Vec::new(),
                matched_targets: pending_triggers.clone(),
                order_key: format!("deb:{}:{stage:?}", view.package_name),
                placement: NativeEventPlacement::TransactionAfter,
            },
        );
        let key = event_key(&event);
        plan.trigger_batches.push(DebTriggerBatch {
            event_key: key.clone(),
            interested,
            pending_triggers,
            awaited_by,
        });
        plan.terminal_failure_states
            .insert(key, DebPackageState::HalfConfigured);
        events.push(event);
    }
    Ok(())
}

fn final_owner_view<'a>(
    bundles: &'a [NativeBundleView<'a>],
    identity: &NativePackageIdentity,
) -> Option<&'a NativeBundleView<'a>> {
    [NativeBundleRole::Installing, NativeBundleRole::Installed]
        .into_iter()
        .find_map(|role| {
            bundles.iter().find(|view| {
                view.role == role
                    && view.package_name == identity.package_name
                    && view.package_version == identity.package_version
                    && view.bundle.source_arch == identity.package_arch
                    && view.bundle.source_format.as_str() == "deb"
            })
        })
}

fn validate_persisted_trigger_states(
    bundles: &[NativeBundleView<'_>],
    state: &NativeTransactionState,
) -> Result<()> {
    let mut identities = BTreeSet::new();
    for persisted in &state.deb_package_states {
        let identity = (
            persisted.package_name.clone(),
            persisted.package_version.clone(),
            persisted.source_arch.clone(),
        );
        if !identities.insert(identity) {
            bail!(
                "persisted Debian trigger state repeats {} {} {:?}",
                persisted.package_name,
                persisted.package_version,
                persisted.source_arch
            );
        }
        validate_unique_trigger_state_values(
            &persisted.pending_triggers,
            "pending trigger",
            &persisted.package_name,
        )?;
        let mut awaited_identities = BTreeSet::new();
        if persisted.awaited_packages.iter().any(|awaited| {
            awaited.package_name.is_empty()
                || awaited.package_version.is_empty()
                || !awaited_identities.insert(awaited)
        }) {
            bail!(
                "persisted Debian awaited package identity for '{}' is empty or repeated",
                persisted.package_name
            );
        }
        if !persisted.pending_triggers.is_empty() {
            let matching_views = bundles
                .iter()
                .filter(|view| {
                    view.package_name == persisted.package_name
                        && view.package_version == persisted.package_version
                        && view.bundle.source_arch == persisted.source_arch
                        && view.bundle.source_format.as_str() == "deb"
                })
                .collect::<Vec<_>>();
            if matching_views.len() != 1 {
                bail!(
                    "persisted Debian pending-trigger state for {} {} {:?} resolves to {} lifecycle owners",
                    persisted.package_name,
                    persisted.package_version,
                    persisted.source_arch,
                    matching_views.len()
                );
            }
            let exact_interests =
                trigger_declarations(matching_views[0], DebTriggerDirective::Interest)
                    .into_iter()
                    .map(|(trigger_name, _)| trigger_name)
                    .collect::<BTreeSet<_>>();
            if let Some(stale) = persisted
                .pending_triggers
                .iter()
                .find(|trigger_name| !exact_interests.contains(*trigger_name))
            {
                bail!(
                    "persisted Debian pending trigger '{stale}' for '{}' is not a current exact interest",
                    persisted.package_name
                );
            }
        }
    }

    for persisted in &state.deb_package_states {
        for awaited in &persisted.awaited_packages {
            if !state.deb_package_states.iter().any(|candidate| {
                persisted_identity(candidate) == *awaited && !candidate.pending_triggers.is_empty()
            }) {
                bail!(
                    "persisted Debian await edge from '{}' targets '{} {} {:?}' without pending triggers",
                    persisted.package_name,
                    awaited.package_name,
                    awaited.package_version,
                    awaited.package_arch
                );
            }
        }
    }
    Ok(())
}

fn validate_unique_trigger_state_values(
    values: &[String],
    kind: &str,
    package_name: &str,
) -> Result<()> {
    let mut unique = BTreeSet::new();
    for value in values {
        if value.is_empty() || !unique.insert(value) {
            bail!("persisted Debian {kind} for '{package_name}' is empty or repeated");
        }
    }
    Ok(())
}

fn persisted_trigger_state<'a>(
    view: &NativeBundleView<'_>,
    state: &'a NativeTransactionState,
) -> Option<&'a crate::ccs::native_transaction::NativeDebPackageState> {
    state.deb_package_states.iter().find(|persisted| {
        persisted.package_name == view.package_name
            && persisted.package_version == view.package_version
            && persisted.source_arch == view.bundle.source_arch
    })
}

fn view_identity(view: &NativeBundleView<'_>) -> NativePackageIdentity {
    NativePackageIdentity {
        package_name: view.package_name.to_string(),
        package_version: view.package_version.to_string(),
        package_arch: view.bundle.source_arch.clone(),
    }
}

fn persisted_identity(
    state: &crate::ccs::native_transaction::NativeDebPackageState,
) -> NativePackageIdentity {
    NativePackageIdentity {
        package_name: state.package_name.clone(),
        package_version: state.package_version.clone(),
        package_arch: state.source_arch.clone(),
    }
}

pub(in crate::ccs::native_transaction) fn deb_file_trigger_matches(
    trigger_name: &str,
    archive_path: &str,
) -> bool {
    debug_assert!(trigger_name.starts_with('/'));
    let normalized_path = if archive_path.starts_with('/') {
        archive_path.to_string()
    } else {
        format!("/{archive_path}")
    };

    normalized_path == trigger_name
        || if trigger_name.ends_with('/') {
            normalized_path.starts_with(trigger_name)
        } else {
            normalized_path
                .strip_prefix(trigger_name)
                .is_some_and(|suffix| suffix.starts_with('/'))
        }
}
