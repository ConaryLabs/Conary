// conary-core/src/repository/metadata.rs

//! Repository metadata data structures
//!
//! Contains types for representing repository and package metadata
//! from JSON repository indexes.

use serde::{Deserialize, Serialize};

use super::dependency_model::RepositoryRequirementGroup;
use super::versioning::VersionScheme;

/// Repository metadata format (simple JSON index)
#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryMetadata {
    pub name: String,
    pub version: String,
    pub security_advisory_source: Option<SecurityAdvisorySourceMetadata>,
    pub packages: Vec<PackageMetadata>,
}

/// Trusted advisory source declaration for a JSON repository index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAdvisorySourceMetadata {
    pub name: String,
    pub trust: String,
    pub url: Option<String>,
}

/// Delta update information for a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaInfo {
    pub from_version: String,
    pub from_hash: String,
    pub delta_url: String,
    pub delta_size: i64,
    pub delta_checksum: String,
    pub compression_ratio: f64,
}

/// Package metadata in repository index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    /// Exact native version ordering contract for this package.
    pub version_scheme: VersionScheme,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub checksum: String,
    pub size: i64,
    pub download_url: String,
    /// Exact positive package requirements.
    #[serde(default)]
    pub requirements: Vec<RepositoryRequirementGroup>,
    /// Exact negative package relations.
    #[serde(default)]
    pub relations: Vec<RepositoryRequirementGroup>,
    /// Available delta updates from previous versions
    pub delta_from: Option<Vec<DeltaInfo>>,
    /// Security advisory metadata proving that this package fixes vulnerabilities.
    pub security_advisory: Option<PackageSecurityAdvisoryMetadata>,
}

/// Per-package security advisory metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSecurityAdvisoryMetadata {
    pub id: String,
    pub source: Option<String>,
    pub source_trust: Option<String>,
    pub severity: Option<String>,
    #[serde(default)]
    pub cves: Vec<String>,
    pub fixed_version: Option<String>,
    pub url: Option<String>,
}
