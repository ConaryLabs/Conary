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

        // Create changeset for this package
        let mut changeset = Changeset::new(format!(
            "Adopt {} {} ({})",
            pkg_name,
            pkg_version,
            if full { "full" } else { "track" }
        ));

        let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
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

            // Query and insert files based on package manager
            let files: Vec<FileInfoTuple> = match pkg_mgr {
                SystemPackageManager::Rpm => rpm_query::query_package_files(&pkg_name)?
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
                    .collect(),
                SystemPackageManager::Dpkg => dpkg_query::query_package_files(&pkg_name)?
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
                    .collect(),
                SystemPackageManager::Pacman => pacman_query::query_package_files(&pkg_name)?
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
                    .collect(),
                _ => Vec::new(),
            };
            progress.set_phase(&pkg_name, AdoptPhase::Inserting);

            for (
                file_path,
                file_size,
                file_mode,
                file_digest,
                file_user,
                file_group,
                file_link_target,
            ) in &files
            {
                let hash = compute_file_hash(
                    file_path,
                    *file_mode,
                    file_digest.as_deref(),
                    file_link_target.as_deref(),
                    full,
                    cas.as_ref(),
                );

                let mut file_entry =
                    FileEntry::new(file_path.clone(), hash, *file_size, *file_mode, trove_id);
                file_entry.owner = file_user.clone();
                file_entry.group_name = file_group.clone();
                file_entry.symlink_target = file_link_target.clone();

                // Use INSERT OR REPLACE to handle shared paths (directories, etc.)
                if let Err(e) = file_entry.insert_or_replace(tx) {
                    debug!("Failed to insert file {}: {}", file_path, e);
                }
            }

            // Query and insert dependencies with version constraints
            let deps: Vec<DependencyInfo> = match pkg_mgr {
                SystemPackageManager::Rpm => {
                    rpm_query::query_package_dependencies_full(&pkg_name).unwrap_or_default()
                }
                SystemPackageManager::Dpkg => {
                    dpkg_query::query_package_dependencies_full(&pkg_name).unwrap_or_default()
                }
                SystemPackageManager::Pacman => {
                    pacman_query::query_package_dependencies_full(&pkg_name).unwrap_or_default()
                }
                _ => Vec::new(),
            };

            for dep in deps {
                if dep.name.is_empty() {
                    continue;
                }

                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep.name.clone(),
                    None, // depends_on_version is for resolved version, not constraint
                    "runtime".to_string(),
                    dep.constraint, // Store the version constraint
                );
                if let Err(e) = dep_entry.insert(tx) {
                    debug!("Failed to insert dependency {}: {}", dep.name, e);
                }
            }

            // Query and insert provides (capabilities this package offers)
            let provides: Vec<String> = match pkg_mgr {
                SystemPackageManager::Rpm => {
                    rpm_query::query_package_provides(&pkg_name).unwrap_or_default()
                }
                SystemPackageManager::Dpkg => {
                    dpkg_query::query_package_provides(&pkg_name).unwrap_or_default()
                }
                SystemPackageManager::Pacman => {
                    pacman_query::query_package_provides(&pkg_name).unwrap_or_default()
                }
                _ => Vec::new(),
            };

            for provide in provides {
                if provide.is_empty() {
                    continue;
                }
                let mut provide_entry = ProvideEntry::new(trove_id, provide.clone(), None);
                if let Err(e) = provide_entry.insert_or_ignore(tx) {
                    debug!("Failed to insert provide {}: {}", provide, e);
                }
            }

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(changeset_id)
        })?;

        // Create state snapshot for rollback safety
        create_state_snapshot(&conn, changeset_id, &format!("Adopt {}", pkg_name))?;

        progress.complete_package(&pkg_name);
    }

    progress.finish("Adoption complete");

    Ok(())
}
