// src/commands/adopt/packages.rs

//! Specific package adoption
//!
//! Adopts individual packages into Conary tracking.

use super::super::create_state_snapshot;
use super::super::open_db;
use super::super::progress::{AdoptPhase, AdoptProgress};
use super::system::{FileInfoTuple, compute_file_hash};
use anyhow::Result;
use conary_core::db::models::{
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, ProvideEntry, Trove,
    TroveType,
};
use conary_core::packages::{
    DependencyInfo, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use std::path::PathBuf;
use tracing::debug;

fn metadata_insert_succeeded(total_inserts: usize, insert_failures: usize) -> bool {
    total_inserts == 0 || insert_failures < total_inserts
}

/// Adopt specific packages
pub async fn cmd_adopt(packages: &[String], db_path: &str, full: bool) -> Result<()> {
    if packages.is_empty() {
        return Err(anyhow::anyhow!("No packages specified"));
    }

    // Hint if source policy is unconfigured (first-run guidance)
    super::super::hint_unconfigured_source_policy();

    // Detect system package manager
    let pkg_mgr = SystemPackageManager::detect();
    if !pkg_mgr.is_available() {
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
        ));
    }
    let source_identity = pkg_mgr.detect_source_identity();

    let mut conn = open_db(db_path)?;

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
        Some(conary_core::filesystem::CasStore::new(&objects_dir)?)
    } else {
        None
    };

    let mut progress = if packages.len() > 1 {
        AdoptProgress::new(packages.len() as u64, "Adopting")
    } else {
        AdoptProgress::single(&format!("Adopting {}", packages[0]))
    };

    for package_name in packages {
        // Check if already tracked
        let existing = Trove::find_by_name(&conn, package_name)?;
        if !existing.is_empty() {
            progress.skip_package();
            continue;
        }

        // Query package info based on package manager
        let (pkg_name, pkg_version, pkg_arch, pkg_desc): (String, String, String, Option<String>) =
            match pkg_mgr {
                SystemPackageManager::Rpm => match rpm_query::query_package(package_name) {
                    Ok(info) => (
                        info.name.clone(),
                        info.version_only(),
                        info.arch.clone(),
                        info.description.clone().or(info.summary.clone()),
                    ),
                    Err(e) => {
                        println!(
                            "Package '{}' not found in RPM database: {}",
                            package_name, e
                        );
                        continue;
                    }
                },
                SystemPackageManager::Dpkg => match dpkg_query::query_package(package_name) {
                    Ok(info) => (
                        info.name.clone(),
                        info.version_only(),
                        info.arch.clone(),
                        info.description.clone(),
                    ),
                    Err(e) => {
                        println!(
                            "Package '{}' not found in dpkg database: {}",
                            package_name, e
                        );
                        continue;
                    }
                },
                SystemPackageManager::Pacman => match pacman_query::query_package(package_name) {
                    Ok(info) => (
                        info.name.clone(),
                        info.version_only(),
                        info.arch.clone(),
                        info.description.clone(),
                    ),
                    Err(e) => {
                        println!(
                            "Package '{}' not found in pacman database: {}",
                            package_name, e
                        );
                        continue;
                    }
                },
                _ => {
                    println!("Unsupported package manager");
                    continue;
                }
            };

        progress.set_phase(&pkg_name, AdoptPhase::Querying);

        // Pre-fetch all PM metadata and perform CAS writes OUTSIDE the
        // transaction so the SQLite write lock is held only for DB inserts.
        // Any CAS objects written before a DB failure become GC-reclaimable
        // orphans -- the same trade-off the install pipeline makes.
        let raw_files: Vec<FileInfoTuple> = match pkg_mgr {
            SystemPackageManager::Rpm => rpm_query::query_package_files(&pkg_name)
                .map_err(|e| anyhow::anyhow!("RPM file query failed: {e}"))?,
            SystemPackageManager::Dpkg => dpkg_query::query_package_files(&pkg_name)
                .map_err(|e| anyhow::anyhow!("dpkg file query failed: {e}"))?,
            SystemPackageManager::Pacman => pacman_query::query_package_files(&pkg_name)
                .map_err(|e| anyhow::anyhow!("pacman file query failed: {e}"))?,
            _ => Vec::new(),
        }
        .into_iter()
        .map(|f| {
            (
                f.path,
                f.size,
                f.mode,
                f.digest,
                f.user,
                f.group,
                f.link_target,
            )
        })
        .collect();

        // Perform CAS writes (hardlinks) before opening the transaction.
        let files_with_hashes: Vec<(FileInfoTuple, String)> = raw_files
            .into_iter()
            .map(|f| {
                let hash = compute_file_hash(
                    &f.0,
                    f.2,
                    f.3.as_deref(),
                    f.6.as_deref(),
                    full,
                    cas.as_ref(),
                );
                (f, hash)
            })
            .collect();

        let deps: Vec<DependencyInfo> = match pkg_mgr {
            SystemPackageManager::Rpm => rpm_query::query_package_dependencies_full(&pkg_name)
                .map_err(|e| anyhow::anyhow!("RPM dep query failed: {e}"))?,
            SystemPackageManager::Dpkg => dpkg_query::query_package_dependencies_full(&pkg_name)
                .map_err(|e| anyhow::anyhow!("dpkg dep query failed: {e}"))?,
            SystemPackageManager::Pacman => {
                pacman_query::query_package_dependencies_full(&pkg_name)
                    .map_err(|e| anyhow::anyhow!("pacman dep query failed: {e}"))?
            }
            _ => Vec::new(),
        };

        let provides: Vec<String> = match pkg_mgr {
            SystemPackageManager::Rpm => rpm_query::query_package_provides(&pkg_name)
                .map_err(|e| anyhow::anyhow!("RPM provides query failed: {e}"))?,
            SystemPackageManager::Dpkg => dpkg_query::query_package_provides(&pkg_name)
                .map_err(|e| anyhow::anyhow!("dpkg provides query failed: {e}"))?,
            SystemPackageManager::Pacman => pacman_query::query_package_provides(&pkg_name)
                .map_err(|e| anyhow::anyhow!("pacman provides query failed: {e}"))?,
            _ => Vec::new(),
        };

        // Create changeset for this package
        let mut changeset = Changeset::new(format!(
            "Adopt {} {} ({})",
            pkg_name,
            pkg_version,
            if full { "full" } else { "track" }
        ));

        // DB-only transaction: all PM queries and CAS writes are already done.
        let (changeset_id, adopted) = conary_core::db::transaction(&mut conn, |tx| {
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
            trove.source_distro = source_identity.source_distro.clone();
            trove.version_scheme = source_identity.version_scheme.clone();

            let trove_id = trove.insert(tx)?;

            progress.set_phase(&pkg_name, AdoptPhase::Inserting);
            let total_inserts = files_with_hashes.len()
                + deps.iter().filter(|dep| !dep.name.is_empty()).count()
                + provides
                    .iter()
                    .filter(|provide| !provide.is_empty())
                    .count();
            let mut insert_failures = 0usize;

            for (
                (file_path, file_size, file_mode, _digest, file_user, file_group, file_link_target),
                hash,
            ) in &files_with_hashes
            {
                let mut file_entry = FileEntry::new(
                    file_path.clone(),
                    hash.clone(),
                    *file_size,
                    *file_mode,
                    trove_id,
                );
                file_entry.owner = file_user.clone();
                file_entry.group_name = file_group.clone();
                file_entry.symlink_target = file_link_target.clone();

                // Use INSERT OR REPLACE to handle shared paths (directories, etc.)
                if let Err(e) = file_entry.insert_or_replace(tx) {
                    debug!("Failed to insert file {}: {}", file_path, e);
                    insert_failures += 1;
                }
            }

            for dep in &deps {
                if dep.name.is_empty() {
                    continue;
                }

                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep.name.clone(),
                    None, // depends_on_version is for resolved version, not constraint
                    "runtime".to_string(),
                    dep.constraint.clone(), // Store the version constraint
                );
                if let Err(e) = dep_entry.insert(tx) {
                    debug!("Failed to insert dependency {}: {}", dep.name, e);
                    insert_failures += 1;
                }
            }

            for provide in &provides {
                if provide.is_empty() {
                    continue;
                }
                let mut provide_entry = ProvideEntry::new(trove_id, provide.clone(), None);
                if let Err(e) = provide_entry.insert_or_ignore(tx) {
                    debug!("Failed to insert provide {}: {}", provide, e);
                    insert_failures += 1;
                }
            }

            if !metadata_insert_succeeded(total_inserts, insert_failures) {
                debug!(
                    "All {} metadata insert(s) failed for {}; removing empty adopted trove",
                    total_inserts, pkg_name
                );
                conary_core::db::models::Trove::delete(tx, trove_id)?;
                changeset.update_status(tx, ChangesetStatus::RolledBack)?;
                return Ok((changeset_id, false));
            }

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok((changeset_id, true))
        })?;

        if !adopted {
            continue;
        }

        // Create state snapshot for rollback safety
        create_state_snapshot(&conn, changeset_id, &format!("Adopt {}", pkg_name))?;

        progress.complete_package(&pkg_name);
    }

    progress.finish("Adoption complete");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::metadata_insert_succeeded;

    #[test]
    fn metadata_insert_succeeded_rejects_empty_troves() {
        assert!(!metadata_insert_succeeded(3, 3));
    }

    #[test]
    fn metadata_insert_succeeded_allows_partial_or_empty_metadata() {
        assert!(metadata_insert_succeeded(3, 2));
        assert!(metadata_insert_succeeded(0, 0));
    }
}
