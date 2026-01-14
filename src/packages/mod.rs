// src/packages/mod.rs

//! Package format support for Conary
//!
//! This module provides parsers and utilities for various package formats
//! (RPM, DEB, Arch). Each format implements the `PackageFormat` trait.

pub mod arch;
pub mod deb;
pub mod dpkg_query;
pub mod pacman_query;
pub mod rpm;
pub mod rpm_query;
pub mod traits;

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

    info!(
        "Extracting {} packages in parallel...",
        package_paths.len()
    );

    package_paths
        .par_iter()
        .map(|(name, path)| {
            let path_str = path.to_string_lossy();
            info!("Extracting package: {} from {}", name, path_str);

            // Detect format and parse
            let package: Box<dyn PackageFormat + Send> = if path_str.ends_with(".rpm") {
                Box::new(rpm::RpmPackage::parse(&path_str)?)
            } else if path_str.ends_with(".deb") {
                Box::new(deb::DebPackage::parse(&path_str)?)
            } else if path_str.ends_with(".pkg.tar.zst") || path_str.ends_with(".pkg.tar.xz") {
                Box::new(arch::ArchPackage::parse(&path_str)?)
            } else {
                return Err(crate::error::Error::InitError(format!(
                    "Unknown package format: {}",
                    path_str
                )));
            };

            // Extract contents
            let files = package.extract_file_contents()?;

            Ok(ExtractedPackage {
                path: path_str.to_string(),
                name: package.name().to_string(),
                version: package.version().to_string(),
                files,
                package,
            })
        })
        .collect()
}
