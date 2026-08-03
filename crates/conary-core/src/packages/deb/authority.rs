// conary-core/src/packages/deb/authority.rs

//! Exact Debian identity and declared-provision authority.

use crate::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianDeclaredCapability {
    pub control_index: u32,
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianPackageAuthority {
    pub name: String,
    pub epoch: Option<u32>,
    pub source_version: String,
    pub version: String,
    pub architecture: String,
    pub multi_arch: DebianMultiArch,
    pub provides: Vec<DebianDeclaredCapability>,
}
