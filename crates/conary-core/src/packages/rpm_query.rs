// conary-core/src/packages/rpm_query.rs

//! Query installed RPM packages from the system database
//!
//! This module provides functions to query the local RPM database
//! using the `rpm` command-line tool.

use crate::error::{Error, Result};
use crate::packages::InstalledPackageIdentity;
use crate::packages::install_reason::{InstallReasonAuthorityError, query_package_names};
use crate::packages::query_common::{InstalledFileInfo, InstalledPackageRecord, run_query_command};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::requirement::parse_native_requirement;
use crate::repository::versioning::VersionScheme;
use std::collections::HashSet;
use std::process::Command;
use tracing::debug;

const RPM_PACKAGE_RECORD_FORMAT: &str = "%{NEVRA}\x1e%{NAME}\x1e%{VERSION}\x1e%{RELEASE}\x1e%{EPOCH}\x1e%{ARCH}\x1e%{DESCRIPTION}\x1e%{SUMMARY}\x1e%{LICENSE}\x1e%{URL}\x1e%{VENDOR}\x1e%{SOURCERPM}\x1e%{BUILDHOST}\x1e%{INSTALLTIME}\x1f";
const RPM_FILE_RECORD_FORMAT: &str = "[%{FILENAMES}\x1e%{LONGFILESIZES}\x1e%{FILEMTIMES}\x1e%{FILEDIGESTS}\x1e%{FILEMODES:octal}\x1e%{FILEUSERNAME}\x1e%{FILEGROUPNAME}\x1e%{FILELINKTOS}\x1f]";
const RPM_OWNER_RECORD_FORMAT: &str = "%{NAME}\x1f";

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
        .map(|(index, record)| {
            let parts = record.split('\x1e').collect::<Vec<_>>();
            if parts.len() != 8 {
                return Err(Error::ParseError(format!(
                    "RPM file record {} has {} fields; expected exactly 8",
                    index + 1,
                    parts.len()
                )));
            }
            if parts[0].is_empty() {
                return Err(Error::ParseError(format!(
                    "RPM file record {} has an empty path",
                    index + 1
                )));
            }
            let size = parts[1].parse::<i64>().map_err(|error| {
                Error::ParseError(format!(
                    "RPM file record {} has invalid size {:?}: {error}",
                    index + 1,
                    parts[1]
                ))
            })?;
            let mtime = parts[2].parse::<i64>().map_err(|error| {
                Error::ParseError(format!(
                    "RPM file record {} has invalid mtime {:?}: {error}",
                    index + 1,
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
                        "RPM file record {} has invalid digest {value:?}",
                        index + 1
                    )));
                }
            };
            let mode = i32::from_str_radix(parts[4], 8).map_err(|error| {
                Error::ParseError(format!(
                    "RPM file record {} has invalid octal mode {:?}: {error}",
                    index + 1,
                    parts[4]
                ))
            })?;
            let user = required_rpm_field(parts[5], index + 1, "FILEUSERNAME")?;
            let group = required_rpm_field(parts[6], index + 1, "FILEGROUPNAME")?;
            Ok(InstalledFileInfo {
                path: parts[0].to_string(),
                size,
                mode,
                digest,
                user: Some(user),
                group: Some(group),
                link_target: rpm_none_to_option(&parts[7]),
                mtime: Some(mtime),
            })
        })
        .collect()
}

/// Query RPM's complete installed `Requires` entries as exact typed groups.
pub fn query_package_requirement_groups(name: &str) -> Result<Vec<RepositoryRequirementGroup>> {
    let output = Command::new("rpm")
        .args(["-qR", name])
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| Error::InitError(format!("Failed to run rpm: {error}")))?;
    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{name}' not found in RPM database"
        )));
    }

    String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("rpm -qR output is not UTF-8: {error}")))?
        .lines()
        .map(str::trim)
        .filter(|requirement| !requirement.is_empty())
        .map(|requirement| {
            parse_native_requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Rpm,
                requirement,
            )
            .map_err(Error::ParseError)
        })
        .collect()
}

/// Query what a package provides (capabilities it offers)
///
/// Returns a list of capability strings like:
/// - "perl(Text::CharWidth)" (virtual provide)
/// - "libc.so.6(GLIBC_2.17)(64bit)" (library)
/// - "/usr/bin/perl" (file path)
/// - "perl-Text-CharWidth = 0.04-58.fc44" (package name = version)
pub fn query_package_provides(name: &str) -> Result<Vec<String>> {
    debug!("Querying provides for RPM package: {}", name);

    let stdout = run_query_command("rpm", &["-q", "--provides", name])?;
    let provides: Vec<String> = stdout
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    debug!("Package {} provides {} capabilities", name, provides.len());
    Ok(provides)
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
/// DNF4 and DNF5 both document `repoquery --installed --userinstalled` as the
/// package-manager authority for this set. Their distinct documented options
/// disable configured excludes so the result cannot silently omit installed
/// packages. A host with RPM but no DNF authority returns a typed error.
pub fn query_user_installed()
-> std::result::Result<std::collections::HashSet<String>, InstallReasonAuthorityError> {
    debug!("Querying user-installed RPM packages via DNF authority");

    let dnf5 = query_package_names(
        "DNF5",
        "dnf5",
        &[
            "--setopt=disable_excludes=*",
            "repoquery",
            "--installed",
            "--userinstalled",
            "--queryformat",
            "%{name}.%{arch}\n",
        ],
    );
    match dnf5 {
        Ok(packages) => Ok(packages),
        Err(error) if error.is_command_unavailable() => query_package_names(
            "DNF4",
            "dnf",
            &[
                "--disableexcludes=all",
                "repoquery",
                "--installed",
                "--userinstalled",
                "--queryformat",
                "%{name}.%{arch}\n",
            ],
        ),
        Err(error) => Err(error),
    }
}

/// Remove a package from the RPM database only (no files deleted).
///
/// Uses `rpm -e --justdb --nodeps` to remove the package record from
/// the RPM database without touching any files on disk. This is used
/// during takeover to transfer ownership from RPM to Conary.
pub fn remove_from_db_only(name: &str) -> Result<()> {
    debug!("Removing {} from RPM database only (--justdb)", name);

    let output = Command::new("rpm")
        .args(["-e", "--justdb", "--nodeps", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run rpm -e --justdb: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "rpm -e --justdb {} failed: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    debug!("Successfully removed {} from RPM database", name);
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
mod tests {
    use super::*;

    #[test]
    fn test_is_rpm_available() {
        // This test just ensures the function runs without panic
        let _ = is_rpm_available();
    }

    #[test]
    fn owner_records_are_queryformat_data_not_human_message_matches() {
        assert_eq!(
            parse_owner_records("filesystem\x1fbash\x1f").unwrap(),
            vec!["filesystem", "bash"]
        );
        assert!(parse_owner_records("file is not owned by any package").is_err());
    }

    #[test]
    fn test_installed_rpm_info_full_version() {
        let info = InstalledRpmInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            release: "1.fc44".to_string(),
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

        assert_eq!(info.full_version(), "2:1.0.0-1.fc44");
        assert_eq!(info.version_only(), "2:1.0.0");
    }

    #[test]
    fn test_installed_rpm_info_no_epoch() {
        let info = InstalledRpmInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            release: "1.fc44".to_string(),
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

        assert_eq!(info.full_version(), "1.0.0-1.fc44");
        assert_eq!(info.version_only(), "1.0.0");
    }

    #[test]
    fn package_query_records_preserve_multiline_descriptions_and_variants() {
        let output = "fixture-1.2.3-4.fc44.x86_64\x1efixture\x1e1.2.3\x1e4.fc44\x1e(none)\x1ex86_64\x1efirst line\nsecond line\x1esummary\x1eMIT\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f\
                      fixture-1.2.3-4.fc44.aarch64\x1efixture\x1e1.2.3\x1e4.fc44\x1e(none)\x1eaarch64\x1edescription\x1esummary\x1eMIT\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f";
        let records = parse_package_query_records(output).unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].info.description.as_deref(),
            Some("first line\nsecond line")
        );
        assert_eq!(records[1].info.arch, "aarch64");
        assert_eq!(
            records[0].identity.selector(),
            "fixture-1.2.3-4.fc44.x86_64"
        );
        assert_eq!(
            records[1].identity.selector(),
            "fixture-1.2.3-4.fc44.aarch64"
        );
    }

    /// Every real Fedora system imports the distro signing key at install time,
    /// so `rpm -qa` always emits at least one keyring record. Adoption failed on
    /// the whole system because of it:
    ///
    /// ```text
    /// Error: Parse error: installed native package selector
    /// "gpg-pubkey-36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6-6786af3b"
    /// disagrees with its typed identity fields
    /// ```
    ///
    /// The record is verbatim from `fedora44-guest-v1`. It cannot satisfy the
    /// RPM identity invariant `name-[epoch:]version-release.architecture`,
    /// because RPM reports its architecture as `(none)` and renders its NEVRA
    /// without an architecture suffix at all.
    #[test]
    fn the_rpm_keyring_is_not_part_of_the_installed_package_inventory() {
        let key = "gpg-pubkey-36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6-6786af3b\x1egpg-pubkey\x1e36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6\x1e6786af3b\x1e(none)\x1e(none)\x1eGnupg public key\x1egpg(Fedora (44))\x1epubkey\x1e(none)\x1e(none)\x1e(none)\x1e(none)\x1e1\x1f";
        let package = "bash-5.3.9-3.fc44.x86_64\x1ebash\x1e5.3.9\x1e3.fc44\x1e(none)\x1ex86_64\x1edescription\x1esummary\x1eGPLv3\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f";

        let records = parse_package_query_records(&format!("{key}{package}")).unwrap();

        assert_eq!(records.len(), 1, "the keyring record must not be inventory");
        assert_eq!(records[0].info.name, "bash");
    }

    /// The name alone does not license skipping a record. A `gpg-pubkey` header
    /// carrying a real architecture is not the keyring shape this recognises,
    /// and must still be parsed rather than silently vanish from the inventory.
    #[test]
    fn only_the_architecture_less_keyring_shape_is_skipped() {
        assert!(is_rpm_public_key_record("gpg-pubkey", "(none)"));
        assert!(is_rpm_public_key_record("gpg-pubkey", ""));
        assert!(!is_rpm_public_key_record("gpg-pubkey", "x86_64"));
        assert!(!is_rpm_public_key_record("bash", "(none)"));
    }

    #[test]
    fn malformed_rpm_keyring_records_fail_before_classification() {
        let malformed_epoch = "gpg-pubkey-keyid-created\x1egpg-pubkey\x1ekeyid\x1ecreated\x1enot-an-epoch\x1e(none)\x1edescription\x1esummary\x1epubkey\x1e(none)\x1e(none)\x1e(none)\x1e(none)\x1e1\x1f";
        let mismatched_nevra = "gpg-pubkey-wrong-created\x1egpg-pubkey\x1ekeyid\x1ecreated\x1e(none)\x1e(none)\x1edescription\x1esummary\x1epubkey\x1e(none)\x1e(none)\x1e(none)\x1e(none)\x1e1\x1f";

        assert!(parse_package_query_records(malformed_epoch).is_err());
        assert!(parse_package_query_records(mismatched_nevra).is_err());
    }

    #[test]
    fn package_inventory_rejects_malformed_or_duplicate_records() {
        assert!(parse_package_query_records("missing-fields\x1f").is_err());

        let record = "fixture-1-1.x86_64\x1efixture\x1e1\x1e1\x1e(none)\x1ex86_64\x1edescription\x1esummary\x1eMIT\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f";
        assert!(parse_package_query_records(&format!("{record}{record}")).is_err());
    }

    #[test]
    fn file_query_uses_exact_parallel_array_records() {
        let records = parse_rpm_file_records(
            "/usr/lib/libfixture.so\x1e42\x1e1700000000\x1eabcdef12\x1e0120777\x1eroot\x1eroot\x1elibfixture.so.1\x1f\
             /usr/share/fixture data\x1e0\x1e1700000001\x1e\x1e040755\x1eroot\x1eroot\x1e\x1f",
        )
        .unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].path, "/usr/lib/libfixture.so");
        assert_eq!(records[0].link_target.as_deref(), Some("libfixture.so.1"));
        assert_eq!(records[1].path, "/usr/share/fixture data");
        assert!(records[1].digest.is_none());
        assert_eq!(records[1].mode, 0o40755);

        let zero_digest = parse_rpm_file_records(
            "/usr/bin/zero\x1e1\x1e1700000002\x1e00000000\x1e0100755\x1eroot\x1eroot\x1e\x1f",
        )
        .unwrap();
        assert_eq!(zero_digest[0].digest.as_deref(), Some("00000000"));
    }

    #[test]
    fn file_query_rejects_malformed_parallel_array_records() {
        assert!(parse_rpm_file_records("/usr/bin/fixture\x1e42\x1f").is_err());
        assert!(
            parse_rpm_file_records(
                "/usr/bin/fixture\x1e42\x1e1700000000\x1enot-a-digest\x1e0100755\x1eroot\x1eroot\x1e\x1f"
            )
            .is_err()
        );
    }
}
