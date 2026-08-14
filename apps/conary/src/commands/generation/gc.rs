// apps/conary/src/commands/generation/gc.rs

//! Generation and local CAS garbage collection.
//!
//! The command holds the canonical mutation lock and resolves every persisted
//! reachability authority before deleting either generation state or CAS
//! objects.

use super::cleanup::remove_generation_etc_state;
use super::metadata::is_generation_pending;
use super::selected_root::load_publication_selected_root;
use crate::commands::changeset_metadata::parse_rollback_authority;
use crate::commands::format_bytes;
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::GenerationPublication;
use conary_core::generation::gc::{CasReachability, gc_cas_objects};
use conary_core::generation::mount::current_generation;
use conary_core::generation::root_manifest::{GenerationRootManifest, MutableStateManifest};
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const GC_ROOTS_SETTING_KEY: &str = "generation.gc_roots";

struct GenerationGcPlan {
    pending_to_remove: Vec<i64>,
    complete_to_remove: Vec<i64>,
    live_cas_hashes: HashSet<String>,
}

pub async fn cmd_generation_gc(keep: usize, db_path: &str) -> Result<()> {
    let runtime_root = runtime_root_for_generation_db_path(db_path);
    let mut engine = TransactionEngine::new(TransactionConfig::from_paths(
        runtime_root.root().to_path_buf(),
        db_path.into(),
    ))?;
    engine.begin()?;
    let result = cmd_generation_gc_locked(keep, db_path, &runtime_root);
    engine.release_lock();
    result
}

pub(super) fn cmd_generation_gc_locked(
    keep: usize,
    db_path: &str,
    runtime_root: &ConaryRuntimeRoot,
) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    let plan = build_gc_plan(keep, &conn, runtime_root)?;

    let mut removed_count = 0u64;
    let mut freed_bytes = 0u64;
    for generation in &plan.pending_to_remove {
        remove_generation_directory(runtime_root, *generation, true, &mut freed_bytes)?;
        removed_count += 1;
    }
    for generation in &plan.complete_to_remove {
        remove_generation_directory(runtime_root, *generation, false, &mut freed_bytes)?;
        remove_bls_entry(*generation)?;
        removed_count += 1;
    }

    println!(
        "Collected {removed_count} generation(s), freed {}.",
        format_bytes(freed_bytes)
    );
    let stats = gc_cas_objects(&runtime_root.objects_dir(), &plan.live_cas_hashes)?;
    if stats.objects_removed > 0 {
        println!(
            "CAS GC: removed {} of {} objects, freed {}.",
            stats.objects_removed,
            stats.objects_checked,
            format_bytes(stats.bytes_freed)
        );
    } else {
        println!(
            "CAS GC: checked {} objects, all reachable or within the creation grace period.",
            stats.objects_checked
        );
    }
    Ok(())
}

fn build_gc_plan(
    keep: usize,
    conn: &Connection,
    runtime_root: &ConaryRuntimeRoot,
) -> Result<GenerationGcPlan> {
    let current = current_generation(runtime_root.root())?;
    let gc_roots = load_gc_roots(conn)?;
    let publication_debts = GenerationPublication::pending_recoverable(conn)?;
    let (mut complete, mut pending) = generation_numbers(runtime_root)?;
    complete.sort_unstable();
    pending.sort_unstable();

    let mut keep_set = HashSet::new();
    if let Some(current) = current {
        keep_set.insert(current);
    }
    if let Some(booted) = booted_generation(runtime_root)? {
        keep_set.insert(booted);
    }
    keep_set.extend(gc_roots);
    keep_set.extend(
        publication_debts
            .iter()
            .filter_map(|debt| debt.generation_number),
    );
    let recent_start = complete.len().saturating_sub(keep);
    keep_set.extend(complete[recent_start..].iter().copied());

    let complete_to_remove = complete
        .iter()
        .filter(|number| !keep_set.contains(number))
        .copied()
        .collect::<Vec<_>>();
    let pending_to_remove = pending
        .iter()
        .filter(|number| !keep_set.contains(number))
        .copied()
        .collect::<Vec<_>>();

    // All authority resolution and validation happens before either removal
    // list is applied.
    let mut reachability = CasReachability::new();
    for generation in complete
        .iter()
        .chain(pending.iter())
        .filter(|number| keep_set.contains(number))
    {
        protect_generation_manifests(&mut reachability, runtime_root, *generation)?;
    }
    for debt in &publication_debts {
        let selected_root = load_publication_selected_root(conn, debt).with_context(|| {
            format!(
                "cannot resolve recoverable publication snapshot {}; refusing generation GC",
                debt.id.unwrap_or_default()
            )
        })?;
        reachability.protect_selected_root(
            &format!(
                "recoverable publication snapshot {}",
                debt.id.unwrap_or_default()
            ),
            &selected_root,
        )?;
        for content in debt.config_transaction.regular_content_authorities() {
            reachability.protect_content(
                &format!(
                    "recoverable publication config transaction {}",
                    debt.id.unwrap_or_default()
                ),
                content,
            )?;
        }
    }
    protect_effective_rollback_authority(conn, &mut reachability)?;
    reachability.protect_current_database(conn)?;
    reachability
        .protect_derived_artifact_contents(conn, &runtime_root.objects_dir())
        .context("derived CAS authority is incomplete; refusing generation GC")?;
    reachability
        .validate_objects_exist(&runtime_root.objects_dir())
        .context("live CAS authority is incomplete; refusing generation GC")?;

    Ok(GenerationGcPlan {
        pending_to_remove,
        complete_to_remove,
        live_cas_hashes: reachability.into_hashes(),
    })
}

fn generation_numbers(runtime_root: &ConaryRuntimeRoot) -> Result<(Vec<i64>, Vec<i64>)> {
    let directory = runtime_root.generations_dir();
    if !directory.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut complete = Vec::new();
    let mut pending = Vec::new();
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        let Some(number) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        if is_generation_pending(&entry.path()) {
            pending.push(number);
        } else {
            complete.push(number);
        }
    }
    Ok((complete, pending))
}

fn protect_generation_manifests(
    reachability: &mut CasReachability,
    runtime_root: &ConaryRuntimeRoot,
    generation: i64,
) -> Result<()> {
    let directory = runtime_root.generation_path(generation);
    let immutable = GenerationRootManifest::read_from(&directory).with_context(|| {
        format!(
            "cannot read surviving generation {generation} root authority; refusing generation GC"
        )
    })?;
    let mutable = MutableStateManifest::read_from(&directory).with_context(|| {
        format!(
            "cannot read surviving generation {generation} mutable-state authority; refusing generation GC"
        )
    })?;
    let context = format!("surviving generation {generation}");
    reachability.protect_generation_manifest(&context, &immutable)?;
    reachability.protect_mutable_state_manifest(&context, &mutable)?;
    Ok(())
}

fn protect_effective_rollback_authority(
    conn: &Connection,
    reachability: &mut CasReachability,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT candidate.id, candidate.metadata,
                candidate.rollback_selected_root_snapshot_id
         FROM changesets AS candidate
         WHERE candidate.kind = 'mutation'
           AND candidate.status = 'applied'
           AND candidate.reversed_by_changeset_id IS NULL
         ORDER BY candidate.id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id = row.get::<_, i64>(0)?;
        let metadata = row.get::<_, Option<String>>(1)?;
        let snapshot_id = row.get::<_, Option<i64>>(2)?;
        let Some(metadata) = metadata else {
            if snapshot_id.is_some() {
                return Err(anyhow!(
                    "changeset {id} has selected-root rollback authority without metadata; refusing generation GC"
                ));
            }
            continue;
        };
        let Some(authority) = parse_rollback_authority(conn, id, &metadata).with_context(|| {
            format!("changeset {id} has invalid current rollback authority; refusing generation GC")
        })?
        else {
            continue;
        };

        let context = format!("rollback-eligible changeset {id}");
        let selected_root = authority.selected_root.materialize(conn)?;
        reachability.protect_selected_root(&context, &selected_root)?;
        for trove in &authority.removed_troves {
            for file in &trove.files {
                if let Some(content) = &file.content {
                    reachability.protect_content(&context, content)?;
                }
            }
            for config in &trove.config_files {
                if let Some(original) = config.original_hash.as_deref() {
                    reachability.protect_hash(&context, original)?;
                }
                for backup in &config.backups {
                    reachability.protect_hash(&context, &backup.backup_hash)?;
                }
            }
        }
    }
    Ok(())
}

fn remove_generation_directory(
    runtime_root: &ConaryRuntimeRoot,
    generation: i64,
    pending: bool,
    freed_bytes: &mut u64,
) -> Result<()> {
    let directory = runtime_root.generation_path(generation);
    let size = dir_size_bytes(&directory);
    std::fs::remove_dir_all(&directory).with_context(|| {
        format!(
            "failed to remove {}generation {generation}; CAS GC was not started",
            if pending { "incomplete " } else { "" }
        )
    })?;
    *freed_bytes += size;
    info!(
        "Removed {}generation {generation}",
        if pending { "incomplete " } else { "" }
    );
    if let Err(error) = remove_generation_etc_state(runtime_root.root(), generation) {
        crate::ui::warn(&format!(
            "failed to remove etc-state directories for generation {generation}: {error}"
        ));
    }
    Ok(())
}

fn remove_bls_entry(generation: i64) -> Result<()> {
    let path = PathBuf::from(format!("/boot/loader/entries/conary-gen-{generation}.conf"));
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove BLS entry {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn runtime_root_for_generation_db_path(db_path: &str) -> ConaryRuntimeRoot {
    ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path))
}

fn booted_generation(runtime_root: &ConaryRuntimeRoot) -> Result<Option<i64>> {
    let cmdline = std::fs::read_to_string("/proc/cmdline")
        .context("Cannot read /proc/cmdline; refusing generation GC without boot authority")?;
    booted_generation_from_cmdline(&cmdline, runtime_root.root())
}

pub(super) fn booted_generation_from_cmdline(
    cmdline: &str,
    conary_root: &Path,
) -> Result<Option<i64>> {
    let Some(generation) = super::activation_intents::generation_from_kernel_cmdline(cmdline)?
    else {
        return Ok(None);
    };
    let directory = conary_root.join("generations").join(generation.to_string());
    if directory.is_dir() {
        Ok(Some(generation))
    } else {
        warn!(
            "Ignoring booted generation {} because {} does not exist",
            generation,
            directory.display()
        );
        Ok(None)
    }
}

pub(super) fn load_gc_roots(conn: &Connection) -> Result<Vec<i64>> {
    use conary_core::db::models::settings;

    match settings::get(conn, GC_ROOTS_SETTING_KEY)? {
        Some(serialized) => parse_gc_root_setting(&serialized),
        None => Ok(Vec::new()),
    }
}

pub(super) fn parse_gc_root_setting(serialized: &str) -> Result<Vec<i64>> {
    let mut generations = serde_json::from_str::<Vec<i64>>(serialized).map_err(|error| {
        anyhow!(
            "Stored generation GC roots are corrupt; refusing to discard pin authority: {error}"
        )
    })?;
    generations.sort_unstable();
    generations.dedup();
    Ok(generations)
}

fn dir_size_bytes(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}

#[cfg(test)]
mod tests;
