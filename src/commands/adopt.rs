// src/commands/adopt.rs

//! Commands for adopting existing system packages into Conary tracking
//!
//! This module provides the ability to import packages already installed
//! by the system package manager (RPM, dpkg, pacman) into Conary's tracking database.

use anyhow::Result;
use conary::db::models::{
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, Trove, TroveType,
};
use conary::packages::{dpkg_query, pacman_query, rpm_query, DependencyInfo, SystemPackageManager};
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// File info tuple: (path, size, mode, digest, user, group, link_target)
type FileInfoTuple = (String, i64, i32, Option<String>, Option<String>, Option<String>, Option<String>);

/// Adopt all installed system packages
pub fn cmd_adopt_system(db_path: &str, full: bool, dry_run: bool) -> Result<()> {
    info!("Adopting all system packages (full={})", full);

    // Detect system package manager
    let pkg_mgr = SystemPackageManager::detect();
    if !pkg_mgr.is_available() {
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
        ));
    }

    println!("Detected package manager: {:?}", pkg_mgr);

    let mut conn = conary::db::open(db_path)?;

    // Get list of already-tracked packages to avoid duplicates
    let tracked_packages: std::collections::HashSet<String> = Trove::list_all(&conn)?
        .into_iter()
        .map(|t| t.name)
        .collect();

    // Get all installed packages based on package manager
    let installed: Vec<(String, String, String, Option<String>)> = match pkg_mgr {
        SystemPackageManager::Rpm => {
            rpm_query::query_all_packages()?
                .into_iter()
                .map(|(name, info)| (name, info.version_only(), info.arch.clone(), info.description.clone().or(info.summary.clone())))
                .collect()
        }
        SystemPackageManager::Dpkg => {
            dpkg_query::query_all_packages()?
                .into_iter()
                .map(|(name, info)| (name, info.version_only(), info.arch.clone(), info.description.clone()))
                .collect()
        }
        SystemPackageManager::Pacman => {
            pacman_query::query_all_packages()?
                .into_iter()
                .map(|(name, info)| (name, info.version_only(), info.arch.clone(), info.description.clone()))
                .collect()
        }
        _ => return Err(anyhow::anyhow!("Unsupported package manager")),
    };
    let total = installed.len();

    if dry_run {
        println!("Dry run: would adopt {} packages", total);
        let mut to_adopt = 0;
        let mut already_tracked = 0;

        for (name, version, _arch, _desc) in &installed {
            if tracked_packages.contains(name) {
                already_tracked += 1;
            } else {
                to_adopt += 1;
                if to_adopt <= 20 {
                    println!("  {} {}", name, version);
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

        for (name, version, arch, description) in &installed {
            // Skip already-tracked packages
            if tracked_packages.contains(name) {
                skipped_count += 1;
                continue;
            }

            debug!("Adopting package: {} {}", name, version);

            // Create trove
            let mut trove = Trove::new_with_source(
                name.clone(),
                version.clone(),
                TroveType::Package,
                install_source.clone(),
            );
            trove.architecture = Some(arch.clone());
            trove.description = description.clone();
            trove.installed_by_changeset_id = Some(changeset_id);

            let trove_id = match trove.insert(tx) {
                Ok(id) => id,
                Err(e) => {
                    warn!("Failed to insert trove for {}: {}", name, e);
                    error_count += 1;
                    continue;
                }
            };

            // Query and insert files based on package manager
            let files: Vec<FileInfoTuple> = match pkg_mgr {
                SystemPackageManager::Rpm => {
                    rpm_query::query_package_files(name)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| (f.path, f.size, f.mode, f.digest, f.user, f.group, f.link_target))
                        .collect()
                }
                SystemPackageManager::Dpkg => {
                    dpkg_query::query_package_files(name)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| (f.path, f.size, f.mode, f.digest, f.user, f.group, f.link_target))
                        .collect()
                }
                SystemPackageManager::Pacman => {
                    pacman_query::query_package_files(name)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| (f.path, f.size, f.mode, f.digest, f.user, f.group, f.link_target))
                        .collect()
                }
                _ => Vec::new(),
            };

            for (file_path, file_size, file_mode, file_digest, file_user, file_group, link_target) in &files {
                // For track mode, we just record metadata
                // For full mode, we hardlink the file into CAS (zero-copy!)
                // Check if this is a symlink (mode & S_IFMT == S_IFLNK)
                let is_symlink = (*file_mode & 0o170000) == 0o120000;
                let is_directory = (*file_mode & 0o170000) == 0o040000;

                let hash = if full {
                    if let Some(ref cas_store) = cas {
                        if is_symlink {
                            // Store symlink target in CAS
                            if let Some(target) = link_target {
                                match cas_store.store_symlink(target) {
                                    Ok(h) => h,
                                    Err(e) => {
                                        debug!("Failed to store symlink {} in CAS: {}", file_path, e);
                                        file_digest.clone().unwrap_or_else(|| {
                                            format!("adopted-{}", file_path.replace('/', "_"))
                                        })
                                    }
                                }
                            } else {
                                // No target provided, try to read it from filesystem
                                match std::fs::read_link(file_path) {
                                    Ok(target) => {
                                        let target_str = target.to_string_lossy().to_string();
                                        match cas_store.store_symlink(&target_str) {
                                            Ok(h) => h,
                                            Err(e) => {
                                                debug!("Failed to store symlink {} in CAS: {}", file_path, e);
                                                format!("adopted-{}", file_path.replace('/', "_"))
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        debug!("Failed to read symlink {}: {}", file_path, e);
                                        format!("adopted-{}", file_path.replace('/', "_"))
                                    }
                                }
                            }
                        } else if is_directory {
                            // Directories don't have content in CAS
                            debug!("Skipping directory: {}", file_path);
                            file_digest.clone().unwrap_or_else(|| {
                                format!("adopted-{}", file_path.replace('/', "_"))
                            })
                        } else {
                            // Regular file - use hardlink_from_existing
                            let path = std::path::Path::new(file_path);
                            if !path.is_file() {
                                debug!("Skipping non-regular file: {}", file_path);
                                file_digest.clone().unwrap_or_else(|| {
                                    format!("adopted-{}", file_path.replace('/', "_"))
                                })
                            } else {
                                match cas_store.hardlink_from_existing(file_path) {
                                    Ok(h) => h,
                                    Err(e) => {
                                        debug!("Failed to hardlink {} into CAS: {}", file_path, e);
                                        file_digest.clone().unwrap_or_else(|| {
                                            format!("untracked-{}", file_path.replace('/', "_"))
                                        })
                                    }
                                }
                            }
                        }
                    } else {
                        file_digest.clone().unwrap_or_else(|| {
                            format!("adopted-{}", file_path.replace('/', "_"))
                        })
                    }
                } else {
                    // Track mode: use digest or generate a placeholder
                    file_digest.clone().unwrap_or_else(|| {
                        format!("adopted-{}", file_path.replace('/', "_"))
                    })
                };

                let mut file_entry = FileEntry::new(
                    file_path.clone(),
                    hash,
                    *file_size,
                    *file_mode,
                    trove_id,
                );
                file_entry.owner = file_user.clone();
                file_entry.group_name = file_group.clone();

                // Use INSERT OR REPLACE to handle shared paths (directories, etc.)
                if let Err(e) = file_entry.insert_or_replace(tx) {
                    // File might already exist from another package
                    debug!("Failed to insert file {}: {}", file_path, e);
                }
            }

            // Query and insert dependencies with version constraints
            let deps: Vec<DependencyInfo> = match pkg_mgr {
                SystemPackageManager::Rpm => {
                    rpm_query::query_package_dependencies_full(name).unwrap_or_default()
                }
                SystemPackageManager::Dpkg => {
                    dpkg_query::query_package_dependencies_full(name).unwrap_or_default()
                }
                SystemPackageManager::Pacman => {
                    pacman_query::query_package_dependencies_full(name).unwrap_or_default()
                }
                _ => Vec::new(),
            };

            for dep in deps {
                if dep.name.is_empty() {
                    continue;
                }

                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep.name,
                    None, // depends_on_version is for resolved version, not constraint
                    "runtime".to_string(),
                    dep.constraint, // Store the version constraint
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
    println!(
        "  Mode: {}",
        if full {
            "full (hardlinked into CAS - zero additional disk space)"
        } else {
            "track (metadata only)"
        }
    );

    Ok(())
}

/// Adopt specific packages
pub fn cmd_adopt(packages: &[String], db_path: &str, full: bool) -> Result<()> {
    info!("Adopting {} specific packages (full={})", packages.len(), full);

    if packages.is_empty() {
        return Err(anyhow::anyhow!("No packages specified"));
    }

    // Detect system package manager
    let pkg_mgr = SystemPackageManager::detect();
    if !pkg_mgr.is_available() {
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
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

        // Query package info based on package manager
        let (pkg_name, pkg_version, pkg_arch, pkg_desc): (String, String, String, Option<String>) = match pkg_mgr {
            SystemPackageManager::Rpm => {
                match rpm_query::query_package(package_name) {
                    Ok(info) => (info.name.clone(), info.version_only(), info.arch.clone(), info.description.clone().or(info.summary.clone())),
                    Err(e) => {
                        println!("Package '{}' not found in RPM database: {}", package_name, e);
                        continue;
                    }
                }
            }
            SystemPackageManager::Dpkg => {
                match dpkg_query::query_package(package_name) {
                    Ok(info) => (info.name.clone(), info.version_only(), info.arch.clone(), info.description.clone()),
                    Err(e) => {
                        println!("Package '{}' not found in dpkg database: {}", package_name, e);
                        continue;
                    }
                }
            }
            SystemPackageManager::Pacman => {
                match pacman_query::query_package(package_name) {
                    Ok(info) => (info.name.clone(), info.version_only(), info.arch.clone(), info.description.clone()),
                    Err(e) => {
                        println!("Package '{}' not found in pacman database: {}", package_name, e);
                        continue;
                    }
                }
            }
            _ => {
                println!("Unsupported package manager");
                continue;
            }
        };

        println!("Adopting: {} {}", pkg_name, pkg_version);

        // Create changeset for this package
        let mut changeset = Changeset::new(format!(
            "Adopt {} {} ({})",
            pkg_name,
            pkg_version,
            if full { "full" } else { "track" }
        ));

        conary::db::transaction(&mut conn, |tx| {
            let changeset_id = changeset.insert(tx)?;

            // Create trove
            let mut trove = Trove::new_with_source(
                pkg_name.clone(),
                pkg_version.clone(),
                TroveType::Package,
                install_source.clone(),
            );
            trove.architecture = Some(pkg_arch.clone());
            trove.description = pkg_desc.clone();
            trove.installed_by_changeset_id = Some(changeset_id);

            let trove_id = trove.insert(tx)?;

            // Query and insert files based on package manager
            let files: Vec<FileInfoTuple> = match pkg_mgr {
                SystemPackageManager::Rpm => {
                    rpm_query::query_package_files(&pkg_name)?
                        .into_iter()
                        .map(|f| (f.path, f.size, f.mode, f.digest, f.user, f.group, f.link_target))
                        .collect()
                }
                SystemPackageManager::Dpkg => {
                    dpkg_query::query_package_files(&pkg_name)?
                        .into_iter()
                        .map(|f| (f.path, f.size, f.mode, f.digest, f.user, f.group, f.link_target))
                        .collect()
                }
                SystemPackageManager::Pacman => {
                    pacman_query::query_package_files(&pkg_name)?
                        .into_iter()
                        .map(|f| (f.path, f.size, f.mode, f.digest, f.user, f.group, f.link_target))
                        .collect()
                }
                _ => Vec::new(),
            };
            println!("  Files: {}", files.len());

            for (file_path, file_size, file_mode, file_digest, file_user, file_group, file_link_target) in &files {
                // Check if this is a symlink (mode & S_IFMT == S_IFLNK)
                let is_symlink = (*file_mode & 0o170000) == 0o120000;
                // Check if this is a directory (mode & S_IFMT == S_IFDIR)
                let is_directory = (*file_mode & 0o170000) == 0o040000;

                let hash = if full {
                    if let Some(ref cas_store) = cas {
                        if is_symlink {
                            // Store symlink target in CAS
                            if let Some(target) = file_link_target {
                                cas_store.store_symlink(target).unwrap_or_else(|_| {
                                    format!("symlink-{}", file_path.replace('/', "_"))
                                })
                            } else {
                                // Try to read symlink target from filesystem
                                std::fs::read_link(file_path)
                                    .ok()
                                    .and_then(|t| cas_store.store_symlink(&t.to_string_lossy()).ok())
                                    .unwrap_or_else(|| format!("symlink-{}", file_path.replace('/', "_")))
                            }
                        } else if is_directory {
                            // Skip directories - they don't have content
                            format!("dir-{}", file_path.replace('/', "_"))
                        } else {
                            // Regular file - use hardlink adoption
                            let path = std::path::Path::new(file_path);
                            if path.is_file() {
                                // Use hardlink_from_existing - zero-copy adoption
                                cas_store.hardlink_from_existing(file_path).unwrap_or_else(|_| {
                                    file_digest.clone().unwrap_or_else(|| {
                                        format!("adopted-{}", file_path.replace('/', "_"))
                                    })
                                })
                            } else {
                                file_digest.clone().unwrap_or_else(|| {
                                    format!("adopted-{}", file_path.replace('/', "_"))
                                })
                            }
                        }
                    } else {
                        file_digest.clone().unwrap_or_else(|| {
                            format!("adopted-{}", file_path.replace('/', "_"))
                        })
                    }
                } else {
                    file_digest.clone().unwrap_or_else(|| {
                        format!("adopted-{}", file_path.replace('/', "_"))
                    })
                };

                let mut file_entry = FileEntry::new(
                    file_path.clone(),
                    hash,
                    *file_size,
                    *file_mode,
                    trove_id,
                );
                file_entry.owner = file_user.clone();
                file_entry.group_name = file_group.clone();

                // Use INSERT OR REPLACE to handle shared paths (directories, etc.)
                if let Err(e) = file_entry.insert_or_replace(tx) {
                    debug!("Failed to insert file {}: {}", file_path, e);
                }
            }

            // Query and insert dependencies with version constraints
            let deps: Vec<DependencyInfo> = match pkg_mgr {
                SystemPackageManager::Rpm => rpm_query::query_package_dependencies_full(&pkg_name).unwrap_or_default(),
                SystemPackageManager::Dpkg => dpkg_query::query_package_dependencies_full(&pkg_name).unwrap_or_default(),
                SystemPackageManager::Pacman => pacman_query::query_package_dependencies_full(&pkg_name).unwrap_or_default(),
                _ => Vec::new(),
            };
            println!("  Dependencies: {}", deps.len());

            for dep in deps {
                if dep.name.is_empty() {
                    continue;
                }

                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep.name,
                    None, // depends_on_version is for resolved version, not constraint
                    "runtime".to_string(),
                    dep.constraint, // Store the version constraint
                );
                let _ = dep_entry.insert(tx);
            }

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })?;

        println!("  [OK] Adopted {}", pkg_name);
    }

    Ok(())
}

/// Show adoption status
pub fn cmd_adopt_status(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = Trove::list_all(&conn)?;

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
    let file_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files",
        [],
        |row| row.get(0),
    ).unwrap_or(0);

    // Get CAS storage stats
    let objects_dir = PathBuf::from(db_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("objects");

    let (cas_files, cas_bytes) = if objects_dir.exists() {
        count_dir_usage(&objects_dir)
    } else {
        (0, 0)
    };

    // Get system package count for comparison
    let system_count = if rpm_query::is_rpm_available() {
        rpm_query::list_installed_packages().map(|p| p.len()).unwrap_or(0)
    } else {
        0
    };

    println!("Conary Adoption Status");
    println!("======================");
    println!();
    println!("Tracked packages: {}", troves.len());
    println!("  Adopted (track mode): {}", adopted_track);
    println!("  Adopted (full mode):  {}", adopted_full);
    println!("  Installed from file:  {}", installed_file);
    println!("  Installed from repo:  {}", installed_repo);
    println!();
    println!("Tracked files: {}", file_count);
    println!();
    println!("CAS Storage:");
    println!("  Objects: {}", cas_files);
    println!("  Size:    {}", format_bytes(cas_bytes));
    println!();
    if system_count > 0 {
        println!("System RPM packages: {}", system_count);
        let coverage = (troves.len() as f64 / system_count as f64 * 100.0).min(100.0);
        println!("Coverage: {:.1}%", coverage);
    }

    Ok(())
}

/// Count files and total bytes in a directory recursively
fn count_dir_usage(dir: &std::path::Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let (f, b) = count_dir_usage(&path);
                files += f;
                bytes += b;
            } else if path.is_file() {
                files += 1;
                if let Ok(meta) = path.metadata() {
                    bytes += meta.len();
                }
            }
        }
    }

    (files, bytes)
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Check for conflicts between tracked packages and files
pub fn cmd_conflicts(db_path: &str, verbose: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    println!("Checking for conflicts...\n");

    let mut issues_found = 0;

    // 1. Check for files with mismatched hashes between adopted and Conary-installed
    //    (e.g., adopted package A has file X, Conary package B also has file X)
    issues_found += check_overlapping_files(&conn, verbose)?;

    // 2. Check for adopted packages that would conflict if updated via Conary
    issues_found += check_adoption_conflicts(&conn, verbose)?;

    // 3. Check for stale file entries (tracked but missing on disk)
    issues_found += check_stale_files(&conn, verbose)?;

    println!();
    if issues_found == 0 {
        println!("No conflicts found.");
    } else {
        println!("Total issues found: {}", issues_found);
    }

    Ok(())
}

/// Check for files that might be owned by multiple packages in RPM but only tracked once in Conary
fn check_overlapping_files(conn: &rusqlite::Connection, verbose: bool) -> Result<usize> {
    // Find files where the tracked owner is adopted but RPM reports different ownership
    let mut count = 0;

    // Get all tracked files with their package info
    let files: Vec<(String, String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT f.path, f.sha256_hash, t.name, t.install_source
             FROM files f
             JOIN troves t ON f.trove_id = t.id
             ORDER BY f.path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // For adopted packages, verify the file still matches what RPM says
    if rpm_query::is_rpm_available() {
        let mut rpm_owners: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

        // Build a cache of file -> rpm owners for faster lookups
        // We'll check a sample of files to avoid being too slow
        let sample_size = if verbose { files.len() } else { files.len().min(1000) };

        for (path, _hash, pkg_name, source) in files.iter().take(sample_size) {
            if source.starts_with("adopted") {
                // Check who RPM thinks owns this file
                if let Ok(owners) = rpm_query::query_file_owner(path) {
                    if owners.len() > 1 {
                        count += 1;
                        if verbose || count <= 10 {
                            println!("File owned by multiple RPM packages:");
                            println!("  Path: {}", path);
                            println!("  Tracked by: {} ({})", pkg_name, source);
                            println!("  RPM owners: {}", owners.join(", "));
                            println!();
                        }
                        rpm_owners.insert(path.clone(), owners);
                    } else if !owners.is_empty() && owners[0] != *pkg_name {
                        count += 1;
                        if verbose || count <= 10 {
                            println!("File ownership mismatch:");
                            println!("  Path: {}", path);
                            println!("  Tracked by: {} ({})", pkg_name, source);
                            println!("  RPM owner: {}", owners[0]);
                            println!();
                        }
                    }
                }
            }
        }

        if count > 10 && !verbose {
            println!("... and {} more ownership issues (use --verbose to see all)\n", count - 10);
        }
    }

    if count > 0 {
        println!("Overlapping file ownership: {} issues", count);
    }

    Ok(count)
}

/// Check for adopted packages that share files with Conary-installed packages
fn check_adoption_conflicts(conn: &rusqlite::Connection, verbose: bool) -> Result<usize> {
    let mut count = 0;

    // Find packages installed via Conary (not adopted)
    let _conary_packages: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, name FROM troves WHERE install_source IN ('file', 'repository')"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // Get all files owned by Conary-installed packages
    let conary_file_paths: std::collections::HashSet<String> = {
        let mut stmt = conn.prepare(
            "SELECT f.path FROM files f
             JOIN troves t ON f.trove_id = t.id
             WHERE t.install_source IN ('file', 'repository')"
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?
    };

    if conary_file_paths.is_empty() {
        return Ok(0);
    }

    // Check if any adopted package files overlap with Conary-installed files
    let adopted_files: Vec<(String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT f.path, t.name, t.version FROM files f
             JOIN troves t ON f.trove_id = t.id
             WHERE t.install_source LIKE 'adopted%'"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut conflicts_by_pkg: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for (path, pkg_name, _version) in &adopted_files {
        if conary_file_paths.contains(path) {
            conflicts_by_pkg
                .entry(pkg_name.clone())
                .or_default()
                .push(path.clone());
            count += 1;
        }
    }

    if !conflicts_by_pkg.is_empty() {
        println!("Adopted packages with file conflicts against Conary-installed packages:");
        for (pkg, paths) in &conflicts_by_pkg {
            println!("  {} ({} conflicting files)", pkg, paths.len());
            if verbose {
                for path in paths.iter().take(5) {
                    println!("    - {}", path);
                }
                if paths.len() > 5 {
                    println!("    - ... and {} more", paths.len() - 5);
                }
            }
        }
        println!();
    }

    Ok(count)
}

/// Check for stale file entries (tracked in DB but missing on disk)
fn check_stale_files(conn: &rusqlite::Connection, verbose: bool) -> Result<usize> {
    let mut count = 0;

    // Get all tracked files
    let files: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT f.path, t.name FROM files f
             JOIN troves t ON f.trove_id = t.id
             ORDER BY t.name, f.path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut missing_by_pkg: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for (path, pkg_name) in &files {
        if !std::path::Path::new(path).exists() {
            missing_by_pkg
                .entry(pkg_name.clone())
                .or_default()
                .push(path.clone());
            count += 1;
        }
    }

    if !missing_by_pkg.is_empty() {
        println!("Packages with missing files:");
        for (pkg, paths) in &missing_by_pkg {
            println!("  {} ({} missing files)", pkg, paths.len());
            if verbose {
                for path in paths.iter().take(5) {
                    println!("    - {}", path);
                }
                if paths.len() > 5 {
                    println!("    - ... and {} more", paths.len() - 5);
                }
            }
        }
        println!();
    }

    Ok(count)
}
