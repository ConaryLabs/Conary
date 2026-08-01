// conary-core/src/packages/dpkg_query.rs

//! Query installed dpkg packages from the system database
//!
//! This module provides functions to query the local dpkg database
//! using the `dpkg-query` command-line tool.

use crate::error::{Error, Result};
use crate::packages::InstalledPackageIdentity;
use crate::packages::archive_utils::get_file_metadata;
use crate::packages::install_reason::{InstallReasonAuthorityError, query_package_names};
use crate::packages::query_common::{
    InstalledFileAbsencePolicy, InstalledFileInfo, InstalledPackageRecord, run_query_command,
};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::requirement::parse_native_requirement;
use crate::repository::versioning::VersionScheme;
use std::collections::{HashMap, HashSet};
use std::process::Command;
use tracing::debug;

const DPKG_PACKAGE_RECORD_FORMAT: &str = "${binary:Package}\x1e${Package}\x1e${Version}\x1e${Architecture}\x1e${Description}\x1e${Maintainer}\x1e${Homepage}\x1e${Section}\x1e${Priority}\x1e${Installed-Size}\x1f";

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

/// Query detailed information about an installed package
pub fn query_package(name: &str) -> Result<InstalledPackageRecord<InstalledDpkgInfo>> {
    debug!("Querying package info: {}", name);

    // ASCII record/unit separators keep multiline descriptions unambiguous.
    let output = Command::new("dpkg-query")
        .args(["-W", "-f", DPKG_PACKAGE_RECORD_FORMAT, name])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{}' not found in dpkg database",
            name
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("dpkg-query output is not UTF-8: {error}")))?;
    let mut records = parse_package_query_records(&stdout)?;
    if records.len() > 1 {
        let variants = records
            .iter()
            .map(|record| record.identity.selector().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::ConflictError(format!(
            "Package '{name}' matches multiple installed dpkg variants: {variants}. Use an architecture-qualified native package name."
        )));
    }
    records.pop().ok_or_else(|| {
        Error::NotFound(format!("Package '{name}' returned no dpkg database record"))
    })
}

fn parse_package_query_records(
    output: &str,
) -> Result<Vec<InstalledPackageRecord<InstalledDpkgInfo>>> {
    let mut selectors = HashSet::new();
    output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .enumerate()
        .map(|(index, record)| {
            let parts = record.split('\x1e').collect::<Vec<_>>();
            if parts.len() != 10 {
                return Err(Error::ParseError(format!(
                    "dpkg inventory record {} has {} fields; expected exactly 10",
                    index + 1,
                    parts.len()
                )));
            }
            let identity = InstalledPackageIdentity::dpkg(parts[0], parts[1], parts[2], parts[3])?;
            if !selectors.insert(identity.selector().to_string()) {
                return Err(Error::ConflictError(format!(
                    "dpkg inventory repeated exact binary package selector '{}'",
                    identity.selector()
                )));
            }
            let installed_size = match parts[9] {
                "" => None,
                value => Some(value.parse::<i64>().map_err(|error| {
                    Error::ParseError(format!(
                        "dpkg inventory record {} has invalid Installed-Size {value:?}: {error}",
                        index + 1
                    ))
                })?),
            };
            Ok(InstalledPackageRecord {
                info: InstalledDpkgInfo {
                    name: parts[1].to_string(),
                    version: parts[2].to_string(),
                    arch: parts[3].to_string(),
                    description: optional_dpkg_field(parts[4]),
                    maintainer: optional_dpkg_field(parts[5]),
                    homepage: optional_dpkg_field(parts[6]),
                    section: optional_dpkg_field(parts[7]),
                    priority: optional_dpkg_field(parts[8]),
                    installed_size,
                },
                identity,
            })
        })
        .collect()
}

fn optional_dpkg_field(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod owner_query_tests {
    use super::*;

    #[test]
    fn owner_records_require_exact_installed_binary_selectors() {
        let installed = HashSet::from(["libc6:amd64".to_string(), "base-files".to_string()]);
        assert_eq!(
            parse_owner_records(
                "libc6:amd64, base-files: /usr/share/doc/example\n",
                "/usr/share/doc/example",
                &installed,
            )
            .unwrap(),
            vec!["libc6:amd64", "base-files"]
        );
        assert!(
            parse_owner_records(
                "diversion by base-files from: /usr/share/doc/example\n",
                "/usr/share/doc/example",
                &installed,
            )
            .is_err()
        );
    }
}

/// Query files installed by a package
pub fn query_package_files(name: &str) -> Result<Vec<InstalledFileInfo>> {
    debug!("Querying files for package: {}", name);

    // Use dpkg -L to list files
    let output = Command::new("dpkg")
        .args(["-L", name])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{}' not found in dpkg database",
            name
        )));
    }

    // Load digest map once to avoid re-reading the md5sums file per file (N+1)
    let digest_map = load_digest_map(name)?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("dpkg -L output is not UTF-8: {error}")))?;

    let mut files = Vec::new();

    for line in stdout.lines() {
        let path = line.to_string();
        if path.is_empty() {
            return Err(Error::ParseError(format!(
                "dpkg -L returned an empty path record for '{name}'"
            )));
        }

        let (size, mode) = get_file_metadata(&path).map_err(|error| {
            Error::IoError(format!(
                "dpkg record '{name}' owns '{path}', but its exact live metadata is unavailable: {error}"
            ))
        })?;

        // Look up md5sum from pre-loaded digest map
        let search_path = path.strip_prefix('/').unwrap_or(&path);
        let digest = digest_map.get(search_path).cloned();

        // Check if this is a symlink and get target
        let link_target = if (mode & 0o170000) == 0o120000 {
            Some(
                std::fs::read_link(&path)
                    .map_err(|error| {
                        Error::IoError(format!(
                            "failed to read exact symlink target for dpkg path '{path}': {error}"
                        ))
                    })?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| {
                        Error::ParseError(format!(
                            "dpkg path '{path}' has a non-UTF-8 symlink target"
                        ))
                    })?,
            )
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
            mtime: None,
            absence_policy: InstalledFileAbsencePolicy::Required,
        });
    }

    debug!("Found {} files for package {}", files.len(), name);
    Ok(files)
}

/// Load the full digest map for a package from the dpkg md5sums file.
///
/// The exact `${binary:Package}` selector maps directly to dpkg's info-file
/// basename, including its architecture qualifier when one is required.
fn load_digest_map(package: &str) -> Result<HashMap<String, String>> {
    let base_path = format!("/var/lib/dpkg/info/{}.md5sums", package);
    match std::fs::read_to_string(&base_path) {
        Ok(content) => parse_md5sums(&content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(Error::IoError(format!(
            "failed to read exact dpkg md5sums record '{base_path}': {error}"
        ))),
    }
}

/// Parse dpkg md5sums file content into a path -> digest map.
fn parse_md5sums(content: &str) -> Result<HashMap<String, String>> {
    let mut digests = HashMap::new();
    for (index, line) in content.lines().enumerate() {
        let (digest, path) = line.split_once("  ").ok_or_else(|| {
            Error::ParseError(format!(
                "dpkg md5sums record {} is outside the documented digest/path grammar",
                index + 1
            ))
        })?;
        if digest.len() != 32
            || !digest
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(Error::ParseError(format!(
                "dpkg md5sums record {} has an invalid MD5 digest",
                index + 1
            )));
        }
        if path.is_empty()
            || digests
                .insert(path.to_string(), digest.to_string())
                .is_some()
        {
            return Err(Error::ParseError(format!(
                "dpkg md5sums record {} has an empty or duplicate path",
                index + 1
            )));
        }
    }
    Ok(digests)
}

/// Query Debian `Pre-Depends` and `Depends` as exact alternative groups.
pub fn query_package_requirement_groups(name: &str) -> Result<Vec<RepositoryRequirementGroup>> {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Pre-Depends}\x1e${Depends}\x1f", name])
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| Error::InitError(format!("Failed to run dpkg-query: {error}")))?;
    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{name}' not found in dpkg database"
        )));
    }

    let output = String::from_utf8(output.stdout).map_err(|error| {
        Error::ParseError(format!(
            "dpkg-query requirement output is not UTF-8: {error}"
        ))
    })?;
    let records = output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    if records.len() != 1 {
        return Err(Error::ParseError(format!(
            "dpkg-query returned {} requirement records for exact selector '{name}'",
            records.len()
        )));
    }
    let fields = records[0].split('\x1e').collect::<Vec<_>>();
    if fields.len() != 2 {
        return Err(Error::ParseError(format!(
            "dpkg-query requirement record for '{name}' has {} fields; expected 2",
            fields.len()
        )));
    }
    let [pre_depends, depends] = [fields[0], fields[1]];
    let mut requirements = Vec::new();
    for (kind, field) in [
        (RepositoryRequirementKind::PreDepends, pre_depends),
        (RepositoryRequirementKind::Depends, depends),
    ] {
        for native_text in split_debian_requirement_field(field)? {
            requirements.push(
                parse_native_requirement(kind, VersionScheme::Debian, native_text)
                    .map_err(Error::ParseError)?,
            );
        }
    }
    Ok(requirements)
}

fn split_debian_requirement_field(field: &str) -> Result<Vec<&str>> {
    let mut entries = Vec::new();
    let mut depth = 0_u32;
    let mut start = 0;
    for (offset, character) in field.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::ParseError("Debian requirement has an unmatched ')'".to_string())
                })?;
            }
            ',' if depth == 0 => {
                let entry = field[start..offset].trim();
                if !entry.is_empty() {
                    entries.push(entry);
                }
                start = offset + character.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(Error::ParseError(
            "Debian requirement has an unmatched '('".to_string(),
        ));
    }
    let tail = field[start..].trim();
    if !tail.is_empty() {
        entries.push(tail);
    }
    Ok(entries)
}

/// Query what a package provides (capabilities it offers)
///
/// Returns a list of capability strings from the Provides field,
/// plus the package name itself as a provide.
pub fn query_package_provides(name: &str) -> Result<Vec<String>> {
    debug!("Querying provides for dpkg package: {}", name);

    let output = Command::new("dpkg-query")
        .args([
            "-W",
            "-f",
            "${binary:Package}\x1e${Package}\x1e${Provides}\x1f",
            name,
        ])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "dpkg-query for provides {} failed: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("dpkg-query output is not UTF-8: {error}")))?;
    let records = stdout
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    if records.len() != 1 {
        return Err(Error::ParseError(format!(
            "dpkg-query returned {} provides records for exact selector '{name}'",
            records.len()
        )));
    }
    let fields = records[0].split('\x1e').collect::<Vec<_>>();
    if fields.len() != 3 || fields[0].is_empty() || fields[1].is_empty() {
        return Err(Error::ParseError(format!(
            "dpkg-query provides record for '{name}' is malformed"
        )));
    }
    let mut provides = vec![fields[1].to_string()];
    if fields[0] != fields[1] {
        provides.push(fields[0].to_string());
    }
    for part in fields[2].split(',') {
        let provide = part.trim();
        if !provide.is_empty() {
            provides.push(provide.to_string());
        }
    }

    debug!("Package {} provides {} capabilities", name, provides.len());
    Ok(provides)
}

/// Query every installed dpkg database record without collapsing variants.
pub fn query_all_packages() -> Result<Vec<InstalledPackageRecord<InstalledDpkgInfo>>> {
    debug!("Querying all installed dpkg packages with info");

    let stdout = run_query_command("dpkg-query", &["-W", "-f", DPKG_PACKAGE_RECORD_FORMAT])?;
    let packages = parse_package_query_records(&stdout)?;

    debug!("Queried {} installed packages", packages.len());
    Ok(packages)
}

/// Query which package(s) own a file
pub fn query_file_owner(path: &str) -> Result<Vec<String>> {
    let output = Command::new("dpkg-query")
        .args(["--search", path])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "dpkg-query could not resolve an owner for {path:?} (status {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let stdout = String::from_utf8(output.stdout).map_err(|error| {
        Error::ParseError(format!("dpkg-query owner output is not UTF-8: {error}"))
    })?;
    let installed_selectors = query_all_packages()?
        .into_iter()
        .map(|record| record.identity.selector().to_string())
        .collect::<HashSet<_>>();
    parse_owner_records(&stdout, path, &installed_selectors)
}

fn parse_owner_records(
    output: &str,
    requested_path: &str,
    installed_selectors: &HashSet<String>,
) -> Result<Vec<String>> {
    let mut owners = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in output.lines().enumerate() {
        let (owner_list, owned_path) = line.rsplit_once(": ").ok_or_else(|| {
            Error::ParseError(format!(
                "dpkg-query owner record {} has no exact owner/path separator",
                index + 1
            ))
        })?;
        if owned_path != requested_path {
            return Err(Error::ParseError(format!(
                "dpkg-query owner record {} names path {owned_path:?}, expected exact path {requested_path:?}",
                index + 1
            )));
        }
        for owner in owner_list.split(", ") {
            if !installed_selectors.contains(owner) {
                return Err(Error::ParseError(format!(
                    "dpkg-query owner record {} names {owner:?}, which is not an exact installed binary package selector",
                    index + 1
                )));
            }
            if seen.insert(owner) {
                owners.push(owner.to_string());
            }
        }
    }
    Ok(owners)
}

/// Query APT's exact manually-installed package set.
///
/// `apt-mark showmanual` is the documented package-manager frontend for this
/// state. Missing APT state/command support is an actionable typed failure; it
/// is never reinterpreted as every dpkg package being manually installed.
pub fn query_user_installed()
-> std::result::Result<std::collections::HashSet<String>, InstallReasonAuthorityError> {
    debug!("Querying manually-installed dpkg packages via apt-mark authority");
    query_package_names("APT", "apt-mark", &["showmanual"])
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

    #[test]
    fn package_query_records_preserve_multiline_descriptions_and_variants() {
        let output = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1efirst line\nsecond line\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1f\
                      fixture:arm64\x1efixture\x1e1.2.3\x1earm64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1f";
        let records = parse_package_query_records(output).unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].info.description.as_deref(),
            Some("first line\nsecond line")
        );
        assert_eq!(records[1].info.arch, "arm64");
        assert_eq!(records[0].identity.selector(), "fixture:amd64");
        assert_eq!(records[1].identity.selector(), "fixture:arm64");
    }

    #[test]
    fn package_inventory_rejects_malformed_or_duplicate_records() {
        assert!(parse_package_query_records("missing-fields\x1f").is_err());

        let record = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1f";
        assert!(parse_package_query_records(&format!("{record}{record}")).is_err());
    }

    #[test]
    fn md5sums_parser_is_exact_and_rejects_malformed_records() {
        let parsed =
            parse_md5sums("d41d8cd98f00b204e9800998ecf8427e  usr/share/fixture\n").unwrap();
        assert_eq!(
            parsed.get("usr/share/fixture").map(String::as_str),
            Some("d41d8cd98f00b204e9800998ecf8427e")
        );

        assert!(parse_md5sums("not-a-record\n").is_err());
        assert!(
            parse_md5sums(
                "d41d8cd98f00b204e9800998ecf8427e  usr/share/fixture\n\
                 d41d8cd98f00b204e9800998ecf8427e  usr/share/fixture\n"
            )
            .is_err()
        );
    }
}
