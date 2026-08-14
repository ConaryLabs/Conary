// apps/conary/src/commands/generation/selected_root/publication_authority.rs

//! SQLite-owned selected-root publication and recovery authority.

use anyhow::{Context, Result};
use conary_core::db::models::GenerationPublication;
use conary_core::generation::root_manifest::{CapturedSelectedRoot, SelectedRootSnapshot};

pub(crate) fn load_publication_snapshot(
    conn: &rusqlite::Connection,
    debt: &GenerationPublication,
) -> Result<SelectedRootSnapshot> {
    let debt_id = debt
        .id
        .context("selected-root publication authority requires a persisted debt id")?;
    let snapshot_id = conn.query_row(
        "SELECT selected_root_snapshot_id FROM generation_publications WHERE id = ?1",
        [debt_id],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    let snapshot_id = snapshot_id.with_context(|| {
        format!("generation publication debt {debt_id} has no selected-root snapshot authority")
    })?;
    SelectedRootSnapshot::find(conn, snapshot_id)?
        .with_context(|| format!("selected-root snapshot {snapshot_id} is missing"))
}

pub(crate) fn load_publication_selected_root(
    conn: &rusqlite::Connection,
    debt: &GenerationPublication,
) -> Result<CapturedSelectedRoot> {
    load_publication_snapshot(conn, debt)?
        .materialize(conn)
        .map_err(Into::into)
}

#[cfg(test)]
pub(crate) fn persist_captured_publication_snapshot(
    conn: &rusqlite::Connection,
    debt: &GenerationPublication,
    captured: &CapturedSelectedRoot,
) -> Result<SelectedRootSnapshot> {
    let snapshot = SelectedRootSnapshot::capture(conn, captured)?;
    persist_publication_snapshot(conn, debt, snapshot)?;
    Ok(snapshot)
}

pub(super) fn latest_selected_root_snapshot(
    conn: &rusqlite::Connection,
) -> Result<Option<(SelectedRootSnapshot, CapturedSelectedRoot)>> {
    let debts = GenerationPublication::pending_recoverable(conn)?;
    let Some(latest) = debts.last() else {
        return Ok(None);
    };
    let snapshot = load_publication_snapshot(conn, latest)?;
    let captured = snapshot.materialize(conn)?;
    Ok(Some((snapshot, captured)))
}

pub(crate) fn persist_publication_snapshot(
    conn: &rusqlite::Connection,
    debt: &GenerationPublication,
    snapshot: SelectedRootSnapshot,
) -> Result<()> {
    debt.bind_selected_root_snapshot(conn, snapshot.id())?;
    Ok(())
}
