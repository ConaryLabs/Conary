// apps/conary/src/commands/adopt/system/live_root.rs

//! Full selected-root adoption when no native package manager is present.

use super::{
    LIVE_ROOT_PACKAGE_NAME, captured_root::capture_live_selected_root,
    captured_root::synchronize_captured_root, create_state_snapshot, open_db, write_db_checkpoint,
};
use anyhow::Result;
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{Changeset, ChangesetStatus};
use conary_core::filesystem::CasStore;

pub(super) fn adopt_live_root_as_full_package(
    db_path: &str,
    dry_run: bool,
    pattern: Option<&str>,
    exclude: Option<&str>,
    explicit_only: bool,
) -> Result<()> {
    if pattern.is_some() || exclude.is_some() || explicit_only {
        return Err(anyhow::anyhow!(
            "Full live-root adoption without a system package manager does not support package filters"
        ));
    }

    println!(
        "No supported package manager found; capturing exact selected-root continuity as {LIVE_ROOT_PACKAGE_NAME}"
    );
    if dry_run {
        println!("Dry run: would synchronize 1 typed captured-root authority");
        println!("  Package: {LIVE_ROOT_PACKAGE_NAME}");
        println!("  Mode: full (CAS storage)");
        return Ok(());
    }

    let cas = CasStore::new(conary_core::db::paths::objects_dir(db_path))?;
    let (captured, _work) = capture_live_selected_root(db_path, &cas)?;
    let mut conn = open_db(db_path)?;
    let mut changeset = Changeset::new(format!(
        "Synchronize selected-root authority ({LIVE_ROOT_PACKAGE_NAME})"
    ));

    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
    let (changeset_id, sync) = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;
        let sync = synchronize_captured_root(tx, changeset_id, &captured)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok((changeset_id, sync))
    })?;
    if sync.changed {
        create_state_snapshot(
            &conn,
            changeset_id,
            &format!("Synchronize selected root as {LIVE_ROOT_PACKAGE_NAME}"),
        )?;
    }
    write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

    println!(
        "Synchronized {LIVE_ROOT_PACKAGE_NAME}: {} unowned entries, {} package-owned entries (full)",
        sync.captured_entries, sync.package_entries
    );
    Ok(())
}
