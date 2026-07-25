// conary-core/src/repository/dependencies.rs

//! Dependency resolution
//!
//! Functions for resolving package dependencies across repositories,
//! including transitive resolution and parallel downloads.

use crate::db::models::RepositoryProvide;
use crate::error::{Error, Result};
use crate::repository::versioning::{
    RepoVersionConstraint, repo_version_satisfies, resolve_package_version_scheme,
};
use crate::version::VersionConstraint;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::download::{
    DownloadOptions, DownloadProgress, download_package_verified_with_progress,
    download_static_package_verified_with_progress,
};
use super::selector::{PackageSelector, PackageWithRepo, SelectionOptions};

/// Convert a `VersionConstraint` to a `RepoVersionConstraint` for native
/// repository version comparison.
///
/// The provide's version is a native string (e.g. RPM or Debian format) so we
/// need to compare using the repository's native scheme rather than Conary's
/// internal `RpmVersion` comparator.
fn to_repo_constraint(constraint: &VersionConstraint) -> RepoVersionConstraint {
    match constraint {
        VersionConstraint::Any => RepoVersionConstraint::Any,
        VersionConstraint::Exact(v) => RepoVersionConstraint::Exact(v.to_string()),
        VersionConstraint::GreaterThan(v) => RepoVersionConstraint::GreaterThan(v.to_string()),
        VersionConstraint::GreaterOrEqual(v) => {
            RepoVersionConstraint::GreaterOrEqual(v.to_string())
        }
        VersionConstraint::LessThan(v) => RepoVersionConstraint::LessThan(v.to_string()),
        VersionConstraint::LessOrEqual(v) => RepoVersionConstraint::LessOrEqual(v.to_string()),
        VersionConstraint::NotEqual(v) => RepoVersionConstraint::NotEqual(v.to_string()),
        VersionConstraint::And(_, _) => {
            // Compound constraints cannot be represented as a single
            // RepoVersionConstraint. Return None and let the caller
            // check both halves separately.
            RepoVersionConstraint::Any
        }
    }
}

#[cfg(test)]
#[path = "dependencies/tests.rs"]
mod tests;

/// Check if a provide's version satisfies the dependency constraint.
///
/// Returns `true` when the constraint is `Any`, the provide has no version, or
/// the provide's version passes the native version comparison.
fn provide_satisfies_constraint(
    provide: &RepositoryProvide,
    constraint: &VersionConstraint,
) -> Result<bool> {
    if matches!(constraint, VersionConstraint::Any) {
        return Ok(true);
    }

    // Handle And constraints by checking both halves.
    if let VersionConstraint::And(a, b) = constraint {
        return Ok(
            provide_satisfies_constraint(provide, a)? && provide_satisfies_constraint(provide, b)?
        );
    }

    let Some(ref provide_version) = provide.version else {
        // Unversioned provide cannot satisfy a versioned constraint.
        return Ok(false);
    };

    let scheme = provide.version_scheme;
    let repo_constraint = to_repo_constraint(constraint);
    Ok(repo_version_satisfies(
        scheme,
        provide_version,
        &repo_constraint,
    )?)
}

fn package_satisfies_constraint(
    candidate: &PackageWithRepo,
    constraint: &VersionConstraint,
) -> Result<bool> {
    if matches!(constraint, VersionConstraint::Any) {
        return Ok(true);
    }
    if let VersionConstraint::And(left, right) = constraint {
        return Ok(package_satisfies_constraint(candidate, left)?
            && package_satisfies_constraint(candidate, right)?);
    }
    let scheme = resolve_package_version_scheme(&candidate.package);
    Ok(repo_version_satisfies(
        scheme,
        &candidate.package.version,
        &to_repo_constraint(constraint),
    )?)
}

fn exact_selected_constraint(dep_name: &str, version: &str) -> Result<VersionConstraint> {
    VersionConstraint::parse(&format!("= {version}")).map_err(|error| {
        Error::VersionParse(format!(
            "repository provider for '{dep_name}' selected invalid package version '{version}': {error}"
        ))
    })
}

/// Resolve a dependency by querying normalized `repository_provides` data.
///
/// This is the preferred path: it queries the indexed `repository_provides`
/// table directly instead of scanning JSON metadata blobs.  The join fetches
/// only the package names that actually declare the capability, avoiding the
/// expensive `list_all()` call.
///
/// When a versioned `constraint` is supplied, providers whose declared version
/// does not satisfy the constraint are filtered out before candidate selection.
fn resolve_repo_dependency_by_capability(
    conn: &Connection,
    dep_name: &str,
    constraint: &VersionConstraint,
    options: &SelectionOptions,
) -> Result<Option<(String, Option<String>)>> {
    // Single JOIN returns each provide alongside its package name, eliminating
    // the per-row `SELECT name FROM repository_packages WHERE id = ?` (N+1).
    let provides_with_name = RepositoryProvide::find_by_capability_with_name(conn, dep_name)?;
    if provides_with_name.is_empty() {
        return Ok(None);
    }

    // Collect distinct package IDs from the provides, then look up each one
    // by name through the selector (which handles arch filtering, version
    // pinning, etc.).  Only consider provides whose version satisfies the
    // dependency constraint.
    let mut seen_ids = HashSet::new();
    let mut candidates = Vec::new();

    for (provide, name) in &provides_with_name {
        // Filter by provide version vs dependency constraint.
        if !provide_satisfies_constraint(provide, constraint)? {
            debug!(
                "Provide {} version {:?} does not satisfy constraint {:?}, skipping",
                provide.capability, provide.version, constraint
            );
            continue;
        }

        if !seen_ids.insert(provide.repository_package_id) {
            continue;
        }

        for candidate in PackageSelector::search_packages(conn, name, options)? {
            if candidate.package.id == Some(provide.repository_package_id) {
                candidates.push(candidate);
                break;
            }
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = PackageSelector::select_best_with_options(conn, candidates, options)?;
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
    // 1. Exact package name and native-version match.
    let mut exact_candidates = Vec::new();
    for candidate in PackageSelector::search_packages(conn, dep_name, options)? {
        if package_satisfies_constraint(&candidate, constraint)? {
            exact_candidates.push(candidate);
        }
    }
    if !exact_candidates.is_empty() {
        let selected = PackageSelector::select_best_with_options(conn, exact_candidates, options)?;
        return Ok((
            dep_name.to_string(),
            exact_selected_constraint(dep_name, &selected.package.version)?,
        ));
    }

    // 2. Normalized capability lookup -- preferred resolution path.
    //    Queries the indexed `repository_provides` table using the exact
    //    capability key supplied by repository metadata.
    if let Some((package_name, package_version)) =
        resolve_repo_dependency_by_capability(conn, dep_name, constraint, options)?
    {
        let resolved_constraint = if let Some(version) = package_version {
            exact_selected_constraint(dep_name, &version)?
        } else {
            constraint.clone()
        };
        return Ok((package_name, resolved_constraint));
    }

    Err(Error::NotFound(format!(
        "Required dependency '{dep_name}' not found in any repository"
    )))
}

/// Resolve dependency requests without transitive expansion.
///
/// Accepts `(name, constraint)` pairs and passes the constraint into the
/// capability/name resolution step.
/// Does **not** invoke the SAT solver or expand transitive dependencies.
///
/// Designed for callers that handle transitive expansion themselves (e.g.
/// recursive CCS install paths where each dep install handles its own deps).
pub fn resolve_dependency_requests(
    conn: &Connection,
    requests: &[(String, VersionConstraint)],
    options: &SelectionOptions,
) -> Result<Vec<(String, PackageWithRepo)>> {
    let mut to_download = Vec::new();
    let mut queued_packages = std::collections::HashSet::new();

    for (dep_name, constraint) in requests {
        let (resolved_name, resolved_constraint) =
            resolve_repo_dependency_request(conn, dep_name, constraint, options)?;

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
                let package_key = (
                    pkg_with_repo.repository.name.clone(),
                    pkg_with_repo.package.name.clone(),
                    pkg_with_repo.package.version.clone(),
                );
                if !queued_packages.insert(package_key) {
                    debug!(
                        "Dependency {} resolves to package {} {}, already queued",
                        dep_name, pkg_with_repo.package.name, pkg_with_repo.package.version
                    );
                    continue;
                }
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

pub fn resolve_dependencies_transitive_requests(
    conn: &Connection,
    initial_requests: &[(String, VersionConstraint)],
    _max_depth: usize,
    options: &SelectionOptions,
) -> Result<Vec<(String, PackageWithRepo)>> {
    use crate::resolver::sat;

    let requests: Vec<_> = initial_requests
        .iter()
        .map(|(d, constraint)| resolve_repo_dependency_request(conn, d, constraint, options))
        .collect::<Result<Vec<_>>>()?;

    if requests.is_empty() {
        return Ok(Vec::new());
    }

    // Use SAT solver for transitive resolution
    let default_policy = crate::repository::resolution_policy::ResolutionPolicy::default();
    let policy = options.policy.as_ref().unwrap_or(&default_policy);
    let resolution = sat::solve_install_with_policy(conn, &requests, policy)?;

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
            ..options.clone()
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
/// * `keyring_dir` - Prepared native repository trust store
///
/// # Returns
/// Vec<(dependency_name, downloaded_path)> on success
pub async fn download_dependencies(
    dependencies: &[(String, PackageWithRepo)],
    dest_dir: &Path,
    keyring_dir: &Path,
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
    let mut individual_results: Vec<Result<(String, PathBuf, u64)>> = Vec::new();
    for ((dep_name, pkg_with_repo), pb) in dependencies.iter().zip(progress_bars.iter()) {
        info!("Downloading dependency: {}", dep_name);

        let trust = if pkg_with_repo.repository.default_strategy.as_deref() == Some("static") {
            None
        } else {
            Some(DownloadOptions::for_repository(
                &pkg_with_repo.repository,
                keyring_dir,
            )?)
        };

        let result =
            match download_dependency_package(pkg_with_repo, dest_dir, trust.as_ref(), Some(pb))
                .await
            {
                Ok(path) => {
                    DownloadProgress::finish_download(pb, dep_name);
                    Ok((dep_name.clone(), path, pkg_with_repo.package.size as u64))
                }
                Err(e) => {
                    DownloadProgress::fail_download(pb, dep_name, &e.to_string());
                    Err(e)
                }
            };
        individual_results.push(result);
    }

    // Calculate statistics and show summary
    let mut succeeded_results = Vec::new();
    let mut failures = Vec::new();
    let mut bytes_downloaded: u64 = 0;

    for result in individual_results {
        match result {
            Ok((name, path, size)) => {
                bytes_downloaded += size;
                succeeded_results.push((name, path));
            }
            Err(e) => {
                failures.push(e.to_string());
            }
        }
    }

    let failed_count = failures.len();
    progress.finish_all(succeeded_results.len(), failed_count, bytes_downloaded);

    // If any downloads failed, return error
    if failed_count > 0 {
        let details = failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let suffix = if failed_count > 5 {
            format!("; ... and {} more", failed_count - 5)
        } else {
            String::new()
        };
        return Err(Error::DownloadError(format!(
            "{} of {} dependency downloads failed: {}{}",
            failed_count,
            dependencies.len(),
            details,
            suffix
        )));
    }

    Ok(succeeded_results)
}

async fn download_dependency_package(
    pkg_with_repo: &PackageWithRepo,
    dest_dir: &Path,
    native_trust: Option<&DownloadOptions>,
    progress_bar: Option<&indicatif::ProgressBar>,
) -> Result<PathBuf> {
    if pkg_with_repo.repository.default_strategy.as_deref() == Some("static") {
        download_static_package_verified_with_progress(
            &pkg_with_repo.package,
            dest_dir,
            None,
            progress_bar,
        )
        .await
    } else {
        let trust = native_trust.ok_or_else(|| {
            Error::ConfigError(format!(
                "native repository '{}' download has no prepared trust policy",
                pkg_with_repo.repository.name
            ))
        })?;
        download_package_verified_with_progress(
            &pkg_with_repo.package,
            dest_dir,
            trust,
            progress_bar,
        )
        .await
    }
}
