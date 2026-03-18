// conary-core/src/packages/common.rs
//! Common structures and utilities shared across package parsers
//!
//! This module provides the `PackageMetadata` struct that captures fields
//! common to all package formats (RPM, DEB, Arch), reducing duplication
//! and ensuring consistent behavior.

use crate::db::models::{Trove, TroveType};
use crate::packages::traits::{ConfigFileInfo, Dependency, PackageFile, Scriptlet};
use std::path::{Path, PathBuf};

/// Maximum size for a single file during package extraction (512 MB).
pub const MAX_EXTRACTION_FILE_SIZE: u64 = 512 * 1024 * 1024;

/// Normalize architecture strings across distros.
///
/// Maps Debian "all", Arch "any", and RPM "noarch" to a canonical "noarch".
/// All other values pass through unchanged.
pub fn normalize_architecture(arch: &str) -> &str {
    match arch {
        "all" | "any" | "noarch" => "noarch",
        other => other,
    }
}

/// Common metadata shared by all package formats
///
/// This struct contains the core fields that every package format provides.
/// Format-specific parsers should embed this struct and delegate trait
/// method implementations to it.
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    /// Path to the package file
    pub package_path: PathBuf,
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Target architecture (e.g., "x86_64", "aarch64", "noarch")
    pub architecture: Option<String>,
    /// Package description/summary
    pub description: Option<String>,
    /// Files contained in the package
    pub files: Vec<PackageFile>,
    /// Package dependencies
    pub dependencies: Vec<Dependency>,
    /// Install/remove scriptlets
    pub scriptlets: Vec<Scriptlet>,
    /// Configuration files with special handling
    pub config_files: Vec<ConfigFileInfo>,
}

impl PackageMetadata {
    /// Create new metadata with required fields
    pub fn new(package_path: PathBuf, name: String, version: String) -> Self {
        Self {
            package_path,
            name,
            version,
            architecture: None,
            description: None,
            files: Vec::new(),
            dependencies: Vec::new(),
            scriptlets: Vec::new(),
            config_files: Vec::new(),
        }
    }

    /// Get the package name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the package version
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Get the package architecture
    pub fn architecture(&self) -> Option<&str> {
        self.architecture.as_deref()
    }

    /// Get the package description
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Get the list of files
    pub fn files(&self) -> &[PackageFile] {
        &self.files
    }

    /// Get the list of dependencies
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    /// Get the scriptlets
    pub fn scriptlets(&self) -> &[Scriptlet] {
        &self.scriptlets
    }

    /// Get the configuration files
    pub fn config_files(&self) -> &[ConfigFileInfo] {
        &self.config_files
    }

    /// Convert to a Trove representation
    ///
    /// This is the standard conversion used by all package formats.
    pub fn to_trove(&self) -> Trove {
        let mut trove = Trove::new(self.name.clone(), self.version.clone(), TroveType::Package);

        trove.architecture = self.architecture.clone();
        trove.description = self.description.clone();

        trove
    }

    /// Get the package file path
    pub fn package_path(&self) -> &Path {
        &self.package_path
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_metadata_new() {
        let meta = PackageMetadata::new(
            PathBuf::from("/tmp/test.pkg"),
            "test-package".to_string(),
            "1.0.0".to_string(),
        );

        assert_eq!(meta.name(), "test-package");
        assert_eq!(meta.version(), "1.0.0");
        assert!(meta.architecture().is_none());
        assert!(meta.description().is_none());
        assert!(meta.files().is_empty());
    }

    #[test]
    fn test_package_metadata_with_optional_fields() {
        let mut meta = PackageMetadata::new(
            PathBuf::from("/tmp/test.rpm"),
            "my-package".to_string(),
            "2.0.0".to_string(),
        );
        meta.architecture = Some("x86_64".to_string());
        meta.description = Some("A test package".to_string());

        assert_eq!(meta.name(), "my-package");
        assert_eq!(meta.version(), "2.0.0");
        assert_eq!(meta.architecture(), Some("x86_64"));
        assert_eq!(meta.description(), Some("A test package"));
    }

    #[test]
    fn test_to_trove() {
        let mut meta = PackageMetadata::new(
            PathBuf::from("/tmp/test.deb"),
            "example".to_string(),
            "1.2.3".to_string(),
        );
        meta.architecture = Some("aarch64".to_string());
        meta.description = Some("Example package".to_string());

        let trove = meta.to_trove();

        assert_eq!(trove.name, "example");
        assert_eq!(trove.version, "1.2.3");
        assert_eq!(trove.architecture, Some("aarch64".to_string()));
        assert_eq!(trove.description, Some("Example package".to_string()));
    }

    #[test]
    fn test_normalize_architecture() {
        use super::normalize_architecture;
        assert_eq!(normalize_architecture("all"), "noarch");
        assert_eq!(normalize_architecture("any"), "noarch");
        assert_eq!(normalize_architecture("noarch"), "noarch");
        assert_eq!(normalize_architecture("x86_64"), "x86_64");
        assert_eq!(normalize_architecture("aarch64"), "aarch64");
    }
}
