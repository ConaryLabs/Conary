// conary-core/src/repository/parsers/mod.rs

//! Repository metadata parsers for different package formats
//!
//! This module provides parsers for native repository metadata formats:
//! - Arch Linux: .db.tar.gz files
//! - Debian/Ubuntu: Packages.gz files
//! - Fedora/RPM: repomd.xml and primary.xml files

pub mod arch;
pub mod common;
pub mod debian;
pub mod eopkg;
pub mod fedora;
mod sink;
mod snapshot;

pub use sink::{
    ArchPackageFragmentKind, ArchPackageRecord, AuthenticatedProjectionInputV1,
    REPOSITORY_SNAPSHOT_PROJECTION_VERSION, RepositorySnapshotSink, SnapshotPackageIdentity,
    SnapshotPackageJoin, SnapshotProvideUpdate, SourceCandidatePreflightOutcome,
};
pub(crate) use sink::{CollectingRepositorySnapshotSink, validation_only_metadata_stream};
pub use snapshot::{
    AuthenticatedMetadataObject, AuthenticatedMetadataObjectRole, AuthenticatedSnapshotIdentity,
};

use crate::error::Result;
use crate::repository::dependency_model::{
    DebianMultiArch, RepositoryDependencyFlavor, RepositoryProvide, RepositoryRequirementGroup,
};
use crate::repository::versioning::VersionScheme;
use serde::{Deserialize, Serialize};

/// Repository metadata parser trait
pub trait RepositoryParser {
    /// Authenticate and stream one repository snapshot into `sink`.
    fn ingest_snapshot<S: RepositorySnapshotSink + Send>(
        &self,
        repo_url: &str,
        sink: &mut S,
    ) -> impl std::future::Future<Output = Result<AuthenticatedSnapshotIdentity>> + Send;
}

/// Package metadata extracted from repository
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    /// Package name
    pub name: String,

    /// Package version (format may vary by distribution)
    pub version: String,

    /// Architecture (x86_64, aarch64, noarch, all, any, etc.)
    pub architecture: Option<String>,

    /// Exact Debian `Multi-Arch` behavior; absent for non-Debian metadata.
    pub debian_multi_arch: Option<DebianMultiArch>,

    /// Short package description
    pub description: Option<String>,

    /// Package checksum (SHA-256 preferred)
    pub checksum: String,

    /// Checksum algorithm type
    pub checksum_type: ChecksumType,

    /// Compressed package size in bytes
    pub size: u64,

    /// Full URL to download the package file
    pub download_url: String,

    /// Additional format-specific metadata (stored as JSON)
    pub extra_metadata: serde_json::Value,

    /// Which native dependency grammar this metadata uses.
    pub dependency_flavor: RepositoryDependencyFlavor,

    /// The version comparison scheme that applies to `version`.
    pub version_scheme: VersionScheme,

    /// Normalized requirement groups (alternatives, conditional markers).
    ///
    /// Native parsers populate these as solver authority.
    pub requirements: Vec<RepositoryRequirementGroup>,

    /// Normalized provides (package name, virtual caps, sonames, files).
    ///
    /// Native parsers populate these as solver authority.
    pub provides: Vec<RepositoryProvide>,
}

/// Checksum algorithm type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChecksumType {
    /// SHA-1 carried by the authenticated eopkg index.
    Sha1,
    /// SHA-256 (preferred)
    Sha256,

    /// SHA-512 (also acceptable)
    Sha512,

    /// MD5 (upstream metadata compatibility only, not for security)
    #[serde(rename = "md5")]
    Md5,
}

impl PackageMetadata {
    /// Create minimal, explicitly typed package metadata for testing.
    pub fn new(
        name: String,
        version: String,
        checksum: String,
        size: u64,
        download_url: String,
        dependency_flavor: RepositoryDependencyFlavor,
        version_scheme: VersionScheme,
    ) -> Self {
        Self {
            name,
            version,
            architecture: None,
            debian_multi_arch: (version_scheme == VersionScheme::Debian)
                .then_some(DebianMultiArch::No),
            description: None,
            checksum,
            checksum_type: ChecksumType::Sha256,
            size,
            download_url,
            extra_metadata: serde_json::Value::Null,
            dependency_flavor,
            version_scheme,
            requirements: Vec::new(),
            provides: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_metadata_creation() {
        let pkg = PackageMetadata::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            "abc123".to_string(),
            1024,
            "https://example.com/package.tar.gz".to_string(),
            RepositoryDependencyFlavor::Conary,
            VersionScheme::Conary,
        );

        assert_eq!(pkg.name, "test-package");
        assert_eq!(pkg.version, "1.0.0");
        assert_eq!(pkg.size, 1024);
        assert_eq!(pkg.checksum_type, ChecksumType::Sha256);
    }
}
