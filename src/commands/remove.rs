// src/commands/remove.rs
//! Package removal commands

use super::create_state_snapshot;
use super::open_db;
use super::progress::{RemovePhase, RemoveProgress};
use super::{FileSnapshot, TroveSnapshot};
use anyhow::{Context, Result};
use conary_core::db::models::ScriptletEntry;
use conary_core::scriptlet::{
    ExecutionMode, PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletExecutor,
};
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

/// Remove an installed package
pub async fn cmd_remove(
    package_name: &str,
    db_path: &str,
    root: &str,
    version: Option<String>,
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

    let mut conn = open_db(db_path)?;
    let troves = conary_core::db::models::Trove::find_by_name(&conn, package_name)
        .with_context(|| format!("Failed to query package '{}'", package_name))?;

    if troves.is_empty() {
        return Err(anyhow::anyhow!(
            "Package '{}' is not installed",
            package_name
        ));
    }

    // Handle version-specific removal
    let trove = if let Some(ref ver) = version {
        // Find the specific version
        troves.iter().find(|t| t.version == *ver).ok_or_else(|| {
            anyhow::anyhow!(
                "Package '{}' version '{}' is not installed. Installed versions: {}",
                package_name,
                ver,
                troves
                    .iter()
                    .map(|t| t.version.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?
    } else if troves.len() > 1 {
        println!("Multiple versions of '{}' found:", package_name);
        for trove in &troves {
            println!("  - version {}", trove.version);
        }
        return Err(anyhow::anyhow!(
            "Multiple versions installed. Use --version to specify which one to remove."
        ));
    } else {
        &troves[0]
    };
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    // Check if package is pinned
    if trove.pinned {
        return Err(anyhow::anyhow!(
            "Package '{}' is pinned and cannot be removed. Use 'conary unpin {}' first.",
            package_name,
            package_name
        ));
    }

    // Check dependency breakage BEFORE any removal (including adopted packages)
    let resolver = conary_core::resolver::Resolver::new(&conn)?;
    let breaking = resolver.check_removal(package_name)?;

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
            "Use 'conary whatbreaks {}' for more information.",
            package_name
        );
        return Err(anyhow::anyhow!(
            "Cannot remove '{}': {} packages depend on it",
            package_name,
            breaking.len()
        ));
    }

    // Check if package is adopted from system PM
    if trove.install_source.is_adopted() && !purge_files {
        // Remove from Conary tracking only -- don't touch files on disk
        info!(
            "Package '{}' is adopted -- removing from Conary tracking only",
            package_name
        );

        let remove_changeset_id = conary_core::db::transaction(&mut conn, |tx| {
            let mut changeset = conary_core::db::models::Changeset::new(format!(
                "Remove tracking for adopted {}-{}",
                trove.name, trove.version
            ));
            let changeset_id = changeset.insert(tx)?;

            // Remove DB records (files, deps, provides, trove)
            tx.execute("DELETE FROM files WHERE trove_id = ?1", [trove_id])?;
            tx.execute("DELETE FROM dependencies WHERE trove_id = ?1", [trove_id])?;
            tx.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;
            conary_core::db::models::Trove::delete(tx, trove_id)?;
            changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
            Ok(changeset_id)
        })?;

        let pkg_mgr = conary_core::packages::SystemPackageManager::detect();
        println!(
            "Removed '{}' from Conary tracking. Use '{}' to fully uninstall.",
            package_name,
            pkg_mgr.remove_command(package_name)
        );

        create_state_snapshot(
            &conn,
            remove_changeset_id,
            &format!("Remove tracking for {}", trove.name),
        )?;
        return Ok(());
    }

    if trove.install_source.is_adopted() && purge_files {
        println!(
            "WARNING: --purge-files specified for adopted package '{}'. \
             Files will be deleted from disk.",
            package_name
        );
    }

    // Get files BEFORE deleting the trove (cascade delete will remove file records)
    let files = conary_core::db::models::FileEntry::find_by_trove(&conn, trove_id)?;

    // Get stored scriptlets BEFORE deleting the trove
    let stored_scriptlets = ScriptletEntry::find_by_trove(&conn, trove_id)?;

    // Determine package format from stored scriptlets (default to RPM if no scriptlets)
    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| ScriptletPackageFormat::parse(&s.package_format))
        .unwrap_or(ScriptletPackageFormat::Rpm);

    // NOTE: Known limitation -- if the pre-remove scriptlet partially executes
    // and then fails, there is no automatic recovery. This is consistent with
    // RPM, dpkg, and pacman which also have no pre-remove rollback mechanism.

    // Execute pre-remove scriptlet (before any changes)
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

    // Create snapshot of trove for rollback support
    let snapshot = TroveSnapshot {
        name: trove.name.clone(),
        version: trove.version.clone(),
        architecture: trove.architecture.clone(),
        description: trove.description.clone(),
        install_source: trove.install_source.as_str().to_string(),
        files: files
            .iter()
            .map(|f| FileSnapshot {
                path: f.path.clone(),
                sha256_hash: f.sha256_hash.clone(),
                size: f.size,
                permissions: f.permissions,
            })
            .collect(),
    };
    let snapshot_json = serde_json::to_string(&snapshot)?;

    // In composefs-native model, we don't remove files from disk directly.
    // Files are served via EROFS images mounted through composefs.
    // Removing a package means: update DB -> rebuild EROFS -> remount.

    // Separate files and directories for logging/summary purposes
    let (directories, regular_files): (Vec<_>, Vec<_>) = files
        .iter()
        .partition(|f| f.path.ends_with('/') || (f.permissions & 0o170000) == 0o040000);

    // DB-first approach: commit the DB transaction before removing files from disk.
    // If a crash occurs after the DB commit but before file removal completes, the
    // package is already correctly marked as removed. Leftover files on disk are
    // harmless orphans rather than a broken state where files are gone but the
    // package is still recorded as installed.
    progress.set_phase(RemovePhase::UpdatingDb);
    let remove_changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = conary_core::db::models::Changeset::new(format!(
            "Remove {}-{}",
            trove.name, trove.version
        ));
        let changeset_id = changeset.insert(tx)?;

        // Store snapshot metadata for rollback
        tx.execute(
            "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
            [&snapshot_json, &changeset_id.to_string()],
        )?;

        // Record file removals in history before deleting
        for file in &files {
            // Check if hash is valid format (64 hex chars) and exists in file_contents
            // Adopted files may have placeholder hashes or real hashes not in the content store
            let use_hash = if file.sha256_hash.len() == 64
                && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit())
            {
                // Check if this hash actually exists in file_contents (FK constraint)
                let hash_exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM file_contents WHERE sha256_hash = ?1)",
                    [&file.sha256_hash],
                    |row| row.get(0),
                )?;
                if hash_exists {
                    Some(file.sha256_hash.as_str())
                } else {
                    None // Hash not in content store (adopted file)
                }
            } else {
                None // Placeholder hash
            };

            // Always record file removal, but only include hash if it exists in file_contents
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
        changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    // Composefs-native: rebuild EROFS image and remount to reflect removal
    progress.set_phase(RemovePhase::RemovingFiles);
    let removed_count = regular_files.len();
    let dirs_removed = directories.len();

    let _gen_num = crate::commands::composefs_ops::rebuild_and_mount(
        &conn,
        &format!("Remove {}", package_name),
    )?;

    // Execute post-remove scriptlet (best effort - warn on failure, don't abort)
    if !no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PostScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        )
        .with_sandbox_mode(sandbox_mode);

        if let Some(post) = stored_scriptlets.iter().find(|s| s.phase == "post-remove") {
            info!("Running post-remove scriptlet...");
            if let Err(e) = executor.execute_entry(post, &ExecutionMode::Remove) {
                // Post-remove failure is not critical - files are already removed
                warn!(
                    "Post-remove scriptlet failed: {}. Package files already removed.",
                    e
                );
                eprintln!("WARNING: Post-remove scriptlet failed: {}", e);
            }
        }
    }

    progress.finish(&format!("Removed {} {}", trove.name, trove.version));

    println!("Removed package: {} version {}", trove.name, trove.version);
    println!(
        "  Architecture: {}",
        trove.architecture.as_deref().unwrap_or("none")
    );
    println!("  Files removed: {}/{}", removed_count, regular_files.len());
    if dirs_removed > 0 {
        println!("  Directories removed: {}", dirs_removed);
    }
    // Note: composefs-native removal rebuilds the entire EROFS image,
    // so individual file failure tracking is not applicable.

    // Create state snapshot after successful remove
    create_state_snapshot(
        &conn,
        remove_changeset_id,
        &format!("Remove {}", trove.name),
    )?;

    Ok(())
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

    println!("Found {} orphaned package(s):", orphans.len());
    for trove in &orphans {
        print!("  {} {}", trove.name, trove.version);
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }

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
    let mut current_orphans = orphans;

    for iteration in 0..MAX_ITERATIONS {
        if iteration > 0 {
            // Re-query orphans after previous round of removals
            let conn = open_db(db_path)?;
            current_orphans = conary_core::db::models::Trove::find_orphans(&conn)?;
            if current_orphans.is_empty() {
                break;
            }
            println!(
                "\nFound {} additional orphan(s) (iteration {}):",
                current_orphans.len(),
                iteration + 1
            );
            for trove in &current_orphans {
                print!("  {} {}", trove.name, trove.version);
                if let Some(arch) = &trove.architecture {
                    print!(" [{}]", arch);
                }
                println!();
            }
        } else {
            println!(
                "\nRemoving {} orphaned package(s)...",
                current_orphans.len()
            );
        }

        let mut round_removed = 0;
        for trove in &current_orphans {
            println!("\nRemoving {} {}...", trove.name, trove.version);
            match cmd_remove(
                &trove.name,
                db_path,
                root,
                Some(trove.version.clone()),
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
    }

    Ok(())
}
