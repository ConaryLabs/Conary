// conary-core/src/repository/sync/types.rs

use crate::db::models::{
    RepositoryPackage, RepositoryPackageKey, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
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
