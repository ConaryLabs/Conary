// conary-core/src/packages/mod.rs

//! Package format support for Conary
//!
//! This module provides parsers and utilities for various package formats
//! (RPM, DEB, Arch). Each format implements the `PackageFormat` trait.

pub mod arch;
pub mod archive_utils;
pub mod config_authority;
pub mod cpio;
pub mod deb;
pub mod dpkg_query;
pub mod install_reason;
pub mod installed_identity;
pub mod native_abi;
#[doc(hidden)]
pub mod native_scriptlet_support;
pub mod pacman_query;
pub mod payload;
pub mod query_common;
pub mod registry;
pub mod rpm;
pub mod rpm_query;
pub mod source_authority;
pub mod traits;

pub use installed_identity::InstalledPackageIdentity;
pub use registry::{PackageFormatType, detect_format, parse_package};

use crate::error::{Error, Result};
use crate::repository::versioning::VersionScheme;
use rayon::prelude::*;
use std::path::Path;
use std::process::Command;
use tracing::info;

pub use payload::{PackagePayload, PackagePayloadFile, ReopenablePayload};
pub use query_common::{InstalledFileAbsencePolicy, InstalledFileInfo, InstalledPackageRecord};
pub use rpm_query::InstalledRpmInfo;
pub use traits::{ExtractedFile, PackageFormat};

/// Detect the system package manager
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPackageManager {
    Rpm,
    Dpkg,
    Pacman,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeDatabaseObservation {
    manager: SystemPackageManager,
    package_count: usize,
}

impl SystemPackageManager {
    /// Resolve a system package manager from recorded package version metadata.
    pub const fn from_version_scheme(version_scheme: VersionScheme) -> Option<Self> {
        match version_scheme {
            VersionScheme::Conary => None,
            VersionScheme::Rpm => Some(Self::Rpm),
            VersionScheme::Debian => Some(Self::Dpkg),
            VersionScheme::Arch => Some(Self::Pacman),
        }
    }

    /// Detect package authority from exact, non-empty native package databases.
    ///
    /// Installed query binaries are only discovery. Their parsed inventories
    /// provide authority. Multiple populated databases are an explicit
    /// ambiguity and never resolve through binary precedence.
    pub fn detect() -> Result<Self> {
        Self::resolve(None)
    }

    /// Resolve native package authority, honoring an explicit typed selection.
    ///
    /// An explicit selection is accepted only when its query binary is
    /// available and its exact native inventory is populated.
    pub fn resolve(requested: Option<Self>) -> Result<Self> {
        if requested == Some(Self::Unknown) {
            return Err(Error::ConfigError(
                "unknown is not a selectable native package-manager authority".to_string(),
            ));
        }

        let mut observations = Vec::new();

        for manager in [Self::Rpm, Self::Dpkg, Self::Pacman] {
            if requested.is_some_and(|requested| requested != manager) {
                continue;
            }
            let (binary, args) = match manager {
                Self::Rpm => ("rpm", &["--version"][..]),
                Self::Dpkg => ("dpkg-query", &["--version"][..]),
                Self::Pacman => ("pacman", &["--version"][..]),
                Self::Unknown => unreachable!("unknown authority was rejected"),
            };
            if query_binary_available(binary, args)? {
                let package_count = match manager {
                    Self::Rpm => rpm_query::query_all_packages()?.len(),
                    Self::Dpkg => dpkg_query::query_all_packages()?.len(),
                    Self::Pacman => pacman_query::query_all_packages()?.len(),
                    Self::Unknown => unreachable!("unknown authority was rejected"),
                };
                observations.push(NativeDatabaseObservation {
                    manager,
                    package_count,
                });
            }
        }

        Self::from_native_database_observations(&observations, requested)
    }

    fn from_native_database_observations(
        observations: &[NativeDatabaseObservation],
        requested: Option<Self>,
    ) -> Result<Self> {
        if let Some(requested) = requested {
            let observation = observations
                .iter()
                .find(|observation| observation.manager == requested)
                .ok_or_else(|| {
                    Error::InitError(format!(
                        "Selected native package-manager authority '{}' is unavailable: its query binary could not be found",
                        requested.cli_name()
                    ))
                })?;
            if observation.package_count == 0 {
                return Err(Error::InitError(format!(
                    "Selected native package-manager authority '{}' has an empty native package database",
                    requested.cli_name()
                )));
            }
            return Ok(requested);
        }

        let populated = observations
            .iter()
            .filter(|observation| observation.package_count > 0)
            .collect::<Vec<_>>();
        match populated.as_slice() {
            [] => Ok(Self::Unknown),
            [observation] => Ok(observation.manager),
            _ => {
                let databases = populated
                    .iter()
                    .map(|observation| {
                        format!(
                            "{} ({} packages)",
                            observation.manager.display_name(),
                            observation.package_count
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(Error::ConflictError(format!(
                    "Multiple populated native package databases were detected: {databases}. \
                     Conary will not guess package authority; select one with \
                     '--package-manager rpm', '--package-manager dpkg', or \
                     '--package-manager pacman'."
                )))
            }
        }
    }

    /// Check if any supported package manager is available
    pub fn is_available(&self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// Human-readable name for display
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Rpm => "RPM",
            Self::Dpkg => "dpkg",
            Self::Pacman => "pacman",
            Self::Unknown => "unknown",
        }
    }

    /// Stable CLI spelling for explicit native authority selection.
    pub fn cli_name(&self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Dpkg => "dpkg",
            Self::Pacman => "pacman",
            Self::Unknown => "unknown",
        }
    }

    /// Build the command string to remove a package via the system PM
    pub fn remove_command(&self, name: &str) -> String {
        match self {
            Self::Rpm => format!("dnf remove {}", name),
            Self::Dpkg => format!("apt remove {}", name),
            Self::Pacman => format!("pacman -R {}", name),
            Self::Unknown => format!("(unknown package manager) remove {}", name),
        }
    }

    /// Build the command string to update a package via the system PM
    pub fn update_command(&self, name: &str) -> String {
        match self {
            Self::Rpm => format!("dnf update {}", name),
            Self::Dpkg => format!("apt upgrade {}", name),
            Self::Pacman => format!("pacman -Syu {}", name),
            Self::Unknown => format!("(unknown package manager) update {}", name),
        }
    }

    pub fn version_scheme_name(&self) -> Option<&'static str> {
        self.version_scheme().map(VersionScheme::as_str)
    }

    pub const fn version_scheme(self) -> Option<VersionScheme> {
        match self {
            Self::Rpm => Some(VersionScheme::Rpm),
            Self::Dpkg => Some(VersionScheme::Debian),
            Self::Pacman => Some(VersionScheme::Arch),
            Self::Unknown => None,
        }
    }
}

fn query_binary_available(binary: &str, version_args: &[&str]) -> Result<bool> {
    match Command::new(binary).args(version_args).output() {
        Ok(output) if output.status.success() => Ok(true),
        Ok(output) => Err(Error::InitError(format!(
            "Native package query binary '{binary}' exists but its version probe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::InitError(format!(
            "Failed to probe native package query binary '{binary}': {error}"
        ))),
    }
}

/// Result of extracting a package in parallel
pub struct ExtractedPackage {
    /// Original package path
    pub path: String,
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Independently reopenable payload sources
    pub files: PackagePayload,
    /// The parsed package (boxed for dynamic dispatch)
    pub package: Box<dyn PackageFormat + Send>,
}

impl std::fmt::Debug for ExtractedPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractedPackage")
            .field("path", &self.path)
            .field("name", &self.name)
            .field("version", &self.version)
            .field("files_count", &self.files.files().len())
            .finish()
    }
}

/// Extract multiple packages in parallel
///
/// This function parses and extracts the contents of multiple packages
/// concurrently, significantly speeding up multi-package installations.
///
/// # Arguments
/// * `package_paths` - List of (name, path) tuples for packages to extract
///
/// # Returns
/// Vector of extracted packages with their file contents
pub fn extract_packages_parallel(
    package_paths: &[(String, &Path)],
) -> Vec<Result<ExtractedPackage>> {
    if package_paths.is_empty() {
        return Vec::new();
    }

    info!("Extracting {} packages in parallel...", package_paths.len());

    package_paths
        .par_iter()
        .map(|(name, path)| {
            info!("Extracting package: {} from {}", name, path.display());

            // Detect format and parse using registry
            let package = parse_package(path)?;

            let files = package.package_payload()?;

            Ok(ExtractedPackage {
                path: path.to_string_lossy().to_string(),
                name: package.name().to_string(),
                version: package.version().to_string(),
                files,
                package,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_update_commands_cover_supported_package_managers() {
        assert_eq!(
            SystemPackageManager::Rpm.update_command("curl"),
            "dnf update curl"
        );
        assert_eq!(
            SystemPackageManager::Dpkg.update_command("curl"),
            "apt upgrade curl"
        );
        assert_eq!(
            SystemPackageManager::Pacman.update_command("curl"),
            "pacman -Syu curl"
        );
    }

    #[test]
    fn native_manager_identity_comes_from_recorded_version_scheme() {
        assert_eq!(
            SystemPackageManager::from_version_scheme(VersionScheme::Rpm),
            Some(SystemPackageManager::Rpm)
        );
        assert_eq!(
            SystemPackageManager::from_version_scheme(VersionScheme::Debian),
            Some(SystemPackageManager::Dpkg)
        );
        assert_eq!(
            SystemPackageManager::from_version_scheme(VersionScheme::Arch),
            Some(SystemPackageManager::Pacman)
        );
        assert_eq!(
            SystemPackageManager::from_version_scheme(VersionScheme::Conary),
            None
        );
    }

    #[test]
    fn native_manager_detection_requires_one_populated_database() {
        assert_eq!(
            SystemPackageManager::from_native_database_observations(&[], None).unwrap(),
            SystemPackageManager::Unknown
        );
        assert_eq!(
            SystemPackageManager::from_native_database_observations(
                &[
                    NativeDatabaseObservation {
                        manager: SystemPackageManager::Rpm,
                        package_count: 0,
                    },
                    NativeDatabaseObservation {
                        manager: SystemPackageManager::Dpkg,
                        package_count: 42,
                    },
                ],
                None
            )
            .unwrap(),
            SystemPackageManager::Dpkg
        );
    }

    #[test]
    fn native_manager_detection_rejects_multiple_populated_databases() {
        let error = SystemPackageManager::from_native_database_observations(
            &[
                NativeDatabaseObservation {
                    manager: SystemPackageManager::Rpm,
                    package_count: 12,
                },
                NativeDatabaseObservation {
                    manager: SystemPackageManager::Dpkg,
                    package_count: 34,
                },
            ],
            None,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("RPM (12 packages)"), "{error}");
        assert!(error.contains("dpkg (34 packages)"), "{error}");
        assert!(
            error.contains("will not guess package authority"),
            "{error}"
        );
        assert!(error.contains("--package-manager dpkg"), "{error}");
    }

    #[test]
    fn explicit_native_manager_selection_requires_its_populated_database() {
        let observations = [
            NativeDatabaseObservation {
                manager: SystemPackageManager::Rpm,
                package_count: 12,
            },
            NativeDatabaseObservation {
                manager: SystemPackageManager::Dpkg,
                package_count: 34,
            },
        ];
        assert_eq!(
            SystemPackageManager::from_native_database_observations(
                &observations,
                Some(SystemPackageManager::Dpkg),
            )
            .unwrap(),
            SystemPackageManager::Dpkg
        );

        let empty = [NativeDatabaseObservation {
            manager: SystemPackageManager::Pacman,
            package_count: 0,
        }];
        let error = SystemPackageManager::from_native_database_observations(
            &empty,
            Some(SystemPackageManager::Pacman),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("empty native package database"), "{error}");

        let error = SystemPackageManager::from_native_database_observations(
            &[],
            Some(SystemPackageManager::Rpm),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("query binary could not be found"), "{error}");
    }
}
