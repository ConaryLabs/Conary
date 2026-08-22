// conary-core/src/repository/remi_metadata.rs

//! Exact normalized package metadata exchanged by Remi and Conary.

use crate::repository::dependency_model::{ProvideArchitectureQualifier, ProvideVersionRelation};
use crate::repository::versioning::VersionScheme;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// Maximum package-name bytes admitted by Remi's public sparse-index routes.
///
/// This is a wire-contract bound, not an observed corpus size. Remi validates
/// every distro and package path component against the same limit before
/// querying its index.
pub const REMI_SPARSE_NAME_MAX_BYTES: usize = 256;

/// Maximum page size admitted by Remi's sparse package-name listing.
pub const REMI_SPARSE_MAX_PAGE_SIZE: usize = 1000;

/// Smallest repository-package artifact admitted by Remi's public index.
///
/// Historical zero-sized rows are discovery placeholders, not downloadable
/// package records. Every sparse list and lookup uses this shared threshold so
/// the list cannot advertise a name that the lookup route hides.
pub const REMI_SPARSE_MIN_PACKAGE_SIZE: i64 = 1;

/// Number of package names Conary requests and persists per sparse sync page.
///
/// This fixed page is the structural owner of the sync working set. Increasing
/// a distro from thousands to millions of package names increases page count,
/// not retained package metadata.
pub const REMI_SPARSE_SYNC_PAGE_SIZE: usize = 128;
const _: () = assert!(REMI_SPARSE_SYNC_PAGE_SIZE <= REMI_SPARSE_MAX_PAGE_SIZE);

/// Wire schema for the exact fields contributing to the sparse revision.
pub const REMI_SPARSE_PROJECTION_SCHEMA_VERSION: u32 = 1;

/// Strict 128-bit identity for one visible sparse projection state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiSparseStateId(String);

impl RemiSparseStateId {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.len() != 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("sparse state ID must be exactly 32 lowercase hexadecimal characters");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RemiSparseStateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for RemiSparseStateId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RemiSparseStateId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Exact activated profile revision for one sparse-resolution page set.
///
/// The sequence is the activation fencing epoch and the state ID is derived
/// from the verified immutable profile-revision digest. A client pins the
/// first value for the complete page set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparseRevision {
    pub projection_schema: u32,
    pub sequence: u64,
    pub state_id: RemiSparseStateId,
}

impl RemiSparseRevision {
    pub fn new(sequence: u64, state_id: impl Into<String>) -> Result<Self, &'static str> {
        Ok(Self {
            projection_schema: REMI_SPARSE_PROJECTION_SCHEMA_VERSION,
            sequence,
            state_id: RemiSparseStateId::parse(state_id)?,
        })
    }
}

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

/// Resolution-only sparse entry emitted by bulk sync pages.
///
/// Conversion state and content hashes belong to on-demand package lookup and
/// are deliberately absent here: repository sync persists native resolution
/// authority and does not consume serving-cache state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparseResolutionEntry {
    pub name: String,
    pub distro: String,
    pub versions: Vec<RemiSparseResolutionVersionEntry>,
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

/// One native resolution version emitted by a bulk sparse page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparseResolutionVersionEntry {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub provides: Vec<RemiProvide>,
    pub requirement_groups: Vec<RemiRequirementGroup>,
    pub architecture: Option<String>,
    pub size: i64,
    /// Persisted package metadata consumed by repository sync, including
    /// explicitly trusted security-advisory authority when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// One page from Remi's sparse package-name listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparsePackageList {
    pub distro: String,
    /// Exact supported source profile behind the stable distribution route.
    pub source_profile: String,
    pub packages: Vec<String>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
}

/// Optional sparse-list expansion selected by the `include` query field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemiSparseListInclude {
    #[default]
    Names,
    Versions,
}

/// One sparse sync page containing complete resolution entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparsePackagePage {
    pub distro: String,
    /// Exact supported source profile behind the stable distribution route.
    pub source_profile: String,
    /// Exact visible sparse-resolution state used to build this page.
    pub revision: RemiSparseRevision,
    pub packages: Vec<RemiSparseResolutionEntry>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
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
    fn sparse_revision_rejects_malformed_or_noncanonical_state_identity() {
        let malformed = serde_json::json!({
            "projection_schema": REMI_SPARSE_PROJECTION_SCHEMA_VERSION,
            "sequence": 7,
            "state_id": "ABCDEF"
        });
        let error = serde_json::from_value::<RemiSparseRevision>(malformed).unwrap_err();
        assert!(error.to_string().contains("32 lowercase hexadecimal"));
    }

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
