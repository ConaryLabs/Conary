// apps/conary/src/commands/install/restore/transaction.rs
//! One durable transaction for an exact system-state restore.

use super::super::inner;
use super::super::native_events::{
    NativeInstallInput, NativeRemoveInput, PreparedNativeTransaction,
};
use super::super::native_graph::{
    NativeGraphPayloadMutation, drive_native_graph, finalize_owned_trove, normalize_archive_path,
};
use super::super::{
    FinalizeInstallOutput, InstallProgress, InstallTransactionResult, TransactionContext,
    build_execution_mode, finalize_install_without_snapshot,
    preflight_generation_file_capabilities_for_install, run_triggers,
};
use super::PreparedInstall;
use crate::commands::generation::selected_root::SelectedRootSession;
use crate::commands::progress::RemoveProgress;
use crate::commands::remove::{
    RemoveLifecycleOptions, commit_remove_db, prepare_remove_for_state_restore, snapshot_trove,
};
use crate::commands::{SandboxMode, TroveSnapshot};
use anyhow::{Context, Result, bail};
use conary_core::ccs::native_lifecycle::SourceFormat;
use conary_core::ccs::native_transaction::{NativePackageIdentity, NativeTransactionOperation};
use conary_core::config_transaction::{ConfigTransactionOperation, GenerationConfigTransaction};
use conary_core::db::models::{
    ActivationRequest, Changeset, ChangesetStatus, InstalledNativeLifecycleBundle, Trove,
};
use conary_core::filesystem::CasStore;
use conary_core::scriptlet::ExecutionMode;
use std::collections::BTreeSet;
use std::path::Path;

/// Apply one exact state transition under one changeset, SQLite transaction,
/// selected-root journal, and generation publication debt.
///
/// Native package graphs remain the lifecycle authority. They are nested
/// below this wrapper so a state restore never leaks per-package changesets or
/// partially committed package authority.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_state_restore_transaction(
    conn: &mut rusqlite::Connection,
    db_path: &str,
    root: &str,
    from_state: i64,
    to_state: i64,
    removal_troves: &[Trove],
    prepared_installs: Vec<PreparedInstall>,
) -> Result<()> {
    let description = format!("Restore state {from_state} -> {to_state}");
    let mut selected_root = SelectedRootSession::begin(conn, db_path, &description)?;
    let selected_path = selected_root.selected_root().to_path_buf();

    let removal_paths = removal_troves
        .iter()
        .map(|trove| {
            let trove_id = trove
                .id
                .with_context(|| format!("restore removal '{}' has no trove ID", trove.name))?;
            Ok(
                crate::commands::remove::PackagePayloadOwnership::load(conn, trove_id)?
                    .lifecycle_paths()
                    .to_vec(),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let removal_inputs = removal_troves
        .iter()
        .zip(&removal_paths)
        .map(|(trove, paths)| NativeRemoveInput {
            trove,
            paths: paths.clone(),
            operation: NativeTransactionOperation::Remove,
        })
        .collect::<Vec<_>>();
    let removal_native = PreparedNativeTransaction::prepare_removals(conn, &removal_inputs)?;
    removal_native.preflight(&selected_path, &ExecutionMode::Remove)?;

    let preflighted_ccs_hooks = prepared_installs
        .iter()
        .map(|prepared| {
            prepared
                .ccs_removal_hook_plan
                .preflight(&selected_path, SandboxMode::Always)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut ccs_hook_executors =
        prepare_ccs_hook_executors(conn, &selected_path, &prepared_installs)?;

    let cas = selected_root.cas().clone();
    execute_locked_restore(
        conn,
        db_path,
        root,
        &description,
        removal_troves,
        &removal_native,
        prepared_installs,
        &preflighted_ccs_hooks,
        &mut ccs_hook_executors,
        &cas,
        &mut selected_root,
    )
}

fn prepare_ccs_hook_executors(
    conn: &rusqlite::Connection,
    selected_path: &Path,
    prepared_installs: &[PreparedInstall],
) -> Result<Vec<Option<conary_core::ccs::HookExecutor>>> {
    prepared_installs
        .iter()
        .map(|prepared| {
            let Some(contract) = prepared.ccs_contract.as_ref() else {
                return Ok(None);
            };
            let host_capabilities =
                if super::super::ccs_transaction::ccs_requires_host_capability_inventory(
                    &contract.hooks,
                ) {
                    conary_core::ccs::HostCapabilityInventory::load_required(conn)
                        .context("CCS state-restore host capability preflight failed")?
                } else {
                    conary_core::ccs::HostCapabilityInventory::default()
                };
            let executor = conary_core::ccs::HookExecutor::new(selected_path, host_capabilities);
            if super::super::ccs_transaction::ccs_has_pre_hooks(&contract.hooks)
                || super::super::ccs_transaction::ccs_has_post_hooks(&contract.hooks)
            {
                executor.preflight_hooks(&contract.hooks).with_context(|| {
                    format!(
                        "CCS lifecycle preflight failed while restoring '{}'",
                        prepared.pkg.name()
                    )
                })?;
            }
            Ok(Some(executor))
        })
        .collect()
}

fn execute_ccs_pre_install_hooks(
    prepared_installs: &[PreparedInstall],
    executors: &mut [Option<conary_core::ccs::HookExecutor>],
) -> Result<()> {
    for (prepared, executor) in prepared_installs.iter().zip(executors) {
        let Some(contract) = prepared.ccs_contract.as_ref() else {
            continue;
        };
        if super::super::ccs_transaction::ccs_has_pre_hooks(&contract.hooks) {
            executor
                .as_mut()
                .context("CCS restore package has no preflighted hook executor")?
                .execute_pre_hooks(&contract.hooks)
                .with_context(|| {
                    format!(
                        "CCS pre-install hooks failed while restoring '{}'",
                        prepared.pkg.name()
                    )
                })?;
        }
    }
    Ok(())
}

fn execute_ccs_post_install_hooks(
    prepared_installs: &[PreparedInstall],
    executors: &[Option<conary_core::ccs::HookExecutor>],
) -> Result<Vec<conary_core::db::models::NewActivationRequest>> {
    let mut activation_requests = Vec::new();
    for (prepared, executor) in prepared_installs.iter().zip(executors) {
        let Some(contract) = prepared.ccs_contract.as_ref() else {
            continue;
        };
        let executor = executor
            .as_ref()
            .context("CCS restore package has no preflighted hook executor")?;
        if super::super::ccs_transaction::ccs_has_post_hooks(&contract.hooks) {
            super::super::ccs_transaction::execute_ccs_post_hooks(
                prepared.pkg.name(),
                prepared.pkg.version(),
                &contract.hooks,
                executor,
            )?;
        }
        activation_requests.extend(super::super::ccs_transaction::ccs_activation_requests(
            prepared.pkg.name(),
            prepared.pkg.version(),
            executor,
        ));
    }
    Ok(activation_requests)
}

#[allow(clippy::too_many_arguments)]
fn execute_locked_restore(
    conn: &mut rusqlite::Connection,
    db_path: &str,
    root: &str,
    description: &str,
    removal_troves: &[Trove],
    removal_native: &PreparedNativeTransaction,
    prepared_installs: Vec<PreparedInstall>,
    preflighted_ccs_hooks: &[super::super::ccs_removal_hooks::PreflightedCcsRemovalHooks],
    ccs_hook_executors: &mut [Option<conary_core::ccs::HookExecutor>],
    cas: &CasStore,
    selected_root: &mut SelectedRootSession,
) -> Result<()> {
    let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path);
    let rollback_root = selected_root.capture_rollback_authority()?;
    let rollback_system = crate::commands::RollbackSystemAuthority::capture(conn)?;
    let rollback_materialized_directories =
        crate::commands::capture_materialized_directory_snapshots(
            conn,
            super::super::shared_directory::capture_existing_directory_materializations(
                conn,
                selected_root.selected_root(),
            )?,
        )?;
    for hooks in preflighted_ccs_hooks {
        hooks.execute()?;
    }
    execute_ccs_pre_install_hooks(&prepared_installs, ccs_hook_executors)?;

    let stored_files = prepared_installs
        .iter()
        .map(|prepared| inner::store_install_files_in_cas(cas, &prepared.extraction))
        .collect::<Result<Vec<_>>>()?;
    let selected_path = selected_root.selected_root().to_path_buf();
    let tx = conn.unchecked_transaction()?;
    let execution = (|| -> Result<_> {
        let mut changeset = Changeset::new(description.to_string());
        let changeset_id = changeset.insert(&tx)?;
        let mut config_transaction = GenerationConfigTransaction::default();
        let mut snapshots = Vec::new();

        execute_removal_graph(RestoreRemovalGraph {
            tx: &tx,
            selected_root,
            cas,
            troves: removal_troves,
            native_transaction: removal_native,
            changeset_id,
            config_transaction: &mut config_transaction,
            snapshots: &mut snapshots,
        })?;
        let mut activation_requests = removal_native.take_activation_requests();

        snapshot_upgraded_troves(&tx, &prepared_installs, &mut snapshots)?;
        let (install_native, native_mode) =
            prepare_install_graph(&tx, &selected_path, &prepared_installs)?;
        let normalized_file_capabilities =
            preflight_install_payloads(&tx, db_path, &selected_path, &prepared_installs)?;
        let installed_trove_ids = execute_install_graph(
            &tx,
            db_path,
            selected_root,
            cas,
            &prepared_installs,
            &stored_files,
            &normalized_file_capabilities,
            &install_native,
            &native_mode,
            changeset_id,
            &mut config_transaction,
        )?;
        activation_requests.extend(install_native.take_activation_requests());
        activation_requests.extend(execute_ccs_post_install_hooks(
            &prepared_installs,
            ccs_hook_executors,
        )?);
        ActivationRequest::append_batch(&tx, changeset_id, &activation_requests)
            .context("failed to persist exact state-restore activation requests")?;

        let incoming_paths = prepared_installs
            .iter()
            .flat_map(|prepared| {
                prepared
                    .extraction
                    .extracted_files
                    .iter()
                    .map(|file| file.path.clone())
            })
            .collect::<Vec<_>>();
        if !incoming_paths.is_empty() {
            run_triggers(&tx, &selected_path, changeset_id, &incoming_paths)?;
        }

        let metadata = crate::commands::metadata_with_removed_troves(
            snapshots,
            rollback_materialized_directories.clone(),
            rollback_root.clone(),
            rollback_system.clone(),
        )?;
        tx.execute(
            "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
            rusqlite::params![metadata, changeset_id],
        )?;
        config_transaction.validate()?;
        changeset.update_status(&tx, ChangesetStatus::Applied)?;
        let publication_debt =
            crate::commands::generation::publication::record_selected_root_state(
                &tx,
                &crate::commands::generation::publication::PublicationRequest {
                    db_path,
                    summary: description,
                    trigger_changeset_id: Some(changeset_id),
                    tx_uuid: changeset.tx_uuid.as_deref(),
                    config_transaction,
                },
            )?;
        Ok((changeset_id, installed_trove_ids, publication_debt))
    })();

    let (changeset_id, installed_trove_ids, publication_debt) = match execution {
        Ok(result) => result,
        Err(error) => {
            drop(tx);
            selected_root
                .rollback()
                .context("failed to roll back isolated state-restore root")?;
            return Err(error);
        }
    };

    if let Err(error) = selected_root.persist_for_publication(&runtime_root, &publication_debt) {
        drop(tx);
        let _ = crate::commands::generation::selected_root::remove_publication_candidate(
            &runtime_root,
            &publication_debt,
        );
        return Err(error.context("failed to persist exact state-restore selected root"));
    }
    if let Err(error) = tx.commit() {
        crate::commands::generation::selected_root::remove_publication_candidate(
            &runtime_root,
            &publication_debt,
        )
        .context("failed to discard state-restore candidate after database commit error")?;
        return Err(error.into());
    }

    let outcome = crate::commands::generation::publication::publish_recorded_selected_root(
        conn,
        db_path,
        description,
        publication_debt,
    )?;
    if outcome.needs_publication {
        crate::commands::append_deferred_follow_up_metadata(
            conn,
            changeset_id,
            crate::commands::publication_deferred_follow_up(
                "generation publication is pending".to_string(),
            ),
        )?;
        crate::commands::generation::publication::warn_if_publication_pending(
            changeset_id,
            &outcome,
        );
    }

    finalize_restore_installs(
        conn,
        root,
        changeset_id,
        &prepared_installs,
        &installed_trove_ids,
    )
}

struct RestoreRemovalGraph<'a, 'conn> {
    tx: &'a rusqlite::Transaction<'conn>,
    selected_root: &'a mut SelectedRootSession,
    cas: &'a CasStore,
    troves: &'a [Trove],
    native_transaction: &'a PreparedNativeTransaction,
    changeset_id: i64,
    config_transaction: &'a mut GenerationConfigTransaction,
    snapshots: &'a mut Vec<TroveSnapshot>,
}

fn execute_removal_graph(graph: RestoreRemovalGraph<'_, '_>) -> Result<()> {
    let RestoreRemovalGraph {
        tx,
        selected_root,
        cas,
        troves,
        native_transaction,
        changeset_id,
        config_transaction,
        snapshots,
    } = graph;
    if troves.is_empty() {
        return Ok(());
    }
    let selected_path = selected_root.selected_root().to_path_buf();
    let mut payload = RestoreRemovalPayload {
        tx,
        selected_root,
        cas,
        troves,
        native_transaction,
        changeset_id,
        config_transaction,
        snapshots,
        completed: BTreeSet::new(),
    };
    drive_native_graph(
        tx,
        native_transaction,
        &selected_path,
        &ExecutionMode::Remove,
        &mut payload,
    )?;
    if payload.completed.len() != troves.len() {
        bail!(
            "native restore removal graph finalized {} of {} packages",
            payload.completed.len(),
            troves.len()
        );
    }
    Ok(())
}

struct RestoreRemovalPayload<'a, 'conn> {
    tx: &'a rusqlite::Transaction<'conn>,
    selected_root: &'a mut SelectedRootSession,
    cas: &'a CasStore,
    troves: &'a [Trove],
    native_transaction: &'a PreparedNativeTransaction,
    changeset_id: i64,
    config_transaction: &'a mut GenerationConfigTransaction,
    snapshots: &'a mut Vec<TroveSnapshot>,
    completed: BTreeSet<usize>,
}

impl NativeGraphPayloadMutation for RestoreRemovalPayload<'_, '_> {
    fn apply_payload(&mut self, _conn: &rusqlite::Connection, change_index: usize) -> Result<()> {
        if change_index >= self.troves.len() {
            bail!("state-restore removal graph refers to change {change_index}");
        }
        Ok(())
    }

    fn finalize_old_payload(
        &mut self,
        _conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        let trove = self.troves.get(change_index).with_context(|| {
            format!("state-restore removal graph refers to change {change_index}")
        })?;
        if !self.completed.insert(change_index) {
            bail!(
                "state-restore removal graph finalized '{}' more than once",
                trove.name
            );
        }
        let trove_id = trove
            .id
            .with_context(|| format!("restore removal '{}' has no trove ID", trove.name))?;
        let progress = RemoveProgress::new(&trove.name);
        let prepared = prepare_remove_for_state_restore(
            self.tx,
            trove,
            self.selected_root
                .selected_root()
                .to_str()
                .context("selected state-restore root is not UTF-8")?,
            RemoveLifecycleOptions::new(SandboxMode::Always),
            &progress,
        )?;
        let mut captured = crate::commands::generation::config_transaction::capture_removal(
            self.tx,
            self.selected_root.selected_root(),
            self.cas,
            trove_id,
            false,
        )?;
        let mut config_plan = super::super::config_files::prepare_config_removal(
            self.tx,
            self.selected_root.selected_root(),
            trove_id,
            prepared.materialized_removal_paths().iter().cloned(),
            false,
        )?;
        let identity =
            NativePackageIdentity::new(&trove.name, &trove.version, trove.architecture.as_deref());
        self.native_transaction
            .mark_remove_payload_started_for(self.tx, &identity)?;
        self.selected_root
            .apply_remove_paths(&config_plan.remove_paths)?;
        self.selected_root.apply_install_files(&config_plan.files)?;
        config_plan.persist_retained(self.tx)?;
        self.snapshots.push(prepared.snapshot.clone());
        let _ = commit_remove_db(self.tx, self.changeset_id, prepared)?;
        self.native_transaction.mark_remove_payload_completed_for(
            self.tx,
            &identity,
            change_index,
        )?;
        merge_config_transaction(self.config_transaction, &mut captured)
    }

    fn purge_config_files(
        &mut self,
        _conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        bail!("state-restore removal graph unexpectedly purges change {change_index}")
    }
}

fn snapshot_upgraded_troves(
    tx: &rusqlite::Transaction<'_>,
    prepared_installs: &[PreparedInstall],
    snapshots: &mut Vec<TroveSnapshot>,
) -> Result<()> {
    let mut captured = BTreeSet::new();
    for prepared in prepared_installs {
        let Some(old_trove) = prepared.old_trove_to_upgrade.as_ref() else {
            continue;
        };
        let trove_id = old_trove
            .id
            .with_context(|| format!("upgrade target '{}' has no trove ID", old_trove.name))?;
        if captured.insert(trove_id) {
            snapshots.push(snapshot_trove(tx, old_trove)?);
        }
    }
    Ok(())
}

fn prepare_install_graph(
    tx: &rusqlite::Transaction<'_>,
    selected_path: &Path,
    prepared_installs: &[PreparedInstall],
) -> Result<(PreparedNativeTransaction, ExecutionMode)> {
    let inputs = prepared_installs
        .iter()
        .map(|prepared| NativeInstallInput {
            package_name: prepared.pkg.name(),
            package_version: prepared.pkg.version(),
            package_arch: prepared.pkg.architecture(),
            version_scheme: prepared.semantics.version_scheme,
            provides: prepared.pkg.provides(),
            new_bundle: prepared.native_lifecycle_state.bundle_to_persist.as_ref(),
            old_trove: prepared.old_trove_to_upgrade.as_ref(),
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: prepared
                .extraction
                .extracted_files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
        })
        .collect::<Vec<_>>();
    let native_transaction = PreparedNativeTransaction::prepare_batch(tx, &inputs)?;
    let mode = build_execution_mode(None);
    native_transaction.preflight(selected_path, &mode)?;
    Ok((native_transaction, mode))
}

fn preflight_install_payloads(
    tx: &rusqlite::Transaction<'_>,
    db_path: &str,
    selected_path: &Path,
    prepared_installs: &[PreparedInstall],
) -> Result<Vec<Option<Vec<conary_core::ccs::manifest::FileCapability>>>> {
    let mut incoming_paths = std::collections::BTreeMap::<String, (&str, bool)>::new();
    let mut normalized_file_capabilities = Vec::with_capacity(prepared_installs.len());
    for prepared in prepared_installs {
        let root = selected_path
            .to_str()
            .context("selected state-restore root is not UTF-8")?;
        let normalized = prepared
            .ccs_contract
            .as_ref()
            .map(|contract| {
                crate::commands::ccs::normalize_ccs_file_capabilities(
                    selected_path,
                    &contract.file_capabilities,
                )
            })
            .transpose()?;
        let ctx =
            restore_transaction_context(prepared, db_path, root, false, normalized.as_deref());
        preflight_generation_file_capabilities_for_install(&ctx, &prepared.extraction)?;
        inner::preflight_file_ownership(
            tx,
            prepared
                .extraction
                .extracted_files
                .iter()
                .map(|file| file.path.as_str()),
            prepared.pkg.name(),
            &[],
        )?;
        for file in &prepared.extraction.extracted_files {
            let path = normalize_archive_path(&file.path);
            let incoming_is_directory = matches!(
                file.node.kind,
                conary_core::payload::PayloadNodeKind::Directory
            );
            if let Some((owner, owner_is_directory)) = incoming_paths.get(&path)
                && !(incoming_is_directory && *owner_is_directory)
            {
                bail!(
                    "state restore has duplicate incoming payload path '{path}' from '{}' and '{}'",
                    owner,
                    prepared.pkg.name()
                );
            } else {
                incoming_paths
                    .entry(path)
                    .or_insert((prepared.pkg.name(), incoming_is_directory));
            }
        }
        normalized_file_capabilities.push(normalized);
    }
    Ok(normalized_file_capabilities)
}

#[allow(clippy::too_many_arguments)]
fn execute_install_graph(
    tx: &rusqlite::Transaction<'_>,
    db_path: &str,
    selected_root: &mut SelectedRootSession,
    cas: &CasStore,
    prepared_installs: &[PreparedInstall],
    stored_files: &[Vec<inner::StoredInstallFile>],
    normalized_file_capabilities: &[Option<Vec<conary_core::ccs::manifest::FileCapability>>],
    native_transaction: &PreparedNativeTransaction,
    native_mode: &ExecutionMode,
    changeset_id: i64,
    config_transaction: &mut GenerationConfigTransaction,
) -> Result<Vec<i64>> {
    if prepared_installs.is_empty() {
        return Ok(Vec::new());
    }
    let final_incoming_paths = prepared_installs
        .iter()
        .flat_map(|prepared| prepared.extraction.extracted_files.iter())
        .map(|file| normalize_archive_path(&file.path))
        .collect::<BTreeSet<_>>();
    let finalization_troves = prepared_installs
        .iter()
        .map(|prepared| {
            prepared
                .old_trove_to_upgrade
                .as_ref()
                .and_then(|trove| trove.id)
        })
        .collect::<Vec<_>>();
    let removing_deb_identities = finalization_troves
        .iter()
        .map(|trove_id| {
            trove_id
                .map(|trove_id| deb_identity_for_trove(tx, trove_id))
                .transpose()
                .map(Option::flatten)
        })
        .collect::<Result<Vec<_>>>()?;
    let selected_path = selected_root.selected_root().to_path_buf();
    let mut payload = RestoreInstallPayload {
        tx,
        db_path,
        selected_root,
        cas,
        prepared_installs,
        stored_files,
        normalized_file_capabilities,
        native_transaction,
        changeset_id,
        config_transaction,
        final_incoming_paths,
        finalization_troves,
        removing_deb_identities,
        results: (0..prepared_installs.len()).map(|_| None).collect(),
    };
    drive_native_graph(
        tx,
        native_transaction,
        &selected_path,
        native_mode,
        &mut payload,
    )?;
    payload
        .results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result
                .map(|result| result.trove_id)
                .with_context(|| format!("native restore graph did not apply package {index}"))
        })
        .collect()
}

struct RestoreInstallPayload<'a, 'conn> {
    tx: &'a rusqlite::Transaction<'conn>,
    db_path: &'a str,
    selected_root: &'a mut SelectedRootSession,
    cas: &'a CasStore,
    prepared_installs: &'a [PreparedInstall],
    stored_files: &'a [Vec<inner::StoredInstallFile>],
    normalized_file_capabilities: &'a [Option<Vec<conary_core::ccs::manifest::FileCapability>>],
    native_transaction: &'a PreparedNativeTransaction,
    changeset_id: i64,
    config_transaction: &'a mut GenerationConfigTransaction,
    final_incoming_paths: BTreeSet<String>,
    finalization_troves: Vec<Option<i64>>,
    removing_deb_identities: Vec<Option<NativePackageIdentity>>,
    results: Vec<Option<inner::InnerInstallResult>>,
}

impl NativeGraphPayloadMutation for RestoreInstallPayload<'_, '_> {
    fn apply_payload(&mut self, _conn: &rusqlite::Connection, change_index: usize) -> Result<()> {
        let prepared = self.prepared_installs.get(change_index).with_context(|| {
            format!("state-restore install graph refers to change {change_index}")
        })?;
        if self.results[change_index].is_some() {
            bail!(
                "state-restore install graph applies '{}' more than once",
                prepared.pkg.name()
            );
        }
        let stored_files = self
            .stored_files
            .get(change_index)
            .context("state-restore install payload has no stored files")?;
        let resolved_files = inner::resolve_stored_install_files(
            self.selected_root.selected_root(),
            stored_files,
            prepared.semantics,
        )?;
        let directory_plan = inner::preflight_resolved_file_ownership(
            self.tx,
            self.selected_root.selected_root(),
            &resolved_files,
            prepared.pkg.name(),
            &[],
            prepared.semantics,
        )?;
        let all_package_files =
            super::super::live_root_files_from_stored_files(self.cas, &resolved_files)?;
        let package_files = all_package_files
            .iter()
            .filter(|file| !directory_plan.preserves_leaf(&file.path))
            .cloned()
            .collect::<Vec<_>>();
        let through_symlink_files = directory_plan.through_symlink_root_files(&resolved_files);
        let retain_old_payload = self.native_transaction.requires_upgrade_payload_boundary();
        let old_trove_id = prepared
            .old_trove_to_upgrade
            .as_ref()
            .and_then(|trove| trove.id);
        let immediately_replaced = if retain_old_payload {
            Vec::new()
        } else {
            old_trove_id.into_iter().collect::<Vec<_>>()
        };
        let mut captured = crate::commands::generation::config_transaction::capture_install(
            self.tx,
            self.selected_root.selected_root(),
            self.cas,
            crate::commands::generation::config_transaction::ConfigInstallCapture {
                source: super::super::config_files::source_for_semantics(prepared.semantics),
                declared: prepared.pkg.config_files(),
                incoming: &package_files,
                replacing_trove_id: old_trove_id,
                replaced_trove_ids: &immediately_replaced,
            },
        )?;
        let mut config_plan = super::super::config_files::prepare_config_install(
            self.tx,
            self.selected_root.selected_root(),
            super::super::config_files::source_for_semantics(prepared.semantics),
            prepared.pkg.config_files(),
            old_trove_id,
            package_files,
        )?;
        let hardlink_references = super::super::execute::prepare_preserved_hardlink_references(
            self.tx,
            &directory_plan,
            &all_package_files,
            &mut config_plan.files,
        )?;
        self.selected_root
            .apply_install_files_with_references(&config_plan.files, &hardlink_references)?;
        self.selected_root
            .apply_install_files(&through_symlink_files)?;
        self.selected_root
            .apply_remove_paths(&config_plan.remove_paths)?;
        let file_capabilities = self
            .normalized_file_capabilities
            .get(change_index)
            .context("state-restore payload has no file-capability contract")?
            .as_deref();
        if let Some(file_capabilities) = file_capabilities {
            super::super::file_capabilities::apply_selected_file_capabilities(
                self.selected_root.selected_root(),
                file_capabilities,
                config_plan.files.iter(),
            )?;
        }
        let root = self
            .selected_root
            .selected_root()
            .to_str()
            .context("selected state-restore root is not UTF-8")?;
        let ctx = restore_transaction_context(
            prepared,
            self.db_path,
            root,
            retain_old_payload,
            file_capabilities,
        );
        let installed = inner::install_inner_with_stored_files(
            self.tx,
            self.changeset_id,
            prepared.pkg.as_ref(),
            &prepared.extraction,
            &ctx,
            &resolved_files,
            &directory_plan,
        )?;
        if let Some(contract) = prepared.ccs_contract.as_ref()
            && let Some(capabilities) = contract.capabilities.as_ref()
        {
            conary_core::capability::store_capabilities(self.tx, installed.trove_id, capabilities)?;
        }
        let identity = NativePackageIdentity::new(
            prepared.pkg.name(),
            prepared.pkg.version(),
            prepared.pkg.architecture(),
        );
        self.native_transaction
            .mark_install_payload_applied_for(self.tx, &identity)?;
        merge_config_transaction(self.config_transaction, &mut captured)?;
        self.results[change_index] = Some(installed);
        Ok(())
    }

    fn finalize_old_payload(
        &mut self,
        _conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        let Some(Some(trove_id)) = self.finalization_troves.get(change_index) else {
            return Ok(());
        };
        let Some(installed) = self.results.get(change_index).and_then(Option::as_ref) else {
            bail!("state-restore graph finalized an old payload before installing its replacement");
        };
        if installed.retained_upgrade_trove_id != Some(*trove_id) {
            return Ok(());
        }
        if let Some(Some(identity)) = self.removing_deb_identities.get(change_index) {
            self.native_transaction
                .mark_remove_payload_started_for(self.tx, identity)?;
        }
        let selected_path = self.selected_root.selected_root().to_path_buf();
        finalize_owned_trove(
            self.tx,
            self.selected_root,
            &selected_path,
            *trove_id,
            &self.final_incoming_paths,
        )?;
        if let Some(Some(identity)) = self.removing_deb_identities.get(change_index) {
            self.native_transaction.mark_remove_payload_completed_for(
                self.tx,
                identity,
                change_index,
            )?;
        }
        Ok(())
    }

    fn purge_config_files(
        &mut self,
        _conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        bail!("state-restore install graph unexpectedly purges change {change_index}")
    }
}

fn restore_transaction_context<'a>(
    prepared: &'a PreparedInstall,
    db_path: &'a str,
    root: &'a str,
    retain_old_payload: bool,
    file_capabilities: Option<&'a [conary_core::ccs::manifest::FileCapability]>,
) -> TransactionContext<'a> {
    TransactionContext {
        db_path,
        root,
        semantics: prepared.semantics,
        selection_reason: prepared.selection_reason.as_deref(),
        old_trove_to_upgrade: prepared.old_trove_to_upgrade.as_ref(),
        ccs_capabilities: prepared
            .ccs_contract
            .as_ref()
            .and_then(|contract| contract.capabilities.as_ref()),
        ccs_file_capabilities: file_capabilities,
        defer_generation: false,
        repository_provenance: prepared.repository_provenance.clone(),
        native_lifecycle_bundle: prepared.native_lifecycle_state.bundle_to_persist.as_ref(),
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: retain_old_payload,
    }
}

fn merge_config_transaction(
    transaction: &mut GenerationConfigTransaction,
    captured: &mut GenerationConfigTransaction,
) -> Result<()> {
    captured.validate()?;
    for next in captured.entries.drain(..) {
        if let Some(existing) = transaction
            .entries
            .iter_mut()
            .find(|entry| entry.path == next.path)
        {
            existing.operation = if next.after.is_some() {
                ConfigTransactionOperation::Install
            } else {
                next.operation
            };
            existing.after = next.after;
        } else {
            transaction.entries.push(next);
        }
    }
    Ok(())
}

fn deb_identity_for_trove(
    conn: &rusqlite::Connection,
    trove_id: i64,
) -> Result<Option<NativePackageIdentity>> {
    let Some(installed) = InstalledNativeLifecycleBundle::find_by_trove(conn, trove_id)? else {
        return Ok(None);
    };
    let bundle = installed.bundle()?;
    Ok((bundle.source_format == SourceFormat::Deb).then(|| {
        NativePackageIdentity::new(
            &installed.source_package,
            &installed.source_version,
            installed.source_arch.as_deref(),
        )
    }))
}

fn finalize_restore_installs(
    conn: &rusqlite::Connection,
    root: &str,
    changeset_id: i64,
    prepared_installs: &[PreparedInstall],
    installed_trove_ids: &[i64],
) -> Result<()> {
    for (prepared, trove_id) in prepared_installs.iter().zip(installed_trove_ids) {
        let progress = InstallProgress::single("Restoring");
        finalize_install_without_snapshot(
            conn,
            prepared.pkg.as_ref(),
            &prepared.extraction,
            root,
            &InstallTransactionResult {
                trove_id: *trove_id,
                changeset_id,
                triggers_executed: true,
            },
            FinalizeInstallOutput::new(&progress, false),
        )?;
    }
    Ok(())
}
