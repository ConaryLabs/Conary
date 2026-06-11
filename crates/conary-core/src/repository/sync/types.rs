// conary-core/src/repository/sync/types.rs

use crate::db::models::{
    RepositoryPackage, RepositoryPackageKey, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
use std::collections::HashMap;

/// A single synced package row with all its normalized capability data.
#[derive(Debug, Clone)]
pub(in crate::repository) struct SyncedPackageRow {
    pub(in crate::repository) package: RepositoryPackage,
    pub(in crate::repository) provides: Vec<RepositoryProvide>,
    pub(in crate::repository) requirements: Vec<RepositoryRequirement>,
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
    JsonFallback(JsonRepositorySyncSnapshot),
}

/// Owned JSON fallback metadata ready to persist.
#[derive(Debug, Clone)]
pub(in crate::repository) struct JsonRepositorySyncSnapshot {
    pub(in crate::repository) packages: Vec<RepositoryPackage>,
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
    #[allow(dead_code)] // Present in wire format; not used by sync logic
    pub(super) converted: bool,
    pub(super) architecture: Option<String>,
    pub(super) dependencies: Option<Vec<String>>,
    pub(super) metadata: Option<serde_json::Value>,
}

/// Owned canonical map response ready for a blocking persistence phase.
#[derive(Debug, serde::Deserialize)]
pub(super) struct CanonicalMapSnapshot {
    #[allow(dead_code)] // Wire format field; only entries is consumed
    pub(super) version: u32,
    #[allow(dead_code)] // Wire format field; only entries is consumed
    pub(super) generated_at: String,
    pub(super) entries: Vec<CanonicalMapEntry>,
}

/// Backward-compatible alias for existing tests and call sites.
#[cfg(test)]
pub(super) type CanonicalMapResponse = CanonicalMapSnapshot;

/// A single entry in the canonical map response.
#[derive(Debug, serde::Deserialize)]
pub(super) struct CanonicalMapEntry {
    pub(super) canonical: String,
    pub(super) implementations: HashMap<String, String>,
}
