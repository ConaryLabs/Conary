// conary-core/src/repository/selector.rs

//! Package selection logic for repository-based installation
//!
//! This module handles selecting the best package when multiple matches exist
//! across different repositories, versions, or architectures.

use crate::db::models::{Repository, RepositoryPackage};
use crate::error::{Error, Result};
use crate::repository::versioning::compare_repo_package_versions;
use rusqlite::Connection;
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
        super::registry::detect_system_arch()
    }

    /// Check if a package architecture is compatible with the system
    pub fn is_architecture_compatible(pkg_arch: Option<&str>, system_arch: &str) -> bool {
        match pkg_arch {
            None => true,           // Unknown architecture - assume compatible
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
        let system_arch = options.architecture.as_deref().unwrap_or(&detected_arch);

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
            if let Some(ref version) = options.version
                && &pkg.version != version
            {
                continue;
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
            let repo = Repository::find_by_id(conn, pkg.repository_id)?.ok_or_else(|| {
                Error::NotFound(format!(
                    "Repository {} not found for package {}",
                    pkg.repository_id, pkg.name
                ))
            })?;

            // Filter by repository if specified
            if let Some(ref repo_name) = options.repository
                && &repo.name != repo_name
            {
                continue;
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
    pub fn select_best(mut candidates: Vec<PackageWithRepo>) -> Result<PackageWithRepo> {
        if candidates.is_empty() {
            return Err(Error::NotFound("No matching packages found".to_string()));
        }

        candidates.sort_by(
            |a, b| match b.repository.priority.cmp(&a.repository.priority) {
                std::cmp::Ordering::Equal => match compare_repo_package_versions(
                    &a.package,
                    &a.repository,
                    &b.package,
                    &b.repository,
                ) {
                    Some(ord) => ord.reverse(),
                    None => b
                        .repository
                        .name
                        .cmp(&a.repository.name)
                        .then_with(|| b.package.version.cmp(&a.package.version)),
                },
                ord => ord,
            },
        );

        // Safe: we verified candidates is non-empty above
        let selected = candidates.into_iter().next().unwrap();
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

            return Err(Error::NotFound(msg));
        }

        Self::select_best(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Repository, RepositoryPackage};
    use crate::db::schema;
    use rusqlite::Connection;

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

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn select_best_uses_debian_version_ordering() {
        let conn = test_db();

        let mut repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        repo.priority = 10;
        repo.insert(&conn).unwrap();
        let repository = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();
        let repo_id = repository.id.unwrap();

        let mut prerelease = RepositoryPackage::new(
            repo_id,
            "demo".to_string(),
            "1.0~beta1".to_string(),
            "sha256:beta".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/demo_1.0~beta1_amd64.deb".to_string(),
        );
        prerelease.architecture = Some("x86_64".to_string());
        prerelease.insert(&conn).unwrap();

        let mut stable = RepositoryPackage::new(
            repo_id,
            "demo".to_string(),
            "1.0".to_string(),
            "sha256:stable".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/demo_1.0_amd64.deb".to_string(),
        );
        stable.architecture = Some("x86_64".to_string());
        stable.insert(&conn).unwrap();

        let candidates = PackageSelector::search_packages(&conn, "demo", &SelectionOptions::default())
            .unwrap();
        let selected = PackageSelector::select_best(candidates).unwrap();

        assert_eq!(selected.package.version, "1.0");
    }
}
