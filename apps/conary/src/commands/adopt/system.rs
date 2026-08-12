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
    Changeset, ChangesetStatus, ExistingDirectoryMaterialization, FileEntry, InstallReason,
    InstallSource, PayloadClaim, Trove, TroveType,
};
use conary_core::packages::{
    InstalledFileAbsencePolicy, InstalledPackageIdentity, SystemPackageManager, dpkg_query,
    pacman_query, rpm_query,
};
use conary_core::repository::dependency_model::{ProvidedCapability, RepositoryRequirementGroup};
use std::collections::HashMap;
use tracing::warn;

mod captured_root;
mod live_root;
use captured_root::{
    bind_package_payloads_to_selected_root, capture_live_selected_root,
    ensure_complete_native_partition, synchronize_captured_root,
};
use live_root::adopt_live_root_as_full_package;

fn parse_package_pattern(field: &str, pattern: Option<&str>) -> Result<Option<glob::Pattern>> {
    pattern
        .map(|value| {
            glob::Pattern::new(value).map_err(|error| {
                anyhow::anyhow!("Invalid {field} package pattern {value:?}: {error}")
            })
        })
        .transpose()
}

/// File info tuple: (path, size, mode, digest, user, group, link_target, absence policy)
pub type FileInfoTuple = (
    String,
    i64,
    i32,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    InstalledFileAbsencePolicy,
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
    provides: Vec<ProvidedCapability>,
    is_dependency: bool,
    promote_trove_id: Option<i64>,
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
    let complete_full_root_scope = full && pattern.is_none() && exclude.is_none() && !explicit_only;

    let mut conn = open_db(db_path)?;

    // Get list of already-tracked packages to avoid duplicates
    let mut tracked_packages = HashMap::new();
    for trove in Trove::list_all(&conn)? {
        let Some(identity) = trove.native_package_identity.as_ref() else {
            continue;
        };
        let selector = identity.selector().to_string();
        if tracked_packages.insert(selector.clone(), trove).is_some() {
            return Err(anyhow::anyhow!(
                "Native package identity {selector} is tracked by more than one trove"
            ));
        }
    }

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
    if complete_full_root_scope {
        ensure_complete_native_partition(
            &tracked_packages,
            installed.iter().map(|package| package.identity.selector()),
        )?;
    }

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
        let mut to_promote = 0;
        let mut already_tracked = 0;
        let mut explicit_count = 0;
        let mut dep_count = 0;

        for package in &installed {
            if let Some(tracked) = tracked_packages.get(package.identity.selector()) {
                if full && tracked.install_source == InstallSource::AdoptedTrack {
                    to_promote += 1;
                } else {
                    already_tracked += 1;
                }
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
        if full {
            println!("  Would CAS-back: {} track-only packages", to_promote);
        }
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
                .filter(|package| tracked_packages.contains_key(package.identity.selector()))
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
    let mut promoted_count = 0;
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
        let promote_trove_id = tracked_packages
            .get(selector)
            .filter(|trove| full && trove.install_source == InstallSource::AdoptedTrack)
            .map(|trove| {
                trove.id.ok_or_else(|| {
                    anyhow::anyhow!("Tracked package {selector} has no database identity")
                })
            })
            .transpose()?;
        // Full mode promotes track-only authority to exact CAS-backed
        // authority. Every other already-tracked source is already at least as
        // authoritative as this command requests.
        if tracked_packages.contains_key(selector) && promote_trove_id.is_none() {
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
        let (requirements, mut provides) = if promote_trove_id.is_some() {
            // Track adoption already persisted the native metadata. Promotion
            // changes only its payload authority from discovery hashes to
            // privately captured CAS content.
            (Vec::new(), Vec::new())
        } else {
            let requirements =
                match super::requirements::query_package_requirements(pkg_mgr, selector) {
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
            let provides = match super::provides::query_package_provides(pkg_mgr, &package.identity)
            {
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
            (requirements, provides)
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
        conary_core::repository::dependency_model::extend_materialized_file_provides(
            &mut provides,
            package.identity.source_package_format(),
            captured_files.iter().map(|file| file.source.0.as_str()),
        )?;

        let is_dependency = !user_installed.contains(&package.identity.install_reason_selector());

        pre_collected.push(PackageData {
            identity: package.identity.clone(),
            description: package.description.clone(),
            files: captured_files,
            requirements,
            provides,
            is_dependency,
            promote_trove_id,
        });
    }

    // A complete, unfiltered full adoption captures one global selected-root
    // snapshot after every package payload was captured successfully. The
    // database transaction below partitions that exact snapshot without using
    // package names or the native database as generation-time authority.
    let captured_selected_root = if complete_full_root_scope && error_count == 0 {
        Some(capture_live_selected_root(
            db_path,
            cas.as_ref()
                .expect("complete full-root capture requires an initialized CAS"),
        )?)
    } else {
        None
    };
    if let Some(captured) = captured_selected_root.as_ref() {
        bind_package_payloads_to_selected_root(
            captured,
            pre_collected
                .iter_mut()
                .flat_map(|package| package.files.iter_mut()),
        )?;
    }

    // DB-only transaction: all PM queries and CAS writes are already done.
    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
    let mut captured_root_sync = None;
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;

        for pkg in &pre_collected {
            let selector = pkg.identity.selector();
            let persisted = execute_package_savepoint(tx, || {
                if let Some(trove_id) = pkg.promote_trove_id {
                    promote_track_package(tx, changeset_id, &pkg.identity, trove_id, &pkg.files)
                        .map_err(|error| PackagePersistenceFailure {
                            stage: BulkAdoptionFailureStage::MetadataInsert,
                            message: error.to_string(),
                        })?;
                    return Ok::<bool, PackagePersistenceFailure>(true);
                }

                let mut trove = Trove::new_with_source(
                    pkg.identity.name().to_string(),
                    pkg.identity.version(),
                    TroveType::Package,
                    install_source.clone(),
                    version_scheme,
                );
                trove.architecture = Some(pkg.identity.architecture().to_string());
                trove.debian_multi_arch = pkg.identity.debian_multi_arch();
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

                super::provides::insert_package_provides(
                    tx,
                    trove_id,
                    &pkg.identity,
                    &pkg.provides,
                )
                .map_err(|error| PackagePersistenceFailure {
                    stage: BulkAdoptionFailureStage::MetadataInsert,
                    message: format!("provides: {error}"),
                })?;
                Ok::<bool, PackagePersistenceFailure>(false)
            })?;

            match persisted {
                PackageSavepointOutcome::Committed(promoted) => {
                    if promoted {
                        promoted_count += 1;
                        already_tracked_packages.push(selector.to_string());
                    } else {
                        adopted_count += 1;
                        adopted_packages.push(selector.to_string());
                    }
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

        // A package persistence failure means the native ownership partition
        // is incomplete. Do not misclassify that package's paths as unowned.
        if error_count == 0
            && let Some(captured) = captured_selected_root.as_ref()
        {
            captured_root_sync = Some(synchronize_captured_root(tx, changeset_id, captured)?);
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    // Create state snapshot for rollback safety
    if adopted_count > 0
        || promoted_count > 0
        || captured_root_sync.as_ref().is_some_and(|sync| sync.changed)
    {
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
            "Adopted {} packages, CAS-backed {}, {} skipped, {} errors ({})",
            adopted_count, promoted_count, skipped_count, error_count, mode_desc
        ));
    } else {
        progress.finish(&format!(
            "Adopted {} packages, CAS-backed {}, {} skipped ({})",
            adopted_count, promoted_count, skipped_count, mode_desc
        ));
    }
    if let Some(sync) = captured_root_sync {
        println!(
            "  Captured root: {} unowned entries, {} package-owned entries{}",
            sync.captured_entries,
            sync.package_entries,
            if sync.changed {
                ""
            } else {
                " (already current)"
            }
        );
    }
    Ok(BulkAdoptionOutcome {
        considered_packages,
        adopted_packages,
        already_tracked_packages,
        failures,
    })
}

fn promote_track_package(
    tx: &rusqlite::Connection,
    changeset_id: i64,
    identity: &InstalledPackageIdentity,
    trove_id: i64,
    files: &[CapturedAdoptionFile],
) -> conary_core::Result<()> {
    let trove = Trove::find_by_id(tx, trove_id)?.ok_or_else(|| {
        conary_core::Error::NotFound(format!(
            "track-only trove {} disappeared before full-adoption commit",
            identity.selector()
        ))
    })?;
    if trove.install_source != InstallSource::AdoptedTrack
        || trove.native_package_identity.as_ref() != Some(identity)
    {
        return Err(conary_core::Error::ConflictError(format!(
            "track-only authority for {} changed after full-adoption planning",
            identity.selector()
        )));
    }

    for captured in files {
        let path = &captured.source.0;
        let existing = FileEntry::find_by_path(tx, path)?.ok_or_else(|| {
            conary_core::Error::NotFound(format!(
                "track-only package {} lost its file authority for {path}",
                identity.selector()
            ))
        })?;
        let owns_path = existing.trove_id == trove_id
            || PayloadClaim::find_by_path(tx, path)?
                .iter()
                .any(|claim| claim.trove_id == trove_id);
        if !owns_path {
            return Err(conary_core::Error::ConflictError(format!(
                "track-only package {} no longer owns {path}",
                identity.selector()
            )));
        }
        FileEntry::replace_claimed_selected_root_materialization(
            tx,
            path,
            trove_id,
            &captured.node,
            captured.content.as_ref(),
        )?;
    }

    let updated = tx.execute(
        "UPDATE troves
         SET install_source = ?1, installed_by_changeset_id = ?2
         WHERE id = ?3 AND install_source = ?4",
        rusqlite::params![
            InstallSource::AdoptedFull.as_str(),
            changeset_id,
            trove_id,
            InstallSource::AdoptedTrack.as_str()
        ],
    )?;
    if updated != 1 {
        return Err(conary_core::Error::ConflictError(format!(
            "track-only package {} was not promoted exactly",
            identity.selector()
        )));
    }
    Ok(())
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
                f.absence_policy,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};

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
    fn full_adoption_promotes_track_authority_without_changing_path_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let identity = InstalledPackageIdentity::rpm(
            "fixture-1-1.x86_64",
            "fixture",
            None,
            "1",
            "1",
            "x86_64",
        )
        .unwrap();
        let mut original = Trove::new_with_source(
            "fixture".to_string(),
            "1-1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        original.architecture = Some("x86_64".to_string());
        original.native_package_identity = Some(identity.clone());
        let trove_id = original.insert(&conn).unwrap();
        let old_content = PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b"old"),
            size: 3,
        };
        let old_node =
            ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap();
        FileEntry::new(
            "/usr/bin/fixture".to_string(),
            old_node,
            Some(old_content),
            trove_id,
        )
        .insert(&conn)
        .unwrap();

        let new_content = PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b"new exact bytes"),
            size: 15,
        };
        let new_node =
            ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755)).unwrap();
        let captured = CapturedAdoptionFile {
            source: (
                "/usr/bin/fixture".to_string(),
                15,
                i32::try_from(libc::S_IFREG | 0o755).unwrap(),
                None,
                None,
                None,
                None,
                InstalledFileAbsencePolicy::Required,
            ),
            node: new_node.clone(),
            content: Some(new_content.clone()),
        };

        let tx = conn.transaction().unwrap();
        let mut changeset = Changeset::new("promote fixture".to_string());
        let changeset_id = changeset.insert(&tx).unwrap();
        promote_track_package(&tx, changeset_id, &identity, trove_id, &[captured]).unwrap();
        changeset
            .update_status(&tx, ChangesetStatus::Applied)
            .unwrap();
        tx.commit().unwrap();

        let promoted = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
        assert_eq!(promoted.install_source, InstallSource::AdoptedFull);
        assert_eq!(promoted.installed_by_changeset_id, Some(changeset_id));
        let file = FileEntry::find_by_path(&conn, "/usr/bin/fixture")
            .unwrap()
            .unwrap();
        assert_eq!(file.trove_id, trove_id);
        assert_eq!(file.node, new_node);
        assert_eq!(file.content, Some(new_content));
    }
}
