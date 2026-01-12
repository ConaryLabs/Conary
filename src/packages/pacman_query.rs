// src/packages/pacman_query.rs

//! Query installed pacman packages from the system database
//!
//! This module provides functions to query the local pacman database
//! using the `pacman` command-line tool for Arch Linux systems.

use crate::error::{Error, Result};
use crate::packages::rpm_query::DependencyInfo;
use std::collections::HashMap;
use std::process::Command;
use tracing::{debug, warn};

/// Information about a file in an installed pacman package
#[derive(Debug, Clone)]
pub struct InstalledFileInfo {
    pub path: String,
    pub size: i64,
    pub mode: i32,
    pub digest: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
    /// For symlinks, the target path
    pub link_target: Option<String>,
}

/// Information about an installed pacman package
#[derive(Debug, Clone)]
pub struct InstalledPacmanInfo {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub licenses: Option<String>,
    pub installed_size: Option<i64>,
}

impl InstalledPacmanInfo {
    /// Get the full version string
    pub fn full_version(&self) -> String {
        self.version.clone()
    }

    /// Get version without release (same as full_version for pacman)
    pub fn version_only(&self) -> String {
        self.version.clone()
    }
}

/// List all installed package names
pub fn list_installed_packages() -> Result<Vec<String>> {
    debug!("Querying installed pacman packages");

    let output = Command::new("pacman")
        .args(["-Qq"])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}. Is pacman installed?", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "pacman -Qq failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let packages: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    debug!("Found {} installed packages", packages.len());
    Ok(packages)
}

/// Query detailed information about an installed package
pub fn query_package(name: &str) -> Result<InstalledPacmanInfo> {
    debug!("Querying package info: {}", name);

    let output = Command::new("pacman")
        .args(["-Qi", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in pacman database",
            name
        )));
    }

    let info_str = String::from_utf8_lossy(&output.stdout);
    let mut pkg_name = name.to_string();
    let mut version = String::new();
    let mut arch = String::new();
    let mut description = None;
    let mut url = None;
    let mut licenses = None;
    let mut installed_size = None;

    for line in info_str.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();

            match key {
                "Name" => pkg_name = value.to_string(),
                "Version" => version = value.to_string(),
                "Architecture" => arch = value.to_string(),
                "Description" => description = Some(value.to_string()),
                "URL" => url = Some(value.to_string()),
                "Licenses" => licenses = Some(value.to_string()),
                "Installed Size" => {
                    // Parse size like "1.5 MiB" or "100 KiB"
                    installed_size = parse_size(value);
                }
                _ => {}
            }
        }
    }

    Ok(InstalledPacmanInfo {
        name: pkg_name,
        version,
        arch,
        description,
        url,
        licenses,
        installed_size,
    })
}

/// Parse pacman size string (e.g., "1.5 MiB") to bytes
fn parse_size(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }

    let num: f64 = parts[0].parse().ok()?;
    let multiplier = match parts[1] {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };

    Some((num * multiplier) as i64)
}

/// Query files installed by a package
pub fn query_package_files(name: &str) -> Result<Vec<InstalledFileInfo>> {
    debug!("Querying files for package: {}", name);

    let output = Command::new("pacman")
        .args(["-Ql", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in pacman database",
            name
        )));
    }

    let mut files = Vec::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // Format: "package_name /path/to/file"
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() != 2 {
            continue;
        }

        let path = parts[1].trim().to_string();
        if path.is_empty() || path.ends_with('/') {
            // Skip directories
            continue;
        }

        // Get file metadata
        let (size, mode) = get_file_metadata(&path);

        // Try to get mtree digest
        let digest = get_file_digest(name, &path);

        // Check if this is a symlink and get target
        let link_target = if (mode & 0o170000) == 0o120000 {
            std::fs::read_link(&path).ok().map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };

        files.push(InstalledFileInfo {
            path,
            size,
            mode,
            digest,
            user: None,
            group: None,
            link_target,
        });
    }

    debug!("Found {} files for package {}", files.len(), name);
    Ok(files)
}

/// Get file metadata (size and mode)
fn get_file_metadata(path: &str) -> (i64, i32) {
    use std::os::unix::fs::MetadataExt;

    match std::fs::metadata(path) {
        Ok(meta) => (meta.len() as i64, meta.mode() as i32),
        Err(_) => (0, 0o644),
    }
}

/// Get file digest from pacman mtree database
fn get_file_digest(package: &str, path: &str) -> Option<String> {
    // Pacman stores mtree files in /var/lib/pacman/local/<package>-<version>/mtree
    // This is complex to parse, so for now return None
    // A full implementation would decompress and parse the mtree file
    let _ = (package, path);
    None
}

/// Query dependencies of an installed package (names only)
pub fn query_package_dependencies(name: &str) -> Result<Vec<String>> {
    debug!("Querying dependencies for package: {}", name);

    let output = Command::new("pacman")
        .args(["-Qi", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in pacman database",
            name
        )));
    }

    let info_str = String::from_utf8_lossy(&output.stdout);
    let mut deps = Vec::new();

    for line in info_str.lines() {
        if let Some((key, value)) = line.split_once(':')
            && key.trim() == "Depends On"
        {
            // Parse dependencies (space-separated, may include version constraints)
            deps = value
                .split_whitespace()
                .filter(|s| *s != "None")
                .map(|s| {
                    // Remove version constraints like ">=1.0"
                    s.split(['>', '<', '='])
                        .next()
                        .unwrap_or(s)
                        .to_string()
                })
                .collect();
            break;
        }
    }

    debug!("Found {} dependencies for package {}", deps.len(), name);
    Ok(deps)
}

/// Query dependencies of an installed package with full version constraints
pub fn query_package_dependencies_full(name: &str) -> Result<Vec<DependencyInfo>> {
    debug!("Querying dependencies with constraints for package: {}", name);

    let output = Command::new("pacman")
        .args(["-Qi", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in pacman database",
            name
        )));
    }

    let info_str = String::from_utf8_lossy(&output.stdout);
    let mut deps = Vec::new();

    for line in info_str.lines() {
        if let Some((key, value)) = line.split_once(':')
            && key.trim() == "Depends On"
        {
            // Parse dependencies (space-separated, may include version constraints)
            deps = value
                .split_whitespace()
                .filter(|s| *s != "None")
                .map(parse_pacman_dependency)
                .collect();
            break;
        }
    }

    debug!(
        "Found {} dependencies with constraints for package {}",
        deps.len(),
        name
    );
    Ok(deps)
}

/// Parse a pacman dependency string like "package>=1.0" into DependencyInfo
fn parse_pacman_dependency(dep: &str) -> DependencyInfo {
    // Pacman dependency format: "package[op version]" (no spaces)
    // Examples: "glibc>=2.17", "bash", "perl>5.10"
    if let Some(pos) = dep.find(['>', '<', '=']) {
        let name = dep[..pos].to_string();
        let constraint = dep[pos..].to_string();
        DependencyInfo {
            name,
            constraint: if constraint.is_empty() {
                None
            } else {
                Some(constraint)
            },
        }
    } else {
        DependencyInfo {
            name: dep.to_string(),
            constraint: None,
        }
    }
}

/// Query all installed packages with their basic info
/// Returns a map of package name -> InstalledPacmanInfo
pub fn query_all_packages() -> Result<HashMap<String, InstalledPacmanInfo>> {
    debug!("Querying all installed pacman packages with info");

    // Use pacman -Q to get all packages with versions
    let output = Command::new("pacman")
        .args(["-Q"])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "pacman -Q failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let mut packages = HashMap::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // Format: "package_name version"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            warn!("Skipping malformed pacman output line: {}", line);
            continue;
        }

        let name = parts[0].to_string();
        let version = parts[1].to_string();

        let info = InstalledPacmanInfo {
            name: name.clone(),
            version,
            arch: "x86_64".to_string(), // Pacman -Q doesn't show arch, assume native
            description: None,
            url: None,
            licenses: None,
            installed_size: None,
        };

        packages.insert(name, info);
    }

    debug!("Queried {} installed packages", packages.len());
    Ok(packages)
}

/// Query which package(s) own a file
pub fn query_file_owner(path: &str) -> Result<Vec<String>> {
    let output = Command::new("pacman")
        .args(["-Qo", path])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        // File not owned by any package
        return Ok(Vec::new());
    }

    // Output format: "/path/to/file is owned by package_name version"
    let output_str = String::from_utf8_lossy(&output.stdout);
    let owners: Vec<String> = output_str
        .lines()
        .filter_map(|line| {
            if let Some(pos) = line.find(" is owned by ") {
                let rest = &line[pos + " is owned by ".len()..];
                rest.split_whitespace().next().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(owners)
}

/// Check if pacman is available on this system
pub fn is_pacman_available() -> bool {
    Command::new("pacman")
        .args(["--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pacman_available() {
        // This test just ensures the function runs without panic
        let _ = is_pacman_available();
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("100 B"), Some(100));
        assert_eq!(parse_size("1 KiB"), Some(1024));
        assert_eq!(parse_size("1.5 MiB"), Some(1572864));
        assert_eq!(parse_size("invalid"), None);
    }

    #[test]
    fn test_installed_pacman_info_version() {
        let info = InstalledPacmanInfo {
            name: "test".to_string(),
            version: "1.0.0-1".to_string(),
            arch: "x86_64".to_string(),
            description: None,
            url: None,
            licenses: None,
            installed_size: None,
        };

        assert_eq!(info.full_version(), "1.0.0-1");
        assert_eq!(info.version_only(), "1.0.0-1");
    }
}
