// src/commands/install/system_pm.rs
//! System package manager query helpers for dependency resolution

use conary::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use tracing::debug;

/// Check if a package is installed via the system package manager
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
        // bash should be installed on any system running these tests
        // Skip if PM is Unknown (e.g. in a minimal CI container)
        let pm = SystemPackageManager::detect();
        if matches!(pm, SystemPackageManager::Unknown) {
            return;
        }
        assert!(is_system_package_installed("bash"));
    }

    #[test]
    fn test_bash_has_version() {
        let pm = SystemPackageManager::detect();
        if matches!(pm, SystemPackageManager::Unknown) {
            return;
        }
        let version = get_system_package_version("bash");
        assert!(version.is_some(), "bash should have a version");
        assert!(!version.unwrap().is_empty(), "bash version should not be empty");
    }
}
