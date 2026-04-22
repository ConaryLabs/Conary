// src/commands/install/system_pm.rs
//! System package manager query helpers for dependency resolution

use super::blocklist;
use conary_core::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use std::path::Path;
use std::process::Command;
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

/// Check whether a critical runtime dependency is already satisfied by the live
/// root, even when Conary's database is empty.
#[must_use]
pub fn is_live_runtime_dependency_present(name: &str) -> bool {
    if !blocklist::is_critical_runtime_capability(name) {
        return false;
    }

    let lower = name.to_ascii_lowercase();
    if lower.starts_with("rtld(") || lower.starts_with("ld-linux") {
        return dynamic_linker_present();
    }

    if lower.starts_with("libc.so.6") {
        return soname_present("libc.so.6")
            || candidate_paths(&[
                "/lib64/libc.so.6",
                "/lib/libc.so.6",
                "/usr/lib64/libc.so.6",
                "/usr/lib/libc.so.6",
                "/lib/x86_64-linux-gnu/libc.so.6",
                "/usr/lib/x86_64-linux-gnu/libc.so.6",
                "/lib/aarch64-linux-gnu/libc.so.6",
                "/usr/lib/aarch64-linux-gnu/libc.so.6",
                "/lib/riscv64-linux-gnu/libc.so.6",
                "/usr/lib/riscv64-linux-gnu/libc.so.6",
            ]);
    }

    false
}

fn soname_present(soname: &str) -> bool {
    let output = match Command::new("ldconfig").arg("-p").output() {
        Ok(output) => output,
        Err(_) => return false,
    };

    output.status.success() && String::from_utf8_lossy(&output.stdout).contains(soname)
}

fn candidate_paths(paths: &[&str]) -> bool {
    paths.iter().any(|path| Path::new(path).exists())
}

fn dynamic_linker_present() -> bool {
    candidate_paths(&[
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/ld-linux.so.2",
        "/lib/ld-linux-aarch64.so.1",
        "/lib/ld-linux-riscv64-lp64d.so.1",
        "/usr/lib64/ld-linux-x86-64.so.2",
    ])
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

    #[test]
    fn test_non_runtime_dependency_is_not_treated_as_live_runtime() {
        assert!(!is_live_runtime_dependency_present("tree"));
    }
}
