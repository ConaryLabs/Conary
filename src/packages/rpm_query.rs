// src/packages/rpm_query.rs

//! Query installed RPM packages from the system database
//!
//! This module provides functions to query the local RPM database
//! using the `rpm` command-line tool.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::process::Command;
use tracing::{debug, warn};

/// Dependency with version constraint
#[derive(Debug, Clone)]
pub struct DependencyInfo {
    pub name: String,
    pub constraint: Option<String>, // e.g., ">= 1.0", "< 2.0"
}

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
    /// For symlinks, the target path
    pub link_target: Option<String>,
}

impl InstalledFileInfo {
    /// Check if this file is a symlink (mode & S_IFMT == S_IFLNK)
    pub fn is_symlink(&self) -> bool {
        // S_IFLNK = 0o120000 = 0xA000
        (self.mode & 0o170000) == 0o120000
    }

    /// Check if this file is a directory (mode & S_IFMT == S_IFDIR)
    pub fn is_directory(&self) -> bool {
        // S_IFDIR = 0o040000
        (self.mode & 0o170000) == 0o040000
    }

    /// Check if this file is a regular file (mode & S_IFMT == S_IFREG)
    pub fn is_regular_file(&self) -> bool {
        // S_IFREG = 0o100000
        (self.mode & 0o170000) == 0o100000
    }
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
        if let Some(epoch) = self.epoch
            && epoch > 0
        {
            v.push_str(&format!("{epoch}:"));
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
        if let Some(epoch) = self.epoch
            && epoch > 0
        {
            v.push_str(&format!("{epoch}:"));
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
        if let Some(info) = parse_rpm_dump_line(line) {
            files.push(info);
        }
    }

    debug!("Found {} files for package {}", files.len(), name);
    Ok(files)
}

/// Parse a single line from `rpm --dump` output
///
/// Format: path size mtime digest mode owner group isconfig isdoc rdev symlink_target
///
/// The challenge is that `path` and `symlink_target` can contain spaces.
/// We find the digest (64 hex chars) as an anchor point and parse around it.
fn parse_rpm_dump_line(line: &str) -> Option<InstalledFileInfo> {
    // Find the digest field - it's always 64 hex characters
    // We scan for " " + 64 hex chars + " "
    let (digest_start, digest_end) = find_digest_position(line)?;

    // Everything before the digest pattern has: path + size + mtime
    let before_digest = &line[..digest_start];

    // Parse backwards from before_digest to find size and mtime
    let before_parts: Vec<&str> = before_digest.rsplitn(3, ' ').collect();
    if before_parts.len() < 3 {
        return None;
    }

    // rsplitn reverses: [mtime, size, path_with_possible_spaces]
    let mtime_str = before_parts[0];
    let size_str = before_parts[1];
    let path = before_parts[2].to_string();

    let size: i64 = size_str.parse().ok()?;
    let mtime: Option<i64> = mtime_str.parse().ok();

    // Get the digest
    let digest_str = &line[digest_start + 1..digest_end]; // +1 to skip leading space
    let digest = if digest_str == "0000000000000000000000000000000000000000000000000000000000000000" {
        None
    } else {
        Some(digest_str.to_string())
    };

    // Everything after the digest: mode owner group isconfig isdoc rdev symlink_target
    let after_digest = &line[digest_end + 1..]; // +1 for the space after digest
    let after_parts: Vec<&str> = after_digest.splitn(7, ' ').collect();

    if after_parts.len() < 6 {
        return None;
    }

    let mode = i32::from_str_radix(after_parts[0], 8).unwrap_or(0o644);
    let user = Some(after_parts[1].to_string());
    let group = Some(after_parts[2].to_string());
    // isconfig = after_parts[3], isdoc = after_parts[4], rdev = after_parts[5]

    // Symlink target is everything after rdev (field 6 onwards, joined back)
    let link_target = if after_parts.len() > 6 {
        let target = after_parts[6..].join(" ");
        if target == "X" || target.is_empty() {
            None
        } else {
            Some(target)
        }
    } else {
        None
    };

    Some(InstalledFileInfo {
        path,
        size,
        mode,
        mtime,
        digest,
        user,
        group,
        link_target,
    })
}

/// Find the position of the 64-character hex digest in an RPM dump line
///
/// Returns (start, end) positions where start is the space before the digest
/// and end is the last character of the digest (before the trailing space).
fn find_digest_position(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let len = bytes.len();

    // We need at least " " + 64 chars + " " = 66 characters
    if len < 66 {
        return None;
    }

    // Scan for a 64-character hex string surrounded by spaces
    // Start from a reasonable position (after at least a path character)
    for i in 1..len.saturating_sub(65) {
        // Check if we have space + 64 hex chars + space
        if bytes[i] == b' ' && i + 65 < len && bytes[i + 65] == b' ' {
            // Verify all 64 characters are hex digits
            let potential_digest = &line[i + 1..i + 65];
            if potential_digest.len() == 64
                && potential_digest.chars().all(|c| c.is_ascii_hexdigit())
            {
                return Some((i, i + 65));
            }
        }
    }

    None
}

/// Query dependencies of an installed package (names only, for backwards compatibility)
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
        .map(|s| {
            // Extract just the name, stripping any version constraint
            s.split_whitespace().next().unwrap_or(&s).to_string()
        })
        .collect();

    debug!("Found {} dependencies for package {}", deps.len(), name);
    Ok(deps)
}

/// Query dependencies of an installed package with full version constraints
pub fn query_package_dependencies_full(name: &str) -> Result<Vec<DependencyInfo>> {
    debug!("Querying dependencies with constraints for package: {}", name);

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

    let deps: Vec<DependencyInfo> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim())
        .filter(|s| {
            // Skip rpmlib deps and file paths
            !s.is_empty() && !s.starts_with("rpmlib(") && !s.starts_with('/')
        })
        .map(parse_rpm_dependency)
        .collect();

    debug!(
        "Found {} dependencies with constraints for package {}",
        deps.len(),
        name
    );
    Ok(deps)
}

/// Parse an RPM dependency string like "filesystem >= 3.6-1" into DependencyInfo
fn parse_rpm_dependency(dep: &str) -> DependencyInfo {
    // RPM dependency format: "name [op version]"
    // Examples: "filesystem >= 3.6-1", "perl(Cwd)", "bash"
    let parts: Vec<&str> = dep.splitn(2, ['>', '<', '=']).collect();

    if parts.len() == 1 {
        // No constraint, just a name
        DependencyInfo {
            name: dep.trim().to_string(),
            constraint: None,
        }
    } else {
        let name = parts[0].trim().to_string();
        // Find where the operator starts
        let name_len = name.len();
        let constraint = dep[name_len..].trim().to_string();
        DependencyInfo {
            name,
            constraint: if constraint.is_empty() {
                None
            } else {
                Some(constraint)
            },
        }
    }
}

/// Query what a package provides (capabilities it offers)
///
/// Returns a list of capability strings like:
/// - "perl(Text::CharWidth)" (virtual provide)
/// - "libc.so.6(GLIBC_2.17)(64bit)" (library)
/// - "/usr/bin/perl" (file path)
/// - "perl-Text-CharWidth = 0.04-58.fc43" (package name = version)
pub fn query_package_provides(name: &str) -> Result<Vec<String>> {
    debug!("Querying provides for RPM package: {}", name);

    let output = Command::new("rpm")
        .args(["-q", "--provides", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "rpm -q --provides {} failed: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let provides: Vec<String> = stdout
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    debug!("Package {} provides {} capabilities", name, provides.len());
    Ok(provides)
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

/// Query which package(s) own a file
pub fn query_file_owner(path: &str) -> Result<Vec<String>> {
    let output = Command::new("rpm")
        .args(["-qf", "--queryformat", "%{NAME}\n", path])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        // File not owned by any package
        return Ok(Vec::new());
    }

    let owners: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains("not owned"))
        .collect();

    Ok(owners)
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

    #[test]
    fn test_parse_rpm_dependency_with_constraint() {
        let dep = parse_rpm_dependency("filesystem >= 3.6-1");
        assert_eq!(dep.name, "filesystem");
        assert_eq!(dep.constraint, Some(">= 3.6-1".to_string()));
    }

    #[test]
    fn test_parse_rpm_dependency_without_constraint() {
        let dep = parse_rpm_dependency("perl(Cwd)");
        assert_eq!(dep.name, "perl(Cwd)");
        assert_eq!(dep.constraint, None);
    }

    #[test]
    fn test_parse_rpm_dependency_less_than() {
        let dep = parse_rpm_dependency("bash < 5.0");
        assert_eq!(dep.name, "bash");
        assert_eq!(dep.constraint, Some("< 5.0".to_string()));
    }

    #[test]
    fn test_parse_rpm_dependency_exact() {
        let dep = parse_rpm_dependency("glibc = 2.38");
        assert_eq!(dep.name, "glibc");
        assert_eq!(dep.constraint, Some("= 2.38".to_string()));
    }
}
