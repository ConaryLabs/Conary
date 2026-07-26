// conary-core/src/repository/sync/types.rs

use crate::db::models::{
    RepositoryPackage, RepositoryPackageKey, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
use crate::repository::remi_metadata::{RemiProvide, RemiRequirementGroup};

/// A single synced package row with all its normalized capability data.
#[derive(Debug, Clone)]
pub(in crate::repository) struct SyncedPackageRow {
    pub(in crate::repository) package: RepositoryPackage,
    pub(in crate::repository) provides: Vec<RepositoryProvide>,
    pub(in crate::repository) requirement_groups: Vec<DbRequirementGroup>,
    pub(in crate::repository) requirement_group_clauses: Vec<Vec<RepositoryRequirement>>,
}

/// Owned package metadata ready to persist for a repository sync.
#[derive(Debug, Clone)]
pub(in crate::repository) enum RepositorySyncSnapshot {
    NativeRows(Vec<SyncedPackageRow>),
    StaticRows {
        packages: Vec<SyncedPackageRow>,
        package_keys: Vec<RepositoryPackageKey>,
    },
    JsonContract(JsonRepositorySyncSnapshot),
}

/// Owned metadata from a repository that declares the Conary JSON contract.
#[derive(Debug, Clone)]
pub(in crate::repository) struct JsonRepositorySyncSnapshot {
    pub(in crate::repository) packages: Vec<SyncedPackageRow>,
    pub(in crate::repository) deltas: Vec<JsonPackageDelta>,
}

/// Owned package delta data from JSON repository metadata.
#[derive(Debug, Clone)]
pub(in crate::repository) struct JsonPackageDelta {
    pub(in crate::repository) package_name: String,
    pub(in crate::repository) from_version: String,
    pub(in crate::repository) to_version: String,
    pub(in crate::repository) from_hash: String,
    pub(in crate::repository) to_hash: String,
    pub(in crate::repository) delta_url: String,
    pub(in crate::repository) delta_size: i64,
    pub(in crate::repository) delta_checksum: String,
    pub(in crate::repository) target_size: i64,
}

/// Response from Remi metadata API (`GET /v1/{distro}/metadata`).
#[derive(Debug, serde::Deserialize)]
pub(super) struct RemiMetadataResponse {
    pub(super) packages: Vec<RemiPackageEntry>,
}

/// Individual package entry from Remi metadata.
#[derive(Debug, serde::Deserialize)]
pub(super) struct RemiPackageEntry {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) release: Option<String>,
    #[allow(dead_code)] // Present in wire format; not used by sync logic
    pub(super) converted: bool,
    pub(super) architecture: Option<String>,
    pub(super) provides: Vec<RemiProvide>,
    pub(super) requirement_groups: Vec<RemiRequirementGroup>,
    pub(super) metadata: Option<serde_json::Value>,
}
