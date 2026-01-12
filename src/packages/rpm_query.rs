// src/packages/rpm_query.rs

//! Query installed RPM packages from the system database
//!
//! This module provides functions to query the local RPM database
//! using the `rpm` command-line tool.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::process::Command;
use tracing::{debug, warn};

/// Information about a file in an installed RPM package
#[derive(Debug, Clone)]
pub struct InstalledFileInfo {
    pub path: String,
    pub size: i64,
    pub mode: i32,
    pub mtime: Option<i64>,
    pub digest: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
}

/// Information about an installed RPM package
#[derive(Debug, Clone)]
pub struct InstalledRpmInfo {
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: Option<u64>,
    pub arch: String,
    pub description: Option<String>,
    pub summary: Option<String>,
    pub license: Option<String>,
    pub url: Option<String>,
    pub vendor: Option<String>,
    pub source_rpm: Option<String>,
    pub build_host: Option<String>,
    pub install_time: Option<String>,
}

impl InstalledRpmInfo {
    /// Get the full version string (epoch:version-release)
    pub fn full_version(&self) -> String {
        let mut v = String::new();
        if let Some(epoch) = self.epoch {
            if epoch > 0 {
                v.push_str(&format!("{}:", epoch));
            }
        }
        v.push_str(&self.version);
        if !self.release.is_empty() {
            v.push('-');
            v.push_str(&self.release);
        }
        v
    }

    /// Get version without release (epoch:version)
    pub fn version_only(&self) -> String {
        let mut v = String::new();
        if let Some(epoch) = self.epoch {
            if epoch > 0 {
                v.push_str(&format!("{}:", epoch));
            }
        }
        v.push_str(&self.version);
        v
    }
}

/// List all installed package names
pub fn list_installed_packages() -> Result<Vec<String>> {
    debug!("Querying installed RPM packages");

    let output = Command::new("rpm")
        .args(["-qa", "--queryformat", "%{NAME}\n"])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}. Is rpm installed?", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "rpm -qa failed: {}",
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
pub fn query_package(name: &str) -> Result<InstalledRpmInfo> {
    debug!("Querying package info: {}", name);

    // Query format: NAME|VERSION|RELEASE|EPOCH|ARCH|DESCRIPTION|SUMMARY|LICENSE|URL|VENDOR|SOURCERPM|BUILDHOST|INSTALLTIME
    let output = Command::new("rpm")
        .args([
            "-q",
            name,
            "--queryformat",
            "%{NAME}|%{VERSION}|%{RELEASE}|%{EPOCH}|%{ARCH}|%{DESCRIPTION}|%{SUMMARY}|%{LICENSE}|%{URL}|%{VENDOR}|%{SOURCERPM}|%{BUILDHOST}|%{INSTALLTIME}\n",
        ])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in RPM database",
            name
        )));
    }

    let line = String::from_utf8_lossy(&output.stdout);
    let line = line.trim();
    let parts: Vec<&str> = line.split('|').collect();

    if parts.len() < 8 {
        return Err(Error::InitError(format!(
            "Unexpected rpm output format: {}",
            line
        )));
    }

    let epoch = parts.get(3).and_then(|s| {
        if *s == "(none)" || s.is_empty() {
            None
        } else {
            s.parse().ok()
        }
    });

    Ok(InstalledRpmInfo {
        name: parts[0].to_string(),
        version: parts[1].to_string(),
        release: parts[2].to_string(),
        epoch,
        arch: parts.get(4).map_or("noarch".to_string(), |s| {
            if *s == "(none)" { "noarch".to_string() } else { s.to_string() }
        }),
        description: parts.get(5).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
        summary: parts.get(6).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
        license: parts.get(7).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
        url: parts.get(8).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
        vendor: parts.get(9).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
        source_rpm: parts.get(10).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
        build_host: parts.get(11).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
        install_time: parts.get(12).and_then(|s| {
            if *s == "(none)" { None } else { Some(s.to_string()) }
        }),
    })
}

/// Query files belonging to an installed package
pub fn query_package_files(name: &str) -> Result<Vec<InstalledFileInfo>> {
    debug!("Querying files for package: {}", name);

    // Use --dump format: path size mtime digest mode owner group ...
    let output = Command::new("rpm")
        .args(["-ql", "--dump", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in RPM database",
            name
        )));
    }

    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            continue;
        }

        // --dump format: path size mtime digest mode owner group isconfig isdoc rdev symlink
        let path = parts[0].to_string();
        let size = parts[1].parse().unwrap_or(0);
        let mtime = parts[2].parse().ok();
        let digest = if parts[3] == "0000000000000000000000000000000000000000000000000000000000000000"
            || parts[3] == "X"
        {
            None
        } else {
            Some(parts[3].to_string())
        };
        let mode = i32::from_str_radix(parts[4], 8).unwrap_or(0o644);
        let user = Some(parts[5].to_string());
        let group = Some(parts[6].to_string());

        files.push(InstalledFileInfo {
            path,
            size,
            mode,
            mtime,
            digest,
            user,
            group,
        });
    }

    debug!("Found {} files for package {}", files.len(), name);
    Ok(files)
}

/// Query dependencies of an installed package
pub fn query_package_dependencies(name: &str) -> Result<Vec<String>> {
    debug!("Querying dependencies for package: {}", name);

    let output = Command::new("rpm")
        .args(["-qR", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFoundError(format!(
            "Package '{}' not found in RPM database",
            name
        )));
    }

    let deps: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| {
            // Skip rpmlib deps and file paths
            !s.is_empty() && !s.starts_with("rpmlib(") && !s.starts_with('/')
        })
        .collect();

    debug!("Found {} dependencies for package {}", deps.len(), name);
    Ok(deps)
}

/// Query all installed packages with their basic info
/// Returns a map of package name -> InstalledRpmInfo
pub fn query_all_packages() -> Result<HashMap<String, InstalledRpmInfo>> {
    debug!("Querying all installed RPM packages with info");

    // Query format: NAME|VERSION|RELEASE|EPOCH|ARCH
    let output = Command::new("rpm")
        .args([
            "-qa",
            "--queryformat",
            "%{NAME}|%{VERSION}|%{RELEASE}|%{EPOCH}|%{ARCH}\n",
        ])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "rpm -qa failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let mut packages = HashMap::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 5 {
            warn!("Skipping malformed rpm output line: {}", line);
            continue;
        }

        let name = parts[0].to_string();
        let epoch = if parts[3] == "(none)" || parts[3].is_empty() {
            None
        } else {
            parts[3].parse().ok()
        };

        let info = InstalledRpmInfo {
            name: name.clone(),
            version: parts[1].to_string(),
            release: parts[2].to_string(),
            epoch,
            arch: if parts[4] == "(none)" {
                "noarch".to_string()
            } else {
                parts[4].to_string()
            },
            description: None,
            summary: None,
            license: None,
            url: None,
            vendor: None,
            source_rpm: None,
            build_host: None,
            install_time: None,
        };

        packages.insert(name, info);
    }

    debug!("Queried {} installed packages", packages.len());
    Ok(packages)
}

/// Check if RPM is available on this system
pub fn is_rpm_available() -> bool {
    Command::new("rpm")
        .args(["--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_rpm_available() {
        // This test just ensures the function runs without panic
        let _ = is_rpm_available();
    }

    #[test]
    fn test_installed_rpm_info_full_version() {
        let info = InstalledRpmInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            release: "1.fc43".to_string(),
            epoch: Some(2),
            arch: "x86_64".to_string(),
            description: None,
            summary: None,
            license: None,
            url: None,
            vendor: None,
            source_rpm: None,
            build_host: None,
            install_time: None,
        };

        assert_eq!(info.full_version(), "2:1.0.0-1.fc43");
        assert_eq!(info.version_only(), "2:1.0.0");
    }

    #[test]
    fn test_installed_rpm_info_no_epoch() {
        let info = InstalledRpmInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            release: "1.fc43".to_string(),
            epoch: None,
            arch: "x86_64".to_string(),
            description: None,
            summary: None,
            license: None,
            url: None,
            vendor: None,
            source_rpm: None,
            build_host: None,
            install_time: None,
        };

        assert_eq!(info.full_version(), "1.0.0-1.fc43");
        assert_eq!(info.version_only(), "1.0.0");
    }
}
