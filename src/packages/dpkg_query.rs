// src/packages/dpkg_query.rs

//! Query installed dpkg packages from the system database
//!
//! This module provides functions to query the local dpkg database
//! using the `dpkg-query` command-line tool.

use crate::error::{Error, Result};
use crate::packages::rpm_query::DependencyInfo;
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
    /// For symlinks, the target path
    pub link_target: Option<String>,
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
        .map_err(|e| {
            Error::InitError(format!(
                "Failed to run dpkg-query: {}. Is dpkg installed?",
                e
            ))
        })?;

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
        return Err(Error::NotFound(format!(
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
        description: parts
            .get(3)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        maintainer: parts
            .get(4)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        homepage: parts
            .get(5)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        section: parts
            .get(6)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        priority: parts
            .get(7)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
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
        return Err(Error::NotFound(format!(
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

        // Check if this is a symlink and get target
        let link_target = if (mode & 0o170000) == 0o120000 {
            std::fs::read_link(&path)
                .ok()
                .map(|p| p.to_string_lossy().to_string())
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

/// Query dependencies of an installed package (names only)
pub fn query_package_dependencies(name: &str) -> Result<Vec<String>> {
    debug!("Querying dependencies for package: {}", name);

    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Depends}\n", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
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
            s.split_whitespace().next().unwrap_or("").to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();

    debug!("Found {} dependencies for package {}", deps.len(), name);
    Ok(deps)
}

/// Query dependencies of an installed package with full version constraints
pub fn query_package_dependencies_full(name: &str) -> Result<Vec<DependencyInfo>> {
    debug!(
        "Querying dependencies with constraints for package: {}",
        name
    );

    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Depends}\n", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{}' not found in dpkg database",
            name
        )));
    }

    let deps_str = String::from_utf8_lossy(&output.stdout);
    let deps: Vec<DependencyInfo> = deps_str
        .split(',')
        .flat_map(|dep| dep.split('|')) // Handle alternatives (a | b)
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            Some(parse_dpkg_dependency(s))
        })
        .collect();

    debug!(
        "Found {} dependencies with constraints for package {}",
        deps.len(),
        name
    );
    Ok(deps)
}

/// Parse a dpkg dependency string like "package (>= 1.0)" into DependencyInfo
fn parse_dpkg_dependency(dep: &str) -> DependencyInfo {
    // Dpkg dependency format: "package [(op version)]"
    // Examples: "libc6 (>= 2.17)", "bash", "perl (>> 5.10)"
    if let Some(paren_start) = dep.find('(') {
        let name = dep[..paren_start].trim().to_string();
        let constraint = dep[paren_start..]
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim()
            .to_string();
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
            name: dep.trim().to_string(),
            constraint: None,
        }
    }
}

/// Query what a package provides (capabilities it offers)
///
/// Returns a list of capability strings from the Provides field,
/// plus the package name itself as a provide.
pub fn query_package_provides(name: &str) -> Result<Vec<String>> {
    debug!("Querying provides for dpkg package: {}", name);

    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Package}\n${Provides}\n", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "dpkg-query for provides {} failed: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut provides = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // First line is the package name itself
        // Subsequent lines are from Provides field (comma-separated)
        if line.contains(',') {
            // Provides field: "foo, bar, baz"
            for part in line.split(',') {
                let provide = part.trim();
                if !provide.is_empty() {
                    provides.push(provide.to_string());
                }
            }
        } else {
            provides.push(line.to_string());
        }
    }

    debug!("Package {} provides {} capabilities", name, provides.len());
    Ok(provides)
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

/// Query the set of package names explicitly installed by the user (not auto-deps).
///
/// Reads `/var/lib/apt/extended_states` which apt maintains with `Auto-Installed: 1`
/// entries for dependency-installed packages. Any package not marked auto-installed
/// is considered user-installed. If the file is absent (pure dpkg without apt, or
/// insufficient permissions) all packages are treated as user-installed.
pub fn query_user_installed() -> Result<std::collections::HashSet<String>> {
    debug!("Querying user-installed dpkg packages via apt extended_states");

    const EXTENDED_STATES: &str = "/var/lib/apt/extended_states";

    let content = match std::fs::read_to_string(EXTENDED_STATES) {
        Ok(c) => c,
        Err(e) => {
            warn!(
                "Could not read {}: {} — treating all packages as user-installed",
                EXTENDED_STATES, e
            );
            // Fall back: return all installed packages as user-installed
            return list_installed_packages().map(|pkgs| pkgs.into_iter().collect());
        }
    };

    // The file consists of stanzas separated by blank lines:
    //   Package: foo
    //   Auto-Installed: 1
    //
    //   Package: bar
    //   Auto-Installed: 0
    //
    // Collect packages whose Auto-Installed field is 1.
    let mut auto_installed = std::collections::HashSet::new();
    let mut current_pkg: Option<String> = None;

    for line in content.lines() {
        if line.is_empty() {
            current_pkg = None;
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "Package" => {
                    current_pkg = Some(value.trim().to_string());
                }
                "Auto-Installed" if value.trim() == "1" => {
                    if let Some(pkg) = current_pkg.take() {
                        auto_installed.insert(pkg);
                    }
                }
                _ => {}
            }
        }
    }

    // User-installed = all installed minus auto-installed
    let all_installed: std::collections::HashSet<String> =
        list_installed_packages()?.into_iter().collect();
    let user_installed = all_installed
        .into_iter()
        .filter(|pkg| !auto_installed.contains(pkg))
        .collect();

    debug!(
        "Found {} auto-installed dpkg packages; remainder are user-installed",
        auto_installed.len()
    );
    Ok(user_installed)
}

/// RAII guard for a dpkg fcntl lock. Lock is released on drop.
struct DpkgLockGuard {
    _file: std::fs::File,
}

/// Acquire a POSIX fcntl write lock on a dpkg lock file.
///
/// dpkg uses `fcntl(F_SETLK)` record locks (not `flock`), which is what
/// apt and dpkg check for mutual exclusion. Using the wrong lock type
/// would allow concurrent access.
fn acquire_dpkg_lock(path: &str) -> Result<DpkgLockGuard> {
    use std::os::unix::io::AsRawFd;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| Error::InitError(format!("Failed to open dpkg lock {}: {}", path, e)))?;

    let mut flock = libc::flock {
        l_type: libc::F_WRLCK as i16,
        l_whence: libc::SEEK_SET as i16,
        l_start: 0,
        l_len: 0, // entire file
        l_pid: 0,
    };

    let ret = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &mut flock) };
    if ret == -1 {
        return Err(Error::InitError(format!(
            "dpkg database is locked by another process ({}). Wait for apt/dpkg to finish.",
            path
        )));
    }

    Ok(DpkgLockGuard { _file: file })
}

/// Remove a package from the dpkg database only (no files deleted).
///
/// Edits `/var/lib/dpkg/status` to remove the package stanza and also
/// removes the `/var/lib/dpkg/info/<name>.*` files. This transfers
/// ownership of the files from dpkg to Conary.
///
/// Follows the dpkg frontend locking protocol (fcntl record locks on
/// lock-frontend then lock) per /usr/share/doc/dpkg/spec/frontend-api.txt,
/// and uses atomic rename to prevent corruption from crashes.
pub fn remove_from_db_only(name: &str) -> Result<()> {
    use std::io::Write;

    debug!("Removing {} from dpkg database only", name);

    let status_path = "/var/lib/dpkg/status";

    // Acquire dpkg locks per the frontend protocol spec:
    // 1. lock-frontend (frontend mutex — excludes apt, aptitude, etc.)
    // 2. lock (dpkg database lock — excludes dpkg itself)
    // Both use POSIX fcntl F_SETLK write locks as dpkg expects.
    let _frontend_lock = acquire_dpkg_lock("/var/lib/dpkg/lock-frontend")?;
    let _db_lock = acquire_dpkg_lock("/var/lib/dpkg/lock")?;

    // Read /var/lib/dpkg/status, filter out the target package stanza
    let content = std::fs::read_to_string(status_path)
        .map_err(|e| Error::InitError(format!("Failed to read {}: {}", status_path, e)))?;

    let mut output_lines = Vec::new();
    let mut in_target_stanza = false;

    for line in content.lines() {
        if line.starts_with("Package: ") {
            let pkg = line.strip_prefix("Package: ").unwrap_or("").trim();
            in_target_stanza = pkg == name;
        } else if line.is_empty() && in_target_stanza {
            in_target_stanza = false;
            continue; // Skip the blank line after the removed stanza
        }

        if !in_target_stanza {
            output_lines.push(line);
        }
    }

    // Atomic write: write to temp file in same directory, then rename
    let new_content = output_lines.join("\n") + "\n";
    let tmp_path = format!("{}.conary-tmp", status_path);

    let mut tmp_file = std::fs::File::create(&tmp_path)
        .map_err(|e| Error::InitError(format!("Failed to create temp file {}: {}", tmp_path, e)))?;
    tmp_file
        .write_all(new_content.as_bytes())
        .map_err(|e| Error::InitError(format!("Failed to write temp file {}: {}", tmp_path, e)))?;
    tmp_file
        .sync_all()
        .map_err(|e| Error::InitError(format!("Failed to sync temp file: {}", e)))?;
    drop(tmp_file);

    std::fs::rename(&tmp_path, status_path).map_err(|e| {
        Error::InitError(format!(
            "Failed to rename {} -> {}: {}",
            tmp_path, status_path, e
        ))
    })?;

    // Lock is released when lock_file is dropped

    // Remove /var/lib/dpkg/info/<name>.* files
    let info_dir = "/var/lib/dpkg/info";
    if let Ok(entries) = std::fs::read_dir(info_dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            // Match <name>.* and <name>:<arch>.*
            if fname_str.starts_with(&format!("{}.", name))
                || fname_str.starts_with(&format!("{}:", name))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    debug!("Successfully removed {} from dpkg database", name);
    Ok(())
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
