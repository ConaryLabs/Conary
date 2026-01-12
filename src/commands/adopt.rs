// src/commands/adopt.rs

//! Commands for adopting existing system packages into Conary tracking
//!
//! This module provides the ability to import packages already installed
//! by the system package manager (RPM) into Conary's tracking database.

use anyhow::Result;
use conary::db::models::{
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, Trove, TroveType,
};
use conary::packages::rpm_query;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Adopt all installed system packages
pub fn cmd_adopt_system(db_path: &str, full: bool, dry_run: bool) -> Result<()> {
    info!("Adopting all system packages (full={})", full);

    // Check if RPM is available
    if !rpm_query::is_rpm_available() {
        return Err(anyhow::anyhow!(
            "RPM is not available on this system. Adopt only works on RPM-based systems."
        ));
    }

    let mut conn = conary::db::open(db_path)?;

    // Get list of already-tracked packages to avoid duplicates
    let tracked_packages: std::collections::HashSet<String> = Trove::list_all(&conn)?
        .into_iter()
        .map(|t| t.name)
        .collect();

    // Get all installed packages
    let installed = rpm_query::query_all_packages()?;
    let total = installed.len();

    if dry_run {
        println!("Dry run: would adopt {} packages", total);
        let mut to_adopt = 0;
        let mut already_tracked = 0;

        for (name, info) in &installed {
            if tracked_packages.contains(name) {
                already_tracked += 1;
            } else {
                to_adopt += 1;
                if to_adopt <= 20 {
                    println!("  {} {}", name, info.full_version());
                }
            }
        }

        if to_adopt > 20 {
            println!("  ... and {} more", to_adopt - 20);
        }

        println!("\nSummary:");
        println!("  Would adopt: {} packages", to_adopt);
        println!("  Already tracked: {} packages", already_tracked);
        println!("  Mode: {}", if full { "full (CAS storage)" } else { "track (metadata only)" });
        return Ok(());
    }

    // Determine install source based on mode
    let install_source = if full {
        InstallSource::AdoptedFull
    } else {
        InstallSource::AdoptedTrack
    };

    // Set up CAS for full mode
    let objects_dir = PathBuf::from(db_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("objects");

    let cas = if full {
        Some(conary::filesystem::CasStore::new(&objects_dir)?)
    } else {
        None
    };

    // Create a single changeset for the entire adoption
    let mut changeset = Changeset::new(format!(
        "Adopt {} system packages ({})",
        installed.len(),
        if full { "full" } else { "track" }
    ));

    let mut adopted_count = 0;
    let mut skipped_count = 0;
    let mut error_count = 0;

    conary::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;

        for (name, info) in &installed {
            // Skip already-tracked packages
            if tracked_packages.contains(name) {
                skipped_count += 1;
                continue;
            }

            debug!("Adopting package: {} {}", name, info.full_version());

            // Create trove
            let mut trove = Trove::new_with_source(
                name.clone(),
                info.version_only(),
                TroveType::Package,
                install_source.clone(),
            );
            trove.architecture = Some(info.arch.clone());
            trove.description = info.description.clone().or(info.summary.clone());
            trove.installed_by_changeset_id = Some(changeset_id);

            let trove_id = match trove.insert(tx) {
                Ok(id) => id,
                Err(e) => {
                    warn!("Failed to insert trove for {}: {}", name, e);
                    error_count += 1;
                    continue;
                }
            };

            // Query and insert files
            let files = match rpm_query::query_package_files(name) {
                Ok(f) => f,
                Err(e) => {
                    warn!("Failed to query files for {}: {}", name, e);
                    Vec::new()
                }
            };

            for file_info in &files {
                // For track mode, we just record metadata
                // For full mode, we would read the file and store in CAS
                let hash = if full {
                    // Read file content and store in CAS
                    if let Some(ref cas_store) = cas {
                        match std::fs::read(&file_info.path) {
                            Ok(content) => {
                                match cas_store.store(&content) {
                                    Ok(h) => h,
                                    Err(e) => {
                                        debug!("Failed to store {} in CAS: {}", file_info.path, e);
                                        // Use digest from RPM if available
                                        file_info.digest.clone().unwrap_or_else(|| {
                                            format!("untracked-{}", file_info.path.replace('/', "_"))
                                        })
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Failed to read {}: {}", file_info.path, e);
                                file_info.digest.clone().unwrap_or_else(|| {
                                    format!("untracked-{}", file_info.path.replace('/', "_"))
                                })
                            }
                        }
                    } else {
                        file_info.digest.clone().unwrap_or_else(|| {
                            format!("adopted-{}", file_info.path.replace('/', "_"))
                        })
                    }
                } else {
                    // Track mode: use RPM's digest or generate a placeholder
                    file_info.digest.clone().unwrap_or_else(|| {
                        format!("adopted-{}", file_info.path.replace('/', "_"))
                    })
                };

                let mut file_entry = FileEntry::new(
                    file_info.path.clone(),
                    hash,
                    file_info.size,
                    file_info.mode,
                    trove_id,
                );
                file_entry.owner = file_info.user.clone();
                file_entry.group_name = file_info.group.clone();

                if let Err(e) = file_entry.insert(tx) {
                    // File might already exist from another package
                    debug!("Failed to insert file {}: {}", file_info.path, e);
                }
            }

            // Query and insert dependencies
            let deps = match rpm_query::query_package_dependencies(name) {
                Ok(d) => d,
                Err(e) => {
                    debug!("Failed to query deps for {}: {}", name, e);
                    Vec::new()
                }
            };

            for dep in deps {
                // Parse dependency (format: "name >= version" or just "name")
                let (dep_name, dep_version) = if let Some(pos) = dep.find(|c| c == '>' || c == '<' || c == '=') {
                    let name_part = dep[..pos].trim();
                    let version_part = dep[pos..].trim_start_matches(|c| c == '>' || c == '<' || c == '=' || c == ' ');
                    (name_part.to_string(), Some(version_part.to_string()))
                } else {
                    (dep.trim().to_string(), None)
                };

                if dep_name.is_empty() {
                    continue;
                }

                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep_name,
                    dep_version,
                    "runtime".to_string(),
                    None,
                );
                if let Err(e) = dep_entry.insert(tx) {
                    debug!("Failed to insert dependency: {}", e);
                }
            }

            adopted_count += 1;

            // Progress update every 100 packages
            if adopted_count % 100 == 0 {
                info!("Adopted {} packages so far...", adopted_count);
            }
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })?;

    println!("Adoption complete:");
    println!("  Adopted: {} packages", adopted_count);
    println!("  Skipped (already tracked): {} packages", skipped_count);
    if error_count > 0 {
        println!("  Errors: {} packages", error_count);
    }
    println!("  Mode: {}", if full { "full (files in CAS)" } else { "track (metadata only)" });

    Ok(())
}

/// Adopt specific packages
pub fn cmd_adopt(packages: &[String], db_path: &str, full: bool) -> Result<()> {
    info!("Adopting {} specific packages (full={})", packages.len(), full);

    if packages.is_empty() {
        return Err(anyhow::anyhow!("No packages specified"));
    }

    // Check if RPM is available
    if !rpm_query::is_rpm_available() {
        return Err(anyhow::anyhow!(
            "RPM is not available on this system. Adopt only works on RPM-based systems."
        ));
    }

    let mut conn = conary::db::open(db_path)?;

    // Determine install source based on mode
    let install_source = if full {
        InstallSource::AdoptedFull
    } else {
        InstallSource::AdoptedTrack
    };

    // Set up CAS for full mode
    let objects_dir = PathBuf::from(db_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("objects");

    let cas = if full {
        Some(conary::filesystem::CasStore::new(&objects_dir)?)
    } else {
        None
    };

    for package_name in packages {
        // Check if already tracked
        let existing = Trove::find_by_name(&conn, package_name)?;
        if !existing.is_empty() {
            println!("Package '{}' is already tracked, skipping", package_name);
            continue;
        }

        // Query package info
        let info = match rpm_query::query_package(package_name) {
            Ok(i) => i,
            Err(e) => {
                println!("Package '{}' not found in RPM database: {}", package_name, e);
                continue;
            }
        };

        println!("Adopting: {} {}", info.name, info.full_version());

        // Create changeset for this package
        let mut changeset = Changeset::new(format!(
            "Adopt {} {} ({})",
            info.name,
            info.full_version(),
            if full { "full" } else { "track" }
        ));

        conary::db::transaction(&mut conn, |tx| {
            let changeset_id = changeset.insert(tx)?;

            // Create trove
            let mut trove = Trove::new_with_source(
                info.name.clone(),
                info.version_only(),
                TroveType::Package,
                install_source.clone(),
            );
            trove.architecture = Some(info.arch.clone());
            trove.description = info.description.clone().or(info.summary.clone());
            trove.installed_by_changeset_id = Some(changeset_id);

            let trove_id = trove.insert(tx)?;

            // Query and insert files
            let files = rpm_query::query_package_files(&info.name)?;
            println!("  Files: {}", files.len());

            for file_info in &files {
                let hash = if full {
                    if let Some(ref cas_store) = cas {
                        match std::fs::read(&file_info.path) {
                            Ok(content) => cas_store.store(&content).unwrap_or_else(|_| {
                                file_info.digest.clone().unwrap_or_else(|| {
                                    format!("adopted-{}", file_info.path.replace('/', "_"))
                                })
                            }),
                            Err(_) => file_info.digest.clone().unwrap_or_else(|| {
                                format!("adopted-{}", file_info.path.replace('/', "_"))
                            }),
                        }
                    } else {
                        file_info.digest.clone().unwrap_or_else(|| {
                            format!("adopted-{}", file_info.path.replace('/', "_"))
                        })
                    }
                } else {
                    file_info.digest.clone().unwrap_or_else(|| {
                        format!("adopted-{}", file_info.path.replace('/', "_"))
                    })
                };

                let mut file_entry = FileEntry::new(
                    file_info.path.clone(),
                    hash,
                    file_info.size,
                    file_info.mode,
                    trove_id,
                );
                file_entry.owner = file_info.user.clone();
                file_entry.group_name = file_info.group.clone();

                if let Err(e) = file_entry.insert(tx) {
                    debug!("Failed to insert file {}: {}", file_info.path, e);
                }
            }

            // Query and insert dependencies
            let deps = rpm_query::query_package_dependencies(&info.name).unwrap_or_default();
            println!("  Dependencies: {}", deps.len());

            for dep in deps {
                let (dep_name, dep_version) = if let Some(pos) = dep.find(|c| c == '>' || c == '<' || c == '=') {
                    let name_part = dep[..pos].trim();
                    let version_part = dep[pos..].trim_start_matches(|c| c == '>' || c == '<' || c == '=' || c == ' ');
                    (name_part.to_string(), Some(version_part.to_string()))
                } else {
                    (dep.trim().to_string(), None)
                };

                if dep_name.is_empty() {
                    continue;
                }

                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep_name,
                    dep_version,
                    "runtime".to_string(),
                    None,
                );
                let _ = dep_entry.insert(tx);
            }

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })?;

        println!("  [OK] Adopted {}", info.name);
    }

    Ok(())
}

/// Show adoption status
pub fn cmd_adopt_status(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = Trove::list_all(&conn)?;

    let file_count;
    let mut adopted_track = 0;
    let mut adopted_full = 0;
    let mut installed_file = 0;
    let mut installed_repo = 0;

    for trove in &troves {
        match trove.install_source {
            InstallSource::AdoptedTrack => adopted_track += 1,
            InstallSource::AdoptedFull => adopted_full += 1,
            InstallSource::File => installed_file += 1,
            InstallSource::Repository => installed_repo += 1,
        }
    }

    // Get total files tracked
    let file_count_result: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files",
        [],
        |row| row.get(0),
    ).unwrap_or(0);
    file_count = file_count_result as usize;

    // Get system package count for comparison
    let system_count = if rpm_query::is_rpm_available() {
        rpm_query::list_installed_packages().map(|p| p.len()).unwrap_or(0)
    } else {
        0
    };

    println!("Conary Adoption Status:");
    println!("------------------------");
    println!("Tracked packages: {}", troves.len());
    println!("  Adopted (track mode): {}", adopted_track);
    println!("  Adopted (full mode): {}", adopted_full);
    println!("  Installed from file: {}", installed_file);
    println!("  Installed from repo: {}", installed_repo);
    println!("");
    println!("Tracked files: {}", file_count);
    println!("");
    if system_count > 0 {
        println!("System RPM packages: {}", system_count);
        let coverage = (troves.len() as f64 / system_count as f64 * 100.0).min(100.0);
        println!("Coverage: {:.1}%", coverage);
    }

    Ok(())
}
