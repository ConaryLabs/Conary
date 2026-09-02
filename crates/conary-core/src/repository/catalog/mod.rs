// crates/conary-core/src/repository/catalog/mod.rs

//! Immutable authenticated source and profile catalog contracts.

mod bundle;
mod candidate;
mod capacity;
mod contract;
mod parity;
mod portable_integrity;
mod portable_vfs;
mod profile;
mod record;
pub(in crate::repository) mod source;
mod store;

pub(in crate::repository) use bundle::retain_source_metadata_object;
pub use bundle::{
    CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME, CATALOG_PORTABLE_MANIFEST_FILE_NAME,
    PublishedVerifiedCatalogBundle, SOURCE_METADATA_DIRECTORY_NAME,
    authenticate_registered_profile_catalog_layout, authenticate_registered_source_catalog_layout,
    publish_profile_catalog_bundle_verified, publish_source_catalog_bundle_verified,
    source_metadata_object_path, verify_profile_catalog_bundle,
    verify_registered_profile_catalog_bundle, verify_registered_profile_catalog_bundle_complete,
    verify_registered_source_catalog_bundle, verify_registered_source_catalog_bundle_complete,
    verify_source_catalog_bundle, write_profile_catalog_manifest,
    write_profile_catalog_manifest_verified, write_source_catalog_manifest,
    write_source_catalog_manifest_verified,
};
pub use candidate::CatalogCandidateWriter;
pub use capacity::{
    CATALOG_COPY_SCRATCH_SCHEMA_V1, CATALOG_FINALIZATION_SCRATCH_SCHEMA_V2,
    CATALOG_METADATA_SCRATCH_SCHEMA_V1, CATALOG_METADATA_STREAM_SCRATCH_SCHEMA_V1,
    CATALOG_PROFILE_CANDIDATE_SCRATCH_SCHEMA_V1, CATALOG_PROJECTION_SPOOL_SCRATCH_SCHEMA_V1,
    CATALOG_SOURCE_CANDIDATE_SCRATCH_SCHEMA_V1, CATALOG_SQLITE_PAGE_SIZE_V1,
    CATALOG_SQLITE_SCHEMA_PAGE_COUNT_V1, CatalogCopyScratchV1, CatalogFinalizationScratchV2,
    CatalogMetadataObjectScratchV1, CatalogMetadataScratchV1, CatalogMetadataStreamAdmission,
    CatalogMetadataStreamScratchV1, CatalogProfileCandidateScratchV1,
    CatalogProfileMemberScratchV1, CatalogProjectionSpoolScratchV1, CatalogScratchAdmission,
    CatalogScratchCapacityError, CatalogSourceCandidateScratchV1,
};
pub use contract::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2, ProfileRevisionV2,
    ProfileSourceMemberV2, SOURCE_CATALOG_PROJECTION_VERSION_V2, SOURCE_SNAPSHOT_SCHEMA_V1,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1,
    SourceSnapshotV1, SourceStreamKindV1, SourceStreamV1,
};
#[cfg(feature = "native-alpm-oracle")]
pub use parity::{
    ALPM_PARITY_PROJECTION_SCHEMA_V1, ALPM_RESOLUTION_PROJECTION_SCHEMA_V2, AlpmParityMemberInput,
    produce_alpm_parity_oracle, produce_alpm_resolution_oracle, produce_alpm_resolution_survey,
};
pub use parity::{
    CONARY_RESOLUTION_PROJECTION_SCHEMA_V2, ConaryResolutionCandidateV1,
    NATIVE_PARITY_COMPARISON_SCHEMA_V1, NATIVE_PARITY_MANIFEST_FILE_NAME,
    NATIVE_PARITY_ORACLE_SCHEMA_V1, NATIVE_PARITY_PACKAGE_FILE_NAME,
    NATIVE_RESOLUTION_COMPARISON_SCHEMA_V2, NATIVE_RESOLUTION_MANIFEST_FILE_NAME,
    NATIVE_RESOLUTION_ORACLE_SCHEMA_V2, NATIVE_RESOLUTION_ROOT_FILE_NAME,
    NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT, NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT,
    NATIVE_RESOLUTION_SURVEY_SCHEMA_V2, NativeParityArtifactV1, NativeParityComparisonError,
    NativeParityComparisonV1, NativeParityCountsV1, NativeParityEcosystemV1, NativeParityFactV1,
    NativeParityImplementationV1, NativeParityMismatchV1, NativeParityOracleReader,
    NativeParityOracleV1, NativeParityOracleWriter, NativeParityPackageIdentityV1,
    NativeParityPackageV1, NativeResolutionArchitectureAdmissionV1, NativeResolutionArtifactV1,
    NativeResolutionComparisonError, NativeResolutionComparisonV1, NativeResolutionCountsV1,
    NativeResolutionInstalledStateV1, NativeResolutionMismatchV1,
    NativeResolutionNotInstallableReasonV1, NativeResolutionOracleReader, NativeResolutionOracleV1,
    NativeResolutionOracleWriter, NativeResolutionOutcomeKindV1, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    NativeResolutionSurveyAlpmConflictV1, NativeResolutionSurveyAlpmMissingV1,
    NativeResolutionSurveyAlpmPackageV1, NativeResolutionSurveyAlpmResultV1,
    NativeResolutionSurveyCountsV1, NativeResolutionSurveyDebianMissingV1,
    NativeResolutionSurveyDebianPackageV1, NativeResolutionSurveyDebianResultV1,
    NativeResolutionSurveyErrorCountV1, NativeResolutionSurveyErrorKindV1,
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyErrorVariantV1,
    NativeResolutionSurveyEvidenceWithheldReasonV1, NativeResolutionSurveyFailureV1,
    NativeResolutionSurveyNativeExplanationV1, NativeResolutionSurveyRpmPackageV1,
    NativeResolutionSurveyRpmProblemV1, NativeResolutionSurveyRpmRuleV1, NativeResolutionSurveyV1,
    NativeUnresolvedDependencyV1, compare_native_parity_oracle, compare_native_resolution_oracle,
    native_requirement_group_sha256, produce_conary_resolution_candidate,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_parity_oracle_manifest, write_native_resolution_oracle_manifest,
    write_native_resolution_survey,
};
#[cfg(feature = "native-debian-oracle")]
pub use parity::{
    DEBIAN_PARITY_PROJECTION_SCHEMA_V1, DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V2,
    DebianParityMemberInput, produce_debian_parity_oracle, produce_debian_resolution_oracle,
    produce_debian_resolution_survey,
};
#[cfg(feature = "native-rpm-oracle")]
pub use parity::{
    RPM_PARITY_PROJECTION_SCHEMA_V1, RPM_RESOLUTION_PROJECTION_SCHEMA_V4, RpmParityMemberInput,
    produce_rpm_parity_oracle, produce_rpm_resolution_oracle, produce_rpm_resolution_survey,
};
pub use portable_integrity::{
    PORTABLE_CHUNK_MANIFEST_SCHEMA_V1, PORTABLE_CHUNK_SIZE_V1, PortableChunkManifestV1,
    PortableChunkRangeV1, PortableIntegrityError, PortableIntegrityResult,
    PortableManifestAttestationV1, portable_chunk_count_v1, portable_manifest_size_v1,
    read_portable_chunk_manifest_v1, write_portable_chunk_manifest_v1,
};
pub use portable_vfs::{
    PortableCatalogConnection, PortableVfsFailureKindV1, PortableVfsFailureV1, PortableVfsMetricsV1,
};
pub use profile::{
    ProfileCatalogCandidateV2, ProfileCatalogMemberInputV2, VerifiedProfileCatalogCandidateV2,
    derive_profile_catalog_members, write_profile_catalog_candidate,
    write_profile_catalog_candidate_verified_with_scratch_admission,
    write_profile_catalog_candidate_with_scratch_admission,
};
pub use record::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1, CatalogScopeV1,
    CatalogSourceEvidenceV1, DebianSourcePocketV1,
};
pub use source::SourceCatalogCandidateV1;
pub use store::{
    CatalogBindingV1, CatalogPackageNamePageV1, CatalogReader, CatalogVerificationEvidenceV1,
    write_catalog_candidate,
};
pub(in crate::repository) use store::{
    CatalogDurableLogicalAttestationV1, CatalogVerificationProofV1,
};
#[cfg(test)]
pub(in crate::repository) use store::{
    logical_verification_passes_for_test, physical_verification_passes_for_test,
};
