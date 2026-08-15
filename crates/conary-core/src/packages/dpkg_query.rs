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
use crate::repository::dependency_model::{
    CapabilityProvenance, DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation,
    ProvidedCapability, RepositoryCapabilityKind, RepositoryRequirementGroup,
    RepositoryRequirementKind, SourcePackageFormat,
};
use crate::repository::requirement::parse_native_requirement;
use crate::repository::versioning::VersionScheme;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tracing::debug;

mod inventory;
pub use inventory::query_installed_inventory;

const DPKG_PACKAGE_RECORD_FORMAT: &str = "${binary:Package}\x1e${Package}\x1e${Version}\x1e${Architecture}\x1e${Description}\x1e${Maintainer}\x1e${Homepage}\x1e${Section}\x1e${Priority}\x1e${Installed-Size}\x1e${Multi-Arch}\x1f";

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
            if parts.len() != 11 {
                return Err(Error::ParseError(format!(
                    "dpkg inventory record {} has {} fields; expected exactly 11",
                    index + 1,
                    parts.len()
                )));
            }
            let multi_arch = if parts[10].is_empty() {
                DebianMultiArch::No
            } else {
                DebianMultiArch::parse_exact(parts[10]).map_err(Error::ParseError)?
            };
            let identity =
                InstalledPackageIdentity::dpkg(parts[0], parts[1], parts[2], parts[3], multi_arch)?;
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

/// Query one installed Debian package's exact identity and declared provides.
pub fn query_package_provides(
    identity: &InstalledPackageIdentity,
) -> Result<Vec<ProvidedCapability>> {
    let InstalledPackageIdentity::Dpkg { selector, .. } = identity else {
        return Err(Error::ConfigError(
            "dpkg provides query requires a dpkg installed identity".to_string(),
        ));
    };
    debug!("Querying provides for dpkg package: {selector}");

    let output = Command::new("dpkg-query")
        .args([
            "-W",
            "-f",
            "${binary:Package}\x1e${Package}\x1e${Version}\x1e${Provides}\x1f",
            selector,
        ])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run dpkg-query: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "dpkg-query for provides {} failed: {}",
            selector,
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
            "dpkg-query returned {} provides records for exact selector '{selector}'",
            records.len()
        )));
    }
    let provides = parse_dpkg_provide_record(identity, records[0])?;

    debug!(
        "Package {selector} provides {} typed capabilities",
        provides.len()
    );
    Ok(provides)
}

fn parse_dpkg_provide_record(
    identity: &InstalledPackageIdentity,
    record: &str,
) -> Result<Vec<ProvidedCapability>> {
    let InstalledPackageIdentity::Dpkg {
        selector,
        name,
        version,
        ..
    } = identity
    else {
        return Err(Error::ConfigError(
            "dpkg provides parser requires a dpkg installed identity".to_string(),
        ));
    };
    let fields = record.split('\x1e').collect::<Vec<_>>();
    if fields.len() != 4 || fields[0] != selector || fields[1] != name || fields[2] != version {
        return Err(Error::ParseError(format!(
            "dpkg-query provides record for '{selector}' disagrees with its exact installed identity"
        )));
    }
    let mut provides = vec![ProvidedCapability {
        kind: RepositoryCapabilityKind::PackageName,
        name: name.clone(),
        version: Some(version.clone()),
        version_relation: Some(ProvideVersionRelation::Equal),
        version_scheme: VersionScheme::Debian,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    for (record_index, part) in fields[3].split(',').enumerate() {
        let provide = part.trim();
        if !provide.is_empty() {
            let parsed = crate::repository::package_relation::parse_debian_provide(provide)
                .map_err(Error::ParseError)?;
            provides.push(ProvidedCapability {
                kind: parsed.kind,
                name: parsed.name,
                version: parsed.version,
                version_relation: parsed.version_relation,
                version_scheme: VersionScheme::Debian,
                architecture_qualifier: parsed.architecture_qualifier,
                provenance: CapabilityProvenance::SourceDeclared {
                    format: SourcePackageFormat::Debian,
                    record_index: u32::try_from(record_index).map_err(|_| {
                        Error::ParseError("Debian provide record index exceeds u32".to_string())
                    })?,
                },
            });
        }
    }
    for provide in &provides {
        provide.validate()?;
    }
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
    let record = query_package(name)?;
    remove_batch_from_db_only(&[record.identity])
}

/// Remove exact dpkg identities in one locked database rewrite.
pub fn remove_batch_from_db_only(identities: &[InstalledPackageIdentity]) -> Result<()> {
    if identities.is_empty() {
        return Ok(());
    }
    debug!(
        "Removing {} package records from dpkg database only",
        identities.len()
    );

    // Acquire dpkg locks per the frontend protocol spec:
    // 1. lock-frontend (frontend mutex — excludes apt, aptitude, etc.)
    // 2. lock (dpkg database lock — excludes dpkg itself)
    // Both use POSIX fcntl F_SETLK write locks as dpkg expects.
    let _frontend_lock = acquire_dpkg_lock("/var/lib/dpkg/lock-frontend")?;
    let _db_lock = acquire_dpkg_lock("/var/lib/dpkg/lock")?;
    remove_dpkg_records_at(
        identities,
        Path::new("/var/lib/dpkg/status"),
        Path::new("/var/lib/dpkg/info"),
    )
}

fn remove_dpkg_records_at(
    identities: &[InstalledPackageIdentity],
    status_path: &Path,
    info_dir: &Path,
) -> Result<()> {
    let mut targets = BTreeSet::new();
    let mut selectors = BTreeSet::new();
    for identity in identities {
        let InstalledPackageIdentity::Dpkg {
            selector,
            name,
            architecture,
            ..
        } = identity
        else {
            return Err(Error::ConfigError(format!(
                "dpkg authority removal received non-dpkg identity '{}'",
                identity.selector()
            )));
        };
        if !targets.insert((name.clone(), architecture.clone())) {
            return Err(Error::ConflictError(format!(
                "dpkg authority-removal batch repeated {name}:{architecture}"
            )));
        }
        selectors.insert(selector.clone());
    }

    let content = std::fs::read_to_string(status_path).map_err(|error| {
        Error::InitError(format!("Failed to read {}: {error}", status_path.display()))
    })?;
    let filtered = filter_dpkg_status(&content, &targets)?;
    let mode = std::fs::metadata(status_path)?.permissions().mode();
    crate::filesystem::durable::write_file_atomic_with_mode(
        status_path,
        filtered.as_bytes(),
        mode,
    )?;

    for entry in std::fs::read_dir(info_dir)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            Error::ParseError("dpkg info directory contains a non-UTF-8 filename".to_string())
        })?;
        if selectors
            .iter()
            .any(|selector| name.starts_with(&format!("{selector}.")))
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::File::open(info_dir)?.sync_all()?;

    debug!(
        "Successfully removed {} package records from dpkg database",
        identities.len()
    );
    Ok(())
}

fn filter_dpkg_status(content: &str, targets: &BTreeSet<(String, String)>) -> Result<String> {
    let mut retained = Vec::new();
    let mut removed = BTreeSet::new();
    for stanza in content
        .split("\n\n")
        .filter(|stanza| !stanza.trim().is_empty())
    {
        let package = control_field(stanza, "Package");
        let architecture = control_field(stanza, "Architecture");
        match (package, architecture) {
            (Some(package), Some(architecture))
                if targets.contains(&(package.to_string(), architecture.to_string())) =>
            {
                if !removed.insert((package.to_string(), architecture.to_string())) {
                    return Err(Error::ConflictError(format!(
                        "dpkg status repeated target stanza {package}:{architecture}"
                    )));
                }
            }
            _ => retained.push(stanza),
        }
    }
    if removed != *targets {
        let missing = targets
            .difference(&removed)
            .map(|(name, architecture)| format!("{name}:{architecture}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::NotFound(format!(
            "dpkg status is missing exact authority-removal target(s): {missing}"
        )));
    }
    let mut output = retained.join("\n\n");
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    Ok(output)
}

fn control_field<'a>(stanza: &'a str, field: &str) -> Option<&'a str> {
    let prefix = format!("{field}: ");
    stanza
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
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

    fn dpkg_identity(selector: &str, architecture: &str) -> InstalledPackageIdentity {
        InstalledPackageIdentity::dpkg(
            selector,
            "fixture",
            "1.0-1",
            architecture,
            DebianMultiArch::Same,
        )
        .unwrap()
    }

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
    fn batch_removal_targets_exact_multiarch_stanzas_and_info_records() {
        let temp = tempfile::TempDir::new().unwrap();
        let status = temp.path().join("status");
        let info = temp.path().join("info");
        std::fs::create_dir(&info).unwrap();
        std::fs::write(
            &status,
            "Package: fixture\nArchitecture: amd64\nStatus: install ok installed\n\nPackage: fixture\nArchitecture: i386\nStatus: install ok installed\n\nPackage: retained\nArchitecture: amd64\nStatus: install ok installed\n\n",
        )
        .unwrap();
        for name in [
            "fixture:amd64.list",
            "fixture:amd64.postinst",
            "fixture:i386.list",
            "retained.list",
        ] {
            std::fs::write(info.join(name), b"fixture").unwrap();
        }

        remove_dpkg_records_at(&[dpkg_identity("fixture:amd64", "amd64")], &status, &info).unwrap();

        let remaining = std::fs::read_to_string(status).unwrap();
        assert!(
            !remaining
                .contains("Architecture: amd64\nStatus: install ok installed\n\nPackage: fixture")
        );
        assert!(remaining.contains("Package: fixture\nArchitecture: i386"));
        assert!(remaining.contains("Package: retained\nArchitecture: amd64"));
        assert!(!info.join("fixture:amd64.list").exists());
        assert!(!info.join("fixture:amd64.postinst").exists());
        assert!(info.join("fixture:i386.list").exists());
        assert!(info.join("retained.list").exists());
    }

    #[test]
    fn batch_removal_rejects_a_missing_exact_variant_without_rewriting_status() {
        let targets = BTreeSet::from([("fixture".to_string(), "arm64".to_string())]);
        let status = "Package: fixture\nArchitecture: amd64\n\n";

        let error = filter_dpkg_status(status, &targets).unwrap_err();

        assert!(error.to_string().contains("fixture:arm64"));
    }

    #[test]
    fn package_query_records_preserve_multiline_descriptions_and_variants() {
        let output = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1efirst line\nsecond line\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1esame\x1f\
                      fixture:arm64\x1efixture\x1e1.2.3\x1earm64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1e\x1f";
        let records = parse_package_query_records(output).unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].info.description.as_deref(),
            Some("first line\nsecond line")
        );
        assert_eq!(records[1].info.arch, "arm64");
        assert_eq!(
            records[0].identity.debian_multi_arch(),
            Some(DebianMultiArch::Same)
        );
        assert_eq!(
            records[1].identity.debian_multi_arch(),
            Some(DebianMultiArch::No)
        );
        assert_eq!(records[0].identity.selector(), "fixture:amd64");
        assert_eq!(records[1].identity.selector(), "fixture:arm64");
    }

    #[test]
    fn package_inventory_rejects_malformed_or_duplicate_records() {
        assert!(parse_package_query_records("missing-fields\x1f").is_err());

        let record = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1eno\x1f";
        assert!(parse_package_query_records(&format!("{record}{record}")).is_err());

        let unknown_multi_arch = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1esometimes\x1f";
        let error = parse_package_query_records(unknown_multi_arch).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid Debian Multi-Arch value")
        );
    }

    #[test]
    fn installed_provides_preserve_debian_versions_and_architecture_qualifiers() {
        let identity = InstalledPackageIdentity::dpkg(
            "fixture:amd64",
            "fixture",
            "2:1.4-3",
            "amd64",
            DebianMultiArch::Foreign,
        )
        .unwrap();
        let provides = parse_dpkg_provide_record(
            &identity,
            "fixture:amd64\x1efixture\x1e2:1.4-3\x1email-api:any (= 2), helper:arm64",
        )
        .unwrap();

        assert_eq!(provides.len(), 3);
        assert_eq!(provides[0].provenance, CapabilityProvenance::ExactIdentity);
        assert_eq!(provides[1].name, "mail-api");
        assert_eq!(provides[1].version.as_deref(), Some("2"));
        assert_eq!(
            provides[1].architecture_qualifier,
            ProvideArchitectureQualifier::Any
        );
        assert_eq!(provides[2].name, "helper");
        assert_eq!(
            provides[2].architecture_qualifier,
            ProvideArchitectureQualifier::Exact("arm64".to_string())
        );
    }

    #[test]
    fn installed_provides_reject_identity_drift() {
        let identity = InstalledPackageIdentity::dpkg(
            "fixture:amd64",
            "fixture",
            "1",
            "amd64",
            DebianMultiArch::No,
        )
        .unwrap();
        assert!(
            parse_dpkg_provide_record(&identity, "fixture:amd64\x1efixture\x1e2\x1email-api")
                .is_err()
        );
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
