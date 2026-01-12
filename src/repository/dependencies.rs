// src/repository/dependencies.rs

//! Dependency resolution
//!
//! Functions for resolving package dependencies across repositories,
//! including transitive resolution and parallel downloads.

use crate::db::models::Trove;
use crate::error::{Error, Result};
use rayon::prelude::*;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use super::download::download_package;
use super::selector::{PackageSelector, PackageWithRepo, SelectionOptions};

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
        match PackageSelector::find_best_package(conn, dep_name, &options) {
            Ok(pkg_with_repo) => {
                info!(
                    "Found dependency {} version {} in repository {}",
                    dep_name, pkg_with_repo.package.version, pkg_with_repo.repository.name
                );
                to_download.push((dep_name.clone(), pkg_with_repo));
            }
            Err(e) => {
                // Dependency not found - this is a critical error
                return Err(Error::NotFoundError(format!(
                    "Required dependency '{dep_name}' not found in any repository: {e}"
                )));
            }
        }
    }

    Ok(to_download)
}

/// Resolve dependencies transitively (recursively resolve all dependencies)
///
/// This function performs a breadth-first search through the dependency tree,
/// resolving all transitive dependencies. It tracks visited packages to avoid
/// cycles and respects a maximum depth to prevent infinite loops.
///
/// Returns: Vec<(dependency_name, PackageWithRepo)> in topological order (dependencies before dependents)
pub fn resolve_dependencies_transitive(
    conn: &Connection,
    initial_dependencies: &[String],
    max_depth: usize,
) -> Result<Vec<(String, PackageWithRepo)>> {
    let mut to_download: HashMap<String, PackageWithRepo> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    // Seed queue with initial dependencies
    for dep in initial_dependencies {
        // Skip rpmlib dependencies and file paths
        if dep.starts_with("rpmlib(") || dep.starts_with('/') {
            continue;
        }
        queue.push_back((dep.clone(), 0));
    }

    while let Some((dep_name, depth)) = queue.pop_front() {
        // Check depth limit
        if depth > max_depth {
            warn!(
                "Maximum dependency depth {} reached for package {}",
                max_depth, dep_name
            );
            continue;
        }

        // Skip if already visited
        if visited.contains(&dep_name) {
            continue;
        }
        visited.insert(dep_name.clone());

        // Check if already installed
        let installed = Trove::find_by_name(conn, &dep_name)?;
        if !installed.is_empty() {
            debug!("Dependency {} already installed, skipping", dep_name);
            continue;
        }

        // Check if already in to_download list
        if to_download.contains_key(&dep_name) {
            continue;
        }

        // Search repositories for this dependency
        let options = SelectionOptions::default();
        let pkg_with_repo = PackageSelector::find_best_package(conn, &dep_name, &options)
            .map_err(|e| {
                Error::NotFoundError(format!(
                    "Required dependency '{dep_name}' not found in any repository: {e}"
                ))
            })?;

        info!(
            "Found dependency {} version {} in repository {} (depth: {})",
            dep_name, pkg_with_repo.package.version, pkg_with_repo.repository.name, depth
        );

        // Parse this package's dependencies and add to queue
        if let Ok(sub_deps) = pkg_with_repo.package.parse_dependencies() {
            for sub_dep in sub_deps {
                if !visited.contains(&sub_dep) {
                    queue.push_back((sub_dep, depth + 1));
                }
            }
        }

        to_download.insert(dep_name, pkg_with_repo);
    }

    // Convert HashMap to Vec and perform topological sort for install order
    let mut result: Vec<(String, PackageWithRepo)> = to_download.into_iter().collect();

    // Build dependency graph for topological sorting
    let mut dep_graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    // Initialize in_degree for all packages
    for (name, _) in &result {
        in_degree.insert(name.clone(), 0);
        dep_graph.insert(name.clone(), Vec::new());
    }

    // Build edges: package -> dependencies
    for (name, pkg_with_repo) in &result {
        if let Ok(deps) = pkg_with_repo.package.parse_dependencies() {
            for dep in deps {
                // Only count edges to packages we're actually installing
                if in_degree.contains_key(&dep) {
                    dep_graph.entry(name.clone()).or_default().push(dep.clone());
                    *in_degree.entry(dep).or_default() += 1;
                }
            }
        }
    }

    // Topological sort using Kahn's algorithm
    let mut sorted = Vec::new();
    let mut zero_in_degree: VecDeque<String> = in_degree
        .iter()
        .filter(|&(_, &degree)| degree == 0)
        .map(|(name, _)| name.clone())
        .collect();

    while let Some(node) = zero_in_degree.pop_front() {
        sorted.push(node.clone());

        if let Some(dependents) = dep_graph.get(&node) {
            for dependent in dependents {
                if let Some(degree) = in_degree.get_mut(dependent) {
                    *degree -= 1;
                    if *degree == 0 {
                        zero_in_degree.push_back(dependent.clone());
                    }
                }
            }
        }
    }

    // If sorted doesn't contain all nodes, there's a cycle
    if sorted.len() != result.len() {
        warn!("Circular dependency detected in transitive resolution, using partial order");
        // Fall back to original order if there's a cycle
    } else {
        // Reorder result based on topological sort (dependencies before dependents)
        let pkg_map: HashMap<String, PackageWithRepo> = result.into_iter().collect();
        result = sorted
            .into_iter()
            .filter_map(|name| pkg_map.get(&name).map(|pkg| (name, pkg.clone())))
            .collect();
    }

    Ok(result)
}

/// Download all dependencies to a directory in parallel
///
/// Downloads are performed concurrently using rayon's parallel iterators.
/// This significantly speeds up the download of multiple dependencies.
///
/// Returns: Vec<(dependency_name, downloaded_path)>
pub fn download_dependencies(
    dependencies: &[(String, PackageWithRepo)],
    dest_dir: &Path,
) -> Result<Vec<(String, PathBuf)>> {
    // Use parallel iterator for concurrent downloads
    let results: Result<Vec<_>> = dependencies
        .par_iter()
        .map(|(dep_name, pkg_with_repo)| {
            info!("Downloading dependency: {}", dep_name);
            let path = download_package(&pkg_with_repo.package, dest_dir)?;
            Ok((dep_name.clone(), path))
        })
        .collect();

    results
}
