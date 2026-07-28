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

/// Maximum page size admitted by Remi's sparse package-name listing.
pub const REMI_SPARSE_MAX_PAGE_SIZE: usize = 1000;

/// Number of package names Conary requests and persists per sparse sync page.
///
/// This fixed page is the structural owner of the sync working set. Increasing
/// a distro from thousands to millions of package names increases page count,
/// not retained package metadata.
pub const REMI_SPARSE_SYNC_PAGE_SIZE: usize = 128;
const _: () = assert!(REMI_SPARSE_SYNC_PAGE_SIZE <= REMI_SPARSE_MAX_PAGE_SIZE);

/// Maximum compact JSON bytes for a requested sparse package-name page.
///
/// A page contains at most `per_page` names, each at most 256 input bytes.
/// JSON escaping can expand one input byte to at most six ASCII bytes
/// (`\u00XX`). The remaining fixed envelope contains the distro, pagination
/// counters, field names, punctuation, and integer text.
pub const fn sparse_package_list_max_bytes(per_page: usize) -> u64 {
    const JSON_ESCAPE_EXPANSION: usize = 6;
    const STRING_DELIMITERS_AND_COMMA: usize = 3;
    const FIXED_ENVELOPE_BYTES: usize = 2048;

    (per_page
        .saturating_mul(
            REMI_SPARSE_NAME_MAX_BYTES
                .saturating_mul(JSON_ESCAPE_EXPANSION)
                .saturating_add(STRING_DELIMITERS_AND_COMMA),
        )
        .saturating_add(FIXED_ENVELOPE_BYTES)) as u64
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

/// One page from Remi's sparse package-name listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiSparsePackageList {
    pub distro: String,
    pub packages: Vec<String>,
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
