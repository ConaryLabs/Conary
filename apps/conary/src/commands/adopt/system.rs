// apps/conary/src/commands/adopt/system.rs

//! Bulk system package adoption
//!
//! Adopts all installed system packages into Conary tracking.

use super::super::create_state_snapshot;
use super::super::open_db;
use super::super::progress::{AdoptPhase, AdoptProgress};
use super::cas_capture::{CapturedAdoptionFile, capture_package_files};
use super::checkpoint::write_db_checkpoint;
use super::outcome::{BulkAdoptionFailure, BulkAdoptionFailureStage, BulkAdoptionOutcome};
use anyhow::Result;
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{
    Changeset, ChangesetStatus, ExistingDirectoryMaterialization, InstallReason, InstallSource,
    ProvideEntry, Trove, TroveType,
};
use conary_core::packages::{
    InstalledPackageIdentity, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use conary_core::repository::dependency_model::RepositoryRequirementGroup;
use std::collections::HashSet;
use tracing::warn;

mod live_root;
use live_root::adopt_live_root_as_full_package;
#[cfg(test)]
use live_root::{live_root_db_path, should_visit_live_root_path};

fn parse_package_pattern(field: &str, pattern: Option<&str>) -> Result<Option<glob::Pattern>> {
    pattern
        .map(|value| {
            glob::Pattern::new(value).map_err(|error| {
                anyhow::anyhow!("Invalid {field} package pattern {value:?}: {error}")
            })
        })
        .transpose()
}

/// File info tuple: (path, size, mode, digest, user, group, link_target)
pub type FileInfoTuple = (
    String,
    i64,
    i32,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Debug)]
struct InstalledSystemPackage {
    identity: InstalledPackageIdentity,
    description: Option<String>,
}

struct PackageData {
    identity: InstalledPackageIdentity,
    description: Option<String>,
    files: Vec<CapturedAdoptionFile>,
    requirements: Vec<RepositoryRequirementGroup>,
    provides: Vec<String>,
    is_dependency: bool,
}

struct PackagePersistenceFailure {
    stage: BulkAdoptionFailureStage,
    message: String,
}

enum PackageSavepointOutcome<T, E> {
    Committed(T),
    RolledBack(E),
}

fn execute_package_savepoint<T, E>(
    conn: &rusqlite::Connection,
    operation: impl FnOnce() -> std::result::Result<T, E>,
) -> conary_core::Result<PackageSavepointOutcome<T, E>> {
    conn.execute_batch("SAVEPOINT conary_bulk_adopt_package")?;
    match operation() {
        Ok(value) => {
            conn.execute_batch("RELEASE SAVEPOINT conary_bulk_adopt_package")?;
            Ok(PackageSavepointOutcome::Committed(value))
        }
        Err(error) => {
            conn.execute_batch(
                "ROLLBACK TO SAVEPOINT conary_bulk_adopt_package;
                 RELEASE SAVEPOINT conary_bulk_adopt_package",
            )?;
            Ok(PackageSavepointOutcome::RolledBack(error))
        }
    }
}

/// Adopt all installed system packages.
///
/// This is the entry point to the ownership ladder: packages begin as
/// `AdoptedTrack` (metadata only) or `AdoptedFull` (CAS-backed).  From there,
/// `adopt --takeover` can promote them to `Taken` (full Conary ownership),
/// and a future Remi-backed reinstall can elevate to `Repository`.
///
/// Optional filters:
/// - `pattern`: only adopt packages matching this glob (e.g., "lib*")
/// - `exclude`: skip packages matching this glob (e.g., "kernel*")
/// - `explicit_only`: only adopt explicitly installed packages (skip auto-deps)
pub async fn cmd_adopt_system(
    db_path: &str,
    full: bool,
    dry_run: bool,
    pattern: Option<&str>,
    exclude: Option<&str>,
    explicit_only: bool,
    requested_manager: Option<SystemPackageManager>,
) -> Result<BulkAdoptionOutcome> {
    // Detect system package manager
    let pkg_mgr = SystemPackageManager::resolve(requested_manager)?;
    if !pkg_mgr.is_available() {
        if full {
            adopt_live_root_as_full_package(db_path, dry_run, pattern, exclude, explicit_only)?;
            return Ok(BulkAdoptionOutcome {
                considered_packages: vec![LIVE_ROOT_PACKAGE_NAME.to_string()],
                adopted_packages: (!dry_run)
                    .then(|| LIVE_ROOT_PACKAGE_NAME.to_string())
                    .into_iter()
                    .collect(),
                ..Default::default()
            });
        }
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
        ));
    }

    println!("Detected package manager: {:?}", pkg_mgr);
    let version_scheme = pkg_mgr.version_scheme().ok_or_else(|| {
        anyhow::anyhow!(
            "Detected package manager {} has no exact version scheme",
            pkg_mgr.display_name()
        )
    })?;
    let include_pattern = parse_package_pattern("--pattern", pattern)?;
    let exclude_pattern = parse_package_pattern("--exclude", exclude)?;

    let mut conn = open_db(db_path)?;

    // Get list of already-tracked packages to avoid duplicates
    let tracked_packages: HashSet<String> = Trove::list_all(&conn)?
        .into_iter()
        .filter_map(|trove| Some(trove.native_package_identity?.selector().to_string()))
        .collect();

    // Get every exact installed package-manager record. The typed selector is
    // kept distinct from the package name so multilib/multiarch variants
    // survive inventory and all follow-up queries target the intended record.
    let installed: Vec<InstalledSystemPackage> = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_all_packages()?
            .into_iter()
            .map(|record| InstalledSystemPackage {
                identity: record.identity,
                description: record.info.description.or(record.info.summary),
            })
            .collect(),
        SystemPackageManager::Dpkg => dpkg_query::query_all_packages()?
            .into_iter()
            .map(|record| InstalledSystemPackage {
                identity: record.identity,
                description: record.info.description,
            })
            .collect(),
        SystemPackageManager::Pacman => pacman_query::query_all_packages()?
            .into_iter()
            .map(|record| InstalledSystemPackage {
                identity: record.identity,
                description: record.info.description,
            })
            .collect(),
        _ => return Err(anyhow::anyhow!("Unsupported package manager")),
    };

    // Exact install-reason authority is required. An empty set means the
    // package manager explicitly reported no user-installed packages; query
    // failure is not reinterpreted as "everything is explicit."
    let user_installed: std::collections::HashSet<String> = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_user_installed()
            .map_err(|error| anyhow::anyhow!("RPM install-reason query failed: {error}"))?,
        SystemPackageManager::Dpkg => dpkg_query::query_user_installed()
            .map_err(|error| anyhow::anyhow!("dpkg install-reason query failed: {error}"))?,
        SystemPackageManager::Pacman => pacman_query::query_user_installed()
            .map_err(|error| anyhow::anyhow!("pacman install-reason query failed: {error}"))?,
        _ => return Err(anyhow::anyhow!("Unsupported package manager")),
    };

    // Apply selective filters
    let pre_filter_count = installed.len();
    let installed: Vec<_> = installed
        .into_iter()
        .filter(|package| {
            if let Some(pat) = &include_pattern
                && !pat.matches(package.identity.name())
            {
                return false;
            }
            if let Some(exc) = &exclude_pattern
                && exc.matches(package.identity.name())
            {
                return false;
            }
            if explicit_only
                && !user_installed.contains(&package.identity.install_reason_selector())
            {
                return false;
            }
            true
        })
        .collect();
    let total = installed.len();
    let considered_packages = installed
        .iter()
        .map(|package| package.identity.selector().to_string())
        .collect::<Vec<_>>();

    if total < pre_filter_count {
        println!("Filtered: {} -> {} packages", pre_filter_count, total);
    }

    if dry_run {
        let mut to_adopt = 0;
        let mut already_tracked = 0;
        let mut explicit_count = 0;
        let mut dep_count = 0;

        for package in &installed {
            if tracked_packages.contains(package.identity.selector()) {
                already_tracked += 1;
            } else {
                to_adopt += 1;
                if !user_installed.contains(&package.identity.install_reason_selector()) {
                    dep_count += 1;
                } else {
                    explicit_count += 1;
                }
            }
        }

        println!("Dry run: would adopt {} packages\n", to_adopt);
        println!("Summary:");
        println!("  Would adopt: {} packages", to_adopt);
        println!("    Explicit: {}", explicit_count);
        println!("    Dependency: {}", dep_count);
        println!("  Already tracked: {} packages", already_tracked);
        println!(
            "  Mode: {}",
            if full {
                "full (CAS storage)"
            } else {
                "track (metadata only)"
            }
        );
        return Ok(BulkAdoptionOutcome {
            considered_packages,
            already_tracked_packages: installed
                .iter()
                .filter(|package| tracked_packages.contains(package.identity.selector()))
                .map(|package| package.identity.selector().to_string())
                .collect(),
            ..Default::default()
        });
    }

    // Determine install source based on mode
    let install_source = if full {
        InstallSource::AdoptedFull
    } else {
        InstallSource::AdoptedTrack
    };

    // Set up CAS for full mode
    let objects_dir = conary_core::db::paths::objects_dir(db_path);

    let cas = if full {
        Some(conary_core::filesystem::CasStore::new(&objects_dir)?)
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
    let mut adopted_packages = Vec::new();
    let mut already_tracked_packages = Vec::new();
    let mut failures = Vec::new();

    let mode_label = if full { "Adopting (full)" } else { "Adopting" };
    let mut progress = AdoptProgress::new(total as u64, mode_label);

    // Pre-fetch all PM metadata and perform CAS writes OUTSIDE the transaction.
    // This keeps the SQLite write lock short (DB inserts only) and avoids
    // CAS-vs-DB inconsistency: if the DB transaction later rolls back, any CAS
    // objects that were already written become unreachable orphans that the GC
    // will clean up -- the same trade-off the install pipeline makes.
    let mut pre_collected: Vec<PackageData> = Vec::new();

    for package in &installed {
        let selector = package.identity.selector();
        // Skip already-tracked packages
        if tracked_packages.contains(selector) {
            skipped_count += 1;
            already_tracked_packages.push(selector.to_string());
            progress.skip_package();
            continue;
        }

        progress.set_phase(selector, AdoptPhase::Querying);

        // Query ALL PM metadata before opening the DB transaction.
        let files: Vec<FileInfoTuple> = match query_pm_files(pkg_mgr, selector) {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to query files for '{}': {}; skipping", selector, e);
                progress.fail_package(selector, &e.to_string());
                error_count += 1;
                failures.push(BulkAdoptionFailure::new(
                    selector,
                    BulkAdoptionFailureStage::FileQuery,
                    e.to_string(),
                ));
                continue;
            }
        };
        let requirements = match super::requirements::query_package_requirements(pkg_mgr, selector)
        {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to query deps for '{}': {}; skipping", selector, e);
                progress.fail_package(selector, &e.to_string());
                error_count += 1;
                failures.push(BulkAdoptionFailure::new(
                    selector,
                    BulkAdoptionFailureStage::RequirementQuery,
                    e.to_string(),
                ));
                continue;
            }
        };
        let provides: Vec<String> = match query_pm_provides(pkg_mgr, selector) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "Failed to query provides for '{}': {}; skipping",
                    selector, e
                );
                progress.fail_package(selector, &e.to_string());
                error_count += 1;
                failures.push(BulkAdoptionFailure::new(
                    selector,
                    BulkAdoptionFailureStage::ProvideQuery,
                    e.to_string(),
                ));
                continue;
            }
        };

        // Capture exact live nodes and bytes outside the transaction. Full
        // adoption additionally stores every regular-file capture in CAS.
        if full {
            progress.set_phase(selector, AdoptPhase::CasStorage);
        }
        let captured_files =
            match capture_package_files(selector, &files, if full { cas.as_ref() } else { None }) {
                Ok(files) => files,
                Err(error) => {
                    warn!(
                        "Failed to capture exact files for '{}': {}",
                        selector, error
                    );
                    progress.fail_package(selector, &error.to_string());
                    error_count += 1;
                    failures.push(BulkAdoptionFailure::new(
                        selector,
                        BulkAdoptionFailureStage::PayloadCapture,
                        error.to_string(),
                    ));
                    continue;
                }
            };

        let is_dependency = !user_installed.contains(&package.identity.install_reason_selector());

        pre_collected.push(PackageData {
            identity: package.identity.clone(),
            description: package.description.clone(),
            files: captured_files,
            requirements,
            provides,
            is_dependency,
        });
    }

    // DB-only transaction: all PM queries and CAS writes are already done.
    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;

        for pkg in &pre_collected {
            let selector = pkg.identity.selector();
            let persisted = execute_package_savepoint(tx, || {
                let mut trove = Trove::new_with_source(
                    pkg.identity.name().to_string(),
                    pkg.identity.version(),
                    TroveType::Package,
                    install_source.clone(),
                    version_scheme,
                );
                trove.architecture = Some(pkg.identity.architecture().to_string());
                trove.description = pkg.description.clone();
                trove.installed_by_changeset_id = Some(changeset_id);
                trove.native_package_identity = Some(pkg.identity.clone());
                if pkg.is_dependency {
                    trove.install_reason = InstallReason::Dependency;
                    trove.selection_reason =
                        Some("Auto-installed dependency (from system package manager)".to_string());
                } else {
                    trove.selection_reason = Some("Adopted from system".to_string());
                }

                let trove_id = trove
                    .insert(tx)
                    .map_err(|error| PackagePersistenceFailure {
                        stage: BulkAdoptionFailureStage::TroveInsert,
                        message: error.to_string(),
                    })?;

                for captured in &pkg.files {
                    let file_path = &captured.source.0;
                    let mut file_entry = captured.file_entry(trove_id);
                    file_entry
                        .insert_or_replace(tx, ExistingDirectoryMaterialization::ApplyIncoming)
                        .map_err(|error| PackagePersistenceFailure {
                            stage: BulkAdoptionFailureStage::MetadataInsert,
                            message: format!("file {file_path}: {error}"),
                        })?;
                }

                super::requirements::insert_package_requirements(
                    tx,
                    trove_id,
                    version_scheme,
                    pkg.identity.name(),
                    &pkg.requirements,
                )
                .map_err(|error| PackagePersistenceFailure {
                    stage: BulkAdoptionFailureStage::MetadataInsert,
                    message: format!("requirements: {error}"),
                })?;

                for provide in &pkg.provides {
                    if provide.is_empty() {
                        continue;
                    }
                    let mut provide_entry =
                        ProvideEntry::new(trove_id, provide.clone(), None, version_scheme);
                    provide_entry.insert_or_ignore(tx).map_err(|error| {
                        PackagePersistenceFailure {
                            stage: BulkAdoptionFailureStage::MetadataInsert,
                            message: format!("provide {provide}: {error}"),
                        }
                    })?;
                }
                Ok::<i64, PackagePersistenceFailure>(trove_id)
            })?;

            match persisted {
                PackageSavepointOutcome::Committed(_) => {
                    adopted_count += 1;
                    adopted_packages.push(selector.to_string());
                    progress.complete_package(selector);
                }
                PackageSavepointOutcome::RolledBack(error) => {
                    warn!(
                        "Adoption of {} rolled back atomically at {}: {}",
                        selector, error.stage, error.message
                    );
                    error_count += 1;
                    failures.push(BulkAdoptionFailure::new(
                        selector,
                        error.stage,
                        error.message,
                    ));
                    progress.fail_package(selector, "package metadata persistence rolled back");
                }
            }
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    // Create state snapshot for rollback safety
    if adopted_count > 0 {
        create_state_snapshot(
            &conn,
            changeset_id,
            &format!("Adopt {} system packages", adopted_count),
        )?;
    }
    write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

    let mode_desc = if full { "full" } else { "track" };
    if error_count > 0 {
        progress.finish_with_error(&format!(
            "Adopted {} packages, {} skipped, {} errors ({})",
            adopted_count, skipped_count, error_count, mode_desc
        ));
    } else {
        progress.finish(&format!(
            "Adopted {} packages, {} skipped ({})",
            adopted_count, skipped_count, mode_desc
        ));
    }
    Ok(BulkAdoptionOutcome {
        considered_packages,
        adopted_packages,
        already_tracked_packages,
        failures,
    })
}

const LIVE_ROOT_PACKAGE_NAME: &str = "conary-live-root";

fn query_pm_files(pkg_mgr: SystemPackageManager, name: &str) -> Result<Vec<FileInfoTuple>> {
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

/// Query provides for a package from the active PM, propagating errors.
fn query_pm_provides(pkg_mgr: SystemPackageManager, name: &str) -> Result<Vec<String>> {
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
    use std::path::Path;

    #[test]
    fn package_pattern_matches_with_upstream_glob_semantics() {
        let pattern = parse_package_pattern("--pattern", Some("lib*"))
            .unwrap()
            .unwrap();
        assert!(pattern.matches("libssl"));
        assert!(pattern.matches("lib"));
        assert!(!pattern.matches("openssl"));
    }

    #[test]
    fn package_pattern_rejects_invalid_glob_instead_of_silently_matching_nothing() {
        let error = parse_package_pattern("--exclude", Some("[broken")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Invalid --exclude package pattern")
        );
    }

    #[test]
    fn bulk_package_savepoint_rolls_back_the_entire_package() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE package_rows (value TEXT NOT NULL)")
            .unwrap();

        let outcome = execute_package_savepoint(&conn, || {
            conn.execute("INSERT INTO package_rows (value) VALUES ('trove')", [])
                .unwrap();
            conn.execute("INSERT INTO package_rows (value) VALUES ('file')", [])
                .unwrap();
            Err::<(), _>("exact requirement persistence failed")
        })
        .unwrap();

        assert!(matches!(
            outcome,
            PackageSavepointOutcome::RolledBack("exact requirement persistence failed")
        ));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM package_rows", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn bulk_package_savepoint_commits_complete_package() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE package_rows (value TEXT NOT NULL)")
            .unwrap();

        let outcome = execute_package_savepoint(&conn, || {
            conn.execute("INSERT INTO package_rows (value) VALUES ('trove')", [])
                .unwrap();
            conn.execute("INSERT INTO package_rows (value) VALUES ('file')", [])
                .unwrap();
            Ok::<_, &str>(2)
        })
        .unwrap();

        assert!(matches!(outcome, PackageSavepointOutcome::Committed(2)));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM package_rows", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn live_root_adoption_excludes_runtime_and_conary_state() {
        for path in [
            "/var/lib/conary/conary.db",
            "/run/systemd/private",
            "/tmp/build",
            "/dev/null",
            "/proc/cpuinfo",
            "/sys/kernel",
            "/mnt/scratch",
            "/media/disk",
            "/conary/objects/aa/bb",
        ] {
            assert!(
                !should_visit_live_root_path(Path::new(path)).unwrap(),
                "{path} should be excluded"
            );
        }

        for path in [
            "/usr/sbin/init",
            "/usr/lib/systemd/systemd",
            "/etc/os-release",
            "/boot/EFI/BOOT/BOOTX64.EFI",
        ] {
            assert!(
                should_visit_live_root_path(Path::new(path)).unwrap(),
                "{path} should be included"
            );
        }
    }

    #[test]
    fn live_root_db_path_maps_test_root_to_absolute_generation_path() {
        let root = Path::new("/tmp/conary-live-root");
        let path = root.join("usr/sbin/init");

        assert_eq!(
            live_root_db_path(root, &path).unwrap(),
            "/usr/sbin/init".to_string()
        );
    }
}
