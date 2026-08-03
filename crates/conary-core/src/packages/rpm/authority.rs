// conary-core/src/packages/rpm/authority.rs

//! Exact RPM identity and declared-provision authority.

use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmDeclaredCapability {
    pub header_index: u32,
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmPackageAuthority {
    pub name: String,
    pub epoch: Option<u32>,
    pub version: String,
    pub release: String,
    pub evr: String,
    pub architecture: String,
    pub provides: Vec<RpmDeclaredCapability>,
}
