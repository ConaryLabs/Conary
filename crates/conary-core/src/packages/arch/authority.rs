// conary-core/src/packages/arch/authority.rs

//! Exact ALPM identity and declared-provision authority.

use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlpmDeclaredCapability {
    pub pkginfo_index: u32,
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlpmPackageAuthority {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub package_type: Option<String>,
    pub provides: Vec<AlpmDeclaredCapability>,
}
