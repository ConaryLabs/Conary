// crates/conary-core/src/repository/catalog/mod.rs

//! Immutable authenticated source and profile catalog contracts.

mod contract;

pub use contract::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V1, ProfileRevisionV1,
    ProfileSourceMemberV1, SOURCE_SNAPSHOT_SCHEMA_V1, SourceEcosystemV1,
    SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1, SourceSnapshotV1,
    SourceStreamKindV1, SourceStreamV1,
};
