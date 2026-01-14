// src/commands/remove.rs
//! Package removal commands

use super::create_state_snapshot;
use super::progress::{RemovePhase, RemoveProgress};
use anyhow::{Context, Result};
use conary::db::models::ScriptletEntry;
use conary::scriptlet::{ExecutionMode, PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletExecutor};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Serializable trove metadata for rollback support
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TroveSnapshot {
    name: String,
    version: String,
    architecture: Option<String>,
    description: Option<String>,
    install_source: String,
    files: Vec<FileSnapshot>,
}

/// Serializable file metadata for rollback support
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileSnapshot {
    path: String,
    sha256_hash: String,
    size: i64,
    permissions: i32,
}

/// Remove an installed package
pub fn cmd_remove(package_name: &str, db_path: &str, root: &str, version: Option<String>, no_scripts: bool, sandbox_mode: SandboxMode) -> Result<()> {
    info!("Removing package: {}", package_name);

    // Create progress tracker for removal
    let progress = RemoveProgress::new(package_name);

    let mut conn = conary::db::open(db_path)
        .context("Failed to open package database")?;
    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)
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
        troves.iter().find(|t| t.version == *ver)
            .ok_or_else(|| anyhow::anyhow!(
                "Package '{}' version '{}' is not installed. Installed versions: {}",
                package_name,
                ver,
                troves.iter().map(|t| t.version.as_str()).collect::<Vec<_>>().join(", ")
            ))?
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
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    // Check if package is pinned
    if trove.pinned {
        return Err(anyhow::anyhow!(
            "Package '{}' is pinned and cannot be removed. Use 'conary unpin {}' first.",
            package_name,
            package_name
        ));
    }

    let resolver = conary::resolver::Resolver::new(&conn)?;
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

    // Get files BEFORE deleting the trove (cascade delete will remove file records)
    let files = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?;
    let _file_count = files.len(); // Used for snapshot, not display

    // Get stored scriptlets BEFORE deleting the trove
    let stored_scriptlets = ScriptletEntry::find_by_trove(&conn, trove_id)?;

    // Determine package format from stored scriptlets (default to RPM if no scriptlets)
    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| ScriptletPackageFormat::parse(&s.package_format))
        .unwrap_or(ScriptletPackageFormat::Rpm);

    // Execute pre-remove scriptlet (before any changes)
    if !no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PreScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);

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

    // Set up file deployer for actual filesystem operations
    let db_dir = std::env::var("CONARY_DB_DIR").unwrap_or_else(|_| "/var/lib/conary".to_string());
    let objects_dir = PathBuf::from(&db_dir).join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    progress.set_phase(RemovePhase::UpdatingDb);
    let remove_changeset_id = conary::db::transaction(&mut conn, |tx| {
        let mut changeset =
            conary::db::models::Changeset::new(format!("Remove {}-{}", trove.name, trove.version));
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

        conary::db::models::Trove::delete(tx, trove_id)?;
        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    // Separate files and directories
    // Directories typically have mode starting with 040xxx (directory bit)
    // or path ending with /
    let (directories, regular_files): (Vec<_>, Vec<_>) = files.iter().partition(|f| {
        f.path.ends_with('/') || (f.permissions & 0o170000) == 0o040000
    });

    // Remove regular files first
    progress.set_phase(RemovePhase::RemovingFiles);
    let mut removed_count = 0;
    let mut failed_count = 0;
    for file in &regular_files {
        match deployer.remove_file(&file.path) {
            Ok(()) => {
                removed_count += 1;
                info!("Removed file: {}", file.path);
            }
            Err(e) => {
                warn!("Failed to remove file {}: {}", file.path, e);
                failed_count += 1;
            }
        }
    }

    // Sort directories by path length (deepest first) to remove children before parents
    let mut sorted_dirs: Vec<_> = directories.iter().collect();
    sorted_dirs.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

    // Remove directories (only if empty)
    progress.set_phase(RemovePhase::RemovingDirs);
    let mut dirs_removed = 0;
    for dir in sorted_dirs {
        let dir_path = dir.path.trim_end_matches('/');
        match deployer.remove_directory(dir_path) {
            Ok(true) => {
                dirs_removed += 1;
                info!("Removed directory: {}", dir_path);
            }
            Ok(false) => {
                debug!("Directory not empty or already removed: {}", dir_path);
            }
            Err(e) => {
                warn!("Failed to remove directory {}: {}", dir_path, e);
            }
        }
    }

    // Execute post-remove scriptlet (best effort - warn on failure, don't abort)
    if !no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PostScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);

        if let Some(post) = stored_scriptlets.iter().find(|s| s.phase == "post-remove") {
            info!("Running post-remove scriptlet...");
            if let Err(e) = executor.execute_entry(post, &ExecutionMode::Remove) {
                // Post-remove failure is not critical - files are already removed
                warn!("Post-remove scriptlet failed: {}. Package files already removed.", e);
                eprintln!("WARNING: Post-remove scriptlet failed: {}", e);
            }
        }
    }

    progress.finish(&format!("Removed {} {}", trove.name, trove.version));

    println!(
        "Removed package: {} version {}",
        trove.name, trove.version
    );
    println!(
        "  Architecture: {}",
        trove.architecture.as_deref().unwrap_or("none")
    );
    println!(
        "  Files removed: {}/{}",
        removed_count,
        regular_files.len()
    );
    if dirs_removed > 0 {
        println!("  Directories removed: {}", dirs_removed);
    }
    if failed_count > 0 {
        println!("  Files failed to remove: {}", failed_count);
    }

    // Create state snapshot after successful remove
    create_state_snapshot(&conn, remove_changeset_id, &format!("Remove {}", trove.name))?;

    Ok(())
}

/// Remove orphaned packages (installed as dependencies but no longer needed)
///
/// Finds packages that were installed as dependencies of other packages,
/// but are no longer required by any installed package.
pub fn cmd_autoremove(db_path: &str, root: &str, dry_run: bool, no_scripts: bool, sandbox_mode: SandboxMode) -> Result<()> {
    info!("Finding orphaned packages...");

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let orphans = conary::db::models::Trove::find_orphans(&conn)?;

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

    println!("\nRemoving {} orphaned package(s)...", orphans.len());

    // Remove each orphaned package
    let mut removed_count = 0;
    let mut failed_count = 0;

    for trove in &orphans {
        println!("\nRemoving {} {}...", trove.name, trove.version);
        match cmd_remove(&trove.name, db_path, root, Some(trove.version.clone()), no_scripts, sandbox_mode) {
            Ok(()) => {
                removed_count += 1;
            }
            Err(e) => {
                eprintln!("  Failed to remove {}: {}", trove.name, e);
                failed_count += 1;
            }
        }
    }

    println!("\nAutoremove complete:");
    println!("  Removed: {} package(s)", removed_count);
    if failed_count > 0 {
        println!("  Failed: {} package(s)", failed_count);
    }

    Ok(())
}
