// src/commands/adopt/system.rs

//! Bulk system package adoption
//!
//! Adopts all installed system packages into Conary tracking.

use anyhow::Result;
use conary::db::models::{
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, ProvideEntry, Trove,
    TroveType,
};
use conary::packages::{dpkg_query, pacman_query, rpm_query, DependencyInfo, SystemPackageManager};
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// File info tuple: (path, size, mode, digest, user, group, link_target)
pub type FileInfoTuple = (String, i64, i32, Option<String>, Option<String>, Option<String>, Option<String>);

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
                let hash = compute_file_hash(
                    file_path,
                    *file_mode,
                    file_digest.as_deref(),
                    link_target.as_deref(),
                    full,
                    cas.as_ref(),
                );

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

            // Query and insert provides (capabilities this package offers)
            let provides: Vec<String> = match pkg_mgr {
                SystemPackageManager::Rpm => {
                    rpm_query::query_package_provides(name).unwrap_or_default()
                }
                SystemPackageManager::Dpkg => {
                    dpkg_query::query_package_provides(name).unwrap_or_default()
                }
                SystemPackageManager::Pacman => {
                    pacman_query::query_package_provides(name).unwrap_or_default()
                }
                _ => Vec::new(),
            };

            for provide in provides {
                if provide.is_empty() {
                    continue;
                }
                let mut provide_entry = ProvideEntry::new(trove_id, provide, None);
                if let Err(e) = provide_entry.insert_or_ignore(tx) {
                    debug!("Failed to insert provide: {}", e);
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

/// Compute the hash for a file, handling symlinks, directories, and regular files
pub fn compute_file_hash(
    file_path: &str,
    file_mode: i32,
    file_digest: Option<&str>,
    link_target: Option<&str>,
    full: bool,
    cas: Option<&conary::filesystem::CasStore>,
) -> String {
    // Check if this is a symlink (mode & S_IFMT == S_IFLNK)
    let is_symlink = (file_mode & 0o170000) == 0o120000;
    let is_directory = (file_mode & 0o170000) == 0o040000;

    if full {
        if let Some(cas_store) = cas {
            if is_symlink {
                // Store symlink target in CAS
                if let Some(target) = link_target {
                    match cas_store.store_symlink(target) {
                        Ok(h) => return h,
                        Err(e) => {
                            debug!("Failed to store symlink {} in CAS: {}", file_path, e);
                        }
                    }
                } else {
                    // No target provided, try to read it from filesystem
                    match std::fs::read_link(file_path) {
                        Ok(target) => {
                            let target_str = target.to_string_lossy().to_string();
                            match cas_store.store_symlink(&target_str) {
                                Ok(h) => return h,
                                Err(e) => {
                                    debug!("Failed to store symlink {} in CAS: {}", file_path, e);
                                }
                            }
                        }
                        Err(e) => {
                            debug!("Failed to read symlink {}: {}", file_path, e);
                        }
                    }
                }
            } else if is_directory {
                // Directories don't have content in CAS
                debug!("Skipping directory: {}", file_path);
            } else {
                // Regular file - use hardlink_from_existing
                let path = std::path::Path::new(file_path);
                if path.is_file() {
                    match cas_store.hardlink_from_existing(file_path) {
                        Ok(h) => return h,
                        Err(e) => {
                            debug!("Failed to hardlink {} into CAS: {}", file_path, e);
                        }
                    }
                } else {
                    debug!("Skipping non-regular file: {}", file_path);
                }
            }
        }
    }

    // Fallback: use digest or generate a placeholder
    file_digest.map(String::from).unwrap_or_else(|| {
        format!("adopted-{}", file_path.replace('/', "_"))
    })
}
