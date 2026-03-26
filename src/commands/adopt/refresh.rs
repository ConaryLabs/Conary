// src/commands/adopt/refresh.rs

//! Drift detection and refresh for adopted packages
//!
//! Compares adopted trove versions against the current system state and
//! updates any that have drifted (version changed, package removed, etc.).

use super::super::create_state_snapshot;
use super::super::open_db;
use super::system::{FileInfoTuple, compute_file_hash};
use anyhow::Result;
use conary_core::db::models::{
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, ProvideEntry, Trove,
};
use conary_core::packages::{
    DependencyInfo, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use tracing::{debug, warn};

/// Map of package name -> (version, arch, description).
type InstalledPackageMap = std::collections::HashMap<String, (String, String, Option<String>)>;

/// Outcome for a single adopted package after drift check
#[derive(Debug)]
enum DriftOutcome {
    /// Version in DB matches system — no action needed
    Unchanged,
    /// Version changed — DB record updated
    Updated {
        old_version: String,
        new_version: String,
    },
    /// Package no longer present in system package manager
    Removed,
}

/// Compare adopted troves against current system state and update drifted entries.
///
/// For each adopted trove:
/// - If the system version matches the DB version: skip (no drift)
/// - If the system version differs: update version, files, deps, provides in DB
/// - If the package is no longer installed: mark the trove as removed from tracking
///   (unless `--dry-run`, in which case just report)
///
/// A single changeset covers all updates, and a state snapshot is created
/// for rollback safety.
pub async fn cmd_adopt_refresh(
    db_path: &str,
    _full: bool,
    dry_run: bool,
    quiet: bool,
) -> Result<()> {
    let pkg_mgr = SystemPackageManager::detect();
    if !pkg_mgr.is_available() {
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
        ));
    }

    let mut conn = open_db(db_path)?;

    // Collect all adopted troves
    let all_troves = Trove::list_all(&conn)?;
    let adopted: Vec<Trove> = all_troves
        .into_iter()
        .filter(|t| {
            matches!(
                t.install_source,
                InstallSource::AdoptedTrack | InstallSource::AdoptedFull
            )
        })
        .collect();

    if adopted.is_empty() {
        if !quiet {
            println!("No adopted packages found. Run 'conary system adopt --system' first.");
        }
        return Ok(());
    }

    if !quiet {
        println!("Checking {} adopted package(s) for drift...", adopted.len());
    }

    // Build current system version map: name -> (version, arch, description)
    let system_packages = query_all_current(pkg_mgr)?;

    // Classify each adopted trove
    let mut results: Vec<(&Trove, DriftOutcome)> = Vec::new();

    for trove in &adopted {
        let outcome = match system_packages.get(&trove.name) {
            None => DriftOutcome::Removed,
            Some((sys_ver, _, _)) if *sys_ver == trove.version => DriftOutcome::Unchanged,
            Some((sys_ver, _, _)) => DriftOutcome::Updated {
                old_version: trove.version.clone(),
                new_version: sys_ver.clone(),
            },
        };
        results.push((trove, outcome));
    }

    let updated_count = results
        .iter()
        .filter(|(_, o)| matches!(o, DriftOutcome::Updated { .. }))
        .count();
    let removed_count = results
        .iter()
        .filter(|(_, o)| matches!(o, DriftOutcome::Removed))
        .count();
    let unchanged_count = results
        .iter()
        .filter(|(_, o)| matches!(o, DriftOutcome::Unchanged))
        .count();

    if !quiet {
        println!(
            "  Unchanged: {}  |  Updated: {}  |  No longer installed: {}",
            unchanged_count, updated_count, removed_count
        );
    }

    if dry_run {
        if !quiet {
            println!("\nDry run — no changes written.\n");
            if updated_count > 0 {
                println!("Would update:");
                for (trove, outcome) in &results {
                    if let DriftOutcome::Updated {
                        old_version,
                        new_version,
                    } = outcome
                    {
                        println!("  {} {} -> {}", trove.name, old_version, new_version);
                    }
                }
            }
            if removed_count > 0 {
                println!("Would remove from tracking (no longer installed):");
                for (trove, outcome) in &results {
                    if matches!(outcome, DriftOutcome::Removed) {
                        println!("  {} {}", trove.name, trove.version);
                    }
                }
            }
        }
        return Ok(());
    }

    if updated_count == 0 && removed_count == 0 {
        if !quiet {
            println!("All adopted packages are up to date. Nothing to do.");
        }
        return Ok(());
    }

    // Set up CAS — needed for AdoptedFull packages regardless of CLI flags.
    // We always initialize CAS so that packages originally adopted with --full
    // retain their CAS-backed hashes even when refresh is called by PM hooks
    // (which don't pass --full).
    let objects_dir = std::path::PathBuf::from(db_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("objects");
    let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;

    let mut changeset = Changeset::new(format!(
        "Refresh adopted packages: {} updated, {} removed",
        updated_count, removed_count
    ));

    let mut actually_updated = 0u32;
    let mut actually_removed = 0u32;

    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;

        for (trove, outcome) in &results {
            let trove_id = match trove.id {
                Some(id) => id,
                None => {
                    warn!("Trove {} has no id, skipping", trove.name);
                    continue;
                }
            };

            match outcome {
                DriftOutcome::Unchanged => {}

                DriftOutcome::Removed => {
                    // Remove from tracking — the system package was uninstalled
                    tx.execute("DELETE FROM files WHERE trove_id = ?1", [trove_id])?;
                    tx.execute("DELETE FROM dependencies WHERE trove_id = ?1", [trove_id])?;
                    tx.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;
                    Trove::delete(tx, trove_id)?;
                    if !quiet {
                        println!(
                            "Removed: {} {} (no longer installed)",
                            trove.name, trove.version
                        );
                    }
                    actually_removed += 1;
                }

                DriftOutcome::Updated {
                    old_version,
                    new_version,
                } => {
                    let (sys_ver, sys_arch, sys_desc) = match system_packages.get(&trove.name) {
                        Some(entry) => entry,
                        None => {
                            warn!(
                                "Trove '{}' marked as updated but missing from system_packages map, skipping",
                                trove.name
                            );
                            continue;
                        }
                    };

                    // Update version and metadata on the trove record
                    tx.execute(
                        "UPDATE troves SET version = ?1, architecture = ?2, description = ?3,
                         installed_by_changeset_id = ?4
                         WHERE id = ?5",
                        rusqlite::params![sys_ver, sys_arch, sys_desc, changeset_id, trove_id,],
                    )?;

                    // Query ALL metadata BEFORE deleting old data so a PM
                    // query failure does not leave the trove with empty tables.
                    let use_cas = trove.install_source == InstallSource::AdoptedFull;

                    let files: Vec<FileInfoTuple> = query_package_files(pkg_mgr, &trove.name)
                        .map_err(|e| conary_core::Error::IoError(e.to_string()))?;
                    let deps: Vec<DependencyInfo> = query_package_deps(pkg_mgr, &trove.name)
                        .map_err(|e| conary_core::Error::IoError(e.to_string()))?;
                    let provides: Vec<String> = query_package_provides(pkg_mgr, &trove.name)
                        .map_err(|e| conary_core::Error::IoError(e.to_string()))?;

                    // All queries succeeded — now safe to delete old data and replace
                    tx.execute("DELETE FROM files WHERE trove_id = ?1", [trove_id])?;
                    for (
                        file_path,
                        file_size,
                        file_mode,
                        file_digest,
                        file_user,
                        file_group,
                        link_target,
                    ) in &files
                    {
                        let hash = compute_file_hash(
                            file_path,
                            *file_mode,
                            file_digest.as_deref(),
                            link_target.as_deref(),
                            use_cas,
                            if use_cas { Some(&cas) } else { None },
                        );
                        let mut fe = FileEntry::new(
                            file_path.clone(),
                            hash,
                            *file_size,
                            *file_mode,
                            trove_id,
                        );
                        fe.owner = file_user.clone();
                        fe.group_name = file_group.clone();
                        fe.symlink_target = link_target.clone();
                        if let Err(e) = fe.insert_or_replace(tx) {
                            debug!(
                                "Failed to insert file {} for {}: {}",
                                file_path, trove.name, e
                            );
                        }
                    }

                    tx.execute("DELETE FROM dependencies WHERE trove_id = ?1", [trove_id])?;
                    for dep in deps {
                        if dep.name.is_empty() {
                            continue;
                        }
                        let mut de = DependencyEntry::new(
                            trove_id,
                            dep.name,
                            None,
                            "runtime".to_string(),
                            dep.constraint,
                        );
                        if let Err(e) = de.insert(tx) {
                            debug!("Failed to insert dep for {}: {}", trove.name, e);
                        }
                    }

                    tx.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;
                    for provide in provides {
                        if provide.is_empty() {
                            continue;
                        }
                        let mut pe = ProvideEntry::new(trove_id, provide, None);
                        if let Err(e) = pe.insert_or_ignore(tx) {
                            debug!("Failed to insert provide for {}: {}", trove.name, e);
                        }
                    }

                    if !quiet {
                        println!("Updated: {} {} -> {}", trove.name, old_version, new_version);
                    }
                    actually_updated += 1;
                }
            }
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    // State snapshot for rollback
    if actually_updated > 0 || actually_removed > 0 {
        create_state_snapshot(
            &conn,
            changeset_id,
            &format!(
                "Refresh adopted packages: {} updated, {} removed",
                actually_updated, actually_removed
            ),
        )?;
    }

    if !quiet {
        println!(
            "\nRefresh complete: {} updated, {} removed from tracking.",
            actually_updated, actually_removed
        );
    }

    Ok(())
}

/// Query all currently installed packages from the active package manager.
/// Returns a map of name -> (version, arch, description).
fn query_all_current(pkg_mgr: SystemPackageManager) -> Result<InstalledPackageMap> {
    let map = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_all_packages()?
            .into_iter()
            .map(|(name, info)| {
                let desc = info.description.clone().or(info.summary.clone());
                (name, (info.version_only(), info.arch.clone(), desc))
            })
            .collect(),
        SystemPackageManager::Dpkg => dpkg_query::query_all_packages()?
            .into_iter()
            .map(|(name, info)| {
                (
                    name,
                    (
                        info.version_only(),
                        info.arch.clone(),
                        info.description.clone(),
                    ),
                )
            })
            .collect(),
        SystemPackageManager::Pacman => pacman_query::query_all_packages()?
            .into_iter()
            .map(|(name, info)| {
                (
                    name,
                    (
                        info.version_only(),
                        info.arch.clone(),
                        info.description.clone(),
                    ),
                )
            })
            .collect(),
        _ => return Err(anyhow::anyhow!("Unsupported package manager")),
    };
    Ok(map)
}

/// Query files for a package from the active package manager.
///
/// Returns an error on PM query failure so callers can skip the package
/// rather than recording it with an empty file list.
fn query_package_files(
    pkg_mgr: SystemPackageManager,
    name: &str,
) -> Result<Vec<FileInfoTuple>> {
    let raw = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_package_files(name)
            .map_err(|e| anyhow::anyhow!("RPM file query failed for '{name}': {e}"))?,
        SystemPackageManager::Dpkg => dpkg_query::query_package_files(name)
            .map_err(|e| anyhow::anyhow!("DPKG file query failed for '{name}': {e}"))?,
        SystemPackageManager::Pacman => pacman_query::query_package_files(name)
            .map_err(|e| anyhow::anyhow!("Pacman file query failed for '{name}': {e}"))?,
        _ => return Ok(Vec::new()),
    };
    Ok(raw
        .into_iter()
        .map(|f| (f.path, f.size, f.mode, f.digest, f.user, f.group, f.link_target))
        .collect())
}

/// Query runtime dependencies for a package from the active package manager.
///
/// Returns an error on PM query failure so callers can handle it explicitly.
fn query_package_deps(
    pkg_mgr: SystemPackageManager,
    name: &str,
) -> Result<Vec<DependencyInfo>> {
    Ok(match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_package_dependencies_full(name)
            .map_err(|e| anyhow::anyhow!("RPM dep query failed for '{name}': {e}"))?,
        SystemPackageManager::Dpkg => dpkg_query::query_package_dependencies_full(name)
            .map_err(|e| anyhow::anyhow!("DPKG dep query failed for '{name}': {e}"))?,
        SystemPackageManager::Pacman => pacman_query::query_package_dependencies_full(name)
            .map_err(|e| anyhow::anyhow!("Pacman dep query failed for '{name}': {e}"))?,
        _ => Vec::new(),
    })
}

/// Query provides for a package from the active package manager.
///
/// Returns an error on PM query failure so callers can handle it explicitly.
fn query_package_provides(
    pkg_mgr: SystemPackageManager,
    name: &str,
) -> Result<Vec<String>> {
    Ok(match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_package_provides(name)
            .map_err(|e| anyhow::anyhow!("RPM provides query failed for '{name}': {e}"))?,
        SystemPackageManager::Dpkg => dpkg_query::query_package_provides(name)
            .map_err(|e| anyhow::anyhow!("DPKG provides query failed for '{name}': {e}"))?,
        SystemPackageManager::Pacman => pacman_query::query_package_provides(name)
            .map_err(|e| anyhow::anyhow!("Pacman provides query failed for '{name}': {e}"))?,
        _ => Vec::new(),
    })
}
