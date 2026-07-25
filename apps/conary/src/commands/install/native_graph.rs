// src/commands/install/native_graph.rs

//! Shared app-layer execution of the typed native transaction graph.

use super::native_events::PreparedNativeTransaction;
use anyhow::Result;
use conary_core::ccs::native_transaction::{NativeEventStage, NativeTransactionStep};
use conary_core::db::models::{FileEntry, Trove};
use conary_core::scriptlet::ExecutionMode;
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) trait TransactionRootMutation {
    fn apply_install_files(&mut self, files: &[crate::commands::LiveRootFile]) -> Result<()>;
    fn apply_remove_paths(&mut self, paths: &[String]) -> Result<()>;
}

impl TransactionRootMutation for crate::commands::LiveRootTransaction {
    fn apply_install_files(&mut self, files: &[crate::commands::LiveRootFile]) -> Result<()> {
        crate::commands::LiveRootTransaction::apply_install_files(self, files)?;
        Ok(())
    }

    fn apply_remove_paths(&mut self, paths: &[String]) -> Result<()> {
        crate::commands::LiveRootTransaction::apply_remove_paths(self, paths)?;
        Ok(())
    }
}

impl TransactionRootMutation for crate::commands::generation::selected_root::SelectedRootSession {
    fn apply_install_files(&mut self, files: &[crate::commands::LiveRootFile]) -> Result<()> {
        Self::apply_install_files(self, files)
    }

    fn apply_remove_paths(&mut self, paths: &[String]) -> Result<()> {
        Self::apply_remove_paths(self, paths)
    }
}

pub(crate) trait NativeGraphPayloadMutation {
    fn apply_payload(&mut self, conn: &rusqlite::Connection, change_index: usize) -> Result<()>;
    fn finalize_old_payload(
        &mut self,
        conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()>;
    fn purge_config_files(
        &mut self,
        conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativePayloadBoundary {
    Apply { change_index: usize },
    FinalizeOld { change_index: usize },
    PurgeConfig { change_index: usize },
}

pub(crate) fn drive_native_graph_with(
    conn: &rusqlite::Connection,
    native_transaction: &PreparedNativeTransaction,
    selected_root: &Path,
    execution_mode: &ExecutionMode,
    callback: impl FnMut(&rusqlite::Connection, NativePayloadBoundary) -> Result<()>,
) -> Result<()> {
    let mut payload = CallbackPayload { callback };
    drive_native_graph(
        conn,
        native_transaction,
        selected_root,
        execution_mode,
        &mut payload,
    )
}

struct CallbackPayload<F> {
    callback: F,
}

impl<F> NativeGraphPayloadMutation for CallbackPayload<F>
where
    F: FnMut(&rusqlite::Connection, NativePayloadBoundary) -> Result<()>,
{
    fn apply_payload(&mut self, conn: &rusqlite::Connection, change_index: usize) -> Result<()> {
        (self.callback)(conn, NativePayloadBoundary::Apply { change_index })
    }

    fn finalize_old_payload(
        &mut self,
        conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        (self.callback)(conn, NativePayloadBoundary::FinalizeOld { change_index })
    }

    fn purge_config_files(
        &mut self,
        conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        (self.callback)(conn, NativePayloadBoundary::PurgeConfig { change_index })
    }
}

pub(crate) fn drive_native_graph(
    conn: &rusqlite::Connection,
    native_transaction: &PreparedNativeTransaction,
    selected_root: &Path,
    execution_mode: &ExecutionMode,
    payload: &mut impl NativeGraphPayloadMutation,
) -> Result<()> {
    let mut arch_finalizer_executed = false;
    for step in native_transaction.graph_steps() {
        match *step {
            NativeTransactionStep::RunEvent { event_index } => {
                if !arch_finalizer_executed
                    && native_transaction.graph_event_stage(event_index)?
                        == NativeEventStage::ArchPostTransaction
                {
                    native_transaction.execute_graph_transaction_finalizer(selected_root)?;
                    arch_finalizer_executed = true;
                }
                native_transaction.execute_graph_event(
                    conn,
                    event_index,
                    selected_root,
                    execution_mode,
                )?;
            }
            NativeTransactionStep::PersistDebTriggerActivations {
                transaction_index,
                boundary,
                ..
            } => {
                native_transaction.persist_trigger_activations_for(
                    conn,
                    transaction_index,
                    boundary,
                )?;
            }
            NativeTransactionStep::ApplyPayload { change_index } => {
                payload.apply_payload(conn, change_index)?;
            }
            NativeTransactionStep::FinalizeOldPayload { change_index } => {
                payload.finalize_old_payload(conn, change_index)?;
            }
            NativeTransactionStep::PurgeConfigFiles { change_index } => {
                payload.purge_config_files(conn, change_index)?;
            }
        }
    }
    if !arch_finalizer_executed {
        native_transaction.execute_graph_transaction_finalizer(selected_root)?;
    }
    native_transaction.refresh_debian_admin_projection(conn, selected_root)?;
    Ok(())
}

pub(crate) fn finalize_owned_trove(
    tx: &rusqlite::Transaction<'_>,
    root_mutation: &mut impl TransactionRootMutation,
    root: &Path,
    trove_id: i64,
    final_incoming_paths: &BTreeSet<String>,
) -> Result<()> {
    let removable_paths = FileEntry::find_by_trove(tx, trove_id)?
        .into_iter()
        .map(|file| file.path)
        .filter(|path| !final_incoming_paths.contains(&normalize_archive_path(path)))
        .collect::<Vec<_>>();
    let mut config_plan =
        super::config_files::prepare_config_removal(tx, root, trove_id, removable_paths, false)?;
    root_mutation.apply_remove_paths(&config_plan.remove_paths)?;
    root_mutation.apply_install_files(&config_plan.files)?;
    config_plan.persist_retained(tx)?;
    super::transaction::delete_non_residual_config_rows(tx, trove_id)?;
    Trove::delete(tx, trove_id)?;
    Ok(())
}

pub(crate) fn normalize_archive_path(path: &str) -> String {
    path.split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect::<Vec<_>>()
        .join("/")
}
