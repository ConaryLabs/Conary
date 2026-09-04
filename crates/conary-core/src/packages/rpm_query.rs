// conary-core/src/packages/rpm_query.rs

//! Query installed RPM packages from the system database
//!
//! This module provides functions to query the local RPM database
//! using the `rpm` command-line tool.

use crate::error::{Error, Result};
use crate::packages::InstalledPackageIdentity;
use crate::packages::install_reason::{InstallReasonAuthorityError, query_package_names};
use crate::packages::query_common::{
    InstalledFileAbsencePolicy, InstalledFileInfo, InstalledPackageRecord, run_query_command,
};
use crate::packages::rpm::decode_rpm_requirement;
use crate::repository::dependency_model::RepositoryRequirementGroup;
use rpm::{DependencyFlags, FileFlags};
use std::collections::HashSet;
use std::process::Command;
use tracing::debug;

const RPM_PACKAGE_RECORD_FORMAT: &str = "%{NEVRA}\x1e%{NAME}\x1e%{VERSION}\x1e%{RELEASE}\x1e%{EPOCH}\x1e%{ARCH}\x1e%{DESCRIPTION}\x1e%{SUMMARY}\x1e%{LICENSE}\x1e%{URL}\x1e%{VENDOR}\x1e%{SOURCERPM}\x1e%{BUILDHOST}\x1e%{INSTALLTIME}\x1f";
const RPM_FILE_RECORD_FORMAT: &str = "[%{FILENAMES}\x1e%{LONGFILESIZES}\x1e%{FILEMTIMES}\x1e%{FILEDIGESTS}\x1e%{FILEMODES:octal}\x1e%{FILEUSERNAME}\x1e%{FILEGROUPNAME}\x1e%{FILELINKTOS}\x1e%{FILEFLAGS:hex}\x1e%{FILESTATES}\x1f]";
const RPM_REQUIREMENT_RECORD_FORMAT: &str =
    "[%{REQUIRENAME}\x1e%{REQUIREFLAGS:hex}\x1e%{REQUIREVERSION}\x1f]";
const RPM_OWNER_RECORD_FORMAT: &str = "%{NAME}\x1f";
const DNF5_USER_INSTALLED_ARGS: &[&str] = &[
    "--setopt=disable_excludes=*",
    "repoquery",
    "--userinstalled",
    "--queryformat",
    r"%{name}.%{arch}\n",
];
const DNF4_USER_INSTALLED_ARGS: &[&str] = &[
    "--disableexcludes=all",
    "repoquery",
    "--installed",
    "--userinstalled",
    "--queryformat",
    r"%{name}.%{arch}\n",
];

mod provides;
pub use provides::query_package_provides;

mod inventory;
pub use inventory::query_installed_inventory;

/// RPM's reserved name for imported public keys.
///
/// `rpmkeys --import` stores each trusted key as a header in the same database
/// as installed packages, under this name, with the key ID as version and its
/// creation timestamp as release. These records are the keyring, not software:
/// they own no files and carry no architecture, so no NEVRA can be formed for
/// them and nothing can be adopted, refreshed, or taken over through one.
const RPM_PUBLIC_KEY_PSEUDO_PACKAGE: &str = "gpg-pubkey";

/// Whether an RPM database record is a stored public key rather than a package.
///
/// Measured on Fedora 44 (`fedora44-guest-v1`), where the distro's own signing
/// key is imported at install time, so every real Fedora system has at least
/// one of these:
///
/// ```text
/// gpg-pubkey  NEVRA=<gpg-pubkey-36f612…d9f90a6-6786af3b>  ARCH=<(none)>  (contains no files)
/// bash        NEVRA=<bash-5.3.9-3.fc44.x86_64>            ARCH=<x86_64>
/// ```
///
/// Both conditions are required. The name alone is RPM's documented reservation
/// and the missing architecture is the structural consequence; demanding both
/// means a malformed record still fails loudly instead of being silently
/// dropped from the inventory.
fn is_rpm_public_key_record(name: &str, architecture: &str) -> bool {
    name == RPM_PUBLIC_KEY_PSEUDO_PACKAGE && matches!(architecture, "" | "(none)")
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

/// Query detailed information about an installed package
pub fn query_package(name: &str) -> Result<InstalledPackageRecord<InstalledRpmInfo>> {
    debug!("Querying package info: {}", name);

    // ASCII record/unit separators keep multiline descriptions unambiguous.
    let output = Command::new("rpm")
        .args(["-q", name, "--queryformat", RPM_PACKAGE_RECORD_FORMAT])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{}' not found in RPM database",
            name
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("rpm output is not UTF-8: {error}")))?;
    let mut records = parse_package_query_records(&stdout)?;
    if records.len() > 1 {
        let variants = records
            .iter()
            .map(|record| record.identity.selector().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::ConflictError(format!(
            "Package '{name}' matches multiple installed RPM variants: {variants}. Use one exact installed NEVRA selector."
        )));
    }
    records
        .pop()
        .ok_or_else(|| Error::NotFound(format!("Package '{name}' returned no RPM database record")))
}

fn parse_package_query_records(
    output: &str,
) -> Result<Vec<InstalledPackageRecord<InstalledRpmInfo>>> {
    let mut selectors = HashSet::new();
    let mut records = Vec::new();
    for (index, record) in output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .enumerate()
    {
        if let Some(parsed) = parse_package_query_record(index, record, &mut selectors)? {
            records.push(parsed);
        }
    }
    Ok(records)
}

/// Parse one inventory record, or `None` when the record is not a package.
fn parse_package_query_record(
    index: usize,
    record: &str,
    selectors: &mut HashSet<String>,
) -> Result<Option<InstalledPackageRecord<InstalledRpmInfo>>> {
    let parts = record.split('\x1e').collect::<Vec<_>>();
    if parts.len() != 14 {
        return Err(Error::ParseError(format!(
            "RPM inventory record {} has {} fields; expected exactly 14",
            index + 1,
            parts.len()
        )));
    }

    let epoch = match parts[4] {
        "" | "(none)" => None,
        value => Some(value.parse::<u64>().map_err(|error| {
            Error::ParseError(format!(
                "RPM inventory record {} has invalid epoch {value:?}: {error}",
                index + 1
            ))
        })?),
    };

    // The keyring shares the package database but is not part of the inventory.
    // Classify it only after validating the same typed fields that distinguish
    // a real key record from malformed inventory output.
    if is_rpm_public_key_record(parts[1], parts[5]) {
        validate_rpm_public_key_record(index + 1, &parts, epoch)?;
        debug!("skipping RPM public-key record {}", parts[0]);
        return Ok(None);
    }

    let identity =
        InstalledPackageIdentity::rpm(parts[0], parts[1], epoch, parts[2], parts[3], parts[5])?;
    if !selectors.insert(identity.selector().to_string()) {
        return Err(Error::ConflictError(format!(
            "RPM inventory repeated exact NEVRA selector '{}'",
            identity.selector()
        )));
    }
    Ok(Some(InstalledPackageRecord {
        info: InstalledRpmInfo {
            name: parts[1].to_string(),
            version: parts[2].to_string(),
            release: parts[3].to_string(),
            epoch,
            arch: required_rpm_field(parts[5], index + 1, "ARCH")?,
            description: rpm_none_to_option(&parts[6]),
            summary: rpm_none_to_option(&parts[7]),
            license: rpm_none_to_option(&parts[8]),
            url: rpm_none_to_option(&parts[9]),
            vendor: rpm_none_to_option(&parts[10]),
            source_rpm: rpm_none_to_option(&parts[11]),
            build_host: rpm_none_to_option(&parts[12]),
            install_time: rpm_none_to_option(&parts[13]),
        },
        identity,
    }))
}

fn validate_rpm_public_key_record(
    record_number: usize,
    parts: &[&str],
    epoch: Option<u64>,
) -> Result<()> {
    let name = required_rpm_field(parts[1], record_number, "NAME")?;
    let version = required_rpm_field(parts[2], record_number, "VERSION")?;
    let release = required_rpm_field(parts[3], record_number, "RELEASE")?;
    if epoch.is_some() {
        return Err(Error::ParseError(format!(
            "RPM public-key record {record_number} unexpectedly declares an epoch"
        )));
    }

    let expected_nevra = format!("{name}-{version}-{release}");
    if parts[0] != expected_nevra {
        return Err(Error::ParseError(format!(
            "RPM public-key record {record_number} NEVRA {:?} disagrees with its typed identity fields; expected {expected_nevra:?}",
            parts[0]
        )));
    }
    Ok(())
}

fn required_rpm_field(value: &str, record: usize, field: &str) -> Result<String> {
    if value.is_empty() || value == "(none)" {
        return Err(Error::ParseError(format!(
            "RPM inventory record {record} has no required {field} field"
        )));
    }
    Ok(value.to_string())
}

/// Query files belonging to an installed package
pub fn query_package_files(name: &str) -> Result<Vec<InstalledFileInfo>> {
    debug!("Querying files for package: {}", name);

    // Use --dump format: path size mtime digest mode owner group ...
    let output = Command::new("rpm")
        .args(["-q", name, "--queryformat", RPM_FILE_RECORD_FORMAT])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{}' not found in RPM database",
            name
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("rpm file output is not UTF-8: {error}")))?;
    let files = parse_rpm_file_records(&stdout)?;

    debug!("Found {} files for package {}", files.len(), name);
    Ok(files)
}

/// Convert an RPM field value to `Option<String>`, treating `"(none)"` and empty as `None`.
fn rpm_none_to_option(s: &&str) -> Option<String> {
    if *s == "(none)" || s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn parse_rpm_file_records(output: &str) -> Result<Vec<InstalledFileInfo>> {
    output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .enumerate()
        .map(|(index, record)| parse_rpm_file_record(index + 1, record))
        .filter_map(Result::transpose)
        .collect()
}

fn parse_rpm_file_record(record_number: usize, record: &str) -> Result<Option<InstalledFileInfo>> {
    let parts = record.split('\x1e').collect::<Vec<_>>();
    if parts.len() != 10 {
        return Err(Error::ParseError(format!(
            "RPM file record {record_number} has {} fields; expected exactly 10",
            parts.len()
        )));
    }
    if parts[0].is_empty() {
        return Err(Error::ParseError(format!(
            "RPM file record {record_number} has an empty path"
        )));
    }
    let size = parts[1].parse::<i64>().map_err(|error| {
        Error::ParseError(format!(
            "RPM file record {record_number} has invalid size {:?}: {error}",
            parts[1]
        ))
    })?;
    let mtime = parts[2].parse::<i64>().map_err(|error| {
        Error::ParseError(format!(
            "RPM file record {record_number} has invalid mtime {:?}: {error}",
            parts[2]
        ))
    })?;
    let digest = match parts[3] {
        "" | "(none)" => None,
        value
            if value.len() % 2 == 0
                && value.chars().all(|character| character.is_ascii_hexdigit()) =>
        {
            Some(value.to_string())
        }
        value => {
            return Err(Error::ParseError(format!(
                "RPM file record {record_number} has invalid digest {value:?}"
            )));
        }
    };
    let mode = i32::from_str_radix(parts[4], 8).map_err(|error| {
        Error::ParseError(format!(
            "RPM file record {record_number} has invalid octal mode {:?}: {error}",
            parts[4]
        ))
    })?;
    let user = required_rpm_field(parts[5], record_number, "FILEUSERNAME")?;
    let group = required_rpm_field(parts[6], record_number, "FILEGROUPNAME")?;
    let flags = u32::from_str_radix(parts[8], 16).map_err(|error| {
        Error::ParseError(format!(
            "RPM file record {record_number} has invalid hexadecimal flags {:?}: {error}",
            parts[8]
        ))
    })?;

    // RPM persists the transaction result for every header file in FILESTATES.
    // Only NORMAL (0) and NETSHARED (3) are installed payload according to
    // RPM's own RPMFILE_IS_INSTALLED contract. REPLACED (1), NOTINSTALLED (2),
    // and WRONGCOLOR (4) remain package metadata, not live ownership authority.
    match parts[9].parse::<i32>() {
        Ok(0 | 3) => {}
        Ok(1 | 2 | 4) => return Ok(None),
        Ok(state) => {
            return Err(Error::ParseError(format!(
                "RPM file record {record_number} has unsupported FILESTATES value {state}"
            )));
        }
        Err(error) => {
            return Err(Error::ParseError(format!(
                "RPM file record {record_number} has invalid FILESTATES value {:?}: {error}",
                parts[9]
            )));
        }
    }

    let flags = FileFlags::from_bits_retain(flags);
    let absence_policy = match (
        flags.contains(FileFlags::GHOST),
        flags.contains(FileFlags::MISSINGOK),
    ) {
        (false, false) => InstalledFileAbsencePolicy::Required,
        (true, false) => InstalledFileAbsencePolicy::RpmGhost,
        (false, true) => InstalledFileAbsencePolicy::RpmMissingOk,
        (true, true) => InstalledFileAbsencePolicy::RpmGhostAndMissingOk,
    };
    Ok(Some(InstalledFileInfo {
        path: parts[0].to_string(),
        size,
        mode,
        digest,
        user: Some(user),
        group: Some(group),
        link_target: rpm_none_to_option(&parts[7]),
        mtime: Some(mtime),
        absence_policy,
    }))
}

/// Query RPM's complete installed `Requires` entries as exact typed groups.
pub fn query_package_requirement_groups(name: &str) -> Result<Vec<RepositoryRequirementGroup>> {
    let output = Command::new("rpm")
        .args(["-q", name, "--queryformat", RPM_REQUIREMENT_RECORD_FORMAT])
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| Error::InitError(format!("Failed to run rpm: {error}")))?;
    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{name}' not found in RPM database"
        )));
    }

    let stdout = String::from_utf8(output.stdout).map_err(|error| {
        Error::ParseError(format!("rpm requirement output is not UTF-8: {error}"))
    })?;
    parse_rpm_requirement_records(&stdout)
}

fn parse_rpm_requirement_records(output: &str) -> Result<Vec<RepositoryRequirementGroup>> {
    output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .enumerate()
        .map(|(index, record)| {
            let parts = record.split('\x1e').collect::<Vec<_>>();
            if parts.len() != 3 {
                return Err(Error::ParseError(format!(
                    "RPM requirement record {} has {} fields; expected exactly 3",
                    index + 1,
                    parts.len()
                )));
            }
            if parts[0].is_empty() {
                return Err(Error::ParseError(format!(
                    "RPM requirement record {} has an empty name",
                    index + 1
                )));
            }
            let flags = u32::from_str_radix(parts[1], 16).map_err(|error| {
                Error::ParseError(format!(
                    "RPM requirement record {} has invalid hexadecimal flags {:?}: {error}",
                    index + 1,
                    parts[1]
                ))
            })?;
            decode_rpm_requirement(parts[0], parts[2], DependencyFlags::from_bits_retain(flags))
        })
        .filter_map(|result| result.transpose())
        .collect()
}

/// Query every installed RPM database record without collapsing variants.
pub fn query_all_packages() -> Result<Vec<InstalledPackageRecord<InstalledRpmInfo>>> {
    debug!("Querying all installed RPM packages with info");

    let stdout = run_query_command("rpm", &["-qa", "--queryformat", RPM_PACKAGE_RECORD_FORMAT])?;
    let packages = parse_package_query_records(&stdout)?;

    debug!("Queried {} installed packages", packages.len());
    Ok(packages)
}

/// Query DNF's exact user-installed package set.
///
/// DNF4 documents `repoquery --installed --userinstalled`, while DNF5 5.4.1.0
/// makes those selectors mutually exclusive because `--userinstalled` is
/// itself installed-only. DNF5's typed `filter_userinstalled` first restricts
/// to installed packages, then excludes dependency and weak-dependency
/// reasons (upstream tag 5.4.1.0, `dnf5/commands/repoquery/repoquery.cpp` and
/// `libdnf5/rpm/package_query.cpp`). Keep the two source-derived command
/// grammars separate rather than assuming DNF4 flag composition applies to
/// DNF5. Their distinct documented options disable configured excludes so the
/// result cannot silently omit installed packages. A host with RPM but no DNF
/// authority returns a typed error.
pub fn query_user_installed()
-> std::result::Result<std::collections::HashSet<String>, InstallReasonAuthorityError> {
    debug!("Querying user-installed RPM packages via DNF authority");
    query_user_installed_with(query_package_names)
}

fn query_user_installed_with(
    mut query: impl FnMut(
        &'static str,
        &str,
        &[&str],
    ) -> std::result::Result<HashSet<String>, InstallReasonAuthorityError>,
) -> std::result::Result<HashSet<String>, InstallReasonAuthorityError> {
    let dnf5 = query("DNF5", "dnf5", DNF5_USER_INSTALLED_ARGS);
    match dnf5 {
        Ok(packages) => Ok(packages),
        Err(error) if error.is_command_unavailable() => {
            query("DNF4", "dnf", DNF4_USER_INSTALLED_ARGS)
        }
        Err(error) => Err(error),
    }
}

/// Remove a package from the RPM database only (no files deleted).
///
/// Uses `rpm -e --justdb --nodeps` to remove the package record from
/// the RPM database without touching any files on disk. This is used
/// during takeover to transfer ownership from RPM to Conary.
pub fn remove_from_db_only(name: &str) -> Result<()> {
    remove_batch_from_db_only(&[name])
}

/// Remove exact package records in one RPM database-only transaction.
pub fn remove_batch_from_db_only(selectors: &[&str]) -> Result<()> {
    if selectors.is_empty() {
        return Ok(());
    }
    debug!(
        "Removing {} package records from RPM database only (--justdb)",
        selectors.len()
    );

    let output = Command::new("rpm")
        .args(["-e", "--justdb", "--nodeps"])
        .args(selectors)
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm -e --justdb: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "rpm -e --justdb batch failed for [{}]: {}",
            selectors.join(", "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    debug!(
        "Successfully removed {} package records from RPM database",
        selectors.len()
    );
    Ok(())
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
        .args(["-qf", "--queryformat", RPM_OWNER_RECORD_FORMAT, path])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "rpm could not resolve an owner for {path:?} (status {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("rpm owner output is not UTF-8: {error}")))?;
    parse_owner_records(&stdout)
}

fn parse_owner_records(output: &str) -> Result<Vec<String>> {
    output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .map(|record| {
            if record
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(Error::ParseError(format!(
                    "rpm owner query returned an invalid package-name record {record:?}"
                )));
            }
            Ok(record.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests;
