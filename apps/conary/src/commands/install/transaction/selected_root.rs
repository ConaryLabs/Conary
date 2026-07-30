// apps/conary/src/commands/install/transaction/selected_root.rs

//! Single-package graph execution in an isolated selected root.

use super::{InstallTransactionResult, TransactionContext};
use crate::commands::generation::selected_root::SelectedRootSession;
use crate::commands::install::inner;
use crate::commands::install::native_events::PreparedNativeTransaction;
use crate::commands::install::native_graph::{
    NativeGraphPayloadMutation, drive_native_graph, finalize_owned_trove, normalize_archive_path,
};
use crate::commands::install::{ExtractionResult, InstallProgress};
use anyhow::{Context, Result, bail};
use conary_core::ccs::native_lifecycle::SourceFormat;
use conary_core::ccs::native_transaction::NativePackageIdentity;
use conary_core::db::models::{ActivationRequest, NewActivationRequest};
use conary_core::db::models::{Changeset, ChangesetStatus, InstalledNativeLifecycleBundle};
use conary_core::packages::PackageFormat;
use conary_core::scriptlet::ExecutionMode;
use conary_core::transaction::validate_package_relation_transitions;
use std::collections::BTreeSet;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_install_transaction_in_selected_root(
    conn: &mut rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    _progress: &InstallProgress,
    selected_root: &mut SelectedRootSession,
    native_transaction: &PreparedNativeTransaction,
    native_execution_mode: &ExecutionMode,
) -> Result<InstallTransactionResult> {
    execute_install_transaction_in_selected_root_with_post_graph(
        conn,
        pkg,
        extraction,
        ctx,
        _progress,
        selected_root,
        native_transaction,
        native_execution_mode,
        || Ok(Vec::new()),
    )
}

/// Execute one exact root mutation after the typed native graph and before
/// trigger capture/publication.
///
/// CCS post-install hooks belong at this boundary: their filesystem effects
/// are part of the selected-root authority and their failure must abort the
/// database transaction instead of failing after publication.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_install_transaction_in_selected_root_with_post_graph<F>(
    conn: &mut rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    _progress: &InstallProgress,
    selected_root: &mut SelectedRootSession,
    native_transaction: &PreparedNativeTransaction,
    native_execution_mode: &ExecutionMode,
    post_graph: F,
) -> Result<InstallTransactionResult>
where
    F: FnOnce() -> Result<Vec<NewActivationRequest>>,
{
    let selected_path = selected_root.selected_root().to_path_buf();
    if Path::new(ctx.root) != selected_path {
        bail!(
            "selected-root transaction path {} does not match install root {}",
            selected_path.display(),
            ctx.root
        );
    }
    validate_package_relation_transitions(
        conn,
        ctx.relation_removals,
        ctx.relation_deconfigurations,
    )
    .context("Package relation transaction preflight failed")?;
    super::preflight_generation_file_capabilities_for_install(ctx, extraction)?;
    inner::preflight_file_ownership(
        conn,
        extraction
            .extracted_files
            .iter()
            .map(|file| file.path.as_str()),
        pkg.name(),
        ctx.relation_removals,
    )?;
    let rollback_root = selected_root.capture_rollback_authority()?;
    let cas = selected_root.cas().clone();
    let result = execute_selected_root_inner(
        conn,
        pkg,
        extraction,
        ctx,
        &cas,
        selected_root,
        native_transaction,
        native_execution_mode,
        rollback_root,
        post_graph,
    );
    if result.is_err() {
        selected_root
            .rollback()
            .context("failed to roll back isolated selected-root transaction")?;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn execute_selected_root_inner<F>(
    conn: &mut rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    cas: &conary_core::filesystem::CasStore,
    selected_root: &mut SelectedRootSession,
    native_transaction: &PreparedNativeTransaction,
    native_execution_mode: &ExecutionMode,
    rollback_root: conary_core::generation::root_manifest::CapturedSelectedRoot,
    post_graph: F,
) -> Result<InstallTransactionResult>
where
    F: FnOnce() -> Result<Vec<NewActivationRequest>>,
{
    let stored_files = inner::store_install_files_in_cas(cas, extraction)?;
    let final_incoming_paths = extraction
        .extracted_files
        .iter()
        .map(|file| normalize_archive_path(&file.path))
        .collect::<BTreeSet<_>>();
    let finalization_troves = std::iter::once(ctx.old_trove_to_upgrade.and_then(|trove| trove.id))
        .chain(
            ctx.relation_removals
                .iter()
                .map(|removal| Some(removal.trove_id)),
        )
        .collect::<Vec<_>>();
    let removing_deb_identities = finalization_troves
        .iter()
        .map(|trove_id| {
            trove_id
                .map(|trove_id| deb_identity_for_trove(conn, trove_id))
                .transpose()
                .map(Option::flatten)
        })
        .collect::<Result<Vec<_>>>()?;

    let tx_description = ctx.old_trove_to_upgrade.map_or_else(
        || format!("Install {}-{}", pkg.name(), pkg.version()),
        |old_trove| {
            format!(
                "Upgrade {} from {} to {}",
                pkg.name(),
                old_trove.version,
                pkg.version()
            )
        },
    );
    let selected_path = selected_root.selected_root().to_path_buf();
    let mut changeset = Changeset::new(tx_description.clone());
    let tx = conn.unchecked_transaction()?;
    let changeset_id = changeset.insert(&tx)?;
    let materialized_directories =
        super::super::shared_directory::capture_existing_directory_materializations(
            &tx,
            &selected_path,
        )?;
    super::super::rollback_snapshot::record_install_rollback_snapshots(
        &tx,
        changeset_id,
        ctx.old_trove_to_upgrade,
        ctx.relation_removals,
        rollback_root,
        materialized_directories,
    )?;
    let installing_identity =
        NativePackageIdentity::new(pkg.name(), pkg.version(), pkg.architecture());
    let mut payload = SelectedRootPayload {
        tx: &tx,
        pkg,
        extraction,
        ctx,
        changeset_id,
        stored_files: &stored_files,
        cas,
        selected_root,
        native_transaction,
        installing_identity,
        finalization_troves,
        removing_deb_identities,
        final_incoming_paths,
        inner_result: None,
        config_transaction: None,
    };
    drive_native_graph(
        &tx,
        native_transaction,
        &selected_path,
        native_execution_mode,
        &mut payload,
    )?;
    let inner_result = payload
        .inner_result
        .take()
        .context("native transaction graph did not apply the incoming package payload")?;
    let config_transaction = payload
        .config_transaction
        .take()
        .context("native transaction graph did not capture the incoming config transaction")?;
    let ccs_activation_requests = post_graph()?;
    let mut activation_requests = native_transaction.take_activation_requests();
    activation_requests.extend(ccs_activation_requests);
    ActivationRequest::append_batch(&tx, changeset_id, &activation_requests)
        .context("failed to persist exact generation activation requests")?;

    let file_paths = extraction
        .extracted_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    crate::commands::install::run_triggers(&tx, &selected_path, changeset_id, &file_paths)?;
    changeset.update_status(&tx, ChangesetStatus::Applied)?;
    config_transaction.validate()?;
    if ctx.defer_generation {
        tx.commit()?;
    } else {
        let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(ctx.db_path);
        let publication_debt =
            crate::commands::generation::publication::record_selected_root_state(
                &tx,
                &crate::commands::generation::publication::PublicationRequest {
                    db_path: ctx.db_path,
                    summary: &tx_description,
                    trigger_changeset_id: Some(changeset_id),
                    tx_uuid: changeset.tx_uuid.as_deref(),
                    config_transaction,
                },
            )?;
        if let Err(error) = selected_root.persist_for_publication(&runtime_root, &publication_debt)
        {
            drop(tx);
            let _ = crate::commands::generation::selected_root::remove_publication_candidate(
                &runtime_root,
                &publication_debt,
            );
            return Err(error.context("failed to persist exact selected-root install input"));
        }
        if let Err(error) = tx.commit() {
            crate::commands::generation::selected_root::remove_publication_candidate(
                &runtime_root,
                &publication_debt,
            )
            .context("failed to discard install candidate after database commit error")?;
            return Err(error.into());
        }
        let outcome = crate::commands::generation::publication::publish_recorded_selected_root(
            conn,
            ctx.db_path,
            &tx_description,
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
    }

    Ok(InstallTransactionResult {
        trove_id: inner_result.trove_id,
        changeset_id,
        triggers_executed: true,
    })
}

struct SelectedRootPayload<'a, 'conn> {
    tx: &'a rusqlite::Transaction<'conn>,
    pkg: &'a dyn PackageFormat,
    extraction: &'a ExtractionResult,
    ctx: &'a TransactionContext<'a>,
    changeset_id: i64,
    stored_files: &'a [inner::StoredInstallFile],
    cas: &'a conary_core::filesystem::CasStore,
    selected_root: &'a mut SelectedRootSession,
    native_transaction: &'a PreparedNativeTransaction,
    installing_identity: NativePackageIdentity,
    finalization_troves: Vec<Option<i64>>,
    removing_deb_identities: Vec<Option<NativePackageIdentity>>,
    final_incoming_paths: BTreeSet<String>,
    inner_result: Option<inner::InnerInstallResult>,
    config_transaction: Option<conary_core::config_transaction::GenerationConfigTransaction>,
}

impl NativeGraphPayloadMutation for SelectedRootPayload<'_, '_> {
    fn apply_payload(&mut self, _conn: &rusqlite::Connection, change_index: usize) -> Result<()> {
        if change_index != 0 {
            return Ok(());
        }
        if self.inner_result.is_some() {
            bail!("native transaction graph applies the incoming package more than once");
        }
        let resolved_files = inner::resolve_stored_install_files(
            self.selected_root.selected_root(),
            self.stored_files,
            self.ctx.semantics,
        )?;
        let directory_plan = inner::preflight_resolved_file_ownership(
            self.tx,
            self.selected_root.selected_root(),
            &resolved_files,
            self.pkg.name(),
            self.ctx.relation_removals,
            self.ctx.semantics,
        )?;
        let all_package_files =
            super::super::live_root_files_from_stored_files(self.cas, &resolved_files)?;
        let package_files = all_package_files
            .iter()
            .filter(|file| !directory_plan.preserves_leaf(&file.path))
            .cloned()
            .collect::<Vec<_>>();
        let through_symlink_files = directory_plan.through_symlink_root_files(&resolved_files);
        let config_transaction = crate::commands::generation::config_transaction::capture_install(
            self.tx,
            self.selected_root.selected_root(),
            self.cas,
            crate::commands::generation::config_transaction::ConfigInstallCapture {
                source: super::super::config_files::source_for_semantics(self.ctx.semantics),
                declared: self.pkg.config_files(),
                incoming: &package_files,
                replacing_trove_id: self.ctx.old_trove_to_upgrade.and_then(|trove| trove.id),
                replaced_trove_ids: &[],
            },
        )?;
        let mut config_plan = super::super::config_files::prepare_config_install(
            self.tx,
            self.selected_root.selected_root(),
            super::super::config_files::source_for_semantics(self.ctx.semantics),
            self.pkg.config_files(),
            self.ctx.old_trove_to_upgrade.and_then(|trove| trove.id),
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
        if let Some(file_capabilities) = self.ctx.ccs_file_capabilities {
            super::super::file_capabilities::apply_selected_file_capabilities(
                self.selected_root.selected_root(),
                file_capabilities,
                config_plan.files.iter(),
            )?;
        }
        let installed = inner::install_inner_with_stored_files(
            self.tx,
            self.changeset_id,
            self.pkg,
            self.extraction,
            self.ctx,
            &resolved_files,
            &directory_plan,
        )?;
        if let Some(capabilities) = self.ctx.ccs_capabilities {
            conary_core::capability::store_capabilities(self.tx, installed.trove_id, capabilities)?;
        }
        self.native_transaction
            .mark_install_payload_applied_for(self.tx, &self.installing_identity)?;
        self.config_transaction = Some(config_transaction);
        self.inner_result = Some(installed);
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
        let Some(installed) = self.inner_result.as_ref() else {
            bail!("native transaction finalized an old payload before applying the incoming one");
        };
        let retained = installed.retained_upgrade_trove_id == Some(*trove_id)
            || installed.retained_relation_trove_ids.contains(trove_id);
        if !retained {
            return Ok(());
        }
        if let Some(Some(package)) = self.removing_deb_identities.get(change_index) {
            self.native_transaction
                .mark_remove_payload_started_for(self.tx, package)?;
        }
        let selected_path = self.selected_root.selected_root().to_path_buf();
        finalize_owned_trove(
            self.tx,
            self.selected_root,
            &selected_path,
            *trove_id,
            &self.final_incoming_paths,
        )?;
        if let Some(Some(package)) = self.removing_deb_identities.get(change_index) {
            self.native_transaction.mark_remove_payload_completed_for(
                self.tx,
                package,
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
        bail!("install transaction graph unexpectedly purges config for change {change_index}")
    }
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
