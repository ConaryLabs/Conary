// crates/conary-core/src/repository/catalog/mod.rs

//! Immutable authenticated source and profile catalog contracts.

mod bundle;
mod contract;
mod profile;
mod record;
pub(in crate::repository) mod source;
mod store;

pub use bundle::{
    CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME, publish_profile_catalog_bundle,
    publish_source_catalog_bundle, verify_profile_catalog_bundle, verify_source_catalog_bundle,
    write_profile_catalog_manifest, write_source_catalog_manifest,
};
pub use contract::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V1, ProfileRevisionV1,
    ProfileSourceMemberV1, SOURCE_SNAPSHOT_SCHEMA_V1, SourceEcosystemV1,
    SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1, SourceSnapshotV1,
    SourceStreamKindV1, SourceStreamV1,
};
pub use profile::{ProfileCatalogCandidateV1, ProfileCatalogMemberInputV1};
pub use record::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1, CatalogScopeV1,
    CatalogSourceEvidenceV1,
};
pub use source::{SOURCE_CATALOG_PROJECTION_VERSION_V1, SourceCatalogCandidateV1};
pub use store::{CatalogBindingV1, CatalogReader, write_catalog_candidate};
