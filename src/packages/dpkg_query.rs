// src/packages/dpkg_query.rs

//! Query installed dpkg packages from the system database
//!
//! This module provides functions to query the local dpkg database
//! using the `dpkg-query` command-line tool.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::process::Command;
use tracing::{debug, warn};

/// Information about a file in an installed dpkg package
#[derive(Debug, Clone)]
pub struct InstalledFileInfo {
    pub path: String,
    pub size: i64,
    pub mode: i32,
    pub digest: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
}

/// Information about an installed dpkg package
#[derive(Debug, Clone)]
pub struct InstalledDpkgInfo {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub description: Option<String>,
    pub maintainer: Option<String>,
    pub homepage: Option<String>,
    pub section: Option<String>,
    pub priority: Option<String>,
    pub installed_size: Option<i64>,
}

impl InstalledDpkgInfo {
    /// Get the full version string
    pub fn full_version(&self) -> String {
        self.version.clone()
    }

    /// Get version without release (same as full_version for dpkg)
    pub fn version_only(&self) -> String {
        // Dpkg versions don't have the same epoch:version-release structure as RPM
        // but they can have epoch:upstream-debian format
        // For simplicity, return the full version
        self.version.clone()
    }
}

/// List all installed package names
pub fn list_installed_packages() -> Result<Vec<String>> {
    debug!("Querying installed dpkg packages");

    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Package}\n"])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}. Is dpkg installed?", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "dpkg-query failed: {}",
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
pub fn query_package(name: &str) -> Result<InstalledDpkgInfo> {
    debug!("Querying package info: {}", name);

    // Query format: Package|Version|Architecture|Description|Maintainer|Homepage|Section|Priority|Installed-Size
    let output = Command::new("dpkg-query")
        .args([
            "-W",
            "-f",
            "${Package}|${Version}|${Architecture}|${Description}|${Maintainer}|${Homepage}|${Section}|${Priority}|${Installed-Size}\n",
            name,
        ])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in dpkg database",
            name
        )));
    }

    let line = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = line.trim().split('|').collect();

    if parts.len() < 3 {
        return Err(Error::InitError(format!(
            "Malformed dpkg-query output for {}",
            name
        )));
    }

    let installed_size = parts.get(8).and_then(|s| s.parse().ok());

    Ok(InstalledDpkgInfo {
        name: parts[0].to_string(),
        version: parts[1].to_string(),
        arch: parts[2].to_string(),
        description: parts.get(3).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        maintainer: parts.get(4).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        homepage: parts.get(5).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        section: parts.get(6).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        priority: parts.get(7).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        installed_size,
    })
}

/// Query files installed by a package
pub fn query_package_files(name: &str) -> Result<Vec<InstalledFileInfo>> {
    debug!("Querying files for package: {}", name);

    // Use dpkg -L to list files
    let output = Command::new("dpkg")
        .args(["-L", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in dpkg database",
            name
        )));
    }

    let mut files = Vec::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let path = line.trim().to_string();
        if path.is_empty() {
            continue;
        }

        // Get file metadata
        let (size, mode) = get_file_metadata(&path);

        // Try to get md5sum from dpkg database
        let digest = get_file_digest(name, &path);

        files.push(InstalledFileInfo {
            path,
            size,
            mode,
            digest,
            user: None,
            group: None,
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

/// Get file digest from dpkg database
fn get_file_digest(package: &str, path: &str) -> Option<String> {
    // dpkg stores md5sums in /var/lib/dpkg/info/<package>.md5sums
    let md5sums_path = format!("/var/lib/dpkg/info/{}.md5sums", package);

    if let Ok(content) = std::fs::read_to_string(&md5sums_path) {
        // Format: <md5sum>  <path>
        // Note: path in md5sums file doesn't have leading /
        let search_path = path.strip_prefix('/').unwrap_or(path);
        for line in content.lines() {
            let parts: Vec<&str> = line.splitn(2, "  ").collect();
            if parts.len() == 2 && parts[1] == search_path {
                return Some(parts[0].to_string());
            }
        }
    }

    // Try with architecture suffix
    let arch_suffixes = ["amd64", "i386", "arm64", "armhf", "all"];
    for arch in &arch_suffixes {
        let md5sums_path = format!("/var/lib/dpkg/info/{}:{}.md5sums", package, arch);
        if let Ok(content) = std::fs::read_to_string(&md5sums_path) {
            let search_path = path.strip_prefix('/').unwrap_or(path);
            for line in content.lines() {
                let parts: Vec<&str> = line.splitn(2, "  ").collect();
                if parts.len() == 2 && parts[1] == search_path {
                    return Some(parts[0].to_string());
                }
            }
        }
    }

    None
}

/// Query dependencies of an installed package
pub fn query_package_dependencies(name: &str) -> Result<Vec<String>> {
    debug!("Querying dependencies for package: {}", name);

    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Depends}\n", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in dpkg database",
            name
        )));
    }

    let deps_str = String::from_utf8_lossy(&output.stdout);
    let deps: Vec<String> = deps_str
        .split(',')
        .flat_map(|dep| dep.split('|')) // Handle alternatives (a | b)
        .map(|s| {
            // Remove version constraints: "package (>= 1.0)" -> "package"
            s.split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();

    debug!("Found {} dependencies for package {}", deps.len(), name);
    Ok(deps)
}

/// Query all installed packages with their basic info
/// Returns a map of package name -> InstalledDpkgInfo
pub fn query_all_packages() -> Result<HashMap<String, InstalledDpkgInfo>> {
    debug!("Querying all installed dpkg packages with info");

    // Query format: Package|Version|Architecture
    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Package}|${Version}|${Architecture}\n"])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "dpkg-query failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let mut packages = HashMap::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 3 {
            warn!("Skipping malformed dpkg-query output line: {}", line);
            continue;
        }

        let name = parts[0].to_string();
        let info = InstalledDpkgInfo {
            name: name.clone(),
            version: parts[1].to_string(),
            arch: parts[2].to_string(),
            description: None,
            maintainer: None,
            homepage: None,
            section: None,
            priority: None,
            installed_size: None,
        };

        packages.insert(name, info);
    }

    debug!("Queried {} installed packages", packages.len());
    Ok(packages)
}

/// Query which package(s) own a file
pub fn query_file_owner(path: &str) -> Result<Vec<String>> {
    let output = Command::new("dpkg")
        .args(["-S", path])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg: {}", e)))?;

    if !output.status.success() {
        // File not owned by any package
        return Ok(Vec::new());
    }

    // Output format: "package: /path/to/file" or "package1, package2: /path"
    let owners: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            line.split(':').next().map(|pkgs| {
                pkgs.split(',')
                    .map(|s| s.trim().to_string())
                    .collect::<Vec<_>>()
            })
        })
        .flatten()
        .filter(|s| !s.is_empty() && !s.contains("diversion"))
        .collect();

    Ok(owners)
}

/// Check if dpkg is available on this system
pub fn is_dpkg_available() -> bool {
    Command::new("dpkg-query")
        .args(["--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_dpkg_available() {
        // This test just ensures the function runs without panic
        let _ = is_dpkg_available();
    }

    #[test]
    fn test_installed_dpkg_info_version() {
        let info = InstalledDpkgInfo {
            name: "test".to_string(),
            version: "1.0.0-1ubuntu1".to_string(),
            arch: "amd64".to_string(),
            description: None,
            maintainer: None,
            homepage: None,
            section: None,
            priority: None,
            installed_size: None,
        };

        assert_eq!(info.full_version(), "1.0.0-1ubuntu1");
        assert_eq!(info.version_only(), "1.0.0-1ubuntu1");
    }
}
