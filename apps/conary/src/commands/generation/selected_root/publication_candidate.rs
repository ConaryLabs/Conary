// apps/conary/src/commands/generation/selected_root/publication_candidate.rs

//! Durable manifest-only inputs for selected-root publication replay.

use anyhow::{Context, Result, bail};
use conary_core::db::models::GenerationPublication;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, GenerationRootManifest, MutableStateManifest,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn publication_candidate_dir(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
) -> Result<PathBuf> {
    let id = debt
        .id
        .context("generation publication candidate requires a persisted debt id")?;
    Ok(runtime_root
        .root()
        .join("publication-roots")
        .join(id.to_string()))
}

pub(crate) fn load_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
) -> Result<CapturedSelectedRoot> {
    let candidate = publication_candidate_dir(runtime_root, debt)?;
    if !candidate.is_dir() {
        bail!(
            "selected-root publication candidate is missing for debt {} at {}",
            debt.id.unwrap_or_default(),
            candidate.display()
        );
    }
    if candidate.join("root").exists() {
        bail!(
            "selected-root publication candidate {} uses retired materialized-root storage; discard pending pre-alpha publication state and rebuild it from current authority",
            candidate.display()
        );
    }
    Ok(CapturedSelectedRoot {
        generation: GenerationRootManifest::read_from(&candidate)?,
        state: MutableStateManifest::read_from(&candidate)?,
    })
}

pub(crate) fn remove_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
) -> Result<()> {
    let candidate = publication_candidate_dir(runtime_root, debt)?;
    match fs::remove_dir_all(&candidate) {
        Ok(()) => {
            conary_core::filesystem::durable::sync_parent_directory(&candidate)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Persist an already-captured selected root as retryable publication input.
///
/// Rollback carries this authority in changeset metadata, so it has no live
/// selected-root session directory to transfer into the candidate.
pub(crate) fn persist_captured_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
    captured: &CapturedSelectedRoot,
) -> Result<()> {
    persist_publication_candidate_inner(runtime_root, debt, captured, true)
}

#[cfg(test)]
pub(crate) fn remove_terminal_publication_candidates(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
    candidates: &[GenerationPublication],
) -> Result<()> {
    for candidate in candidates {
        let id = candidate
            .id
            .context("publication candidate cleanup requires a persisted debt id")?;
        let current = GenerationPublication::find_by_id(conn, id)?
            .context("publication candidate cleanup debt disappeared")?;
        if current.recoverable {
            continue;
        }
        remove_publication_candidate(runtime_root, &current)?;
    }
    Ok(())
}

/// Remove publication inputs after the aggregate generation backup is durable.
///
/// A debt in `DatabaseBackedUp` replays from the generation artifact and
/// verified backup, so it no longer depends on its selected-root candidate.
/// Deleting candidates before the terminal DB update makes the crash tail
/// idempotent: retry can repeat deletion and only then complete the debt.
pub(crate) fn remove_backed_up_publication_candidates(
    runtime_root: &ConaryRuntimeRoot,
    candidates: &[GenerationPublication],
) -> Result<()> {
    for candidate in candidates {
        candidate
            .id
            .context("backed-up publication candidate requires a persisted debt id")?;
        remove_publication_candidate(runtime_root, candidate)?;
    }
    Ok(())
}

pub(super) fn latest_selected_root_candidate(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
) -> Result<Option<CapturedSelectedRoot>> {
    let debts = GenerationPublication::pending_recoverable(conn)?;
    let Some(latest) = debts.last() else {
        return Ok(None);
    };
    load_publication_candidate(runtime_root, latest).map(Some)
}

pub(super) fn persist_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
    captured: &CapturedSelectedRoot,
) -> Result<()> {
    persist_publication_candidate_inner(runtime_root, debt, captured, false)
}

fn persist_publication_candidate_inner(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
    captured: &CapturedSelectedRoot,
    remove_candidate_on_error: bool,
) -> Result<()> {
    let candidate = publication_candidate_dir(runtime_root, debt)?;
    remove_uncommitted_candidate_collision(&candidate)?;
    let candidates_root = candidate
        .parent()
        .context("publication candidate has no parent")?;
    fs::create_dir_all(candidates_root)?;
    let temporary = candidates_root.join(format!(".candidate-{}.tmp", uuid::Uuid::new_v4()));
    fs::create_dir(&temporary)?;
    let result = (|| -> Result<()> {
        captured.generation.write_to(&temporary)?;
        captured.state.write_to(&temporary)?;
        fs::rename(&temporary, &candidate)?;
        conary_core::filesystem::durable::sync_parent_directory(&candidate)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
        if remove_candidate_on_error {
            let _ = fs::remove_dir_all(&candidate);
        }
    }
    result
}

/// Remove a candidate left by a process that died before its SQLite insert
/// committed.
///
/// Candidate IDs come from an uncommitted AUTOINCREMENT row and are therefore
/// reusable after SQLite rolls that transaction back. Every caller reaches
/// this boundary while holding the single runtime mutation lock and immediately
/// after creating a new debt row, so an existing directory at the same ID
/// cannot belong to another live or committed transaction.
fn remove_uncommitted_candidate_collision(candidate: &Path) -> Result<()> {
    match fs::symlink_metadata(candidate) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(candidate)?;
            conary_core::filesystem::durable::sync_parent_directory(candidate)?;
            Ok(())
        }
        Ok(_) => bail!(
            "selected-root publication candidate has an unexpected file type: {}",
            candidate.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
