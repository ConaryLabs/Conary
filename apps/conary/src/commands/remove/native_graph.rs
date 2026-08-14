// apps/conary/src/commands/remove/native_graph.rs

//! Graph-driven removal inside one isolated selected root.

use super::transaction::{commit_remove_db, prepare_remove};
use super::types::{RemoveInnerResult, RemoveLifecycleOptions};
use crate::commands::generation::selected_root::{LockedRuntimeRoot, SelectedRootSession};
use crate::commands::install::native_events::PreparedNativeTransaction;
use crate::commands::install::native_graph::{NativePayloadBoundary, drive_native_graph_with};
use crate::commands::progress::{RemovePhase, RemoveProgress};
use anyhow::{Context, Result, bail};
use conary_core::ccs::native_transaction::{NativePackageIdentity, NativeTransactionStep};
use conary_core::db::models::{ActivationRequest, Changeset, ChangesetStatus, Trove};
use conary_core::scriptlet::ExecutionMode;
use std::path::PathBuf;

pub(crate) struct GraphRemoveResult {
    pub(crate) removal: RemoveInnerResult,
    pub(crate) stats: crate::commands::LiveRootStats,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_installed_trove_remove_graph(
    conn: &rusqlite::Connection,
    trove: &Trove,
    db_path: &str,
    package_name: &str,
    lifecycle_options: RemoveLifecycleOptions,
    progress: &RemoveProgress,
) -> Result<GraphRemoveResult> {
    let trove_id = trove
        .id
        .context("selected package has no installed trove id")?;
    let identity =
        NativePackageIdentity::new(&trove.name, &trove.version, trove.architecture.as_deref());
    let locked_root = LockedRuntimeRoot::acquire(db_path)?;
    let ownership = super::PackagePayloadOwnership::load(conn, trove_id)?;
    let native_transaction = PreparedNativeTransaction::prepare_remove(
        conn,
        trove_id,
        &trove.name,
        &trove.version,
        ownership.lifecycle_paths().to_vec(),
        lifecycle_options.purge_config_files,
    )?;
    conary_core::repository::enrollment::transaction::preflight_removal(conn, trove_id)
        .context("Package repository removal preflight failed")?;
    let selected = locked_root.prepare(conn, format!("Remove {}-{}", trove.name, trove.version))?;
    native_transaction.preflight(selected.selected_root(), &ExecutionMode::Remove)?;

    execute_selected_root_graph(
        conn,
        &native_transaction,
        &identity,
        trove,
        db_path,
        package_name,
        lifecycle_options,
        progress,
        selected,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_selected_root_graph(
    conn: &rusqlite::Connection,
    native_transaction: &PreparedNativeTransaction,
    identity: &NativePackageIdentity,
    trove: &Trove,
    db_path: &str,
    package_name: &str,
    lifecycle_options: RemoveLifecycleOptions,
    progress: &RemoveProgress,
    mut selected: SelectedRootSession,
) -> Result<GraphRemoveResult> {
    let trove_id = trove
        .id
        .context("selected package has no installed trove id")?;
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let rollback_root = selected.capture_rollback_authority()?;
    let rollback_system = crate::commands::RollbackSystemAuthority::capture(conn)?;
    let selected_path = selected.selected_root().to_path_buf();
    let cas = selected.cas().clone();
    let selected_root = selected_path
        .to_str()
        .context("selected root is not UTF-8")?
        .to_string();
    let summary = format!("Remove {package_name}");
    let mut changeset = Changeset::new(format!("Remove {}-{}", trove.name, trove.version));
    let tx = conn.unchecked_transaction()?;
    let changeset_id = changeset.insert(&tx)?;
    let mut output: Option<(RemoveInnerResult, crate::commands::LiveRootStats)> = None;
    let mut config_transaction = None;
    let split_debian_purge = native_transaction.graph_steps().iter().any(|step| {
        matches!(
            step,
            NativeTransactionStep::PurgeConfigFiles { change_index: 0 }
        )
    });
    let mut purge_plan = None;
    let graph_result = drive_native_graph_with(
        &tx,
        changeset_id,
        native_transaction,
        &selected_path,
        &ExecutionMode::Remove,
        |conn, boundary| match boundary {
            NativePayloadBoundary::Apply { change_index: 0 } => Ok(()),
            NativePayloadBoundary::FinalizeOld { change_index: 0 } => {
                if output.is_some() {
                    bail!("native removal graph finalizes the package more than once");
                }
                let prepared =
                    prepare_remove(conn, trove, &selected_root, lifecycle_options, progress)?;
                if split_debian_purge {
                    purge_plan = Some(
                        crate::commands::install::config_files::prepare_debian_config_purge(
                            conn,
                            &selected_path,
                            trove_id,
                        )?,
                    );
                }
                let captured_config =
                    crate::commands::generation::config_transaction::capture_removal(
                        conn,
                        &selected_path,
                        &cas,
                        trove_id,
                        lifecycle_options.purge_config_files,
                    )?;
                let mut config_plan =
                    crate::commands::install::config_files::prepare_config_removal(
                        conn,
                        &selected_path,
                        trove_id,
                        prepared.materialized_removal_paths().iter().cloned(),
                        lifecycle_options.purge_config_files && !split_debian_purge,
                    )?;
                native_transaction.mark_remove_payload_started_for(conn, identity)?;
                progress.set_phase(RemovePhase::RemovingFiles);
                let root_stats = selected.apply_remove_paths(&config_plan.remove_paths)?;
                selected.apply_install_files(&config_plan.files)?;

                progress.set_phase(RemovePhase::UpdatingDb);
                config_plan.persist_retained(conn)?;
                conary_core::repository::enrollment::transaction::apply_removal(&tx, trove_id)
                    .context("Failed to release package repository enrollment")?;
                let removal = commit_remove_db(&tx, changeset_id, prepared)?;
                let snapshot_json = crate::commands::metadata_with_removed_troves(
                    vec![removal.snapshot.clone()],
                    Vec::new(),
                    rollback_root,
                    rollback_system.clone(),
                )?;
                conn.execute(
                    "UPDATE changesets
                     SET metadata = ?1, rollback_selected_root_snapshot_id = ?2
                     WHERE id = ?3",
                    rusqlite::params![snapshot_json, rollback_root.id(), changeset_id],
                )?;
                output = Some((removal, root_stats));
                config_transaction = Some(captured_config);
                Ok(())
            }
            NativePayloadBoundary::PurgeConfig { change_index: 0 } => {
                let plan = purge_plan
                    .take()
                    .context("Debian purge boundary has no captured conffile plan")?;
                if output.is_none() {
                    bail!("Debian purge ran before ordinary payload removal");
                }
                selected.apply_remove_paths(&plan.remove_paths)?;
                plan.delete_rows(conn)?;
                Ok(())
            }
            NativePayloadBoundary::Apply { change_index }
            | NativePayloadBoundary::FinalizeOld { change_index }
            | NativePayloadBoundary::PurgeConfig { change_index } => {
                bail!("single-package removal graph refers to change {change_index}")
            }
        },
    );
    if let Err(error) = graph_result {
        drop(tx);
        selected
            .rollback()
            .context("failed to discard selected root after removal graph failure")?;
        return Err(error);
    }
    let (removal, stats) =
        output.context("native removal graph did not finalize the installed payload")?;
    let config_transaction =
        config_transaction.context("native removal graph did not capture config state")?;
    let activation_requests = native_transaction.take_activation_requests();
    ActivationRequest::append_batch(&tx, changeset_id, &activation_requests)
        .context("failed to persist exact generation activation requests from removal")?;
    changeset.update_status(&tx, ChangesetStatus::Applied)?;
    let publication_debt = crate::commands::generation::publication::record_selected_root_state(
        &tx,
        &crate::commands::generation::publication::PublicationRequest {
            db_path,
            summary: &summary,
            trigger_changeset_id: Some(changeset_id),
            tx_uuid: changeset.tx_uuid.as_deref(),
            config_transaction,
        },
    )?;
    if let Err(error) = selected.persist_for_publication(&tx, &runtime_root, &publication_debt) {
        drop(tx);
        return Err(error.context("failed to persist exact selected-root removal input"));
    }
    if let Err(error) = tx.commit() {
        return Err(error.into());
    }

    let outcome = crate::commands::generation::publication::publish_recorded_selected_root(
        conn,
        db_path,
        &summary,
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
    Ok(GraphRemoveResult { removal, stats })
}

#[cfg(test)]
#[path = "native_graph_tests.rs"]
mod tests;
