// crates/conary-core/src/repository/catalog/mod.rs

//! Immutable authenticated source and profile catalog contracts.

mod bundle;
mod candidate;
mod contract;
mod parity;
mod profile;
mod record;
pub(in crate::repository) mod source;
mod store;

pub use bundle::{
    CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME, PublishedCatalogBundle,
    publish_profile_catalog_bundle, publish_profile_catalog_bundle_with_provenance,
    publish_source_catalog_bundle, publish_source_catalog_bundle_with_provenance,
    verify_profile_catalog_bundle, verify_source_catalog_bundle, write_profile_catalog_manifest,
    write_source_catalog_manifest,
};
pub use candidate::CatalogCandidateWriter;
pub use contract::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2, ProfileRevisionV2,
    ProfileSourceMemberV2, SOURCE_SNAPSHOT_SCHEMA_V1, SourceEcosystemV1,
    SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1, SourceSnapshotV1,
    SourceStreamKindV1, SourceStreamV1,
};
#[cfg(feature = "native-alpm-oracle")]
pub use parity::{
    ALPM_PARITY_PROJECTION_SCHEMA_V1, AlpmParityMemberInput, produce_alpm_parity_oracle,
};
pub use parity::{
    NATIVE_PARITY_COMPARISON_SCHEMA_V1, NATIVE_PARITY_MANIFEST_FILE_NAME,
    NATIVE_PARITY_ORACLE_SCHEMA_V1, NATIVE_PARITY_PACKAGE_FILE_NAME, NativeParityArtifactV1,
    NativeParityComparisonError, NativeParityComparisonV1, NativeParityCountsV1,
    NativeParityEcosystemV1, NativeParityFactV1, NativeParityImplementationV1,
    NativeParityMismatchV1, NativeParityOracleReader, NativeParityOracleV1,
    NativeParityOracleWriter, NativeParityPackageIdentityV1, NativeParityPackageV1,
    compare_native_parity_oracle, verify_native_parity_oracle_bundle,
    write_native_parity_oracle_manifest,
};
pub use profile::{
    ProfileCatalogCandidateV2, ProfileCatalogMemberInputV2, write_profile_catalog_candidate,
};
pub use record::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1, CatalogScopeV1,
    CatalogSourceEvidenceV1,
};
pub use source::{SOURCE_CATALOG_PROJECTION_VERSION_V1, SourceCatalogCandidateV1};
pub use store::{
    CatalogBindingV1, CatalogPackageNamePageV1, CatalogReader, write_catalog_candidate,
};
