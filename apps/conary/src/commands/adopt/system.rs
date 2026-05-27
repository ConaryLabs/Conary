// src/commands/adopt/system.rs

//! Bulk system package adoption
//!
//! Adopts all installed system packages into Conary tracking.

use super::super::create_state_snapshot;
use super::super::open_db;
use super::super::progress::{AdoptPhase, AdoptProgress};
use super::cas_capture::prepare_cas_backed_package_files;
use super::checkpoint::write_db_checkpoint;
use super::outcome::{metadata_insert_succeeded, write_warning_metadata};
use crate::commands::AdoptionWarning;
use anyhow::Result;
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallReason, InstallSource,
    ProvideEntry, Trove, TroveType,
};
use conary_core::packages::{
    DependencyInfo, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use tracing::{debug, warn};
use walkdir::WalkDir;

/// Match a package name against a glob pattern using the `glob` crate.
/// Returns false on invalid patterns (treated as no match).
fn glob_match(pattern: &str, name: &str) -> bool {
    glob::Pattern::new(pattern)
        .map(|p| p.matches(name))
        .unwrap_or(false)
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
) -> Result<()> {
    // Detect system package manager
    let pkg_mgr = SystemPackageManager::detect();
    if !pkg_mgr.is_available() {
        if full {
            return adopt_live_root_as_full_package(
                db_path,
                dry_run,
                pattern,
                exclude,
                explicit_only,
            );
        }
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
        ));
    }

    println!("Detected package manager: {:?}", pkg_mgr);
    let source_identity = pkg_mgr.detect_source_identity();

    let mut conn = open_db(db_path)?;

    // Get list of already-tracked packages to avoid duplicates
    let tracked_packages: HashSet<String> = Trove::list_all(&conn)?
        .into_iter()
        .map(|t| t.name)
        .collect();

    // Get all installed packages based on package manager
    let installed: Vec<(String, String, String, Option<String>)> = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_all_packages()?
            .into_iter()
            .map(|(name, info)| {
                (
                    name,
                    // Use full_version (epoch:version-release) for RPM so that
                    // drift detection in refresh.rs compares apples to apples.
                    info.full_version(),
                    info.arch.clone(),
                    info.description.clone().or(info.summary.clone()),
                )
            })
            .collect(),
        SystemPackageManager::Dpkg => dpkg_query::query_all_packages()?
            .into_iter()
            .map(|(name, info)| {
                (
                    name,
                    info.version_only(),
                    info.arch.clone(),
                    info.description.clone(),
                )
            })
            .collect(),
        SystemPackageManager::Pacman => pacman_query::query_all_packages()?
            .into_iter()
            .map(|(name, info)| {
                (
                    name,
                    info.version_only(),
                    info.arch.clone(),
                    info.description.clone(),
                )
            })
            .collect(),
        _ => return Err(anyhow::anyhow!("Unsupported package manager")),
    };

    // Query which packages were explicitly installed by the user vs auto-installed as deps.
    // Failures are non-fatal: we fall back to marking everything as Explicit.
    let user_installed: std::collections::HashSet<String> = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_user_installed().unwrap_or_else(|e| {
            warn!(
                "Could not determine RPM install reasons ({}); marking all as explicit",
                e
            );
            std::collections::HashSet::new()
        }),
        SystemPackageManager::Dpkg => dpkg_query::query_user_installed().unwrap_or_else(|e| {
            warn!(
                "Could not determine dpkg install reasons ({}); marking all as explicit",
                e
            );
            std::collections::HashSet::new()
        }),
        SystemPackageManager::Pacman => pacman_query::query_user_installed().unwrap_or_else(|e| {
            warn!(
                "Could not determine pacman install reasons ({}); marking all as explicit",
                e
            );
            std::collections::HashSet::new()
        }),
        _ => std::collections::HashSet::new(),
    };
    // If the set is empty (query failed / unsupported), treat all as explicit.
    let has_install_reason_data = !user_installed.is_empty();

    // Apply selective filters
    let pre_filter_count = installed.len();
    let installed: Vec<_> = installed
        .into_iter()
        .filter(|(name, _version, _arch, _desc)| {
            if let Some(pat) = pattern
                && !glob_match(pat, name)
            {
                return false;
            }
            if let Some(exc) = exclude
                && glob_match(exc, name)
            {
                return false;
            }
            if explicit_only && has_install_reason_data && !user_installed.contains(name) {
                return false;
            }
            true
        })
        .collect();
    let total = installed.len();

    if total < pre_filter_count {
        println!("Filtered: {} -> {} packages", pre_filter_count, total);
    }

    if dry_run {
        let mut to_adopt = 0;
        let mut already_tracked = 0;
        let mut explicit_count = 0;
        let mut dep_count = 0;

        for (name, _version, _arch, _desc) in &installed {
            if tracked_packages.contains(name) {
                already_tracked += 1;
            } else {
                to_adopt += 1;
                if has_install_reason_data && !user_installed.contains(name) {
                    dep_count += 1;
                } else {
                    explicit_count += 1;
                }
            }
        }

        println!("Dry run: would adopt {} packages\n", to_adopt);
        println!("Summary:");
        println!("  Would adopt: {} packages", to_adopt);
        if has_install_reason_data {
            println!("    Explicit: {}", explicit_count);
            println!("    Dependency: {}", dep_count);
        }
        println!("  Already tracked: {} packages", already_tracked);
        println!(
            "  Mode: {}",
            if full {
                "full (CAS storage)"
            } else {
                "track (metadata only)"
            }
        );
        return Ok(());
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
    let mut degraded_count = 0;
    let mut error_count = 0;

    let mode_label = if full { "Adopting (full)" } else { "Adopting" };
    let mut progress = AdoptProgress::new(total as u64, mode_label);

    // Pre-fetch all PM metadata and perform CAS writes OUTSIDE the transaction.
    // This keeps the SQLite write lock short (DB inserts only) and avoids
    // CAS-vs-DB inconsistency: if the DB transaction later rolls back, any CAS
    // objects that were already written become unreachable orphans that the GC
    // will clean up -- the same trade-off the install pipeline makes.
    struct PackageData {
        name: String,
        version: String,
        arch: String,
        description: Option<String>,
        files: Vec<(FileInfoTuple, String)>, // (file tuple, pre-computed hash)
        deps: Vec<DependencyInfo>,
        provides: Vec<String>,
        is_dependency: bool,
    }

    let mut pre_collected: Vec<PackageData> = Vec::new();

    for (name, version, arch, description) in &installed {
        // Skip already-tracked packages
        if tracked_packages.contains(name) {
            skipped_count += 1;
            progress.skip_package();
            continue;
        }

        progress.set_phase(name, AdoptPhase::Querying);

        // Query ALL PM metadata before opening the DB transaction.
        let files: Vec<FileInfoTuple> = match query_pm_files(pkg_mgr, name) {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to query files for '{}': {}; skipping", name, e);
                progress.fail_package(name, &e.to_string());
                error_count += 1;
                continue;
            }
        };
        let deps: Vec<DependencyInfo> = match query_pm_deps(pkg_mgr, name) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to query deps for '{}': {}; skipping", name, e);
                progress.fail_package(name, &e.to_string());
                error_count += 1;
                continue;
            }
        };
        let provides: Vec<String> = match query_pm_provides(pkg_mgr, name) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to query provides for '{}': {}; skipping", name, e);
                progress.fail_package(name, &e.to_string());
                error_count += 1;
                continue;
            }
        };

        // Perform CAS writes OUTSIDE the transaction.
        let files_with_hashes: Vec<(FileInfoTuple, String)> = if full {
            progress.set_phase(name, AdoptPhase::CasStorage);
            match prepare_cas_backed_package_files(
                name,
                &files,
                cas.as_ref()
                    .expect("CAS store must be available for full adoption"),
            ) {
                Ok(files) => files,
                Err(e) => {
                    warn!("Failed to prepare CAS-backed files for '{}': {}", name, e);
                    progress.fail_package(name, &e.to_string());
                    error_count += 1;
                    continue;
                }
            }
        } else {
            files
                .into_iter()
                .map(|f| {
                    let hash =
                        compute_file_hash(&f.0, f.2, f.3.as_deref(), f.6.as_deref(), false, None);
                    (f, hash)
                })
                .collect()
        };

        let is_dependency = has_install_reason_data && !user_installed.contains(name);

        pre_collected.push(PackageData {
            name: name.clone(),
            version: version.clone(),
            arch: arch.clone(),
            description: description.clone(),
            files: files_with_hashes,
            deps,
            provides,
            is_dependency,
        });
    }

    // DB-only transaction: all PM queries and CAS writes are already done.
    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;
        let mut adoption_warnings = Vec::new();

        for pkg in &pre_collected {
            let mut trove = Trove::new_with_source(
                pkg.name.clone(),
                pkg.version.clone(),
                TroveType::Package,
                install_source.clone(),
            );
            trove.architecture = Some(pkg.arch.clone());
            trove.description = pkg.description.clone();
            trove.installed_by_changeset_id = Some(changeset_id);
            trove.source_distro = source_identity.source_distro.clone();
            trove.version_scheme = source_identity.version_scheme.clone();
            if pkg.is_dependency {
                trove.install_reason = InstallReason::Dependency;
                trove.selection_reason =
                    Some("Auto-installed dependency (from system package manager)".to_string());
            } else {
                trove.selection_reason = Some("Adopted from system".to_string());
            }

            let trove_id = match trove.insert(tx) {
                Ok(id) => id,
                Err(e) => {
                    warn!("Failed to insert trove for {}: {}", pkg.name, e);
                    error_count += 1;
                    continue;
                }
            };

            // Track insert successes/failures for files, deps, and provides.
            // If every insert for this package fails, the trove record is
            // effectively empty — skip it so we don't pollute the DB with
            // ghost entries.
            let total_inserts = pkg.files.len() + pkg.deps.len() + pkg.provides.len();
            let mut insert_failures: usize = 0;

            for (
                (file_path, file_size, file_mode, _digest, file_user, file_group, link_target),
                hash,
            ) in &pkg.files
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
                file_entry.symlink_target = link_target.clone();

                if let Err(e) = file_entry.insert_or_replace(tx) {
                    debug!("Failed to insert file {}: {}", file_path, e);
                    insert_failures += 1;
                }
            }

            for dep in &pkg.deps {
                if dep.name.is_empty() {
                    continue;
                }

                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep.name.clone(),
                    None,
                    "runtime".to_string(),
                    dep.constraint.clone(),
                );
                if let Err(e) = dep_entry.insert(tx) {
                    debug!("Failed to insert dependency: {}", e);
                    insert_failures += 1;
                }
            }

            for provide in &pkg.provides {
                if provide.is_empty() {
                    continue;
                }
                let mut provide_entry = ProvideEntry::new(trove_id, provide.clone(), None);
                if let Err(e) = provide_entry.insert_or_ignore(tx) {
                    debug!("Failed to insert provide: {}", e);
                    insert_failures += 1;
                }
            }

            let has_partial_failure = total_inserts > 0 && insert_failures > 0;
            if !finalize_bulk_metadata_insert_outcome(
                tx,
                trove_id,
                &pkg.name,
                total_inserts,
                insert_failures,
                &mut adoption_warnings,
            )? {
                error_count += 1;
                continue;
            }

            if has_partial_failure {
                degraded_count += 1;
            }
            adopted_count += 1;
            progress.complete_package(&pkg.name);
        }

        write_warning_metadata(tx, changeset_id, adoption_warnings)
            .map_err(|e| conary_core::Error::Io(std::io::Error::other(e.to_string())))?;
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
    if degraded_count > 0 {
        println!(
            "Adopted with warnings: {degraded_count} package(s). Run `conary system history` to inspect adoption warning metadata."
        );
    }

    Ok(())
}

fn finalize_bulk_metadata_insert_outcome(
    tx: &rusqlite::Connection,
    trove_id: i64,
    package_name: &str,
    total_inserts: usize,
    insert_failures: usize,
    adoption_warnings: &mut Vec<AdoptionWarning>,
) -> conary_core::Result<bool> {
    if metadata_insert_succeeded(total_inserts, insert_failures) {
        if total_inserts > 0 && insert_failures > 0 {
            adoption_warnings.push(AdoptionWarning::partial_insert_failure(
                package_name.to_string(),
                total_inserts,
                insert_failures,
            ));
        }
        return Ok(true);
    }

    warn!(
        "All {} insert(s) failed for '{}'; removing empty adopted trove",
        total_inserts, package_name
    );
    Trove::delete(tx, trove_id)?;
    adoption_warnings.push(AdoptionWarning::all_insert_failure(
        package_name.to_string(),
        total_inserts,
    ));
    Ok(false)
}

const LIVE_ROOT_PACKAGE_NAME: &str = "conary-live-root";

const LIVE_ROOT_CRITICAL_PACKAGE_MARKERS: &[(&str, &[&str])] = &[
    ("bash", &["/usr/bin/bash", "/bin/bash"]),
    (
        "coreutils",
        &[
            "/usr/bin/coreutils",
            "/usr/bin/ls",
            "/bin/ls",
            "/usr/bin/cp",
            "/bin/cp",
            "/usr/bin/true",
            "/bin/true",
        ],
    ),
    (
        "filesystem",
        &["/usr", "/etc", "/var", "/usr/lib", "/usr/bin"],
    ),
    ("setup", &["/etc/passwd", "/etc/group"]),
    (
        "systemd",
        &[
            "/usr/lib/systemd/systemd",
            "/lib/systemd/systemd",
            "/usr/sbin/init",
            "/sbin/init",
        ],
    ),
    (
        "systemd-libs",
        &[
            "/usr/lib64/libsystemd.so.0",
            "/lib64/libsystemd.so.0",
            "/usr/lib/x86_64-linux-gnu/libsystemd.so.0",
            "/lib/x86_64-linux-gnu/libsystemd.so.0",
        ],
    ),
    (
        "systemd-udev",
        &[
            "/usr/bin/udevadm",
            "/bin/udevadm",
            "/usr/sbin/udevadm",
            "/sbin/udevadm",
            "/usr/lib/systemd/systemd-udevd",
            "/lib/systemd/systemd-udevd",
        ],
    ),
    (
        "udev",
        &[
            "/usr/bin/udevadm",
            "/bin/udevadm",
            "/usr/sbin/udevadm",
            "/sbin/udevadm",
            "/usr/lib/systemd/systemd-udevd",
            "/lib/systemd/systemd-udevd",
        ],
    ),
    (
        "util-linux",
        &[
            "/usr/bin/mount",
            "/bin/mount",
            "/usr/bin/umount",
            "/bin/umount",
        ],
    ),
    (
        "util-linux-core",
        &[
            "/usr/bin/mount",
            "/bin/mount",
            "/usr/bin/umount",
            "/bin/umount",
        ],
    ),
    (
        "glibc",
        &[
            "/usr/lib64/libc.so.6",
            "/lib64/libc.so.6",
            "/usr/lib/libc.so.6",
            "/lib/libc.so.6",
            "/usr/lib/x86_64-linux-gnu/libc.so.6",
            "/lib/x86_64-linux-gnu/libc.so.6",
            "/usr/lib/aarch64-linux-gnu/libc.so.6",
            "/lib/aarch64-linux-gnu/libc.so.6",
            "/usr/lib/riscv64-linux-gnu/libc.so.6",
            "/lib/riscv64-linux-gnu/libc.so.6",
        ],
    ),
    (
        "libc6",
        &[
            "/usr/lib64/libc.so.6",
            "/lib64/libc.so.6",
            "/usr/lib/libc.so.6",
            "/lib/libc.so.6",
            "/usr/lib/x86_64-linux-gnu/libc.so.6",
            "/lib/x86_64-linux-gnu/libc.so.6",
            "/usr/lib/aarch64-linux-gnu/libc.so.6",
            "/lib/aarch64-linux-gnu/libc.so.6",
            "/usr/lib/riscv64-linux-gnu/libc.so.6",
            "/lib/riscv64-linux-gnu/libc.so.6",
        ],
    ),
    (
        "gcc-libs",
        &[
            "/usr/lib64/libgcc_s.so.1",
            "/lib64/libgcc_s.so.1",
            "/usr/lib/x86_64-linux-gnu/libgcc_s.so.1",
            "/lib/x86_64-linux-gnu/libgcc_s.so.1",
        ],
    ),
    (
        "openssl-libs",
        &[
            "/usr/lib64/libssl.so.3",
            "/lib64/libssl.so.3",
            "/usr/lib64/libcrypto.so.3",
            "/lib64/libcrypto.so.3",
            "/usr/lib/x86_64-linux-gnu/libssl.so.3",
            "/lib/x86_64-linux-gnu/libssl.so.3",
            "/usr/lib/x86_64-linux-gnu/libcrypto.so.3",
            "/lib/x86_64-linux-gnu/libcrypto.so.3",
        ],
    ),
    ("openssl", &["/usr/bin/openssl", "/bin/openssl"]),
    (
        "pam",
        &[
            "/usr/lib64/security/pam_unix.so",
            "/lib64/security/pam_unix.so",
            "/usr/lib/x86_64-linux-gnu/security/pam_unix.so",
            "/lib/x86_64-linux-gnu/security/pam_unix.so",
        ],
    ),
    (
        "linux-pam",
        &[
            "/usr/lib64/security/pam_unix.so",
            "/lib64/security/pam_unix.so",
            "/usr/lib/x86_64-linux-gnu/security/pam_unix.so",
            "/lib/x86_64-linux-gnu/security/pam_unix.so",
        ],
    ),
    (
        "shadow-utils",
        &["/usr/sbin/useradd", "/sbin/useradd", "/usr/bin/passwd"],
    ),
    ("sudo", &["/usr/bin/sudo", "/bin/sudo"]),
    (
        "polkit",
        &[
            "/usr/lib/polkit-1/polkitd",
            "/usr/libexec/polkitd",
            "/usr/lib/policykit-1/polkitd",
        ],
    ),
    (
        "polkit-libs",
        &[
            "/usr/lib64/libpolkit-gobject-1.so.0",
            "/lib64/libpolkit-gobject-1.so.0",
            "/usr/lib/x86_64-linux-gnu/libpolkit-gobject-1.so.0",
            "/lib/x86_64-linux-gnu/libpolkit-gobject-1.so.0",
        ],
    ),
    (
        "nss-softokn",
        &[
            "/usr/lib64/libsoftokn3.so",
            "/lib64/libsoftokn3.so",
            "/usr/lib/x86_64-linux-gnu/nss/libsoftokn3.so",
        ],
    ),
    (
        "nspr",
        &[
            "/usr/lib64/libnspr4.so",
            "/lib64/libnspr4.so",
            "/usr/lib/x86_64-linux-gnu/libnspr4.so",
            "/lib/x86_64-linux-gnu/libnspr4.so",
        ],
    ),
    (
        "ca-certificates",
        &[
            "/etc/pki/tls/certs/ca-bundle.crt",
            "/etc/ssl/certs/ca-certificates.crt",
        ],
    ),
];

fn live_root_critical_identity_provides(files: &[FileInfoTuple]) -> Vec<&'static str> {
    let file_paths: HashSet<&str> = files.iter().map(|(path, ..)| path.as_str()).collect();
    let mut provides = BTreeSet::new();

    for (name, marker_paths) in LIVE_ROOT_CRITICAL_PACKAGE_MARKERS {
        if marker_paths.iter().any(|path| file_paths.contains(path)) {
            provides.insert(*name);
        }
    }

    provides.into_iter().collect()
}

fn adopt_live_root_as_full_package(
    db_path: &str,
    dry_run: bool,
    pattern: Option<&str>,
    exclude: Option<&str>,
    explicit_only: bool,
) -> Result<()> {
    if pattern.is_some() || exclude.is_some() || explicit_only {
        return Err(anyhow::anyhow!(
            "Full live-root adoption without a system package manager does not support package filters"
        ));
    }

    println!(
        "No supported package manager found; adopting the live root as one CAS-backed package"
    );

    let mut conn = open_db(db_path)?;
    let tracked_packages: std::collections::HashSet<String> = Trove::list_all(&conn)?
        .into_iter()
        .map(|t| t.name)
        .collect();
    if tracked_packages.contains(LIVE_ROOT_PACKAGE_NAME) {
        println!("Live root is already adopted as {LIVE_ROOT_PACKAGE_NAME}; nothing to do");
        return Ok(());
    }

    let files = collect_live_root_file_info(Path::new("/"))?;
    if dry_run {
        println!("Dry run: would adopt 1 synthetic package");
        println!("  Package: {LIVE_ROOT_PACKAGE_NAME}");
        println!("  Files: {}", files.len());
        println!("  Mode: full (CAS storage)");
        return Ok(());
    }

    let objects_dir = conary_core::db::paths::objects_dir(db_path);
    let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;
    let files_with_hashes = prepare_cas_backed_package_files(LIVE_ROOT_PACKAGE_NAME, &files, &cas)?;
    let live_root_identity_provides = live_root_critical_identity_provides(&files);

    let mut changeset = Changeset::new(format!(
        "Adopt live root as CAS-backed package ({LIVE_ROOT_PACKAGE_NAME})"
    ));
    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;
        let mut trove = Trove::new_with_source(
            LIVE_ROOT_PACKAGE_NAME.to_string(),
            live_root_adoption_version(),
            TroveType::Package,
            InstallSource::AdoptedFull,
        );
        trove.architecture = Some(std::env::consts::ARCH.to_string());
        trove.description = Some(
            "Synthetic CAS-backed package imported from a system without a native package manager"
                .to_string(),
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.selection_reason = Some(
            "Adopted live root because no supported package manager was available".to_string(),
        );
        let trove_id = trove.insert(tx)?;

        for (
            (file_path, file_size, file_mode, _digest, file_user, file_group, link_target),
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
            file_entry.symlink_target = link_target.clone();
            file_entry.insert_or_replace(tx)?;
        }

        let mut provide = ProvideEntry::new(
            trove_id,
            LIVE_ROOT_PACKAGE_NAME.to_string(),
            Some(live_root_adoption_version()),
        );
        provide.insert_or_ignore(tx)?;

        for provide_name in &live_root_identity_provides {
            let mut provide = ProvideEntry::new(
                trove_id,
                (*provide_name).to_string(),
                Some(live_root_adoption_version()),
            );
            provide.insert_or_ignore(tx)?;
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    create_state_snapshot(
        &conn,
        changeset_id,
        &format!("Adopt live root as {LIVE_ROOT_PACKAGE_NAME}"),
    )?;
    write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

    println!(
        "Adopted live root as {LIVE_ROOT_PACKAGE_NAME}: {} files (full)",
        files_with_hashes.len()
    );

    Ok(())
}

fn live_root_adoption_version() -> String {
    "snapshot".to_string()
}

fn collect_live_root_file_info(root: &Path) -> Result<Vec<FileInfoTuple>> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root).follow_links(false).into_iter();

    for entry in walker.filter_entry(|entry| should_visit_live_root_path(entry.path())) {
        let entry = entry?;
        let path = entry.path();
        if path == root || !should_visit_live_root_path(path) {
            continue;
        }

        let metadata = std::fs::symlink_metadata(path)?;
        let link_target = if metadata.file_type().is_symlink() {
            Some(std::fs::read_link(path)?.to_string_lossy().to_string())
        } else {
            None
        };
        let size = if let Some(target) = &link_target {
            i64::try_from(target.len())?
        } else {
            i64::try_from(metadata.len())?
        };

        files.push((
            live_root_db_path(root, path)?,
            size,
            live_root_file_mode(&metadata),
            None,
            Some("root".to_string()),
            Some("root".to_string()),
            link_target,
        ));
    }

    Ok(files)
}

#[cfg(unix)]
fn live_root_file_mode(metadata: &std::fs::Metadata) -> i32 {
    use std::os::unix::fs::PermissionsExt;

    #[allow(clippy::cast_possible_wrap)]
    {
        metadata.permissions().mode() as i32
    }
}

#[cfg(not(unix))]
fn live_root_file_mode(_metadata: &std::fs::Metadata) -> i32 {
    0
}

fn live_root_db_path(root: &Path, path: &Path) -> Result<String> {
    if root == Path::new("/") {
        return Ok(path.to_string_lossy().to_string());
    }
    let rel = path.strip_prefix(root)?;
    Ok(format!("/{}", rel.to_string_lossy()))
}

fn should_visit_live_root_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    !conary_core::generation::metadata::is_excluded(&path)
        && path != "/conary"
        && !path.starts_with("/conary/")
}

/// Compute the hash for a file, handling symlinks, directories, and regular files
pub fn compute_file_hash(
    file_path: &str,
    file_mode: i32,
    file_digest: Option<&str>,
    link_target: Option<&str>,
    full: bool,
    cas: Option<&conary_core::filesystem::CasStore>,
) -> String {
    // Check if this is a symlink (mode & S_IFMT == S_IFLNK)
    let is_symlink = (file_mode & 0o170000) == 0o120000;
    let is_directory = (file_mode & 0o170000) == 0o040000;

    if full && let Some(cas_store) = cas {
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
            // Regular live file - copy into a private CAS inode.
            let path = std::path::Path::new(file_path);
            if path.is_file() {
                match cas_store.store_file_copy_from_existing(file_path) {
                    Ok(h) => return h,
                    Err(e) => {
                        debug!("Failed to copy {} into CAS: {}", file_path, e);
                    }
                }
            } else {
                debug!("Skipping non-regular file: {}", file_path);
            }
        }
    }

    // Fallback: use digest from the package manager if available,
    // otherwise compute SHA-256 from the actual file on disk
    if let Some(digest) = file_digest {
        return digest.to_string();
    }
    // Try to compute actual hash from the file on disk
    let path = std::path::Path::new(file_path);
    if path.is_file() {
        match std::fs::read(path) {
            Ok(contents) => return conary_core::hash::sha256(&contents),
            Err(e) => {
                debug!(
                    "Cannot read {} for hashing: {}; using placeholder",
                    file_path, e
                );
            }
        }
    }
    // Last resort: placeholder for files we cannot read (e.g., permission denied)
    format!("adopted-{}", file_path.replace('/', "_"))
}

/// Query files for a package from the active PM, propagating errors.
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

/// Query dependencies for a package from the active PM, propagating errors.
fn query_pm_deps(pkg_mgr: SystemPackageManager, name: &str) -> Result<Vec<DependencyInfo>> {
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

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("lib*", "libssl"));
        assert!(glob_match("lib*", "lib"));
        assert!(!glob_match("lib*", "openssl"));
    }

    #[test]
    fn test_glob_match_question() {
        assert!(glob_match("lib?", "liba"));
        assert!(!glob_match("lib?", "lib"));
        assert!(!glob_match("lib?", "libab"));
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("nginx", "nginx"));
        assert!(!glob_match("nginx", "nginx-core"));
    }

    #[test]
    fn test_glob_match_middle_star() {
        assert!(glob_match("*ssl*", "libssl3"));
        assert!(glob_match("*ssl*", "openssl"));
        assert!(!glob_match("*ssl*", "libcurl"));
    }

    #[test]
    fn test_glob_match_complex() {
        assert!(glob_match("kernel*", "kernel-core"));
        assert!(glob_match("kernel*", "kernel-modules"));
        assert!(!glob_match("kernel*", "linux-kernel"));
    }

    #[test]
    fn all_failed_bulk_outcome_helper_deletes_seeded_trove() {
        use conary_core::db;
        use conary_core::db::models::{Trove, TroveType};

        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db").to_string_lossy().into_owned();
        db::init(&db_path).unwrap();
        let mut conn = db::open(&db_path).unwrap();

        db::transaction(&mut conn, |tx| {
            let mut trove = Trove::new("ghost".to_string(), "1.0".to_string(), TroveType::Package);
            let trove_id = trove.insert(tx)?;

            let mut warnings = Vec::new();
            let keep_trove =
                finalize_bulk_metadata_insert_outcome(tx, trove_id, "ghost", 3, 3, &mut warnings)?;
            assert!(!keep_trove);
            assert_eq!(warnings.len(), 1);

            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM troves WHERE id = ?1",
                [trove_id],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
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
                !should_visit_live_root_path(Path::new(path)),
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
                should_visit_live_root_path(Path::new(path)),
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

    fn live_root_test_file(path: &str) -> FileInfoTuple {
        (
            path.to_string(),
            0,
            0o100644,
            None,
            Some("root".to_string()),
            Some("root".to_string()),
            None,
        )
    }

    #[test]
    fn live_root_adoption_derives_critical_package_identity_provides() {
        let files = vec![
            live_root_test_file("/usr/lib/systemd/systemd"),
            live_root_test_file("/usr/bin/ls"),
            live_root_test_file("/etc/passwd"),
            live_root_test_file("/usr/bin/sudo"),
        ];

        let provides = live_root_critical_identity_provides(&files);

        assert!(provides.contains(&"systemd"));
        assert!(provides.contains(&"coreutils"));
        assert!(provides.contains(&"setup"));
        assert!(provides.contains(&"sudo"));
        assert!(!provides.contains(&"openssl"));
    }

    #[test]
    fn live_root_adoption_derives_glibc_identity_from_usr_lib_libc() {
        let files = vec![live_root_test_file("/usr/lib/libc.so.6")];

        let provides = live_root_critical_identity_provides(&files);

        assert!(provides.contains(&"glibc"));
        assert!(provides.contains(&"libc6"));
    }

    #[test]
    fn live_root_adoption_identity_provides_are_deduplicated_and_ordered() {
        let files = vec![
            live_root_test_file("/usr/bin/ls"),
            live_root_test_file("/usr/bin/cp"),
            live_root_test_file("/bin/true"),
        ];

        let provides = live_root_critical_identity_provides(&files);

        assert_eq!(provides, vec!["coreutils"]);
    }
}
