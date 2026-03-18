// conary-core/src/packages/mod.rs

//! Package format support for Conary
//!
//! This module provides parsers and utilities for various package formats
//! (RPM, DEB, Arch). Each format implements the `PackageFormat` trait.

pub mod arch;
pub mod archive_utils;
pub mod common;
pub mod cpio;
pub mod deb;
pub mod dpkg_query;
pub mod pacman_query;
pub mod query_common;
pub mod registry;
pub mod rpm;
pub mod rpm_query;
pub mod traits;

pub use common::{PackageMetadata, PackageMetadataBuilder};
pub use registry::{PackageFormatType, detect_format, parse_package};

use crate::error::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

pub use query_common::{DependencyInfo, InstalledFileInfo};
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstalledSourceIdentity {
    pub source_distro: Option<String>,
    pub version_scheme: Option<String>,
}

impl SystemPackageManager {
    /// Detect the available system package manager
    pub fn detect() -> Self {
        if rpm_query::is_rpm_available() {
            Self::Rpm
        } else if dpkg_query::is_dpkg_available() {
            Self::Dpkg
        } else if pacman_query::is_pacman_available() {
            Self::Pacman
        } else {
            Self::Unknown
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
        match self {
            Self::Rpm => Some("rpm"),
            Self::Dpkg => Some("debian"),
            Self::Pacman => Some("arch"),
            Self::Unknown => None,
        }
    }

    pub fn detect_source_identity(&self) -> InstalledSourceIdentity {
        let Some(version_scheme) = self.version_scheme_name() else {
            return InstalledSourceIdentity::default();
        };

        let mut identity = std::fs::read_to_string("/etc/os-release")
            .ok()
            .map(|contents| detect_source_identity_from_os_release(*self, &contents))
            .unwrap_or_default();
        if identity.version_scheme.is_none() {
            identity.version_scheme = Some(version_scheme.to_string());
        }
        identity
    }
}

fn detect_source_identity_from_os_release(
    pkg_mgr: SystemPackageManager,
    contents: &str,
) -> InstalledSourceIdentity {
    let entries = parse_os_release(contents);
    let source_distro = entries.get("ID").map(|id| {
        let normalized_id = id.trim().to_ascii_lowercase();
        match entries
            .get("VERSION_ID")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            Some(version_id) if normalized_id != "arch" => format!("{normalized_id}-{version_id}"),
            _ => normalized_id,
        }
    });

    InstalledSourceIdentity {
        source_distro,
        version_scheme: pkg_mgr.version_scheme_name().map(str::to_string),
    }
}

fn parse_os_release(contents: &str) -> HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((
                key.trim().to_string(),
                strip_os_release_quotes(value.trim()),
            ))
        })
        .collect()
}

fn strip_os_release_quotes(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|inner| inner.strip_suffix('\''))
        })
        .unwrap_or(value)
        .to_string()
}

/// Result of extracting a package in parallel
pub struct ExtractedPackage {
    /// Original package path
    pub path: String,
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Extracted file contents
    pub files: Vec<ExtractedFile>,
    /// The parsed package (boxed for dynamic dispatch)
    pub package: Box<dyn PackageFormat + Send>,
}

impl std::fmt::Debug for ExtractedPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractedPackage")
            .field("path", &self.path)
            .field("name", &self.name)
            .field("version", &self.version)
            .field("files_count", &self.files.len())
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

            // Extract contents
            let files = package.extract_file_contents()?;

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
    fn detect_source_identity_uses_real_distro_for_fedora() {
        let identity = detect_source_identity_from_os_release(
            SystemPackageManager::Rpm,
            "ID=fedora\nVERSION_ID=43\nNAME=Fedora Linux\n",
        );

        assert_eq!(identity.source_distro.as_deref(), Some("fedora-43"));
        assert_eq!(identity.version_scheme.as_deref(), Some("rpm"));
    }

    #[test]
    fn detect_source_identity_uses_real_distro_for_ubuntu() {
        let identity = detect_source_identity_from_os_release(
            SystemPackageManager::Dpkg,
            "ID=ubuntu\nVERSION_ID=\"24.04\"\nVERSION_CODENAME=noble\n",
        );

        assert_eq!(identity.source_distro.as_deref(), Some("ubuntu-24.04"));
        assert_eq!(identity.version_scheme.as_deref(), Some("debian"));
    }

    #[test]
    fn detect_source_identity_handles_rolling_arch() {
        let identity = detect_source_identity_from_os_release(
            SystemPackageManager::Pacman,
            "ID=arch\nPRETTY_NAME=\"Arch Linux\"\n",
        );

        assert_eq!(identity.source_distro.as_deref(), Some("arch"));
        assert_eq!(identity.version_scheme.as_deref(), Some("arch"));
    }
}
