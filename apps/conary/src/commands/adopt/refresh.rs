// src/commands/adopt/refresh.rs

//! Drift detection and refresh for adopted packages
//!
//! Compares adopted trove versions against the current system state and
//! updates any that have drifted (version changed, package removed, etc.).

use super::super::create_state_snapshot;
use super::super::open_db;
use super::cas_capture::{CapturedAdoptionFile, capture_package_files};
use super::checkpoint::write_db_checkpoint;
use super::outcome::write_warning_metadata;
use super::system::FileInfoTuple;
use crate::commands::AdoptionWarning;
use anyhow::Result;
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{
    Changeset, ChangesetStatus, ExistingDirectoryMaterialization, InstallSource, Trove,
};
use conary_core::packages::{
    InstalledPackageIdentity, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use conary_core::repository::dependency_model::{ProvidedCapability, RepositoryRequirementGroup};
use conary_core::repository::versioning::VersionScheme;
use tracing::warn;

#[derive(Debug)]
struct CurrentNativePackage {
    identity: InstalledPackageIdentity,
    description: Option<String>,
}

/// Outcome for a single adopted package after drift check
#[derive(Debug)]
enum DriftOutcome {
    /// Version in DB matches system — no action needed
    Unchanged,
    /// Version changed — DB record updated
    Updated {
        old_version: String,
        new_version: String,
        identity: InstalledPackageIdentity,
    },
    /// Package no longer present in system package manager
    Removed,
}

fn classify_current_package(
    trove: &Trove,
    current: &[CurrentNativePackage],
) -> Result<DriftOutcome> {
    let previous = trove.native_package_identity.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "adopted package '{}' has no exact native package identity",
            trove.name
        )
    })?;
    if current.iter().any(|package| package.identity == *previous) {
        return Ok(DriftOutcome::Unchanged);
    }
    let mut candidates = current
        .iter()
        .filter(|package| previous.same_name_and_architecture(&package.identity))
        .collect::<Vec<_>>();
    match candidates.len() {
        0 => Ok(DriftOutcome::Removed),
        1 => {
            let package = candidates.pop().expect("one candidate");
            Ok(DriftOutcome::Updated {
                old_version: trove.version.clone(),
                new_version: package.identity.version(),
                identity: package.identity.clone(),
            })
        }
        count => Err(anyhow::anyhow!(
            "adopted package '{}' {} [{}] has {count} installed native replacement candidates; exact refresh cannot choose one",
            trove.name,
            trove.version,
            previous.architecture()
        )),
    }
}

struct RefreshReplacement {
    files: Vec<CapturedAdoptionFile>,
    requirements: Vec<RepositoryRequirementGroup>,
    provides: Vec<ProvidedCapability>,
}

impl RefreshReplacement {
    #[cfg(test)]
    fn test_fixture(_trove_id: i64) -> Self {
        Self {
            files: Vec::new(),
            requirements: Vec::new(),
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
    requested_manager: Option<SystemPackageManager>,
) -> Result<()> {
    let pkg_mgr = SystemPackageManager::resolve(requested_manager)?;
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

    // Preserve every exact native package-manager record.
    let system_packages = query_all_current(pkg_mgr)?;

    // Classify each adopted trove
    let mut results: Vec<(&Trove, DriftOutcome)> = Vec::new();

    for trove in &adopted {
        let outcome = classify_current_package(trove, &system_packages)?;
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
                        ..
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
        native_identity: InstalledPackageIdentity,
        sys_ver: String,
        sys_arch: String,
        sys_desc: Option<String>,
        replacement: RefreshReplacement,
    }

    let mut update_data: Vec<UpdateData<'_>> = Vec::new();
    let mut skip_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();

    for (trove, outcome) in &results {
        if let DriftOutcome::Updated { identity, .. } = outcome {
            let trove_id = match trove.id {
                Some(id) => id,
                None => {
                    warn!("Trove {} has no id, skipping", trove.name);
                    continue;
                }
            };

            let current = system_packages
                .iter()
                .find(|package| package.identity == *identity)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "exact native refresh identity '{}' disappeared from inventory",
                        identity.selector()
                    )
                })?;

            let use_cas = trove.install_source == InstallSource::AdoptedFull;

            // Query PM metadata outside the transaction.
            let raw_files = match query_package_files(pkg_mgr, identity.selector()) {
                Ok(f) => f,
                Err(e) => {
                    warn!(
                        "Failed to query files for '{}': {}; skipping",
                        identity.selector(),
                        e
                    );
                    skip_ids.insert(trove_id);
                    continue;
                }
            };
            let requirements =
                match super::requirements::query_package_requirements(pkg_mgr, identity.selector())
                {
                    Ok(d) => d,
                    Err(e) => {
                        warn!(
                            "Failed to query deps for '{}': {}; skipping",
                            identity.selector(),
                            e
                        );
                        skip_ids.insert(trove_id);
                        continue;
                    }
                };
            let mut provides = match super::provides::query_package_provides(pkg_mgr, identity) {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        "Failed to query provides for '{}': {}; skipping",
                        identity.selector(),
                        e
                    );
                    skip_ids.insert(trove_id);
                    continue;
                }
            };

            // Capture exact live nodes and bytes outside the transaction.
            let captured_files = match capture_package_files(
                identity.selector(),
                &raw_files,
                if use_cas { Some(&cas) } else { None },
            ) {
                Ok(files) => files,
                Err(error) => {
                    warn!(
                        "Failed to capture exact refresh payload for '{}': {}",
                        identity.selector(),
                        error
                    );
                    skip_ids.insert(trove_id);
                    continue;
                }
            };
            conary_core::repository::dependency_model::extend_materialized_file_provides(
                &mut provides,
                identity.source_package_format(),
                captured_files.iter().map(|file| file.source.0.as_str()),
            )?;

            update_data.push(UpdateData {
                trove,
                trove_id,
                native_identity: identity.clone(),
                sys_ver: identity.version(),
                sys_arch: identity.architecture().to_string(),
                sys_desc: current.description.clone(),
                replacement: RefreshReplacement {
                    files: captured_files,
                    requirements,
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
    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
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
                    ..
                } => {
                    // Skip packages whose pre-fetch failed.
                    let Some(trove_id) = trove.id else {
                        continue;
                    };
                    if skip_ids.contains(&trove_id) {
                        continue;
                    }

                    let data = match update_data.iter().find(|data| data.trove_id == trove_id) {
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
                        data.trove.version_scheme,
                        &data.native_identity,
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
    write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

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
    version_scheme: VersionScheme,
    native_identity: &InstalledPackageIdentity,
    replacement: &RefreshReplacement,
    injection: RefreshFailureInjection,
) -> Result<()> {
    with_refresh_savepoint(tx, trove_id, |tx| {
        native_identity.validate()?;
        let native_identity_json = serde_json::to_string(native_identity)?;
        tx.execute(
            "UPDATE troves SET version = ?1, architecture = ?2, description = ?3,
             installed_by_changeset_id = ?4, native_package_identity_json = ?5
             WHERE id = ?6",
            rusqlite::params![
                sys_ver,
                sys_arch,
                sys_desc,
                changeset_id,
                native_identity_json,
                trove_id
            ],
        )?;

        delete_refresh_payload_authority(tx, trove_id)?;
        tx.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;

        if injection == RefreshFailureInjection::AfterDelete {
            return Err(anyhow::anyhow!(
                "injected refresh child replacement failure"
            ));
        }

        for captured in &replacement.files {
            let file_path = &captured.source.0;
            let mut fe = captured.file_entry(trove_id);
            fe.insert_or_replace(tx, ExistingDirectoryMaterialization::ApplyIncoming)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "failed to insert refreshed file {file_path} for {trove_name}: {e}"
                    )
                })?;
        }

        super::requirements::replace_package_requirements(
            tx,
            trove_id,
            version_scheme,
            trove_name,
            &replacement.requirements,
        )
        .map_err(|error| {
            anyhow::anyhow!("failed to replace exact requirements for {trove_name}: {error}")
        })?;

        super::provides::insert_package_provides(
            tx,
            trove_id,
            native_identity,
            &replacement.provides,
        )
        .map_err(|error| {
            anyhow::anyhow!("failed to insert refreshed provides for {trove_name}: {error}")
        })?;

        Ok(())
    })
}

fn delete_refresh_payload_authority(tx: &rusqlite::Transaction<'_>, trove_id: i64) -> Result<()> {
    for claim in conary_core::db::models::PayloadClaim::find_by_trove(tx, trove_id)? {
        conary_core::db::models::PayloadClaim::delete(tx, &claim.path, trove_id)?;
    }
    Ok(())
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
        "8.9.0-1",
        "x86_64",
        Some("refreshed fixture"),
        VersionScheme::Rpm,
        &InstalledPackageIdentity::rpm("curl-8.9.0-1.x86_64", "curl", None, "8.9.0", "1", "x86_64")
            .unwrap(),
        &replacement,
        RefreshFailureInjection::after_delete(fail_after_delete),
    )
}

/// Query every exact currently installed native package-manager record.
fn query_all_current(pkg_mgr: SystemPackageManager) -> Result<Vec<CurrentNativePackage>> {
    let packages = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_all_packages()?
            .into_iter()
            .map(|record| CurrentNativePackage {
                identity: record.identity,
                description: record.info.description.or(record.info.summary),
            })
            .collect(),
        SystemPackageManager::Dpkg => dpkg_query::query_all_packages()?
            .into_iter()
            .map(|record| CurrentNativePackage {
                identity: record.identity,
                description: record.info.description,
            })
            .collect(),
        SystemPackageManager::Pacman => pacman_query::query_all_packages()?
            .into_iter()
            .map(|record| CurrentNativePackage {
                identity: record.identity,
                description: record.info.description,
            })
            .collect(),
        _ => return Err(anyhow::anyhow!("Unsupported package manager")),
    };
    Ok(packages)
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
                f.absence_policy,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db;
    use conary_core::db::models::{
        Changeset, ChangesetStatus, FileEntry, InstallSource, InstalledRequirementGroup,
        ProvideEntry, Trove, TroveType,
    };
    use conary_core::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};

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
                "8.8.0-1".to_string(),
                TroveType::Package,
                InstallSource::AdoptedFull,
                VersionScheme::Rpm,
            );
            trove.architecture = Some("x86_64".to_string());
            trove.installed_by_changeset_id = Some(changeset_id);
            trove.native_package_identity = Some(
                InstalledPackageIdentity::rpm(
                    "curl-8.8.0-1.x86_64",
                    "curl",
                    None,
                    "8.8.0",
                    "1",
                    "x86_64",
                )
                .unwrap(),
            );
            let trove_id = trove.insert(tx)?;
            let mut file = FileEntry::new(
                "/usr/bin/curl".to_string(),
                ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755)).unwrap(),
                Some(PayloadContentAuthority {
                    sha256: conary_core::hash::sha256(b"curl"),
                    size: 4,
                }),
                trove_id,
            );
            file.insert(tx)?;
            let requirement = conary_core::repository::requirement::parse_native_requirement(
                conary_core::repository::dependency_model::RepositoryRequirementKind::Depends,
                conary_core::repository::versioning::VersionScheme::Rpm,
                "openssl",
            )
            .unwrap();
            InstalledRequirementGroup::insert_groups(
                tx,
                trove_id,
                conary_core::repository::versioning::VersionScheme::Rpm,
                &[requirement],
            )?;
            let mut provide = ProvideEntry::new(
                trove_id,
                "curl".to_string(),
                None,
                conary_core::repository::versioning::VersionScheme::Rpm,
            );
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
                "SELECT COUNT(*) FROM package_requirement_groups WHERE trove_id = ?1",
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

    fn adopted_dpkg_trove(selector: &str, architecture: &str, version: &str) -> Trove {
        let mut trove = Trove::new_with_source(
            "libc6".to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            VersionScheme::Debian,
        );
        trove.architecture = Some(architecture.to_string());
        trove.native_package_identity =
            Some(InstalledPackageIdentity::dpkg(selector, "libc6", version, architecture).unwrap());
        trove
    }

    #[test]
    fn multiarch_refresh_matches_the_intended_native_variant() {
        let amd64 = CurrentNativePackage {
            identity: InstalledPackageIdentity::dpkg("libc6:amd64", "libc6", "2.42-1", "amd64")
                .unwrap(),
            description: None,
        };
        let i386 = CurrentNativePackage {
            identity: InstalledPackageIdentity::dpkg("libc6:i386", "libc6", "2.41-1", "i386")
                .unwrap(),
            description: None,
        };
        let current = vec![amd64, i386];

        let old_amd64 = adopted_dpkg_trove("libc6:amd64", "amd64", "2.41-1");
        let unchanged_i386 = adopted_dpkg_trove("libc6:i386", "i386", "2.41-1");

        match classify_current_package(&old_amd64, &current).unwrap() {
            DriftOutcome::Updated {
                new_version,
                identity,
                ..
            } => {
                assert_eq!(new_version, "2.42-1");
                assert_eq!(identity.selector(), "libc6:amd64");
            }
            other => panic!("expected exact amd64 update, got {other:?}"),
        }
        assert!(matches!(
            classify_current_package(&unchanged_i386, &current).unwrap(),
            DriftOutcome::Unchanged
        ));
    }
}
