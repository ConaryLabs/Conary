// conary-core/src/packages/eopkg/authority.rs

//! Exact eopkg package identity and declaration authority.

use crate::packages::config_authority::ConfigPayloadAssociation;
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EopkgConfigDeclaration {
    pub files_index: u32,
    pub path: String,
    pub permanent: bool,
    pub payload: ConfigPayloadAssociation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EopkgDeclaredCapability {
    pub metadata_index: u32,
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

/// The pair `(version, release)` is eopkg identity. Dependency ranges may bind
/// either field; update selection compares the monotonically increasing
/// integer release after distribution-release handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EopkgPackageAuthority {
    pub name: String,
    pub version: String,
    pub release: u64,
    pub distribution: String,
    pub distribution_release: String,
    pub architecture: String,
    pub package_format: String,
    pub provides: Vec<EopkgDeclaredCapability>,
    pub config: Vec<EopkgConfigDeclaration>,
}

impl EopkgPackageAuthority {
    #[must_use]
    pub fn selector(&self) -> String {
        format!(
            "{}-{}-{}-{}",
            self.name, self.version, self.release, self.architecture
        )
    }

    #[must_use]
    pub fn release_string(&self) -> String {
        self.release.to_string()
    }
}
