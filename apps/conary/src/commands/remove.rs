// src/commands/remove.rs
//! Package removal commands

use super::open_db;
use super::progress::{RemovePhase, RemoveProgress};
use super::{FileSnapshot, InstalledPackageSelector, TroveSnapshot, resolve_installed_package};
use anyhow::{Context, Result};
use conary_core::db::models::{FileEntry, ScriptletEntry, Trove};
use conary_core::scriptlet::{
    ExecutionMode, PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletExecutor,
};
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

pub(crate) struct RemoveInnerResult {
    pub(crate) snapshot: TroveSnapshot,
    trove: Trove,
    stored_scriptlets: Vec<ScriptletEntry>,
    scriptlet_format: ScriptletPackageFormat,
    removed_count: usize,
    dirs_removed: usize,
}

struct PreparedRemove {
    snapshot: TroveSnapshot,
    trove: Trove,
    stored_scriptlets: Vec<ScriptletEntry>,
    scriptlet_format: ScriptletPackageFormat,
    removed_count: usize,
    dirs_removed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoveExecutionPath {
    GenerationAware,
    MutableLiveRoot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AutoremoveSkipReason {
    AdoptedNativeAuthority,
    Pinned,
    Critical,
}

#[derive(Debug, Clone)]
struct AutoremovePlan {
    removable: Vec<Trove>,
    skipped: Vec<(Trove, AutoremoveSkipReason)>,
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
struct DirectRemovalStats {
    files_removed: usize,
    dirs_removed: usize,
}

/// Remove an installed package
#[allow(clippy::too_many_arguments)]
pub async fn cmd_remove(
    package_name: &str,
    db_path: &str,
    root: &str,
    version: Option<String>,
    architecture: Option<String>,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    purge_files: bool,
) -> Result<()> {
    info!("Removing package: {}", package_name);
    println!("Removing package: {}", package_name);
    std::io::stdout().flush()?;
    if let Ok(delay_ms) = std::env::var("CONARY_TEST_HOLD_DURING_REMOVE_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
        && delay_ms > 0
    {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }

    // Create progress tracker for removal
    let progress = RemoveProgress::new(package_name);

    let conn = open_db(db_path)?;
    let selector =
        InstalledPackageSelector::new(package_name.to_string(), version.clone(), architecture);
    let resolved = resolve_installed_package(&conn, &selector)
        .with_context(|| format!("Failed to select package '{}'", package_name))?;
    let trove = resolved.trove;

    // Check if package is pinned
    if trove.pinned {
        return Err(anyhow::anyhow!(
            "Package '{}' is pinned and cannot be removed. Use 'conary unpin {}' first.",
            package_name,
            package_name
        ));
    }

    if crate::commands::install::is_package_blocked(&trove.name) {
        anyhow::bail!(
            "Refusing to remove critical package '{}'. Use the native package manager for this system package.",
            trove.name
        );
    }

    if trove.install_source.is_adopted() && !purge_files {
        let pkg_mgr = conary_core::packages::SystemPackageManager::detect();
        anyhow::bail!(
            "Refusing to remove adopted package '{}': native package manager authority is preserved. \
             Use '{}' to uninstall it, 'conary system unadopt {}' to remove Conary tracking only, \
             or rerun with --purge-files only if deleting native-owned files is intentional.",
            package_name,
            pkg_mgr.remove_command(package_name),
            package_name
        );
    }

    // Check dependency breakage BEFORE any removal (including adopted packages)
    let breaking = conary_core::resolver::solve_removal(&conn, &[package_name.to_string()])?;

    if !breaking.is_empty() {
        println!(
            "WARNING: Removing '{}' would break the following packages:",
            package_name
        );
        for pkg in &breaking {
            println!("  {}", pkg);
        }
        println!("\nRefusing to remove package with dependencies.");
        println!(
            "Use 'conary query whatbreaks {}' for more information.",
            package_name
        );
        return Err(anyhow::anyhow!(
            "Cannot remove '{}': {} packages depend on it",
            package_name,
            breaking.len()
        ));
    }

    let mut engine = TransactionEngine::new(TransactionConfig::from_paths(
        PathBuf::from(root),
        db_path.into(),
    ))?;
    engine.begin()?;

    if trove.install_source.is_adopted() && purge_files {
        println!(
            "WARNING: --purge-files specified for adopted package '{}'. \
             Files will be deleted from disk.",
            package_name
        );
    }

    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    if remove_execution_path(db_path)? == RemoveExecutionPath::MutableLiveRoot {
        let result = (|| -> Result<(RemoveInnerResult, crate::commands::LiveRootStats)> {
            super::live_root::recover_pending_journals_with_changesets(
                runtime_root.root(),
                Path::new(root),
                &conn,
            )?;

            let tx_uuid = uuid::Uuid::new_v4().to_string();
            let tx_description = format!("Remove {}-{}", trove.name, trove.version);
            let prepared =
                prepare_remove(&conn, &trove, root, no_scripts, sandbox_mode, &progress)?;
            let remove_paths = prepared
                .snapshot
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>();
            let mut live_tx = crate::commands::LiveRootTransaction::begin(
                runtime_root.root(),
                Path::new(root),
                tx_uuid.clone(),
                format!("Remove {}", package_name),
            )?;
            progress.set_phase(RemovePhase::RemovingFiles);
            let stats = live_tx.apply_remove_paths(&remove_paths)?;

            progress.set_phase(RemovePhase::UpdatingDb);
            let tx = conn.unchecked_transaction()?;
            let mut changeset =
                conary_core::db::models::Changeset::with_tx_uuid(tx_description, tx_uuid.clone());
            let remove_changeset_id = changeset.insert(&tx)?;
            let remove_result = match commit_remove_db(&tx, remove_changeset_id, prepared) {
                Ok(result) => result,
                Err(error) => {
                    live_tx.rollback()?;
                    return Err(error);
                }
            };
            let snapshot_json = crate::commands::metadata_with_removed_troves(vec![
                remove_result.snapshot.clone(),
            ])?;
            tx.execute(
                "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
                rusqlite::params![snapshot_json, remove_changeset_id],
            )?;
            changeset.update_status(&tx, conary_core::db::models::ChangesetStatus::Applied)?;
            if let Err(error) = tx.commit() {
                if let Err(rollback_error) = live_tx.rollback() {
                    return Err(error)
                        .context(format!("Failed to rollback live root: {rollback_error}"));
                }
                return Err(error.into());
            }
            live_tx.commit()?;
            Ok((remove_result, stats))
        })();
        engine.release_lock();
        let (remove_result, stats) = result?;

        run_post_remove_scriptlet(&remove_result, root, no_scripts, sandbox_mode, &progress);
        progress.finish(&format!(
            "Removed {} {}",
            remove_result.trove.name, remove_result.trove.version
        ));
        print_remove_summary(&remove_result, &stats);
        return Ok(());
    }

    // DB-first approach: commit the DB transaction before removing files from disk.
    // If a crash occurs after the DB commit but before file removal completes, the
    // package is already correctly marked as removed. Leftover files on disk are
    // harmless orphans rather than a broken state where files are gone but the
    // package is still recorded as installed.
    // Capture /etc snapshot BEFORE the DB transaction so the three-way merge
    // can distinguish pre- from post-removal state.
    let prev_etc = crate::commands::composefs_ops::collect_etc_files(&conn)?;

    progress.set_phase(RemovePhase::UpdatingDb);
    let mut changeset =
        conary_core::db::models::Changeset::new(format!("Remove {}-{}", trove.name, trove.version));
    let tx = conn.unchecked_transaction()?;
    let remove_changeset_id = changeset.insert(&tx)?;

    let remove_result = match remove_inner(
        &tx,
        remove_changeset_id,
        &trove,
        root,
        no_scripts,
        sandbox_mode,
        &progress,
    ) {
        Ok(result) => result,
        Err(e) => {
            engine.release_lock();
            return Err(e);
        }
    };
    let snapshot_json =
        crate::commands::metadata_with_removed_troves(vec![remove_result.snapshot.clone()])?;
    tx.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![snapshot_json, remove_changeset_id],
    )?;
    changeset.update_status(&tx, conary_core::db::models::ChangesetStatus::Applied)?;
    tx.commit()?;

    // Composefs-native: rebuild EROFS image and remount to reflect removal
    progress.set_phase(RemovePhase::RemovingFiles);
    let post_commit_result = (|| -> Result<()> {
        let summary = format!("Remove {}", package_name);
        let outcome = crate::commands::generation::publication::publish_current_db_state(
            &conn,
            crate::commands::generation::publication::PublicationRequest {
                db_path,
                summary: &summary,
                trigger_changeset_id: Some(remove_changeset_id),
                tx_uuid: changeset.tx_uuid.as_deref(),
                prev_etc_snapshot: Some(prev_etc),
            },
        )?;
        if outcome.needs_publication {
            crate::commands::append_deferred_follow_up_metadata(
                &conn,
                remove_changeset_id,
                crate::commands::publication_deferred_follow_up(
                    "generation publication is pending".to_string(),
                ),
            )?;
            crate::commands::generation::publication::warn_if_publication_pending(
                remove_changeset_id,
                &outcome,
            );
        }
        Ok(())
    })();
    engine.release_lock();
    post_commit_result?;

    run_post_remove_scriptlet(&remove_result, root, no_scripts, sandbox_mode, &progress);

    progress.finish(&format!(
        "Removed {} {}",
        remove_result.trove.name, remove_result.trove.version
    ));

    let stats = crate::commands::LiveRootStats {
        files_removed: remove_result.removed_count,
        dirs_removed: remove_result.dirs_removed,
        ..Default::default()
    };
    print_remove_summary(&remove_result, &stats);
    // Note: composefs-native removal rebuilds the entire EROFS image,
    // so individual file failure tracking is not applicable.

    Ok(())
}

fn remove_execution_path(db_path: &str) -> Result<RemoveExecutionPath> {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let current_link = runtime_root.root().join("current");
    let has_current_link = match std::fs::symlink_metadata(&current_link) {
        Ok(metadata) if metadata.file_type().is_symlink() && !current_link.exists() => {
            let target = std::fs::read_link(&current_link)
                .with_context(|| format!("Failed to read {}", current_link.display()))?;
            anyhow::bail!(
                "current generation symlink {} -> {} is dangling",
                current_link.display(),
                target.display()
            );
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect {}", current_link.display()));
        }
    };
    if !has_current_link && std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        return Ok(RemoveExecutionPath::GenerationAware);
    }
    let current = conary_core::generation::mount::current_generation(runtime_root.root())?;
    Ok(match current {
        Some(_) => RemoveExecutionPath::GenerationAware,
        None => RemoveExecutionPath::MutableLiveRoot,
    })
}

fn run_post_remove_scriptlet(
    remove_result: &RemoveInnerResult,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    progress: &RemoveProgress,
) {
    // Execute post-remove scriptlet (best effort - warn on failure, don't abort)
    if no_scripts || remove_result.stored_scriptlets.is_empty() {
        return;
    }

    progress.set_phase(RemovePhase::PostScript);
    let executor = ScriptletExecutor::new(
        Path::new(root),
        &remove_result.trove.name,
        &remove_result.trove.version,
        remove_result.scriptlet_format,
    )
    .with_sandbox_mode(sandbox_mode);

    if let Some(post) = remove_result
        .stored_scriptlets
        .iter()
        .find(|s| s.phase == "post-remove")
    {
        info!("Running post-remove scriptlet...");
        if let Err(e) = executor.execute_entry(post, &ExecutionMode::Remove) {
            warn!(
                "Post-remove scriptlet failed: {}. Package files already removed.",
                e
            );
            eprintln!("WARNING: Post-remove scriptlet failed: {}", e);
        }
    }
}

fn print_remove_summary(remove_result: &RemoveInnerResult, stats: &crate::commands::LiveRootStats) {
    println!(
        "Removed package: {} version {}",
        remove_result.trove.name, remove_result.trove.version
    );
    println!(
        "  Architecture: {}",
        remove_result
            .trove
            .architecture
            .as_deref()
            .unwrap_or("none")
    );
    println!("  Files removed: {}", stats.files_removed);
    if stats.dirs_removed > 0 {
        println!("  Directories removed: {}", stats.dirs_removed);
    }
}

#[cfg(test)]
fn snapshot_path_under_root(root: &Path, path: &str) -> PathBuf {
    root.join(path.strip_prefix('/').unwrap_or(path))
}

#[cfg(test)]
fn snapshot_entry_is_dir(file: &FileSnapshot) -> bool {
    file.path.ends_with('/') || (file.permissions as u32 & 0o170000) == 0o040000
}

#[cfg(test)]
fn remove_files_from_live_root(
    root: &Path,
    snapshot: &TroveSnapshot,
) -> Result<DirectRemovalStats> {
    let mut stats = DirectRemovalStats::default();
    let mut dirs = Vec::new();

    for file in &snapshot.files {
        let path = snapshot_path_under_root(root, &file.path);
        if snapshot_entry_is_dir(file) {
            dirs.push(path);
            continue;
        }

        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                dirs.push(path);
            }
            Ok(_) => {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove package file {}", path.display()))?;
                stats.files_removed += 1;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    "Package file {} was already absent during removal",
                    path.display()
                );
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect package file {}", path.display()));
            }
        }
    }

    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    dirs.dedup();
    for dir in dirs {
        match std::fs::remove_dir(&dir) {
            Ok(()) => stats.dirs_removed += 1,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Failed to remove package directory {}", dir.display())
                });
            }
        }
    }

    Ok(stats)
}

/// Inner remove helper for callers that own the transaction lifecycle.
///
/// Performs pre-remove scriptlets and DB writes using a caller-provided DB
/// transaction and `changeset_id`. Returns the rollback snapshot plus enough
/// metadata for the caller to run post-remove handling after rebuild.
pub(crate) fn remove_inner(
    tx: &rusqlite::Transaction<'_>,
    changeset_id: i64,
    trove: &Trove,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    progress: &RemoveProgress,
) -> Result<RemoveInnerResult> {
    let prepared = prepare_remove(tx, trove, root, no_scripts, sandbox_mode, progress)?;
    commit_remove_db(tx, changeset_id, prepared)
}

fn prepare_remove(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    progress: &RemoveProgress,
) -> Result<PreparedRemove> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let files = FileEntry::find_by_trove(conn, trove_id)?;
    let stored_scriptlets = ScriptletEntry::find_by_trove(conn, trove_id)?;
    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| ScriptletPackageFormat::parse(&s.package_format))
        .unwrap_or(ScriptletPackageFormat::Rpm);

    // NOTE: Known limitation -- if the pre-remove scriptlet partially executes
    // and then fails, there is no automatic recovery. This is consistent with
    // RPM, dpkg, and pacman which also have no pre-remove rollback mechanism.
    if !no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PreScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        )
        .with_sandbox_mode(sandbox_mode);

        if let Some(pre) = stored_scriptlets.iter().find(|s| s.phase == "pre-remove") {
            info!("Running pre-remove scriptlet...");
            executor.execute_entry(pre, &ExecutionMode::Remove)?;
        }
    }

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

    let (directories, regular_files): (Vec<_>, Vec<_>) = files
        .iter()
        .partition(|f| f.path.ends_with('/') || (f.permissions & 0o170000) == 0o040000);

    Ok(PreparedRemove {
        snapshot: TroveSnapshot {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
            description: trove.description.clone(),
            install_source: trove.install_source.as_str().to_string(),
            installed_from_repository_id: trove.installed_from_repository_id,
            files: files
                .iter()
                .map(|f| FileSnapshot {
                    path: f.path.clone(),
                    sha256_hash: f.sha256_hash.clone(),
                    size: f.size,
                    permissions: f.permissions,
                    symlink_target: f.symlink_target.clone(),
                })
                .collect(),
        },
        trove: trove.clone(),
        stored_scriptlets,
        scriptlet_format,
        removed_count: regular_files.len(),
        dirs_removed: directories.len(),
    })
}

fn commit_remove_db(
    tx: &rusqlite::Transaction<'_>,
    changeset_id: i64,
    prepared: PreparedRemove,
) -> Result<RemoveInnerResult> {
    let trove_id = prepared
        .trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    for file in &prepared.snapshot.files {
        let use_hash = if file.sha256_hash.len() == 64
            && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            let hash_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_contents WHERE sha256_hash = ?1)",
                [&file.sha256_hash],
                |row| row.get(0),
            )?;
            if hash_exists {
                Some(file.sha256_hash.as_str())
            } else {
                None
            }
        } else {
            None
        };

        match use_hash {
            Some(hash) => {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                    [&changeset_id.to_string(), &file.path, hash, "delete"],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, NULL, ?3)",
                    [&changeset_id.to_string(), &file.path, "delete"],
                )?;
            }
        }
    }

    conary_core::db::models::Trove::delete(tx, trove_id)?;

    Ok(RemoveInnerResult {
        snapshot: prepared.snapshot,
        trove: prepared.trove,
        stored_scriptlets: prepared.stored_scriptlets,
        scriptlet_format: prepared.scriptlet_format,
        removed_count: prepared.removed_count,
        dirs_removed: prepared.dirs_removed,
    })
}

/// Remove orphaned packages (installed as dependencies but no longer needed)
///
/// Finds packages that were installed as dependencies of other packages,
/// but are no longer required by any installed package.
pub async fn cmd_autoremove(
    db_path: &str,
    root: &str,
    dry_run: bool,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    info!("Finding orphaned packages...");

    let conn = open_db(db_path)?;

    let orphans = conary_core::db::models::Trove::find_orphans(&conn)?;
    if orphans.is_empty() {
        println!("No orphaned packages found.");
        return Ok(());
    }

    let plan = plan_autoremove(orphans);
    if plan.removable.is_empty() {
        println!("No Conary-owned orphaned packages can be autoremoved.");
        print_autoremove_skips(&plan.skipped);
        return Ok(());
    }
    print_autoremove_candidates("Found", &plan.removable);
    print_autoremove_skips(&plan.skipped);

    if dry_run {
        println!("\nDry run - no packages will be removed.");
        println!("Run without --dry-run to remove these packages.");
        return Ok(());
    }

    // Fixed-point iteration: removing orphans may expose new orphans (transitive chains).
    // Re-query after each round until no more orphans are found.
    const MAX_ITERATIONS: usize = 100;
    let mut total_removed = 0;
    let mut total_failed = 0;
    let mut current_plan = plan;
    let mut failed_orphans = HashSet::new();

    for iteration in 0..MAX_ITERATIONS {
        if iteration > 0 {
            // Re-query orphans after previous round of removals
            let conn = open_db(db_path)?;
            let current_orphans = conary_core::db::models::Trove::find_orphans(&conn)?;
            if current_orphans.is_empty() {
                break;
            }
            current_plan = plan_autoremove(current_orphans);
            current_plan
                .removable
                .retain(|trove| !failed_orphans.contains(&autoremove_identity(trove)));
            if current_plan.removable.is_empty() {
                println!("\nNo additional Conary-owned orphaned packages can be autoremoved.");
                print_autoremove_skips(&current_plan.skipped);
                break;
            }
            print_autoremove_candidates("Found additional", &current_plan.removable);
            print_autoremove_skips(&current_plan.skipped);
        } else {
            println!(
                "\nRemoving {} orphaned package(s)...",
                current_plan.removable.len()
            );
        }

        let mut round_removed = 0;
        for trove in &current_plan.removable {
            println!("\nRemoving {} {}...", trove.name, trove.version);
            match cmd_remove(
                &trove.name,
                db_path,
                root,
                Some(trove.version.clone()),
                trove.architecture.clone(),
                no_scripts,
                sandbox_mode,
                false,
            )
            .await
            {
                Ok(()) => {
                    round_removed += 1;
                }
                Err(e) => {
                    eprintln!("  Failed to remove {}: {}", trove.name, e);
                    failed_orphans.insert(autoremove_identity(trove));
                    total_failed += 1;
                }
            }
        }

        total_removed += round_removed;

        // If nothing was removed this round, no point continuing
        if round_removed == 0 {
            break;
        }
    }

    println!("\nAutoremove complete:");
    println!("  Removed: {} package(s)", total_removed);
    if total_failed > 0 {
        println!("  Failed: {} package(s)", total_failed);
        anyhow::bail!(
            "Autoremove failed for {} package(s); see summary above",
            total_failed
        );
    }

    Ok(())
}

fn plan_autoremove(orphaned: Vec<Trove>) -> AutoremovePlan {
    let mut removable = Vec::new();
    let mut skipped = Vec::new();

    for trove in orphaned {
        if trove.install_source.is_adopted() {
            skipped.push((trove, AutoremoveSkipReason::AdoptedNativeAuthority));
        } else if trove.pinned {
            skipped.push((trove, AutoremoveSkipReason::Pinned));
        } else if crate::commands::install::is_package_blocked(&trove.name) {
            skipped.push((trove, AutoremoveSkipReason::Critical));
        } else {
            removable.push(trove);
        }
    }

    AutoremovePlan { removable, skipped }
}

fn print_autoremove_candidates(prefix: &str, troves: &[Trove]) {
    println!("{prefix} {} orphaned package(s):", troves.len());
    for trove in troves {
        print_autoremove_trove(trove);
    }
}

fn print_autoremove_skips(skipped: &[(Trove, AutoremoveSkipReason)]) {
    if skipped.is_empty() {
        return;
    }

    let adopted = skipped
        .iter()
        .filter(|(_, reason)| *reason == AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !adopted.is_empty() {
        println!(
            "Skipping adopted orphaned package(s); native package-manager authority is preserved:"
        );
        for (trove, _) in adopted {
            print_autoremove_trove(trove);
        }
    }

    let blocked = skipped
        .iter()
        .filter(|(_, reason)| *reason != AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !blocked.is_empty() {
        println!("Skipping blocked orphaned package(s):");
        for (trove, reason) in blocked {
            print!("  {} {}", trove.name, trove.version);
            if let Some(arch) = &trove.architecture {
                print!(" [{}]", arch);
            }
            println!(" ({:?})", reason);
        }
    }
}

fn print_autoremove_trove(trove: &Trove) {
    print!("  {} {}", trove.name, trove.version);
    if let Some(arch) = &trove.architecture {
        print!(" [{}]", arch);
    }
    println!();
}

fn autoremove_identity(trove: &Trove) -> (String, String, Option<String>) {
    (
        trove.name.clone(),
        trove.version.clone(),
        trove.architecture.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn file_snapshot(path: &str, permissions: i32) -> FileSnapshot {
        FileSnapshot {
            path: path.to_string(),
            sha256_hash: "0".repeat(64),
            size: 1,
            permissions,
            symlink_target: None,
        }
    }

    fn remove_snapshot(files: Vec<FileSnapshot>) -> TroveSnapshot {
        TroveSnapshot {
            name: "fixture".to_string(),
            version: "1.0.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: None,
            install_source: "Package".to_string(),
            installed_from_repository_id: None,
            files,
        }
    }

    #[test]
    fn autoremove_plan_classifies_authority_and_safety_skips() {
        use conary_core::db::models::{InstallSource, TroveType};

        let owned = Trove::new_with_source(
            "owned-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        let adopted = Trove::new_with_source(
            "adopted-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
        );
        let mut pinned = Trove::new_with_source(
            "pinned-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        pinned.pinned = true;
        let critical = Trove::new_with_source(
            "bash".to_string(),
            "5.2.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );

        let plan = plan_autoremove(vec![owned, adopted, pinned, critical]);

        assert_eq!(plan.removable.len(), 1);
        assert_eq!(plan.removable[0].name, "owned-orphan");
        assert_eq!(
            plan.skipped
                .iter()
                .map(|(trove, reason)| (trove.name.as_str(), reason))
                .collect::<Vec<_>>(),
            vec![
                (
                    "adopted-orphan",
                    &AutoremoveSkipReason::AdoptedNativeAuthority
                ),
                ("pinned-orphan", &AutoremoveSkipReason::Pinned),
                ("bash", &AutoremoveSkipReason::Critical),
            ]
        );
    }

    #[test]
    fn direct_live_root_removal_deletes_files_symlinks_and_empty_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::create_dir_all(root.join("usr/share/fixture")).unwrap();
        std::fs::write(root.join("usr/bin/fixture"), "fixture").unwrap();
        std::fs::write(root.join("usr/share/fixture/readme"), "fixture").unwrap();
        std::os::unix::fs::symlink("fixture", root.join("usr/bin/fixture-link")).unwrap();

        let snapshot = remove_snapshot(vec![
            file_snapshot("/usr/bin/fixture", 0o100755),
            file_snapshot("/usr/bin/fixture-link", 0o120777),
            file_snapshot("/usr/share/fixture/readme", 0o100644),
            file_snapshot("/usr/share/fixture/", 0o040755),
        ]);

        let stats = remove_files_from_live_root(root, &snapshot).unwrap();

        assert_eq!(stats.files_removed, 3);
        assert_eq!(stats.dirs_removed, 1);
        assert!(!root.join("usr/bin/fixture").exists());
        assert!(!root.join("usr/bin/fixture-link").exists());
        assert!(!root.join("usr/share/fixture").exists());
        assert!(root.join("usr/share").exists());
    }

    #[test]
    fn direct_live_root_removal_ignores_already_missing_paths() {
        let tmp = TempDir::new().unwrap();
        let snapshot = remove_snapshot(vec![file_snapshot("/usr/bin/missing", 0o100755)]);

        let stats = remove_files_from_live_root(tmp.path(), &snapshot).unwrap();

        assert_eq!(stats.files_removed, 0);
        assert_eq!(stats.dirs_removed, 0);
    }

    #[tokio::test]
    async fn no_generation_remove_deletes_files_and_db_rows() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_clear_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let payload = root.join("usr/bin/fixture");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "fixture").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "/usr/bin/fixture".to_string(),
            "0".repeat(64),
            "fixture".len() as i64,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
        )
        .await
        .unwrap();

        assert!(!payload.exists());
        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            conary_core::db::models::Trove::find_by_name(&conn, "fixture")
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn no_generation_remove_fails_closed_on_dangling_current_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        std::os::unix::fs::symlink("generations/7", root.join("current")).unwrap();

        let payload = root.join("usr/bin/fixture");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "fixture").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "/usr/bin/fixture".to_string(),
            "0".repeat(64),
            "fixture".len() as i64,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        let err = cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("dangling"), "{err}");
        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "fixture");
        let conn = conary_core::db::open(&db_path).unwrap();
        assert_eq!(
            conary_core::db::models::Trove::find_by_name(&conn, "fixture")
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn no_generation_remove_live_root_failure_leaves_no_pending_changeset() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_clear_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "../escape".to_string(),
            "0".repeat(64),
            7,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        let err = cmd_remove(
            "fixture",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("escapes the target root"), "{err}");
        let conn = conary_core::db::open(&db_path).unwrap();
        let changesets: i64 = conn
            .query_row("SELECT COUNT(*) FROM changesets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(changesets, 0);
        assert_eq!(
            conary_core::db::models::Trove::find_by_name(&conn, "fixture")
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn remove_refuses_critical_package_before_file_mutation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let payload = root.join("usr/bin/bash");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "bash").unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "bash".to_string(),
            "5.2".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = conary_core::db::models::FileEntry::new(
            "/usr/bin/bash".to_string(),
            "0".repeat(64),
            "bash".len() as i64,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        drop(conn);

        let err = cmd_remove(
            "bash",
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            None,
            None,
            true,
            SandboxMode::None,
            false,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("critical package"));
        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "bash");
    }
}
