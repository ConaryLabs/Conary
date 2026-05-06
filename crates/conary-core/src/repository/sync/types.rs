// conary-core/src/repository/sync/types.rs

use crate::db::models::{
    RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
use std::collections::HashMap;

/// A single synced package row with all its normalized capability data.
#[derive(Debug, Clone)]
pub(super) struct SyncedPackageRow {
    pub(super) package: RepositoryPackage,
    pub(super) provides: Vec<RepositoryProvide>,
    pub(super) requirements: Vec<RepositoryRequirement>,
    pub(super) requirement_groups: Vec<DbRequirementGroup>,
    pub(super) requirement_group_clauses: Vec<Vec<RepositoryRequirement>>,
}

/// Owned package metadata ready to persist for a repository sync.
#[derive(Debug, Clone)]
pub(super) enum RepositorySyncSnapshot {
    NativeRows(Vec<SyncedPackageRow>),
    JsonFallback(JsonRepositorySyncSnapshot),
}

/// Owned JSON fallback metadata ready to persist.
#[derive(Debug, Clone)]
pub(super) struct JsonRepositorySyncSnapshot {
    pub(super) packages: Vec<RepositoryPackage>,
    pub(super) deltas: Vec<JsonPackageDelta>,
}

/// Owned package delta data from JSON repository metadata.
#[derive(Debug, Clone)]
pub(super) struct JsonPackageDelta {
    pub(super) package_name: String,
    pub(super) from_version: String,
    pub(super) to_version: String,
    pub(super) from_hash: String,
    pub(super) to_hash: String,
    pub(super) delta_url: String,
    pub(super) delta_size: i64,
    pub(super) delta_checksum: String,
    pub(super) target_size: i64,
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
