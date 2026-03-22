// src/commands/restore.rs

//! Restore command - redeploy files from CAS to filesystem
//!
//! In the composefs-native model, "restore" means:
//! - For a package: verify CAS objects exist for all file_entries, then rebuild
//!   the EROFS image and remount. Individual file restore can use CAS directly.
//! - For a generation: mount a previous generation via `mount_generation()` +
//!   `update_current_symlink()`.
//!
//! The CAS still holds file content, so we can check what's restorable.

use super::open_db;
use anyhow::Result;
use conary_core::db::models::{FileEntry, Trove};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use tracing::info;

/// Restore files for a package from CAS
pub async fn cmd_restore(
    package_name: &str,
    db_path: &str,
    _root: &str,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    info!(
        "Restoring package: {} (force={}, dry_run={})",
        package_name, force, dry_run
    );

    let conn = open_db(db_path)?;

    // Find the package
    let trove = Trove::find_one_by_name(&conn, package_name)?
        .ok_or_else(|| anyhow::anyhow!("Package '{}' not found in database", package_name))?;
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    println!("Package: {} {}", trove.name, trove.version);

    // Get all files for this package
    let files = FileEntry::find_by_trove(&conn, trove_id)?;
    if files.is_empty() {
        println!("No files tracked for this package.");
        return Ok(());
    }

    println!("Tracked files: {}", files.len());

    // Set up CAS store to check file availability
    let objects_dir = objects_dir(db_path);
    let cas = CasStore::new(&objects_dir)?;

    // Categorize files by CAS availability
    let mut in_cas_count = 0;
    let mut not_in_cas = Vec::new();

    for file in &files {
        if cas.exists(&file.sha256_hash) {
            in_cas_count += 1;
        } else {
            not_in_cas.push(file);
        }
    }

    println!("\nFile status:");
    println!("  Available in CAS:  {}", in_cas_count);
    if !not_in_cas.is_empty() {
        println!("  Missing from CAS:  {} (cannot restore)", not_in_cas.len());
    }

    // Determine what to restore
    let files_to_restore: Vec<&FileEntry> = if force {
        files
            .iter()
            .filter(|f| cas.exists(&f.sha256_hash))
            .collect()
    } else {
        // In composefs-native, all files come from the EROFS image.
        // "Restore" means rebuild the image from DB state.
        files
            .iter()
            .filter(|f| cas.exists(&f.sha256_hash))
            .collect()
    };

    if files_to_restore.is_empty() {
        println!("\nNo files available in CAS to restore.");
        return Ok(());
    }

    println!("\nFiles to restore: {}", files_to_restore.len());

    if dry_run {
        println!("\nDry run - would restore:");
        for file in &files_to_restore {
            println!("  {} (mode: {:o})", file.path, file.permissions);
        }
        return Ok(());
    }

    // Composefs-native: rebuild EROFS image from DB state and remount.
    // This restores all files atomically via the new generation.
    let restored = files_to_restore.len();
    let gen_num = crate::commands::composefs_ops::rebuild_and_mount(
        &conn,
        &format!("Restore {}", package_name),
    )?;

    println!("\nRestore complete (generation {}):", gen_num);
    println!("  Files restored via EROFS: {}", restored);

    // Show warnings for files not in CAS
    if !not_in_cas.is_empty() {
        println!(
            "\nWarning: {} files could not be restored (not in CAS):",
            not_in_cas.len()
        );
        for file in not_in_cas.iter().take(10) {
            println!("  {}", file.path);
        }
        if not_in_cas.len() > 10 {
            println!("  ... and {} more", not_in_cas.len() - 10);
        }
        println!("\nThese files were likely adopted in track mode (metadata only).");
        println!("Re-adopt with --full to store content in CAS for future restores.");
    }

    Ok(())
}

/// Restore all packages with missing files
pub async fn cmd_restore_all(db_path: &str, _root: &str, dry_run: bool) -> Result<()> {
    info!(
        "Restoring all packages with missing files (dry_run={})",
        dry_run
    );

    let conn = open_db(db_path)?;

    // Set up CAS store
    let objects_dir = objects_dir(db_path);
    let cas = CasStore::new(&objects_dir)?;

    // Get all troves
    let troves = Trove::list_all(&conn)?;
    println!(
        "Checking {} packages for CAS availability...\n",
        troves.len()
    );

    let mut total_available = 0;
    let mut total_missing = 0;
    let mut packages_checked = 0;

    for trove in &troves {
        let trove_id = match trove.id {
            Some(id) => id,
            None => continue,
        };

        let files = FileEntry::find_by_trove(&conn, trove_id)?;

        // Find files missing from CAS
        let missing: Vec<&FileEntry> = files
            .iter()
            .filter(|f| !cas.exists(&f.sha256_hash))
            .collect();

        if missing.is_empty() {
            continue;
        }

        let available = files.len() - missing.len();
        println!(
            "{}: {} available, {} missing from CAS",
            trove.name,
            available,
            missing.len()
        );

        if dry_run {
            for file in &missing {
                println!("  MISSING: {}", file.path);
            }
        }

        packages_checked += 1;
        total_available += available;
        total_missing += missing.len();
    }

    if packages_checked == 0 {
        println!("All files are present in CAS. Nothing to restore.");
    } else if dry_run {
        println!(
            "\nDry run summary: {} files available, {} missing from CAS across {} packages",
            total_available, total_missing, packages_checked
        );
    } else {
        // Composefs-native: rebuild EROFS from DB state
        let gen_num =
            crate::commands::composefs_ops::rebuild_and_mount(&conn, "Restore all packages")?;
        println!("\nComposefs-native restore (generation {}):", gen_num);
        println!("  Packages checked: {}", packages_checked);
        println!("  Files in CAS:     {}", total_available);
        println!("  Missing from CAS: {}", total_missing);
    }

    Ok(())
}
