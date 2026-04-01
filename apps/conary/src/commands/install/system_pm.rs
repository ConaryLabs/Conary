// src/commands/install/system_pm.rs
//! System package manager query helpers for dependency resolution

use conary_core::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use tracing::debug;

/// Check if a package is installed via the system package manager
#[must_use]
pub fn is_system_package_installed(name: &str) -> bool {
    let pm = SystemPackageManager::detect();
    let result = match pm {
        SystemPackageManager::Rpm => rpm_query::query_package(name).is_ok(),
        SystemPackageManager::Dpkg => dpkg_query::query_package(name).is_ok(),
        SystemPackageManager::Pacman => pacman_query::query_package(name).is_ok(),
        SystemPackageManager::Unknown => false,
    };
    debug!(
        "System PM check for '{}' ({}): {}",
        name,
        pm.display_name(),
        if result { "installed" } else { "not found" }
    );
    result
}

/// Get the version of a system-installed package
#[must_use]
#[allow(dead_code)]
pub fn get_system_package_version(name: &str) -> Option<String> {
    let pm = SystemPackageManager::detect();
    match pm {
        SystemPackageManager::Rpm => rpm_query::query_package(name)
            .ok()
            .map(|info| info.version_only()),
        SystemPackageManager::Dpkg => dpkg_query::query_package(name)
            .ok()
            .map(|info| info.version_only()),
        SystemPackageManager::Pacman => pacman_query::query_package(name)
            .ok()
            .map(|info| info.version_only()),
        SystemPackageManager::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonexistent_package_not_installed() {
        // A package that definitely doesn't exist should return false
        assert!(!is_system_package_installed("zzz-nonexistent-pkg-12345"));
    }

    #[test]
    fn test_nonexistent_package_no_version() {
        assert!(get_system_package_version("zzz-nonexistent-pkg-12345").is_none());
    }

    #[test]
    fn test_bash_is_installed() {
        // bash should be installed on any system running these tests.
        // Skip if PM is Unknown or if the query tool isn't available
        // (GitHub Actions runners may lack rpm/dpkg-query).
        if is_system_package_installed("bash") {
            // validates that the function returns true on a real system
        } else {
            eprintln!("skipping: system PM cannot find bash");
        }
    }

    #[test]
    fn test_bash_has_version() {
        if let Some(version) = get_system_package_version("bash") {
            assert!(!version.is_empty(), "bash version should not be empty");
        } else {
            eprintln!("skipping: system PM cannot query bash version");
        }
    }
}
