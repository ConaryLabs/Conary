// crates/conary-core/src/repository/catalog/mod.rs

//! Immutable authenticated source and profile catalog contracts.

mod contract;
mod record;
mod store;

pub use contract::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V1, ProfileRevisionV1,
    ProfileSourceMemberV1, SOURCE_SNAPSHOT_SCHEMA_V1, SourceEcosystemV1,
    SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1, SourceSnapshotV1,
    SourceStreamKindV1, SourceStreamV1,
};
pub use record::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1, CatalogScopeV1,
    CatalogSourceEvidenceV1,
};
pub use store::{CatalogBindingV1, CatalogReader, write_catalog_candidate};
