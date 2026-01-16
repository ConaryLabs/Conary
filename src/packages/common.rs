// src/packages/common.rs
//! Common structures and utilities shared across package parsers
//!
//! This module provides the `PackageMetadata` struct that captures fields
//! common to all package formats (RPM, DEB, Arch), reducing duplication
//! and ensuring consistent behavior.

use crate::db::models::{Trove, TroveType};
use crate::packages::traits::{ConfigFileInfo, Dependency, PackageFile, Scriptlet};
use std::path::PathBuf;

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
    pub fn scriptlets(&self) -> Vec<Scriptlet> {
        self.scriptlets.clone()
    }

    /// Get the configuration files
    pub fn config_files(&self) -> Vec<ConfigFileInfo> {
        self.config_files.clone()
    }

    /// Convert to a Trove representation
    ///
    /// This is the standard conversion used by all package formats.
    pub fn to_trove(&self) -> Trove {
        let mut trove = Trove::new(
            self.name.clone(),
            self.version.clone(),
            TroveType::Package,
        );

        trove.architecture = self.architecture.clone();
        trove.description = self.description.clone();

        trove
    }

    /// Get the package file path
    pub fn package_path(&self) -> &PathBuf {
        &self.package_path
    }
}

/// Builder for PackageMetadata to make construction cleaner
#[derive(Debug, Default)]
pub struct PackageMetadataBuilder {
    package_path: Option<PathBuf>,
    name: Option<String>,
    version: Option<String>,
    architecture: Option<String>,
    description: Option<String>,
    files: Vec<PackageFile>,
    dependencies: Vec<Dependency>,
    scriptlets: Vec<Scriptlet>,
    config_files: Vec<ConfigFileInfo>,
}

impl PackageMetadataBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the package path
    pub fn package_path(mut self, path: PathBuf) -> Self {
        self.package_path = Some(path);
        self
    }

    /// Set the package name
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the package version
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set the architecture
    pub fn architecture(mut self, arch: impl Into<String>) -> Self {
        self.architecture = Some(arch.into());
        self
    }

    /// Set the description
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the files list
    pub fn files(mut self, files: Vec<PackageFile>) -> Self {
        self.files = files;
        self
    }

    /// Set the dependencies list
    pub fn dependencies(mut self, deps: Vec<Dependency>) -> Self {
        self.dependencies = deps;
        self
    }

    /// Set the scriptlets list
    pub fn scriptlets(mut self, scriptlets: Vec<Scriptlet>) -> Self {
        self.scriptlets = scriptlets;
        self
    }

    /// Set the config files list
    pub fn config_files(mut self, config_files: Vec<ConfigFileInfo>) -> Self {
        self.config_files = config_files;
        self
    }

    /// Build the PackageMetadata
    ///
    /// # Panics
    /// Panics if package_path, name, or version are not set.
    pub fn build(self) -> PackageMetadata {
        PackageMetadata {
            package_path: self.package_path.expect("package_path is required"),
            name: self.name.expect("name is required"),
            version: self.version.expect("version is required"),
            architecture: self.architecture,
            description: self.description,
            files: self.files,
            dependencies: self.dependencies,
            scriptlets: self.scriptlets,
            config_files: self.config_files,
        }
    }

    /// Try to build the PackageMetadata, returning None if required fields are missing
    pub fn try_build(self) -> Option<PackageMetadata> {
        Some(PackageMetadata {
            package_path: self.package_path?,
            name: self.name?,
            version: self.version?,
            architecture: self.architecture,
            description: self.description,
            files: self.files,
            dependencies: self.dependencies,
            scriptlets: self.scriptlets,
            config_files: self.config_files,
        })
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
    fn test_package_metadata_builder() {
        let meta = PackageMetadataBuilder::new()
            .package_path(PathBuf::from("/tmp/test.rpm"))
            .name("my-package")
            .version("2.0.0")
            .architecture("x86_64")
            .description("A test package")
            .build();

        assert_eq!(meta.name(), "my-package");
        assert_eq!(meta.version(), "2.0.0");
        assert_eq!(meta.architecture(), Some("x86_64"));
        assert_eq!(meta.description(), Some("A test package"));
    }

    #[test]
    fn test_to_trove() {
        let meta = PackageMetadataBuilder::new()
            .package_path(PathBuf::from("/tmp/test.deb"))
            .name("example")
            .version("1.2.3")
            .architecture("aarch64")
            .description("Example package")
            .build();

        let trove = meta.to_trove();

        assert_eq!(trove.name, "example");
        assert_eq!(trove.version, "1.2.3");
        assert_eq!(trove.architecture, Some("aarch64".to_string()));
        assert_eq!(trove.description, Some("Example package".to_string()));
    }

    #[test]
    fn test_try_build_missing_fields() {
        let result = PackageMetadataBuilder::new()
            .name("incomplete")
            .try_build();

        assert!(result.is_none());
    }
}
