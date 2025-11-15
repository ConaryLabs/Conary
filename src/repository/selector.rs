// src/repository/selector.rs

//! Package selection logic for repository-based installation
//!
//! This module handles selecting the best package when multiple matches exist
//! across different repositories, versions, or architectures.

use crate::db::models::{Repository, RepositoryPackage};
use crate::error::{Error, Result};
use crate::version::RpmVersion;
use rusqlite::Connection;
use std::env;
use tracing::{debug, info};

/// Options for package selection
#[derive(Debug, Clone, Default)]
pub struct SelectionOptions {
    /// Specific version to select (if None, select latest)
    pub version: Option<String>,
    /// Specific repository to search (if None, search all enabled)
    pub repository: Option<String>,
    /// Specific architecture to filter (if None, use system architecture)
    pub architecture: Option<String>,
}

/// Information about a package with its repository
#[derive(Debug, Clone)]
pub struct PackageWithRepo {
    pub package: RepositoryPackage,
    pub repository: Repository,
}

/// Package selector for choosing the best package from multiple matches
pub struct PackageSelector;

impl PackageSelector {
    /// Detect the current system architecture
    pub fn detect_architecture() -> String {
        // env::consts::ARCH returns the target architecture
        // Common values: "x86_64", "aarch64", "x86", "arm", etc.
        env::consts::ARCH.to_string()
    }

    /// Check if a package architecture is compatible with the system
    pub fn is_architecture_compatible(pkg_arch: Option<&str>, system_arch: &str) -> bool {
        match pkg_arch {
            None => true, // Unknown architecture - assume compatible
            Some("noarch") => true, // noarch is compatible with everything
            Some(arch) => arch == system_arch,
        }
    }

    /// Search for packages by name with selection options
    ///
    /// Returns all matching packages with their repository information,
    /// filtered by the selection options.
    pub fn search_packages(
        conn: &Connection,
        package_name: &str,
        options: &SelectionOptions,
    ) -> Result<Vec<PackageWithRepo>> {
        let detected_arch = Self::detect_architecture();
        let system_arch = options
            .architecture
            .as_deref()
            .unwrap_or(&detected_arch);

        debug!(
            "Searching for package '{}' (arch: {})",
            package_name, system_arch
        );

        // Find all matching packages
        let packages = RepositoryPackage::find_by_name(conn, package_name)?;

        if packages.is_empty() {
            return Ok(Vec::new());
        }

        // Get repository information for each package
        let mut results = Vec::new();
        for pkg in packages {
            // Filter by version if specified
            if let Some(ref version) = options.version {
                if &pkg.version != version {
                    continue;
                }
            }

            // Filter by architecture
            if !Self::is_architecture_compatible(pkg.architecture.as_deref(), system_arch) {
                debug!(
                    "Skipping package {} {} with incompatible arch {:?}",
                    pkg.name, pkg.version, pkg.architecture
                );
                continue;
            }

            // Get repository information
            let repo = Repository::find_by_id(conn, pkg.repository_id)?
                .ok_or_else(|| {
                    Error::NotFoundError(format!(
                        "Repository {} not found for package {}",
                        pkg.repository_id, pkg.name
                    ))
                })?;

            // Filter by repository if specified
            if let Some(ref repo_name) = options.repository {
                if &repo.name != repo_name {
                    continue;
                }
            }

            // Only include enabled repositories
            if !repo.enabled {
                debug!(
                    "Skipping package {} from disabled repository {}",
                    pkg.name, repo.name
                );
                continue;
            }

            results.push(PackageWithRepo {
                package: pkg,
                repository: repo,
            });
        }

        Ok(results)
    }

    /// Select the best package from a list of candidates
    ///
    /// Selection criteria (in order of priority):
    /// 1. Repository priority (higher is better)
    /// 2. Version (latest version)
    /// 3. First match (stable tie-breaker)
    pub fn select_best(candidates: Vec<PackageWithRepo>) -> Result<PackageWithRepo> {
        if candidates.is_empty() {
            return Err(Error::NotFoundError(
                "No matching packages found".to_string(),
            ));
        }

        if candidates.len() == 1 {
            return Ok(candidates.into_iter().next().unwrap());
        }

        // Sort by priority (descending) and version (descending)
        let mut sorted = candidates;
        sorted.sort_by(|a, b| {
            // First compare repository priority (higher is better)
            match b.repository.priority.cmp(&a.repository.priority) {
                std::cmp::Ordering::Equal => {
                    // Then compare versions (newer is better)
                    match (
                        RpmVersion::parse(&a.package.version),
                        RpmVersion::parse(&b.package.version),
                    ) {
                        (Ok(v_a), Ok(v_b)) => v_b.cmp(&v_a),
                        // If version parsing fails, fall back to string comparison
                        _ => b.package.version.cmp(&a.package.version),
                    }
                }
                ord => ord,
            }
        });

        let selected = sorted.into_iter().next().unwrap();
        info!(
            "Selected package {} {} from repository {} (priority {})",
            selected.package.name,
            selected.package.version,
            selected.repository.name,
            selected.repository.priority
        );

        Ok(selected)
    }

    /// Find and select the best package matching the given name and options
    ///
    /// This is a convenience function that combines search and selection.
    pub fn find_best_package(
        conn: &Connection,
        package_name: &str,
        options: &SelectionOptions,
    ) -> Result<PackageWithRepo> {
        let candidates = Self::search_packages(conn, package_name, options)?;

        if candidates.is_empty() {
            let mut msg = format!("Package '{}' not found in any repository", package_name);

            if let Some(ref repo) = options.repository {
                msg.push_str(&format!(" (searched repository: {})", repo));
            }

            if let Some(ref version) = options.version {
                msg.push_str(&format!(" (version: {})", version));
            }

            return Err(Error::NotFoundError(msg));
        }

        Self::select_best(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_architecture() {
        let arch = PackageSelector::detect_architecture();
        // Should return one of the known architectures
        assert!(!arch.is_empty());
        // On most development machines, this will be x86_64
        println!("Detected architecture: {}", arch);
    }

    #[test]
    fn test_architecture_compatibility() {
        let system_arch = "x86_64";

        // noarch is compatible with everything
        assert!(PackageSelector::is_architecture_compatible(
            Some("noarch"),
            system_arch
        ));

        // Exact match is compatible
        assert!(PackageSelector::is_architecture_compatible(
            Some("x86_64"),
            system_arch
        ));

        // Different arch is not compatible
        assert!(!PackageSelector::is_architecture_compatible(
            Some("aarch64"),
            system_arch
        ));

        // None (unknown) is compatible
        assert!(PackageSelector::is_architecture_compatible(
            None,
            system_arch
        ));
    }

    #[test]
    fn test_selection_options_default() {
        let opts = SelectionOptions::default();
        assert!(opts.version.is_none());
        assert!(opts.repository.is_none());
        assert!(opts.architecture.is_none());
    }
}
