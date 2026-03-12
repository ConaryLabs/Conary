// conary-core/src/repository/dependencies.rs

//! Dependency resolution
//!
//! Functions for resolving package dependencies across repositories,
//! including transitive resolution and parallel downloads.

use crate::db::models::{Trove, generate_capability_variations};
use crate::error::{Error, Result};
use rayon::prelude::*;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::download::{DownloadOptions, DownloadProgress, download_package_verified_with_progress};
use super::selector::{PackageSelector, PackageWithRepo, SelectionOptions};

fn resolve_repo_dependency_name(
    conn: &Connection,
    dep_name: &str,
    options: &SelectionOptions,
) -> Result<String> {
    if PackageSelector::find_best_package(conn, dep_name, options).is_ok() {
        return Ok(dep_name.to_string());
    }

    for variation in generate_capability_variations(dep_name) {
        if PackageSelector::find_best_package(conn, &variation, options).is_ok() {
            return Ok(variation);
        }
    }

    Err(Error::NotFound(format!(
        "Required dependency '{dep_name}' not found in any repository"
    )))
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
        let resolved_name = resolve_repo_dependency_name(conn, dep_name, &options)?;
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
    use crate::resolver::sat;
    use crate::version::VersionConstraint;

    // Filter out rpmlib dependencies and file paths
    let options = SelectionOptions::default();
    let requests: Vec<_> = initial_dependencies
        .iter()
        .filter(|d| !d.starts_with("rpmlib(") && !d.starts_with('/'))
        .map(|d| {
            resolve_repo_dependency_name(conn, d, &options)
                .map(|resolved| (resolved, VersionConstraint::Any))
        })
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
    use crate::db::models::{Repository, RepositoryPackage};
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

        let resolved =
            resolve_repo_dependency_name(&conn, "libjq.so.1", &SelectionOptions::default())
                .unwrap();
        assert_eq!(resolved, "libjq");
    }
}
