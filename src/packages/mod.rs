// src/packages/mod.rs

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
pub mod registry;
pub mod rpm;
pub mod rpm_query;
pub mod traits;

pub use common::{PackageMetadata, PackageMetadataBuilder};
pub use registry::{PackageFormatType, detect_format, parse_package};

use crate::error::Result;
use rayon::prelude::*;
use std::path::Path;
use tracing::info;

pub use rpm_query::{DependencyInfo, InstalledFileInfo, InstalledRpmInfo};
pub use traits::{ExtractedFile, PackageFormat};

/// Detect the system package manager
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPackageManager {
    Rpm,
    Dpkg,
    Pacman,
    Unknown,
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
