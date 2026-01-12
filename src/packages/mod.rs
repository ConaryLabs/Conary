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

pub use rpm_query::{InstalledFileInfo, InstalledRpmInfo};
pub use traits::PackageFormat;

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
