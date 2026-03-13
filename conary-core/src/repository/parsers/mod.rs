// conary-core/src/repository/parsers/mod.rs

//! Repository metadata parsers for different package formats
//!
//! This module provides parsers for native repository metadata formats:
//! - Arch Linux: .db.tar.gz files
//! - Debian/Ubuntu: Packages.gz files
//! - Fedora/RPM: repomd.xml and primary.xml files

pub mod arch;
pub mod debian;
pub mod fedora;

use crate::error::Result;
use crate::repository::dependency_model::{
    RepositoryDependencyFlavor, RepositoryProvide, RepositoryRequirementGroup,
};
use crate::repository::versioning::VersionScheme;
use serde::{Deserialize, Serialize};

/// Repository metadata parser trait
pub trait RepositoryParser {
    /// Parse repository metadata from a base URL
    ///
    /// Downloads and parses the repository's metadata files, returning
    /// a list of all packages available in the repository.
    fn sync_metadata(&self, repo_url: &str) -> Result<Vec<PackageMetadata>>;
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

    /// Package dependencies (legacy flat list, kept for transition compatibility).
    pub dependencies: Vec<Dependency>,

    /// Additional format-specific metadata (stored as JSON)
    pub extra_metadata: serde_json::Value,

    // -- Normalized native semantics (Step 4) ----------------------------------

    /// Which distro ecosystem this metadata came from.
    ///
    /// `None` for legacy parsers that have not been updated yet.
    pub source_distro: Option<RepositoryDependencyFlavor>,

    /// The version comparison scheme that applies to `version`.
    ///
    /// `None` for legacy parsers that have not been updated yet.
    pub version_scheme: Option<VersionScheme>,

    /// Normalized requirement groups (alternatives, conditional markers).
    ///
    /// Empty until the parser populates them; the legacy `dependencies` field
    /// remains as a transition compatibility layer.
    pub requirements: Vec<RepositoryRequirementGroup>,

    /// Normalized provides (package name, virtual caps, sonames, files).
    ///
    /// Empty until the parser populates them.
    pub provides: Vec<RepositoryProvide>,
}

/// Package dependency information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// Dependency package name
    pub name: String,

    /// Version constraint (e.g., ">= 1.0.0", "= 2.3.4-1")
    pub constraint: Option<String>,

    /// Type of dependency
    pub dep_type: DependencyType,

    /// Optional description (for optional dependencies)
    pub description: Option<String>,
}

/// Type of package dependency
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyType {
    /// Required runtime dependency
    Runtime,

    /// Optional/recommended dependency
    Optional,

    /// Build-time only dependency
    Build,
}

/// Checksum algorithm type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChecksumType {
    /// SHA-256 (preferred)
    Sha256,

    /// SHA-512 (also acceptable)
    Sha512,

    /// MD5 (legacy, not for security)
    #[serde(rename = "md5")]
    Md5,
}

impl PackageMetadata {
    /// Create minimal package metadata for testing
    pub fn new(
        name: String,
        version: String,
        checksum: String,
        size: u64,
        download_url: String,
    ) -> Self {
        Self {
            name,
            version,
            architecture: None,
            description: None,
            checksum,
            checksum_type: ChecksumType::Sha256,
            size,
            download_url,
            dependencies: Vec::new(),
            extra_metadata: serde_json::Value::Null,
            source_distro: None,
            version_scheme: None,
            requirements: Vec::new(),
            provides: Vec::new(),
        }
    }
}

impl Dependency {
    /// Create a runtime dependency with no version constraint
    pub fn runtime(name: String) -> Self {
        Self {
            name,
            constraint: None,
            dep_type: DependencyType::Runtime,
            description: None,
        }
    }

    /// Create a runtime dependency with a version constraint
    pub fn runtime_versioned(name: String, constraint: String) -> Self {
        Self {
            name,
            constraint: Some(constraint),
            dep_type: DependencyType::Runtime,
            description: None,
        }
    }

    /// Create an optional dependency
    pub fn optional(name: String, description: Option<String>) -> Self {
        Self {
            name,
            constraint: None,
            dep_type: DependencyType::Optional,
            description,
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
        );

        assert_eq!(pkg.name, "test-package");
        assert_eq!(pkg.version, "1.0.0");
        assert_eq!(pkg.size, 1024);
        assert_eq!(pkg.checksum_type, ChecksumType::Sha256);
    }

    #[test]
    fn test_dependency_creation() {
        let dep = Dependency::runtime("glibc".to_string());
        assert_eq!(dep.name, "glibc");
        assert_eq!(dep.dep_type, DependencyType::Runtime);
        assert!(dep.constraint.is_none());

        let versioned = Dependency::runtime_versioned("libc".to_string(), ">= 2.34".to_string());
        assert_eq!(versioned.constraint, Some(">= 2.34".to_string()));
    }
}
