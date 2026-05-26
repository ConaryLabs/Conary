// src/commands/adopt/refresh.rs

//! Drift detection and refresh for adopted packages
//!
//! Compares adopted trove versions against the current system state and
//! updates any that have drifted (version changed, package removed, etc.).

use super::super::create_state_snapshot;
use super::super::open_db;
use super::cas_capture::prepare_cas_backed_package_files;
use super::outcome::write_warning_metadata;
use super::system::{FileInfoTuple, compute_file_hash};
use crate::commands::AdoptionWarning;
use anyhow::Result;
use conary_core::db::models::{
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, ProvideEntry, Trove,
};
use conary_core::packages::{
    DependencyInfo, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use tracing::warn;

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

struct RefreshReplacement {
    files: Vec<(FileInfoTuple, String)>,
    deps: Vec<DependencyInfo>,
    provides: Vec<String>,
}

impl RefreshReplacement {
    #[cfg(test)]
    fn test_fixture(_trove_id: i64) -> Self {
        Self {
            files: Vec::new(),
            deps: Vec::new(),
            provides: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefreshFailureInjection {
    None,
    AfterDelete,
}

impl RefreshFailureInjection {
    #[cfg(test)]
    fn after_delete(enabled: bool) -> Self {
        if enabled {
            Self::AfterDelete
        } else {
            Self::None
        }
    }
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
            println!(
                "No adopted packages found. Run 'conary --allow-live-system-mutation system adopt --system' first."
            );
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

    // Pre-fetch all PM metadata and perform CAS writes OUTSIDE the transaction
    // for packages that need updating. This keeps the SQLite write lock short
    // and avoids CAS-vs-DB inconsistency (orphaned CAS objects are GC-reclaimable).
    struct UpdateData<'a> {
        trove: &'a Trove,
        trove_id: i64,
        sys_ver: String,
        sys_arch: String,
        sys_desc: Option<String>,
        replacement: RefreshReplacement,
    }

    let mut update_data: Vec<UpdateData<'_>> = Vec::new();
    let mut skip_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (trove, outcome) in &results {
        if let DriftOutcome::Updated { .. } = outcome {
            let trove_id = match trove.id {
                Some(id) => id,
                None => {
                    warn!("Trove {} has no id, skipping", trove.name);
                    skip_names.insert(trove.name.clone());
                    continue;
                }
            };

            let (sys_ver, sys_arch, sys_desc) = match system_packages.get(&trove.name) {
                Some(entry) => entry,
                None => {
                    warn!(
                        "Trove '{}' marked as updated but missing from system_packages map, skipping",
                        trove.name
                    );
                    skip_names.insert(trove.name.clone());
                    continue;
                }
            };

            let use_cas = trove.install_source == InstallSource::AdoptedFull;

            // Query PM metadata outside the transaction.
            let raw_files = match query_package_files(pkg_mgr, &trove.name) {
                Ok(f) => f,
                Err(e) => {
                    warn!(
                        "Failed to query files for '{}': {}; skipping",
                        trove.name, e
                    );
                    skip_names.insert(trove.name.clone());
                    continue;
                }
            };
            let deps = match query_package_deps(pkg_mgr, &trove.name) {
                Ok(d) => d,
                Err(e) => {
                    warn!("Failed to query deps for '{}': {}; skipping", trove.name, e);
                    skip_names.insert(trove.name.clone());
                    continue;
                }
            };
            let provides = match query_package_provides(pkg_mgr, &trove.name) {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        "Failed to query provides for '{}': {}; skipping",
                        trove.name, e
                    );
                    skip_names.insert(trove.name.clone());
                    continue;
                }
            };

            // Perform CAS writes outside the transaction.
            let files_with_hashes: Vec<(FileInfoTuple, String)> = if use_cas {
                match prepare_cas_backed_package_files(&trove.name, &raw_files, &cas) {
                    Ok(files) => files,
                    Err(e) => {
                        warn!(
                            "Failed to prepare CAS-backed refresh for '{}': {}",
                            trove.name, e
                        );
                        skip_names.insert(trove.name.clone());
                        continue;
                    }
                }
            } else {
                raw_files
                    .into_iter()
                    .map(|f| {
                        let hash = compute_file_hash(
                            &f.0,
                            f.2,
                            f.3.as_deref(),
                            f.6.as_deref(),
                            false,
                            None,
                        );
                        (f, hash)
                    })
                    .collect()
            };

            update_data.push(UpdateData {
                trove,
                trove_id,
                sys_ver: sys_ver.clone(),
                sys_arch: sys_arch.clone(),
                sys_desc: sys_desc.clone(),
                replacement: RefreshReplacement {
                    files: files_with_hashes,
                    deps,
                    provides,
                },
            });
        }
    }

    let mut changeset = Changeset::new(format!(
        "Refresh adopted packages: {} updated, {} removed",
        updated_count, removed_count
    ));

    let mut actually_updated = 0u32;
    let mut actually_removed = 0u32;
    let mut degraded_count = 0u32;

    // DB-only transaction: all PM queries and CAS writes are already done.
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;
        let mut adoption_warnings = Vec::new();

        for (trove, outcome) in &results {
            match outcome {
                DriftOutcome::Unchanged => {}

                DriftOutcome::Removed => {
                    let trove_id = match trove.id {
                        Some(id) => id,
                        None => continue, // already warned above
                    };
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
                    // Skip packages whose pre-fetch failed.
                    if skip_names.contains(&trove.name) {
                        continue;
                    }

                    let data = match update_data.iter().find(|d| d.trove.name == trove.name) {
                        Some(d) => d,
                        None => continue,
                    };

                    if let Err(error) = replace_refresh_children_for_package(
                        tx,
                        trove.name.as_str(),
                        data.trove_id,
                        changeset_id,
                        data.sys_ver.as_str(),
                        data.sys_arch.as_str(),
                        data.sys_desc.as_deref(),
                        &data.replacement,
                        RefreshFailureInjection::None,
                    ) {
                        warn!(
                            "Failed to refresh metadata for '{}'; preserving old metadata: {}",
                            trove.name, error
                        );
                        adoption_warnings.push(AdoptionWarning::refresh_replacement_failure(
                            trove.name.clone(),
                            error.to_string(),
                        ));
                        degraded_count += 1;
                        continue;
                    }

                    if !quiet {
                        println!("Updated: {} {} -> {}", trove.name, old_version, new_version);
                    }
                    actually_updated += 1;
                }
            }
        }

        write_warning_metadata(tx, changeset_id, adoption_warnings)
            .map_err(|e| conary_core::Error::Io(std::io::Error::other(e.to_string())))?;
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
        if degraded_count > 0 {
            println!(
                "Refreshed with warnings: {degraded_count} package(s). Run `conary system history` to inspect adoption warning metadata."
            );
        }
    }

    Ok(())
}

fn with_refresh_savepoint<T>(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let savepoint = format!("refresh_trove_{trove_id}");
    tx.execute_batch(&format!("SAVEPOINT {savepoint}"))?;
    match f(tx) {
        Ok(value) => {
            tx.execute_batch(&format!("RELEASE {savepoint}"))?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.execute_batch(&format!("ROLLBACK TO {savepoint}"));
            let _ = tx.execute_batch(&format!("RELEASE {savepoint}"));
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn replace_refresh_children_for_package(
    tx: &rusqlite::Transaction<'_>,
    trove_name: &str,
    trove_id: i64,
    changeset_id: i64,
    sys_ver: &str,
    sys_arch: &str,
    sys_desc: Option<&str>,
    replacement: &RefreshReplacement,
    injection: RefreshFailureInjection,
) -> Result<()> {
    with_refresh_savepoint(tx, trove_id, |tx| {
        tx.execute(
            "UPDATE troves SET version = ?1, architecture = ?2, description = ?3,
             installed_by_changeset_id = ?4
             WHERE id = ?5",
            rusqlite::params![sys_ver, sys_arch, sys_desc, changeset_id, trove_id],
        )?;

        tx.execute("DELETE FROM files WHERE trove_id = ?1", [trove_id])?;
        tx.execute("DELETE FROM dependencies WHERE trove_id = ?1", [trove_id])?;
        tx.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;

        if injection == RefreshFailureInjection::AfterDelete {
            return Err(anyhow::anyhow!(
                "injected refresh child replacement failure"
            ));
        }

        for (
            (file_path, file_size, file_mode, _digest, file_user, file_group, link_target),
            hash,
        ) in &replacement.files
        {
            let mut fe = FileEntry::new(
                file_path.clone(),
                hash.clone(),
                *file_size,
                *file_mode,
                trove_id,
            );
            fe.owner = file_user.clone();
            fe.group_name = file_group.clone();
            fe.symlink_target = link_target.clone();
            fe.insert_or_replace(tx).map_err(|e| {
                anyhow::anyhow!("failed to insert refreshed file {file_path} for {trove_name}: {e}")
            })?;
        }

        for dep in &replacement.deps {
            if dep.name.is_empty() {
                continue;
            }
            let mut de = DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                None,
                "runtime".to_string(),
                dep.constraint.clone(),
            );
            de.insert(tx).map_err(|e| {
                anyhow::anyhow!("failed to insert refreshed dependency for {trove_name}: {e}")
            })?;
        }

        for provide in &replacement.provides {
            if provide.is_empty() {
                continue;
            }
            let mut pe = ProvideEntry::new(trove_id, provide.clone(), None);
            pe.insert_or_ignore(tx).map_err(|e| {
                anyhow::anyhow!("failed to insert refreshed provide for {trove_name}: {e}")
            })?;
        }

        Ok(())
    })
}

#[cfg(test)]
fn replace_refresh_children_for_package_for_test(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    fail_after_delete: bool,
) -> Result<()> {
    let replacement = RefreshReplacement::test_fixture(trove_id);
    replace_refresh_children_for_package(
        tx,
        "curl",
        trove_id,
        1,
        "8.9.0",
        "x86_64",
        Some("refreshed fixture"),
        &replacement,
        RefreshFailureInjection::after_delete(fail_after_delete),
    )
}

/// Query all currently installed packages from the active package manager.
/// Returns a map of name -> (version, arch, description).
fn query_all_current(pkg_mgr: SystemPackageManager) -> Result<InstalledPackageMap> {
    let map = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_all_packages()?
            .into_iter()
            .map(|(name, info)| {
                let desc = info.description.clone().or(info.summary.clone());
                // Use full_version (epoch:version-release) to match the version
                // stored during adopt, so drift detection compares apples to apples.
                (name, (info.full_version(), info.arch.clone(), desc))
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
fn query_package_files(pkg_mgr: SystemPackageManager, name: &str) -> Result<Vec<FileInfoTuple>> {
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
        .collect())
}

/// Query runtime dependencies for a package from the active package manager.
///
/// Returns an error on PM query failure so callers can handle it explicitly.
fn query_package_deps(pkg_mgr: SystemPackageManager, name: &str) -> Result<Vec<DependencyInfo>> {
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
fn query_package_provides(pkg_mgr: SystemPackageManager, name: &str) -> Result<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db;
    use conary_core::db::models::{
        Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, ProvideEntry, Trove,
        TroveType,
    };

    fn create_refresh_test_db() -> (tempfile::TempDir, String, rusqlite::Connection, i64) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db").to_string_lossy().into_owned();
        db::init(&db_path).unwrap();
        let mut conn = db::open(&db_path).unwrap();
        let trove_id = db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new("seed adopted".to_string());
            let changeset_id = changeset.insert(tx)?;
            let mut trove = Trove::new_with_source(
                "curl".to_string(),
                "8.8.0".to_string(),
                TroveType::Package,
                InstallSource::AdoptedFull,
            );
            trove.installed_by_changeset_id = Some(changeset_id);
            let trove_id = trove.insert(tx)?;
            let mut file = FileEntry::new(
                "/usr/bin/curl".to_string(),
                "old-hash".to_string(),
                4,
                0o100755,
                trove_id,
            );
            file.insert(tx)?;
            let mut dep = DependencyEntry::new(
                trove_id,
                "openssl".to_string(),
                None,
                "runtime".to_string(),
                None,
            );
            dep.insert(tx)?;
            let mut provide = ProvideEntry::new(trove_id, "curl".to_string(), None);
            provide.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(trove_id)
        })
        .unwrap();
        (tmp, db_path, conn, trove_id)
    }

    #[test]
    fn refresh_savepoint_preserves_old_children_when_replacement_fails() {
        let (_tmp, _db_path, mut conn, trove_id) = create_refresh_test_db();
        let result = db::transaction(&mut conn, |tx| {
            let err = replace_refresh_children_for_package_for_test(tx, trove_id, true)
                .expect_err("injected replacement failure should be isolated to savepoint");
            assert!(
                err.to_string()
                    .contains("injected refresh child replacement failure")
            );

            tx.execute(
                "UPDATE troves SET description = ?1 WHERE id = ?2",
                ("outer transaction committed", trove_id),
            )?;
            Ok(())
        });

        assert!(result.is_ok());

        let file_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();
        let dep_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dependencies WHERE trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();
        let provide_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provides WHERE trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(file_count, 1);
        assert_eq!(dep_count, 1);
        assert_eq!(provide_count, 1);
        let description: String = conn
            .query_row(
                "SELECT description FROM troves WHERE id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(description, "outer transaction committed");
    }
}
