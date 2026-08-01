// conary-core/src/packages/pacman_query.rs

//! Query installed pacman packages from the system database
//!
//! This module provides functions to query the local pacman database
//! using the `pacman` command-line tool for Arch Linux systems.

use crate::error::{Error, Result};
use crate::packages::InstalledPackageIdentity;
use crate::packages::archive_utils::get_file_metadata;
use crate::packages::install_reason::{InstallReasonAuthorityError, query_package_names};
use crate::packages::query_common::{
    InstalledFileAbsencePolicy, InstalledFileInfo, InstalledPackageRecord,
};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::requirement::parse_native_requirement;
use crate::repository::versioning::VersionScheme;
use std::collections::HashSet;
use std::process::Command;
use tracing::debug;

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

/// Query detailed information about an installed package
pub fn query_package(name: &str) -> Result<InstalledPackageRecord<InstalledPacmanInfo>> {
    debug!("Querying package info: {}", name);

    let output = Command::new("pacman")
        .args(["-Qi", name])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{}' not found in pacman database",
            name
        )));
    }

    let info_str = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("pacman -Qi output is not UTF-8: {error}")))?;
    let mut records = parse_all_package_info(&info_str)?;
    if records.len() > 1 {
        return Err(Error::ConflictError(format!(
            "Package '{name}' matches multiple installed pacman packages: {}. Use an exact native package name.",
            records
                .iter()
                .map(|record| record.identity.selector())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    records.pop().ok_or_else(|| {
        Error::NotFound(format!(
            "Package '{name}' returned no pacman database record"
        ))
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
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{}' not found in pacman database",
            name
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("pacman -Ql output is not UTF-8: {error}")))?;
    let mut files = Vec::new();

    for (index, line) in stdout.lines().enumerate() {
        // Format: "package_name /path/to/file"
        let (reported_name, reported_path) = line.split_once(' ').ok_or_else(|| {
            Error::ParseError(format!(
                "pacman -Ql record {} is outside the documented package/path grammar",
                index + 1
            ))
        })?;
        if reported_name != name {
            return Err(Error::ConflictError(format!(
                "pacman -Ql query for '{name}' returned a record owned by '{reported_name}'"
            )));
        }
        let mut path = reported_path;
        if path.is_empty() {
            return Err(Error::ParseError(format!(
                "pacman -Ql record {} has an empty path",
                index + 1
            )));
        }
        if path.len() > 1 {
            path = path.trim_end_matches('/');
        }
        let path = path.to_string();

        let (size, mode) = get_file_metadata(&path).map_err(|error| {
            Error::IoError(format!(
                "pacman record '{name}' owns '{path}', but its exact live metadata is unavailable: {error}"
            ))
        })?;

        // Check if this is a symlink and get target
        let link_target = if (mode & 0o170000) == 0o120000 {
            Some(
                std::fs::read_link(&path)
                    .map_err(|error| {
                        Error::IoError(format!(
                            "failed to read exact symlink target for pacman path '{path}': {error}"
                        ))
                    })?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| {
                        Error::ParseError(format!(
                            "pacman path '{path}' has a non-UTF-8 symlink target"
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
            digest: None,
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

/// Query ALPM's installed runtime requirements as exact typed atoms.
pub fn query_package_requirement_groups(name: &str) -> Result<Vec<RepositoryRequirementGroup>> {
    let output = Command::new("pacman")
        .args(["-Qi", name])
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| Error::InitError(format!("Failed to run pacman: {error}")))?;
    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "Package '{name}' not found in pacman database"
        )));
    }

    let info = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("pacman -Qi output is not UTF-8: {error}")))?;
    let depends = parse_pacman_info_field(&info, "Depends On")?;
    depends
        .split_whitespace()
        .filter(|requirement| *requirement != "None")
        .map(|requirement| {
            parse_native_requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Arch,
                requirement,
            )
            .map_err(Error::ParseError)
        })
        .collect()
}

/// Query what a package provides (capabilities it offers)
///
/// Returns a list of capability strings from the Provides field,
/// plus the package name itself as a provide.
pub fn query_package_provides(name: &str) -> Result<Vec<String>> {
    debug!("Querying provides for pacman package: {}", name);

    // Use pacman -Qi and extract the Provides line
    let output = Command::new("pacman")
        .args(["-Qi", name])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "pacman -Qi {} failed: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("pacman -Qi output is not UTF-8: {error}")))?;
    let values = parse_pacman_info_field(&stdout, "Provides")?;
    let mut provides = vec![name.to_string()];
    if values != "None" {
        provides.extend(values.split_whitespace().map(str::to_string));
    }

    debug!("Package {} provides {} capabilities", name, provides.len());
    Ok(provides)
}

fn parse_pacman_info_field(output: &str, requested_key: &str) -> Result<String> {
    let mut value: Option<String> = None;
    let mut collecting = false;
    for (index, line) in output.lines().enumerate() {
        if line
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            if collecting {
                let continuation = line.trim();
                if !continuation.is_empty() {
                    let target = value.as_mut().expect("collecting field has value");
                    if !target.is_empty() {
                        target.push(' ');
                    }
                    target.push_str(continuation);
                }
            }
            continue;
        }
        let (key, field_value) = line.split_once(':').ok_or_else(|| {
            Error::ParseError(format!(
                "pacman -Qi line {} is outside the documented key/value grammar",
                index + 1
            ))
        })?;
        collecting = key.trim() == requested_key;
        if collecting && value.replace(field_value.trim().to_string()).is_some() {
            return Err(Error::ParseError(format!(
                "pacman -Qi repeats required field '{requested_key}'"
            )));
        }
    }
    value.ok_or_else(|| {
        Error::ParseError(format!(
            "pacman -Qi omitted required field '{requested_key}'"
        ))
    })
}

/// Query every installed pacman database record without collapsing variants.
pub fn query_all_packages() -> Result<Vec<InstalledPackageRecord<InstalledPacmanInfo>>> {
    debug!("Querying all installed pacman packages with info");

    // `pacman -Qi` reads every record from ALPM's local package database and
    // includes each package's exact Architecture field. Force the documented
    // C field names so parsing never depends on the operator's locale.
    let output = Command::new("pacman")
        .arg("-Qi")
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| Error::InitError(format!("Failed to run pacman -Qi: {error}")))?;
    if !output.status.success() {
        return Err(Error::InitError(format!(
            "pacman -Qi failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("pacman -Qi output is not UTF-8: {error}")))?;
    let packages = parse_all_package_info(&stdout)?;
    debug!("Queried {} installed packages", packages.len());
    Ok(packages)
}

#[derive(Default)]
struct PacmanInfoFields {
    name: Option<String>,
    version: Option<String>,
    arch: Option<String>,
    description: Option<String>,
    url: Option<String>,
    licenses: Option<String>,
    installed_size: Option<String>,
}

fn parse_all_package_info(
    output: &str,
) -> Result<Vec<InstalledPackageRecord<InstalledPacmanInfo>>> {
    fn finish_record(
        fields: &mut PacmanInfoFields,
        packages: &mut Vec<InstalledPackageRecord<InstalledPacmanInfo>>,
        selectors: &mut HashSet<String>,
        record: usize,
    ) -> Result<()> {
        if fields.name.is_none()
            && fields.version.is_none()
            && fields.arch.is_none()
            && fields.description.is_none()
            && fields.url.is_none()
            && fields.licenses.is_none()
            && fields.installed_size.is_none()
        {
            return Ok(());
        }

        let name = fields.name.take().ok_or_else(|| {
            Error::ParseError(format!("pacman -Qi record {record} has no Name field"))
        })?;
        let version = fields.version.take().ok_or_else(|| {
            Error::ParseError(format!(
                "pacman -Qi record {record} for '{name}' has no Version field"
            ))
        })?;
        let arch = fields.arch.take().ok_or_else(|| {
            Error::ParseError(format!(
                "pacman -Qi record {record} for '{name}' has no Architecture field"
            ))
        })?;
        if name.is_empty() || version.is_empty() || arch.is_empty() {
            return Err(Error::ParseError(format!(
                "pacman -Qi record {record} has an empty required Name, Version, or Architecture field"
            )));
        }

        let identity = InstalledPackageIdentity::pacman(&name, &name, &version, &arch)?;
        if !selectors.insert(identity.selector().to_string()) {
            return Err(Error::ConflictError(format!(
                "pacman -Qi reported duplicate installed package identity '{name}'"
            )));
        }
        let installed_size = match fields.installed_size.take() {
            Some(value) => Some(parse_size(&value).ok_or_else(|| {
                Error::ParseError(format!(
                    "pacman -Qi record {record} for '{name}' has invalid Installed Size {value:?}"
                ))
            })?),
            None => None,
        };
        let info = InstalledPacmanInfo {
            name: name.clone(),
            version,
            arch,
            description: fields.description.take().filter(|value| value != "None"),
            url: fields.url.take().filter(|value| value != "None"),
            licenses: fields.licenses.take().filter(|value| value != "None"),
            installed_size,
        };
        packages.push(InstalledPackageRecord { identity, info });
        Ok(())
    }

    let mut packages = Vec::new();
    let mut selectors = HashSet::new();
    let mut fields = PacmanInfoFields::default();
    let mut record = 1;
    for (line_index, line) in output.lines().enumerate() {
        if line.is_empty() {
            finish_record(&mut fields, &mut packages, &mut selectors, record)?;
            fields = PacmanInfoFields::default();
            record += 1;
            continue;
        }
        if line
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            // Continuation lines belong to descriptive fields that are not
            // package identity authority.
            continue;
        }
        let (key, value) = line.split_once(':').ok_or_else(|| {
            Error::ParseError(format!(
                "pacman -Qi line {} is outside the documented key/value grammar: {line:?}",
                line_index + 1
            ))
        })?;
        let value = value.trim().to_string();
        let target = match key.trim() {
            "Name" => Some(&mut fields.name),
            "Version" => Some(&mut fields.version),
            "Architecture" => Some(&mut fields.arch),
            "Description" => Some(&mut fields.description),
            "URL" => Some(&mut fields.url),
            "Licenses" => Some(&mut fields.licenses),
            "Installed Size" => Some(&mut fields.installed_size),
            _ => None,
        };
        if let Some(target) = target
            && target.replace(value).is_some()
        {
            return Err(Error::ParseError(format!(
                "pacman -Qi record {record} repeats field '{}'",
                key.trim()
            )));
        }
    }
    finish_record(&mut fields, &mut packages, &mut selectors, record)?;
    Ok(packages)
}

/// Query which package(s) own a file
pub fn query_file_owner(path: &str) -> Result<Vec<String>> {
    let output = Command::new("pacman")
        .args(["-Qqo", path])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman: {}", e)))?;

    if !output.status.success() {
        return Err(Error::NotFound(format!(
            "pacman could not resolve an owner for {path:?} (status {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let output = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("pacman owner output is not UTF-8: {error}")))?;
    output
        .lines()
        .map(|owner| {
            if owner.is_empty()
                || owner
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(Error::ParseError(format!(
                    "pacman quiet owner query returned an invalid package name {owner:?}"
                )));
            }
            Ok(owner.to_string())
        })
        .collect()
}

/// Query ALPM's exact explicitly-installed package set.
///
/// Pacman's documented `-Qe` filter selects explicit installs and `-q`
/// produces one exact package name per line.
pub fn query_user_installed()
-> std::result::Result<std::collections::HashSet<String>, InstallReasonAuthorityError> {
    debug!("Querying explicitly-installed pacman packages via pacman -Qqe");
    query_package_names("ALPM", "pacman", &["-Qqe"])
}

/// Remove a package from the pacman database only (no files deleted).
///
/// Uses `pacman -Rdd --dbonly --noconfirm` to remove the package record
/// from the pacman database without touching any files on disk. This transfers
/// ownership of the files from pacman to Conary.
pub fn remove_from_db_only(name: &str) -> Result<()> {
    debug!("Removing {} from pacman database only (--dbonly)", name);

    let output = Command::new("pacman")
        .args(["-Rdd", "--dbonly", "--noconfirm", name])
        .output()
        .map_err(|e| Error::InitError(format!("Failed to run pacman -Rdd --dbonly: {}", e)))?;

    if !output.status.success() {
        return Err(Error::InitError(format!(
            "pacman -Rdd --dbonly {} failed: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    debug!("Successfully removed {} from pacman database", name);
    Ok(())
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

    #[test]
    fn all_package_query_uses_each_alpm_architecture() {
        let output = "\
Name            : native-tool
Version         : 1.0-1
Description     : Native tool
Architecture    : x86_64
URL             : https://example.invalid/native
Licenses        : MIT

Name            : portable-data
Version         : 2.0-3
Description     : Portable data
Architecture    : any
URL             : None
Licenses        : CC0
";

        let packages = parse_all_package_info(output).unwrap();
        assert_eq!(packages[0].info.arch, "x86_64");
        assert_eq!(packages[0].identity.selector(), "native-tool");
        assert_eq!(packages[1].info.arch, "any");
        assert_eq!(packages[1].info.url, None);
    }

    #[test]
    fn all_package_query_rejects_missing_architecture() {
        let error = parse_all_package_info(
            "\
Name            : ambiguous
Version         : 1.0-1
",
        )
        .unwrap_err();

        assert!(error.to_string().contains("has no Architecture field"));
    }

    #[test]
    fn all_package_query_rejects_duplicate_identity() {
        let error = parse_all_package_info(
            "\
Name            : duplicate
Version         : 1.0-1
Architecture    : x86_64

Name            : duplicate
Version         : 1.0-2
Architecture    : any
",
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("duplicate installed package identity")
        );
    }

    #[test]
    fn info_field_parser_preserves_documented_continuation_lines() {
        let output = "\
Name            : fixture
Depends On      : glibc>=2.41  openssl
                  zlib
Provides        : fixture-abi
";
        assert_eq!(
            parse_pacman_info_field(output, "Depends On").unwrap(),
            "glibc>=2.41  openssl zlib"
        );
        assert!(parse_pacman_info_field(output, "Missing").is_err());
    }
}
