// src/commands/restore.rs

//! Restore command - redeploy files from CAS to filesystem
//!
//! This command restores missing files for a package from the Content-Addressable
//! Storage (CAS). It's particularly useful after:
//! - Native package manager removes files (RPM/dpkg/pacman)
//! - Files are accidentally deleted
//! - System corruption
//!
//! Files are deployed via hardlinks when possible (zero additional disk space).

use anyhow::Result;
use conary::db::models::{FileEntry, Trove};
use conary::db::paths::objects_dir;
use conary::filesystem::FileDeployer;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Restore files for a package from CAS
pub fn cmd_restore(
    package_name: &str,
    db_path: &str,
    root: &str,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    info!("Restoring package: {} (force={}, dry_run={})", package_name, force, dry_run);

    let conn = conary::db::open(db_path)?;

    // Find the package
    let troves = Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' not found in database", package_name));
    }

    let trove = &troves[0];
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    println!("Package: {} {}", trove.name, trove.version);

    // Get all files for this package
    let files = FileEntry::find_by_trove(&conn, trove_id)?;
    if files.is_empty() {
        println!("No files tracked for this package.");
        return Ok(());
    }

    println!("Tracked files: {}", files.len());

    // Set up deployer
    let objects_dir = objects_dir(db_path);
    let install_root = PathBuf::from(root);

    let deployer = FileDeployer::new(&objects_dir, &install_root)?;

    // Categorize files
    let mut missing_files = Vec::new();
    let mut existing_files = Vec::new();
    let mut not_in_cas = Vec::new();

    for file in &files {
        let target_path = install_root.join(file.path.trim_start_matches('/'));
        let in_cas = deployer.cas().exists(&file.sha256_hash);

        if !target_path.exists() {
            if in_cas {
                missing_files.push(file);
            } else {
                not_in_cas.push(file);
            }
        } else {
            existing_files.push(file);
        }
    }

    println!("\nFile status:");
    println!("  Missing (can restore): {}", missing_files.len());
    println!("  Existing on disk:      {}", existing_files.len());
    if !not_in_cas.is_empty() {
        println!("  Missing from CAS:      {} (cannot restore)", not_in_cas.len());
    }

    // Determine what to restore
    let files_to_restore: Vec<&FileEntry> = if force {
        // Force mode: restore all files that are in CAS
        files.iter().filter(|f| deployer.cas().exists(&f.sha256_hash)).collect()
    } else {
        // Normal mode: only restore missing files
        missing_files
    };

    if files_to_restore.is_empty() {
        if force {
            println!("\nNo files available in CAS to restore.");
        } else {
            println!("\nNo files need to be restored.");
        }
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

    // Restore files
    let mut restored = 0;
    let mut failed = 0;

    for file in &files_to_restore {
        debug!("Restoring: {}", file.path);

        match deployer.deploy_auto(&file.path, &file.sha256_hash, file.permissions as u32) {
            Ok(()) => {
                restored += 1;
                debug!("Restored: {}", file.path);
            }
            Err(e) => {
                warn!("Failed to restore {}: {}", file.path, e);
                failed += 1;
            }
        }
    }

    println!("\nRestore complete:");
    println!("  Restored: {} files", restored);
    if failed > 0 {
        println!("  Failed:   {} files", failed);
    }

    // Show warnings for files not in CAS
    if !not_in_cas.is_empty() {
        println!("\nWarning: {} files could not be restored (not in CAS):", not_in_cas.len());
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
pub fn cmd_restore_all(db_path: &str, root: &str, dry_run: bool) -> Result<()> {
    info!("Restoring all packages with missing files (dry_run={})", dry_run);

    let conn = conary::db::open(db_path)?;

    // Set up deployer
    let objects_dir = objects_dir(db_path);
    let install_root = PathBuf::from(root);

    let deployer = FileDeployer::new(&objects_dir, &install_root)?;

    // Get all troves
    let troves = Trove::list_all(&conn)?;
    println!("Checking {} packages for missing files...\n", troves.len());

    let mut total_restored = 0;
    let mut total_failed = 0;
    let mut packages_restored = 0;

    for trove in &troves {
        let trove_id = match trove.id {
            Some(id) => id,
            None => continue,
        };

        let files = FileEntry::find_by_trove(&conn, trove_id)?;

        // Find missing files that are in CAS
        let missing: Vec<&FileEntry> = files
            .iter()
            .filter(|f| {
                let target_path = install_root.join(f.path.trim_start_matches('/'));
                !target_path.exists() && deployer.cas().exists(&f.sha256_hash)
            })
            .collect();

        if missing.is_empty() {
            continue;
        }

        println!("{}: {} missing files", trove.name, missing.len());

        if dry_run {
            for file in &missing {
                println!("  {}", file.path);
            }
            packages_restored += 1;
            total_restored += missing.len();
            continue;
        }

        // Restore missing files
        for file in &missing {
            match deployer.deploy_auto(&file.path, &file.sha256_hash, file.permissions as u32) {
                Ok(()) => {
                    total_restored += 1;
                }
                Err(e) => {
                    warn!("Failed to restore {}: {}", file.path, e);
                    total_failed += 1;
                }
            }
        }
        packages_restored += 1;
    }

    if packages_restored == 0 {
        println!("All files are present. Nothing to restore.");
    } else if dry_run {
        println!("\nDry run - would restore {} files across {} packages", total_restored, packages_restored);
    } else {
        println!("\nRestore complete:");
        println!("  Packages:  {}", packages_restored);
        println!("  Restored:  {} files", total_restored);
        if total_failed > 0 {
            println!("  Failed:    {} files", total_failed);
        }
    }

    Ok(())
}
