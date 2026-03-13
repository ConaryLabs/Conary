// conary-core/src/repository/dependencies.rs

//! Dependency resolution
//!
//! Functions for resolving package dependencies across repositories,
//! including transitive resolution and parallel downloads.

use crate::db::models::{RepositoryProvide, Trove, generate_capability_variations};
use crate::error::{Error, Result};
use crate::version::VersionConstraint;
use rayon::prelude::*;
use rusqlite::Connection;
use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::download::{DownloadOptions, DownloadProgress, download_package_verified_with_progress};
use super::selector::{PackageSelector, PackageWithRepo, SelectionOptions};

/// Resolve a dependency by querying normalized `repository_provides` data.
///
/// This is the preferred path: it queries the indexed `repository_provides`
/// table directly instead of scanning JSON metadata blobs.  The join fetches
/// only the package names that actually declare the capability, avoiding the
/// expensive `list_all()` call.
fn resolve_repo_dependency_by_capability(
    conn: &Connection,
    dep_name: &str,
    options: &SelectionOptions,
) -> Result<Option<(String, Option<String>)>> {
    let provides = RepositoryProvide::find_by_capability(conn, dep_name)?;
    if provides.is_empty() {
        return Ok(None);
    }

    // Collect distinct package IDs from the provides, then look up each one
    // by name through the selector (which handles arch filtering, version
    // pinning, etc.).
    let mut seen_ids = HashSet::new();
    let mut candidates = Vec::new();

    for provide in &provides {
        if !seen_ids.insert(provide.repository_package_id) {
            continue;
        }

        // Direct ID-based lookup: fetch name from the provide's package row.
        let pkg_name: Option<String> = conn
            .prepare_cached("SELECT name FROM repository_packages WHERE id = ?1")?
            .query_row([provide.repository_package_id], |row| row.get(0))
            .ok();

        let Some(name) = pkg_name else { continue };

        for candidate in PackageSelector::search_packages(conn, &name, options)? {
            if candidate.package.id == Some(provide.repository_package_id) {
                candidates.push(candidate);
                break;
            }
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = PackageSelector::select_best(candidates)?;
    Ok(Some((
        selected.package.name.clone(),
        Some(selected.package.version.clone()),
    )))
}

/// Fallback: scan JSON metadata blobs for capability information.
///
/// This handles packages that were sync'd before normalized provide data was
/// available.  It is only reached when the `repository_provides` table has no
/// matching rows for the requested capability.
fn resolve_repo_dependency_by_metadata(
    conn: &Connection,
    dep_name: &str,
    options: &SelectionOptions,
) -> Result<Option<(String, Option<String>)>> {
    let pattern = format!("%{dep_name}%");
    let mut stmt = conn.prepare(
        "SELECT DISTINCT name FROM repository_packages
         WHERE metadata LIKE ?1
         ORDER BY LENGTH(name), name",
    )?;

    let rows = stmt.query_map([pattern], |row| row.get::<_, String>(0))?;
    let mut candidates = Vec::new();

    for row in rows {
        let name = row?;
        for candidate in PackageSelector::search_packages(conn, &name, options)? {
            let Some(metadata_json) = candidate.package.metadata.as_ref() else {
                continue;
            };
            let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
                continue;
            };
            let Some(provides) = metadata
                .get("rpm_provides")
                .and_then(|value| value.as_array())
            else {
                continue;
            };
            if provides.iter().any(|value| {
                value.as_str().is_some_and(|provide| {
                    provide == dep_name
                        || provide.starts_with(&format!("{dep_name} "))
                        || provide.starts_with(&format!("{dep_name}("))
                })
            }) {
                candidates.push(candidate);
            }
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = PackageSelector::select_best(candidates)?;
    Ok(Some((
        selected.package.name.clone(),
        Some(selected.package.version.clone()),
    )))
}

fn resolve_repo_dependency_request(
    conn: &Connection,
    dep_name: &str,
    constraint: &VersionConstraint,
    options: &SelectionOptions,
) -> Result<(String, VersionConstraint)> {
    // 1. Exact package name match -- cheapest check.
    if PackageSelector::find_best_package(conn, dep_name, options).is_ok() {
        return Ok((dep_name.to_string(), constraint.clone()));
    }

    // 2. Normalized capability lookup -- preferred resolution path.
    //    Queries the indexed `repository_provides` table before falling back
    //    to cross-distro heuristics or JSON blob scans.
    if let Some((package_name, package_version)) =
        resolve_repo_dependency_by_capability(conn, dep_name, options)?
    {
        let resolved_constraint = if let Some(version) = package_version {
            VersionConstraint::parse(&format!("= {version}")).unwrap_or(VersionConstraint::Any)
        } else {
            constraint.clone()
        };
        return Ok((package_name, resolved_constraint));
    }

    // 3. Cross-distro heuristics (repology / name-variation helpers).
    //    Only runs after exact native-format lookup fails.
    for variation in generate_capability_variations(dep_name) {
        if PackageSelector::find_best_package(conn, &variation, options).is_ok() {
            return Ok((variation, constraint.clone()));
        }
    }

    // 4. Legacy JSON metadata blob scan -- backward compat for packages that
    //    haven't been re-sync'd with normalized provide data yet.
    if let Some((package_name, package_version)) =
        resolve_repo_dependency_by_metadata(conn, dep_name, options)?
    {
        let resolved_constraint = if let Some(version) = package_version {
            VersionConstraint::parse(&format!("= {version}")).unwrap_or(VersionConstraint::Any)
        } else {
            constraint.clone()
        };
        return Ok((package_name, resolved_constraint));
    }

    // 5. Soname-based fuzzy search -- last resort for shared library deps.
    if let Some(candidate) = resolve_repo_dependency_by_search(conn, dep_name, options)? {
        return Ok((candidate, constraint.clone()));
    }

    Err(Error::NotFound(format!(
        "Required dependency '{dep_name}' not found in any repository"
    )))
}

fn resolve_repo_dependency_by_search(
    conn: &Connection,
    dep_name: &str,
    options: &SelectionOptions,
) -> Result<Option<String>> {
    if !dep_name.ends_with(".so") && !dep_name.contains(".so.") {
        return Ok(None);
    }

    let mut search_terms = HashSet::new();
    for variation in generate_capability_variations(dep_name) {
        if variation.len() >= 3 {
            search_terms.insert(variation);
        }
    }

    let mut candidates = Vec::new();
    for term in search_terms {
        let pattern = format!("%{term}%");
        let mut stmt = conn.prepare(
            "SELECT DISTINCT name FROM repository_packages
             WHERE name LIKE ?1
             ORDER BY LENGTH(name), name",
        )?;

        let rows = stmt.query_map([pattern], |row| row.get::<_, String>(0))?;
        for row in rows {
            let name = row?;
            if PackageSelector::find_best_package(conn, &name, options).is_ok() {
                candidates.push(name);
            }
        }
    }

    candidates.sort_by_key(|name| {
        let lower = name.to_lowercase();
        let starts_with_non_dev = (!lower.ends_with("-devel")
            && !lower.ends_with("-dev")
            && !lower.contains('+')
            && !lower.starts_with("rust-")
            && !lower.starts_with("ghc-")
            && !lower.starts_with("python")) as u8;
        let soname_stem = dep_name
            .split(".so")
            .next()
            .unwrap_or(dep_name)
            .trim_start_matches("lib")
            .to_lowercase();
        let contains_stem = lower.contains(&soname_stem) as u8;
        (
            Reverse(starts_with_non_dev),
            Reverse(contains_stem),
            lower.len(),
            lower,
        )
    });
    candidates.dedup();

    Ok(candidates.into_iter().next())
}

/// Resolve dependencies and return list of packages to download
///
/// This function takes a list of dependency names and searches repositories
/// for matching packages. It checks which dependencies are already installed
/// and returns only the ones that need to be downloaded.
///
/// Returns: Vec<(dependency_name, PackageWithRepo)>
pub fn resolve_dependencies(
    conn: &Connection,
    dependencies: &[String],
) -> Result<Vec<(String, PackageWithRepo)>> {
    let mut to_download = Vec::new();

    for dep_name in dependencies {
        // Skip rpmlib dependencies and file paths
        if dep_name.starts_with("rpmlib(") || dep_name.starts_with('/') {
            continue;
        }

        // Check if already installed
        let installed = Trove::find_by_name(conn, dep_name)?;
        if !installed.is_empty() {
            debug!("Dependency {} already installed, skipping", dep_name);
            continue;
        }

        // Search repositories for this dependency
        let options = SelectionOptions::default();
        let (resolved_name, _) =
            resolve_repo_dependency_request(conn, dep_name, &VersionConstraint::Any, &options)?;
        match PackageSelector::find_best_package(conn, &resolved_name, &options) {
            Ok(pkg_with_repo) => {
                info!(
                    "Found dependency {} as package {} version {} in repository {}",
                    dep_name,
                    resolved_name,
                    pkg_with_repo.package.version,
                    pkg_with_repo.repository.name
                );
                to_download.push((dep_name.clone(), pkg_with_repo));
            }
            Err(e) => {
                // Dependency not found - this is a critical error
                return Err(Error::NotFound(format!(
                    "Required dependency '{dep_name}' not found in any repository: {e}"
                )));
            }
        }
    }

    Ok(to_download)
}

/// Resolve dependency requests without transitive expansion.
///
/// Like [`resolve_dependencies`] but accepts `(name, constraint)` pairs
/// and passes the constraint into the capability/name resolution step.
/// Does **not** invoke the SAT solver or expand transitive dependencies.
///
/// Designed for callers that handle transitive expansion themselves (e.g.
/// recursive CCS install paths where each dep install handles its own deps).
pub fn resolve_dependency_requests(
    conn: &Connection,
    requests: &[(String, VersionConstraint)],
) -> Result<Vec<(String, PackageWithRepo)>> {
    let mut to_download = Vec::new();
    let options = SelectionOptions::default();

    for (dep_name, constraint) in requests {
        if dep_name.starts_with("rpmlib(") || dep_name.starts_with('/') {
            continue;
        }

        let installed = Trove::find_by_name(conn, dep_name)?;
        if !installed.is_empty() {
            debug!("Dependency {} already installed, skipping", dep_name);
            continue;
        }

        let (resolved_name, resolved_constraint) =
            resolve_repo_dependency_request(conn, dep_name, constraint, &options)?;

        // Use the resolved constraint's version to pin selection when possible
        let select_options = match &resolved_constraint {
            VersionConstraint::Exact(v) => SelectionOptions {
                version: Some(v.to_string()),
                ..options.clone()
            },
            _ => options.clone(),
        };

        match PackageSelector::find_best_package(conn, &resolved_name, &select_options) {
            Ok(pkg_with_repo) => {
                info!(
                    "Resolved dependency {} -> {} {} (repo {})",
                    dep_name,
                    resolved_name,
                    pkg_with_repo.package.version,
                    pkg_with_repo.repository.name
                );
                to_download.push((dep_name.clone(), pkg_with_repo));
            }
            Err(e) => {
                return Err(Error::NotFound(format!(
                    "Required dependency '{dep_name}' not found in any repository: {e}"
                )));
            }
        }
    }

    Ok(to_download)
}

/// Resolve dependencies transitively using the SAT solver
///
/// Uses resolvo's CDCL SAT solver for dependency resolution with backtracking.
/// The solver handles transitive resolution, cycle detection, and topological
/// ordering natively — no manual BFS or Kahn's sort needed.
///
/// Returns: Vec<(dependency_name, PackageWithRepo)> in dependency order
pub fn resolve_dependencies_transitive(
    conn: &Connection,
    initial_dependencies: &[String],
    _max_depth: usize,
) -> Result<Vec<(String, PackageWithRepo)>> {
    let requests: Vec<_> = initial_dependencies
        .iter()
        .filter(|d| !d.starts_with("rpmlib(") && !d.starts_with('/'))
        .map(|d| Ok((d.clone(), VersionConstraint::Any)))
        .collect::<Result<Vec<_>>>()?;

    resolve_dependencies_transitive_requests(conn, &requests, _max_depth)
}

pub fn resolve_dependencies_transitive_requests(
    conn: &Connection,
    initial_requests: &[(String, VersionConstraint)],
    _max_depth: usize,
) -> Result<Vec<(String, PackageWithRepo)>> {
    use crate::resolver::sat;

    let options = SelectionOptions::default();
    let requests: Vec<_> = initial_requests
        .iter()
        .filter(|(d, _)| !d.starts_with("rpmlib(") && !d.starts_with('/'))
        .map(|(d, constraint)| resolve_repo_dependency_request(conn, d, constraint, &options))
        .collect::<Result<Vec<_>>>()?;

    if requests.is_empty() {
        return Ok(Vec::new());
    }

    // Use SAT solver for transitive resolution
    let resolution = sat::solve_install(conn, &requests)?;

    if let Some(conflict_msg) = resolution.conflict_message {
        return Err(Error::NotFound(format!(
            "Dependency resolution failed: {conflict_msg}"
        )));
    }

    // Map SAT results back to downloadable packages, skipping already-installed.
    // Use the SAT-resolved version to select the exact package the solver chose.
    let mut to_download = Vec::new();

    for pkg in &resolution.install_order {
        if pkg.source == sat::SatSource::Installed {
            debug!("Dependency {} already installed, skipping", pkg.name);
            continue;
        }

        // Pin selection to the exact version the SAT solver chose
        let options = SelectionOptions {
            version: Some(pkg.version.to_string()),
            ..SelectionOptions::default()
        };

        // Look up the package in repos for download info
        match PackageSelector::find_best_package(conn, &pkg.name, &options) {
            Ok(pkg_with_repo) => {
                info!(
                    "Resolved dependency {} version {} from repository {}",
                    pkg.name, pkg_with_repo.package.version, pkg_with_repo.repository.name
                );
                to_download.push((pkg.name.clone(), pkg_with_repo));
            }
            Err(e) => {
                return Err(Error::NotFound(format!(
                    "Required dependency '{}' version {} not found in any repository: {e}",
                    pkg.name, pkg.version
                )));
            }
        }
    }

    Ok(to_download)
}

/// Download all dependencies to a directory in parallel
///
/// Downloads are performed concurrently using rayon's parallel iterators.
/// This significantly speeds up the download of multiple dependencies.
///
/// # Arguments
/// * `dependencies` - List of (name, package info) tuples to download
/// * `dest_dir` - Directory to download packages to
/// * `keyring_dir` - Optional keyring directory for GPG verification
///
/// # Returns
/// Vec<(dependency_name, downloaded_path)> on success
pub fn download_dependencies(
    dependencies: &[(String, PackageWithRepo)],
    dest_dir: &Path,
    keyring_dir: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }

    // Calculate total size for aggregate progress
    let total_size: u64 = dependencies
        .iter()
        .map(|(_, pkg)| pkg.package.size as u64)
        .sum();
    let total_mb = total_size as f64 / 1_048_576.0;

    info!(
        "Downloading {} dependencies in parallel ({:.2} MB total)...",
        dependencies.len(),
        total_mb
    );

    // Create multi-progress manager with aggregate tracking
    let progress = DownloadProgress::with_aggregate(dependencies.len(), total_size);

    // Pre-create progress bars for all downloads
    let progress_bars: Vec<_> = dependencies
        .iter()
        .map(|(dep_name, pkg_with_repo)| {
            progress.add_download(dep_name, pkg_with_repo.package.size as u64)
        })
        .collect();

    // Use parallel iterator for concurrent downloads with progress
    // Collect as Vec<Result<_>> to track individual successes/failures
    let individual_results: Vec<Result<(String, PathBuf, u64)>> = dependencies
        .par_iter()
        .zip(progress_bars.par_iter())
        .map(|((dep_name, pkg_with_repo), pb)| {
            info!("Downloading dependency: {}", dep_name);

            // Build GPG options if keyring_dir provided and repo has gpg_check enabled
            let gpg_options = if let Some(keyring) = keyring_dir {
                if pkg_with_repo.repository.gpg_check {
                    Some(DownloadOptions {
                        gpg_check: true,
                        gpg_strict: pkg_with_repo.repository.gpg_strict,
                        keyring_dir: keyring.to_path_buf(),
                        repository_name: pkg_with_repo.repository.name.clone(),
                    })
                } else {
                    None
                }
            } else {
                None
            };

            match download_package_verified_with_progress(
                &pkg_with_repo.package,
                dest_dir,
                gpg_options.as_ref(),
                Some(pb),
            ) {
                Ok(path) => {
                    DownloadProgress::finish_download(pb, dep_name);
                    Ok((dep_name.clone(), path, pkg_with_repo.package.size as u64))
                }
                Err(e) => {
                    DownloadProgress::fail_download(pb, dep_name, &e.to_string());
                    Err(e)
                }
            }
        })
        .collect();

    // Calculate statistics and show summary
    let mut succeeded_results = Vec::new();
    let mut failed_count = 0;
    let mut bytes_downloaded: u64 = 0;

    for result in individual_results {
        match result {
            Ok((name, path, size)) => {
                bytes_downloaded += size;
                succeeded_results.push((name, path));
            }
            Err(_) => {
                failed_count += 1;
            }
        }
    }

    progress.finish_all(succeeded_results.len(), failed_count, bytes_downloaded);

    // If any downloads failed, return error
    if failed_count > 0 {
        return Err(Error::DownloadError(format!(
            "{} of {} dependency downloads failed",
            failed_count,
            dependencies.len()
        )));
    }

    Ok(succeeded_results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Repository, RepositoryPackage, RepositoryProvide};
    use crate::db::schema;
    use rusqlite::Connection;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn resolves_soname_dependency_to_repo_package_name() {
        let conn = test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "libjq".to_string(),
            "1.8.1-1.fc43".to_string(),
            "sha256:test".to_string(),
            123,
            "https://example.invalid/libjq.rpm".to_string(),
        );
        pkg.insert(&conn).unwrap();

        let (resolved, _constraint) = resolve_repo_dependency_request(
            &conn,
            "libjq.so.1",
            &VersionConstraint::Any,
            &SelectionOptions::default(),
        )
        .unwrap();
        assert_eq!(resolved, "libjq");
    }

    #[test]
    fn resolves_soname_dependency_to_repo_package_name_by_search_stem() {
        let conn = test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        for name in ["oniguruma", "oniguruma-devel", "rust-onig-devel"] {
            let mut pkg = RepositoryPackage::new(
                repo_id,
                name.to_string(),
                "6.9.10-3.fc43".to_string(),
                format!("sha256:{name}"),
                123,
                format!("https://example.invalid/{name}.rpm"),
            );
            pkg.insert(&conn).unwrap();
        }

        let (resolved, _constraint) = resolve_repo_dependency_request(
            &conn,
            "libonig.so.5",
            &VersionConstraint::Any,
            &SelectionOptions::default(),
        )
        .unwrap();
        assert_eq!(resolved, "oniguruma");
    }

    #[test]
    fn resolves_capability_dependency_from_repo_metadata_provides() {
        let conn = test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "kernel-core".to_string(),
            "6.19.6-200.fc43".to_string(),
            "sha256:test".to_string(),
            123,
            "https://example.invalid/kernel-core.rpm".to_string(),
        );
        pkg.metadata = Some(
            serde_json::json!({
                "rpm_provides": ["kernel-core-uname-r = 6.19.6-200.fc43.x86_64"]
            })
            .to_string(),
        );
        pkg.insert(&conn).unwrap();

        let (resolved, constraint) = resolve_repo_dependency_request(
            &conn,
            "kernel-core-uname-r",
            &VersionConstraint::parse("= 6.19.6-200.fc43.x86_64").unwrap(),
            &SelectionOptions::default(),
        )
        .unwrap();
        assert_eq!(resolved, "kernel-core");
        assert_eq!(
            constraint,
            VersionConstraint::parse("= 6.19.6-200.fc43").unwrap()
        );
    }

    #[test]
    fn resolves_capability_dependency_from_normalized_repo_provides() {
        let conn = test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "kernel-core".to_string(),
            "6.19.6-200.fc43".to_string(),
            "sha256:test".to_string(),
            123,
            "https://example.invalid/kernel-core.rpm".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let repo_package_id = pkg.id.unwrap();

        let mut provide = RepositoryProvide::new(
            repo_package_id,
            "kernel-core-uname-r".to_string(),
            Some("6.19.6-200.fc43.x86_64".to_string()),
            "package".to_string(),
            Some("kernel-core-uname-r = 6.19.6-200.fc43.x86_64".to_string()),
        );
        provide.insert(&conn).unwrap();

        let (resolved, constraint) = resolve_repo_dependency_request(
            &conn,
            "kernel-core-uname-r",
            &VersionConstraint::parse("= 6.19.6-200.fc43.x86_64").unwrap(),
            &SelectionOptions::default(),
        )
        .unwrap();
        assert_eq!(resolved, "kernel-core");
        assert_eq!(
            constraint,
            VersionConstraint::parse("= 6.19.6-200.fc43").unwrap()
        );
    }

    #[test]
    fn resolve_dependency_requests_finds_direct_packages_without_transitive_expansion() {
        let conn = test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        // Package A depends on B (but we only resolve A, not transitively)
        let mut pkg_a = RepositoryPackage::new(
            repo_id,
            "pkg-a".to_string(),
            "1.0-1.fc43".to_string(),
            "sha256:a".to_string(),
            100,
            "https://example.invalid/pkg-a.rpm".to_string(),
        );
        pkg_a.insert(&conn).unwrap();

        let mut pkg_b = RepositoryPackage::new(
            repo_id,
            "pkg-b".to_string(),
            "2.0-1.fc43".to_string(),
            "sha256:b".to_string(),
            200,
            "https://example.invalid/pkg-b.rpm".to_string(),
        );
        pkg_b.insert(&conn).unwrap();

        let requests = vec![
            ("pkg-a".to_string(), VersionConstraint::Any),
            ("pkg-b".to_string(), VersionConstraint::Any),
        ];

        let result = resolve_dependency_requests(&conn, &requests).unwrap();

        // Both should be found (non-transitive: just the requested packages)
        assert_eq!(result.len(), 2);
        let names: Vec<&str> = result.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"pkg-a"));
        assert!(names.contains(&"pkg-b"));
    }

    #[test]
    fn resolve_dependency_requests_skips_installed() {
        let conn = test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "already-here".to_string(),
            "1.0-1.fc43".to_string(),
            "sha256:ah".to_string(),
            100,
            "https://example.invalid/already-here.rpm".to_string(),
        );
        pkg.insert(&conn).unwrap();

        // Install a trove with the same name
        conn.execute(
            "INSERT INTO troves (name, version, type, install_source, install_reason)
             VALUES ('already-here', '1.0-1.fc43', 'package', 'repository', 'explicit')",
            [],
        )
        .unwrap();

        let requests = vec![("already-here".to_string(), VersionConstraint::Any)];
        let result = resolve_dependency_requests(&conn, &requests).unwrap();

        assert!(result.is_empty(), "installed packages should be skipped");
    }

    #[test]
    fn resolves_rpm_provided_capability_via_normalized_provides() {
        let conn = test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "glibc".to_string(),
            "2.39-1.fc43".to_string(),
            "sha256:glibc".to_string(),
            500,
            "https://example.invalid/glibc.rpm".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        // RPM-style soname provide
        let mut provide = RepositoryProvide::new(
            pkg_id,
            "libc.so.6(GLIBC_2.17)(64bit)".to_string(),
            Some("2.39".to_string()),
            "soname".to_string(),
            Some("libc.so.6(GLIBC_2.17)(64bit)".to_string()),
        );
        provide.insert(&conn).unwrap();

        let (resolved, _) = resolve_repo_dependency_request(
            &conn,
            "libc.so.6(GLIBC_2.17)(64bit)",
            &VersionConstraint::Any,
            &SelectionOptions::default(),
        )
        .unwrap();
        assert_eq!(resolved, "glibc");
    }

    #[test]
    fn resolves_debian_virtual_package_via_normalized_provides() {
        let conn = test_db();

        let mut repo = Repository::new("ubuntu".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "postfix".to_string(),
            "3.8.4-1".to_string(),
            "sha256:postfix".to_string(),
            300,
            "https://example.invalid/postfix.deb".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        // Debian virtual package provide
        let mut provide = RepositoryProvide::new(
            pkg_id,
            "mail-transport-agent".to_string(),
            None,
            "virtual".to_string(),
            Some("mail-transport-agent".to_string()),
        );
        provide.insert(&conn).unwrap();

        let (resolved, _) = resolve_repo_dependency_request(
            &conn,
            "mail-transport-agent",
            &VersionConstraint::Any,
            &SelectionOptions::default(),
        )
        .unwrap();
        assert_eq!(resolved, "postfix");
    }

    #[test]
    fn resolves_arch_versioned_provide_via_normalized_provides() {
        let conn = test_db();

        let mut repo = Repository::new("arch".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "sh".to_string(),
            "5.2.37-1".to_string(),
            "sha256:sh".to_string(),
            200,
            "https://example.invalid/sh.pkg.tar.zst".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        // Arch versioned provide
        let mut provide = RepositoryProvide::new(
            pkg_id,
            "sh".to_string(),
            Some("5.2.37".to_string()),
            "package".to_string(),
            Some("sh=5.2.37".to_string()),
        );
        provide.insert(&conn).unwrap();

        let (resolved, constraint) = resolve_repo_dependency_request(
            &conn,
            "sh",
            &VersionConstraint::Any,
            &SelectionOptions::default(),
        )
        .unwrap();
        // Direct package name match takes precedence
        assert_eq!(resolved, "sh");
        assert_eq!(constraint, VersionConstraint::Any);
    }

    #[test]
    fn no_fallback_to_name_guessing_when_normalized_provide_exists() {
        // When a normalized provide exists, we should use it rather than
        // falling through to cross-distro heuristics or fuzzy search.
        let conn = test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        // Two packages: one that matches by name heuristics, one that
        // actually declares the provide.
        let mut wrong_pkg = RepositoryPackage::new(
            repo_id,
            "libfoo".to_string(),
            "1.0-1.fc43".to_string(),
            "sha256:wrong".to_string(),
            100,
            "https://example.invalid/libfoo.rpm".to_string(),
        );
        wrong_pkg.insert(&conn).unwrap();

        let mut correct_pkg = RepositoryPackage::new(
            repo_id,
            "libfoo-compat".to_string(),
            "2.0-1.fc43".to_string(),
            "sha256:correct".to_string(),
            100,
            "https://example.invalid/libfoo-compat.rpm".to_string(),
        );
        correct_pkg.insert(&conn).unwrap();
        let correct_id = correct_pkg.id.unwrap();

        // Only the compat package actually provides the soname
        let mut provide = RepositoryProvide::new(
            correct_id,
            "libfoo.so.1".to_string(),
            None,
            "soname".to_string(),
            Some("libfoo.so.1".to_string()),
        );
        provide.insert(&conn).unwrap();

        let (resolved, _) = resolve_repo_dependency_request(
            &conn,
            "libfoo.so.1",
            &VersionConstraint::Any,
            &SelectionOptions::default(),
        )
        .unwrap();
        // Must resolve to the package that declares the provide, not the
        // one whose name happens to match via heuristics.
        assert_eq!(resolved, "libfoo-compat");
    }

    #[test]
    fn capability_lookup_preferred_over_heuristics() {
        // Verify that normalized capability lookup runs before
        // `generate_capability_variations` heuristics.
        let conn = test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        // A package whose name would match a variation of the dep name
        let mut heuristic_pkg = RepositoryPackage::new(
            repo_id,
            "libssl3".to_string(),
            "3.2.0-1.fc43".to_string(),
            "sha256:heur".to_string(),
            100,
            "https://example.invalid/libssl3.rpm".to_string(),
        );
        heuristic_pkg.insert(&conn).unwrap();

        // A different package that actually provides the capability
        let mut provider_pkg = RepositoryPackage::new(
            repo_id,
            "openssl-libs".to_string(),
            "3.2.0-1.fc43".to_string(),
            "sha256:provider".to_string(),
            200,
            "https://example.invalid/openssl-libs.rpm".to_string(),
        );
        provider_pkg.insert(&conn).unwrap();
        let provider_id = provider_pkg.id.unwrap();

        let mut provide = RepositoryProvide::new(
            provider_id,
            "libssl.so.3".to_string(),
            None,
            "soname".to_string(),
            Some("libssl.so.3()(64bit)".to_string()),
        );
        provide.insert(&conn).unwrap();

        let (resolved, _) = resolve_repo_dependency_request(
            &conn,
            "libssl.so.3",
            &VersionConstraint::Any,
            &SelectionOptions::default(),
        )
        .unwrap();
        // Should resolve via capability, not via name heuristic
        assert_eq!(resolved, "openssl-libs");
    }
}
