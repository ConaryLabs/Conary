// apps/conary/src/commands/remove/transaction.rs

use super::types::{RemoveInnerResult, RemoveLifecycleOptions};
use super::{
    PackagePayloadOwnership, execute_preflighted_ccs_remove_hook, preflight_ccs_remove_hook,
};
use crate::commands::progress::{RemovePhase, RemoveProgress};
use crate::commands::{TroveSnapshot, capture_trove_snapshot};
use anyhow::Result;
use conary_core::db::models::{
    ConfigFile, FileEntry, PackageTransactionStaging, StagedHistoryAction, StagedHistoryRow, Trove,
};
use std::collections::BTreeMap;

pub(crate) struct PreparedRemove {
    pub(crate) snapshot: TroveSnapshot,
    trove: Trove,
    materialized_removal_paths: Vec<String>,
}

impl PreparedRemove {
    pub(crate) fn materialized_removal_paths(&self) -> &[String] {
        &self.materialized_removal_paths
    }
}

pub(super) fn prepare_remove(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    lifecycle_options: RemoveLifecycleOptions,
    progress: &RemoveProgress,
) -> Result<PreparedRemove> {
    prepare_remove_with_dependency_validation(conn, trove, root, lifecycle_options, progress, true)
}

pub(crate) fn prepare_remove_for_state_restore(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    lifecycle_options: RemoveLifecycleOptions,
    progress: &RemoveProgress,
) -> Result<PreparedRemove> {
    prepare_remove_with_dependency_validation(conn, trove, root, lifecycle_options, progress, false)
}

fn prepare_remove_with_dependency_validation(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    lifecycle_options: RemoveLifecycleOptions,
    progress: &RemoveProgress,
    validate_dependencies: bool,
) -> Result<PreparedRemove> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let files = FileEntry::find_by_trove(conn, trove_id)?;
    let ownership = PackagePayloadOwnership::load(conn, trove_id)?;
    let ccs_remove_hook =
        preflight_ccs_remove_hook(conn, trove, root, lifecycle_options.sandbox_mode)?;
    if validate_dependencies {
        let breaking_now =
            conary_core::resolver::solve_removal(conn, std::slice::from_ref(&trove.name))?;
        if !breaking_now.is_empty() {
            return Err(conary_core::Error::IoError(format!(
                "Concurrent change: '{}' now required by: {}",
                trove.name,
                breaking_now.join(", ")
            ))
            .into());
        }
    }

    // NOTE: If the CCS pre-remove hook partially executes and then fails, there
    // is no automatic recovery of the hook's external side effects.
    if let Some(hook) = ccs_remove_hook.as_ref() {
        progress.set_phase(RemovePhase::PreScript);
        execute_preflighted_ccs_remove_hook(trove, hook, root, lifecycle_options.sandbox_mode)?;
    }

    Ok(PreparedRemove {
        snapshot: snapshot_trove_with_files(conn, trove, ccs_remove_hook.as_ref(), &files)?,
        trove: trove.clone(),
        materialized_removal_paths: ownership.materialized_removal_paths().to_vec(),
    })
}

pub(crate) fn snapshot_trove(conn: &rusqlite::Connection, trove: &Trove) -> Result<TroveSnapshot> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;
    let files = FileEntry::find_by_trove(conn, trove_id)?;
    let ccs_remove_hook = super::load_ccs_remove_hook(conn, trove)?;
    snapshot_trove_with_files(conn, trove, ccs_remove_hook.as_ref(), &files)
}

fn snapshot_trove_with_files(
    conn: &rusqlite::Connection,
    trove: &Trove,
    ccs_remove_hook: Option<&conary_core::db::models::InstalledCcsRemoveHook>,
    files: &[FileEntry],
) -> Result<TroveSnapshot> {
    capture_trove_snapshot(conn, trove, ccs_remove_hook, files)
}

pub(crate) fn commit_remove_db(
    tx: &rusqlite::Transaction<'_>,
    changeset_id: i64,
    prepared: PreparedRemove,
) -> Result<RemoveInnerResult> {
    let trove_id = prepared
        .trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let snapshot_by_path = prepared
        .snapshot
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut package_rows = PackageTransactionStaging::begin(tx)?;
    for path in &prepared.materialized_removal_paths {
        package_rows.stage_history(&StagedHistoryRow {
            changeset_id,
            path: path.clone(),
            sha256_hash: snapshot_by_path
                .get(path.as_str())
                .and_then(|file| file.content.as_ref())
                .map(|content| content.sha256.clone()),
            action: StagedHistoryAction::Delete,
        })?;
    }
    package_rows.validate_and_reconcile()?;
    let sqlite_work = package_rows.finish()?;
    tracing::debug!(
        rows_loaded = sqlite_work.rows_loaded,
        statements = sqlite_work.total_statement_executions(),
        query_shapes = sqlite_work.query_shapes,
        "reconciled staged package removal rows"
    );

    ConfigFile::delete_non_debian_by_trove(tx, trove_id)?;
    Trove::delete(tx, trove_id)?;

    Ok(RemoveInnerResult {
        snapshot: prepared.snapshot,
        trove: prepared.trove,
    })
}
