// conary-core/src/repository/remi_metadata.rs

//! Exact normalized package metadata exchanged by Remi and Conary.

use crate::repository::dependency_model::{ProvideArchitectureQualifier, ProvideVersionRelation};
use crate::repository::versioning::VersionScheme;
use serde::{Deserialize, Serialize};

/// Maximum package-name bytes admitted by Remi's public sparse-index routes.
///
/// This is a wire-contract bound, not an observed corpus size. Remi validates
/// every distro and package path component against the same limit before
/// querying its index.
pub const REMI_SPARSE_NAME_MAX_BYTES: usize = 256;

/// Smallest repository-package artifact admitted by Remi's public index.
///
/// Historical zero-sized rows are discovery placeholders, not downloadable
/// package records. Every sparse list and lookup uses this shared threshold so
/// the list cannot advertise a name that the lookup route hides.
pub const REMI_SPARSE_MIN_PACKAGE_SIZE: i64 = 1;

/// Validate one public Remi route component or sparse package name.
///
/// The server applies this before exposing database-owned names and the client
/// applies the same contract before persisting a page. Keeping the rule here
/// prevents the two sides from admitting different sparse-index names.
pub fn validate_remi_public_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name must not be empty");
    }
    if name.len() > REMI_SPARSE_NAME_MAX_BYTES {
        return Err("name exceeds the public repository wire limit");
    }
    if name.contains('/') || name.contains("..") || name.contains('\0') {
        return Err("name contains invalid characters");
    }
    Ok(())
}

/// One sparse-index document for a package name across all versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparseIndexEntry {
    pub name: String,
    pub distro: String,
    pub versions: Vec<RemiSparseVersionEntry>,
}

/// One version emitted by a sparse package document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparseVersionEntry {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub provides: Vec<RemiProvide>,
    pub requirement_groups: Vec<RemiRequirementGroup>,
    pub architecture: Option<String>,
    pub size: i64,
    pub converted: bool,
    pub content_hash: Option<String>,
}

/// A normalized capability provided by one repository package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemiProvide {
    pub capability: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub kind: String,
    pub raw: Option<String>,
    pub version_scheme: VersionScheme,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

/// A normalized requirement clause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemiRequirement {
    pub capability: String,
    pub version_constraint: Option<String>,
    pub kind: String,
    pub dependency_type: String,
    pub raw: Option<String>,
}

/// A normalized native requirement group and its exact clauses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemiRequirementGroup {
    pub kind: String,
    pub behavior: String,
    pub description: Option<String>,
    pub native_text: Option<String>,
    pub expression_json: String,
    pub clauses: Vec<RemiRequirement>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_name_validation_is_the_shared_sparse_wire_contract() {
        assert!(validate_remi_public_name("kernel-core").is_ok());
        assert!(validate_remi_public_name("").is_err());
        assert!(validate_remi_public_name("../escape").is_err());
        assert!(validate_remi_public_name("bad/name").is_err());
        assert!(validate_remi_public_name("bad\0name").is_err());
        assert!(validate_remi_public_name(&"x".repeat(REMI_SPARSE_NAME_MAX_BYTES + 1)).is_err());
    }
}
