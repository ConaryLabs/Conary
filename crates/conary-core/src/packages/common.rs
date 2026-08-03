// conary-core/src/packages/common.rs
//! Common structures and utilities shared across package parsers
//!
//! This module provides the `PackageMetadata` struct that captures fields
//! common to all package formats (RPM, DEB, Arch), reducing duplication
//! and ensuring consistent behavior.

use crate::db::models::{Trove, TroveType};
use crate::packages::source_authority::{CcsPackageAuthority, SourcePackageAuthority};
use crate::packages::traits::{
    ConfigFileInfo, DiagnosticScriptletEvidence, NativeScriptletEntry, PackageFile,
};
use crate::repository::dependency_model::ProvidedCapability;
use crate::repository::dependency_model::{DebianMultiArch, RepositoryRequirementGroup};
use crate::repository::versioning::VersionScheme;
use std::path::{Path, PathBuf};

/// Common metadata shared by all package formats
///
/// This struct contains the core fields that every package format provides.
/// Format-specific parsers should embed this struct and delegate trait
/// method implementations to it.
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    /// Path to the package file
    pub package_path: PathBuf,
    /// Sole source identity and declared-capability authority.
    pub source_authority: SourcePackageAuthority,
    /// Package description/summary
    pub description: Option<String>,
    /// Files contained in the package
    pub files: Vec<PackageFile>,
    /// Exact positive package requirements and sole dependency authority.
    pub requirements: Vec<RepositoryRequirementGroup>,
    /// Exact source-native conflict and replacement relations.
    pub relations: Vec<RepositoryRequirementGroup>,
    /// Non-authoritative flattened script text for conversion diagnostics only.
    pub diagnostic_scriptlet_evidence: Vec<DiagnosticScriptletEvidence>,
    /// Byte-preserving native package-manager scriptlet ABI entries.
    pub native_scriptlet_abi: Vec<NativeScriptletEntry>,
    /// Configuration files with special handling
    pub config_files: Vec<ConfigFileInfo>,
}

impl PackageMetadata {
    pub fn from_package(
        package_path: PathBuf,
        package: &dyn crate::packages::traits::PackageFormat,
    ) -> crate::error::Result<Self> {
        Ok(Self {
            package_path,
            source_authority: package.source_authority()?,
            description: package.description().map(str::to_string),
            files: package.files().to_vec(),
            requirements: package.requirements().to_vec(),
            relations: package.relations().to_vec(),
            diagnostic_scriptlet_evidence: Vec::new(),
            native_scriptlet_abi: package.native_scriptlet_abi().to_vec(),
            config_files: package.config_files().to_vec(),
        })
    }

    /// Create new metadata with required fields
    pub fn new(
        package_path: PathBuf,
        name: String,
        version: String,
        version_scheme: VersionScheme,
    ) -> Self {
        Self {
            package_path,
            source_authority: SourcePackageAuthority::Ccs(CcsPackageAuthority {
                name,
                version,
                version_scheme,
                architecture: None,
                debian_multi_arch: None,
                capabilities: Vec::new(),
            }),
            description: None,
            files: Vec::new(),
            requirements: Vec::new(),
            relations: Vec::new(),
            diagnostic_scriptlet_evidence: Vec::new(),
            native_scriptlet_abi: Vec::new(),
            config_files: Vec::new(),
        }
    }

    /// Get the package name
    pub fn name(&self) -> &str {
        self.source_authority.name()
    }

    /// Get the package version
    pub fn version(&self) -> &str {
        self.source_authority.version()
    }

    /// Get the native version algebra.
    pub fn version_scheme(&self) -> VersionScheme {
        self.source_authority.version_scheme()
    }

    /// Get the package architecture
    pub fn architecture(&self) -> Option<&str> {
        self.source_authority.architecture()
    }

    /// Get exact Debian `Multi-Arch` behavior.
    pub fn debian_multi_arch(&self) -> Option<DebianMultiArch> {
        self.source_authority.debian_multi_arch()
    }

    /// Get the package description
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Get the list of files
    pub fn files(&self) -> &[PackageFile] {
        &self.files
    }

    /// Get the exact positive package requirements and sole dependency authority.
    pub fn requirements(&self) -> &[RepositoryRequirementGroup] {
        &self.requirements
    }

    /// Get the list of native provides
    pub fn resolution_capabilities(&self) -> crate::error::Result<Vec<ProvidedCapability>> {
        self.source_authority.resolution_capabilities()
    }

    /// Get exact source-native conflict and replacement relations.
    pub fn relations(&self) -> &[RepositoryRequirementGroup] {
        &self.relations
    }

    /// Get non-authoritative flattened script text for diagnostics.
    pub fn diagnostic_scriptlet_evidence(&self) -> &[DiagnosticScriptletEvidence] {
        &self.diagnostic_scriptlet_evidence
    }

    /// Get byte-preserving native package-manager scriptlet ABI entries.
    pub fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        &self.native_scriptlet_abi
    }

    /// Get the configuration files
    pub fn config_files(&self) -> &[ConfigFileInfo] {
        &self.config_files
    }

    /// Convert to a Trove representation
    ///
    /// This is the standard conversion used by all package formats.
    pub fn to_trove(&self) -> Trove {
        let mut trove = Trove::new(
            self.name().to_string(),
            self.version().to_string(),
            TroveType::Package,
            self.version_scheme(),
        );

        trove.architecture = self.architecture().map(str::to_string);
        trove.debian_multi_arch = self.debian_multi_arch();
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
            VersionScheme::Conary,
        );

        assert_eq!(meta.name(), "test-package");
        assert_eq!(meta.version(), "1.0.0");
        assert!(meta.architecture().is_none());
        assert!(meta.description().is_none());
        assert!(meta.files().is_empty());
        assert!(meta.native_scriptlet_abi().is_empty());
    }

    #[test]
    fn test_package_metadata_with_optional_fields() {
        let mut meta = PackageMetadata::new(
            PathBuf::from("/tmp/test.rpm"),
            "my-package".to_string(),
            "2.0.0".to_string(),
            VersionScheme::Rpm,
        );
        let SourcePackageAuthority::Ccs(authority) = &mut meta.source_authority else {
            unreachable!()
        };
        authority.architecture = Some("x86_64".to_string());
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
            VersionScheme::Debian,
        );
        let SourcePackageAuthority::Ccs(authority) = &mut meta.source_authority else {
            unreachable!()
        };
        authority.architecture = Some("aarch64".to_string());
        meta.description = Some("Example package".to_string());

        let trove = meta.to_trove();

        assert_eq!(trove.name, "example");
        assert_eq!(trove.version, "1.2.3");
        assert_eq!(trove.version_scheme, VersionScheme::Debian);
        assert_eq!(trove.architecture, Some("aarch64".to_string()));
        assert_eq!(trove.description, Some("Example package".to_string()));
    }
}
