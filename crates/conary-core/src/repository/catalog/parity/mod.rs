// crates/conary-core/src/repository/catalog/parity/mod.rs

//! Strict native full-catalog parity oracle artifacts and comparison.

mod candidate_resolution;
mod compare;
mod contract;
mod io;
mod resolution_compare;
mod resolution_contract;
mod resolution_io;
mod resolution_survey;

#[cfg(feature = "native-alpm-oracle")]
mod alpm;

#[cfg(feature = "native-rpm-oracle")]
mod rpm;

#[cfg(feature = "native-debian-oracle")]
mod debian;

#[cfg(feature = "native-alpm-oracle")]
pub use alpm::{
    ALPM_PARITY_PROJECTION_SCHEMA_V1, ALPM_RESOLUTION_PROJECTION_SCHEMA_V2, AlpmParityMemberInput,
    produce_alpm_parity_oracle, produce_alpm_resolution_oracle, produce_alpm_resolution_survey,
};

#[cfg(feature = "native-rpm-oracle")]
pub use rpm::{
    RPM_PARITY_PROJECTION_SCHEMA_V1, RPM_RESOLUTION_PROJECTION_SCHEMA_V4, RpmParityMemberInput,
    produce_rpm_parity_oracle, produce_rpm_resolution_oracle, produce_rpm_resolution_survey,
};

#[cfg(feature = "native-debian-oracle")]
pub use debian::{
    DEBIAN_PARITY_PROJECTION_SCHEMA_V1, DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V2,
    DebianParityMemberInput, produce_debian_parity_oracle, produce_debian_resolution_oracle,
    produce_debian_resolution_survey,
};

pub use crate::repository::architecture::{
    NativeMachineEndiannessV1, NativeMachineIdentityV1, NativeResolutionArchitectureDecisionV1,
};
pub use candidate_resolution::{
    CONARY_RESOLUTION_PROJECTION_SCHEMA_V2, ConaryResolutionCandidateV1,
    produce_conary_resolution_candidate,
};
pub use compare::{
    NATIVE_PARITY_COMPARISON_SCHEMA_V1, NativeParityComparisonError, NativeParityComparisonV1,
    NativeParityFactV1, NativeParityMismatchV1, NativeParityPackageIdentityV1,
    compare_native_parity_oracle,
};
pub use contract::{
    NATIVE_PARITY_ORACLE_SCHEMA_V1, NativeParityArtifactV1, NativeParityCountsV1,
    NativeParityEcosystemV1, NativeParityImplementationV1, NativeParityOracleV1,
    NativeParityPackageV1,
};
pub use io::{
    NATIVE_PARITY_MANIFEST_FILE_NAME, NATIVE_PARITY_PACKAGE_FILE_NAME, NativeParityOracleReader,
    NativeParityOracleWriter, verify_native_parity_oracle_bundle,
    write_native_parity_oracle_manifest,
};
pub use resolution_compare::{
    NATIVE_RESOLUTION_COMPARISON_SCHEMA_V2, NativeResolutionComparisonError,
    NativeResolutionComparisonV1, NativeResolutionMismatchV1, NativeResolutionOutcomeKindV1,
    compare_native_resolution_oracle,
};
pub use resolution_contract::{
    NATIVE_RESOLUTION_ORACLE_SCHEMA_V2, NativeResolutionArchitectureAdmissionV1,
    NativeResolutionArtifactV1, NativeResolutionCountsV1, NativeResolutionInstalledStateV1,
    NativeResolutionNotInstallableReasonV1, NativeResolutionOracleV1, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    NativeUnresolvedDependencyV1, native_requirement_group_sha256,
};
pub use resolution_io::{
    NATIVE_RESOLUTION_MANIFEST_FILE_NAME, NATIVE_RESOLUTION_ROOT_FILE_NAME,
    NativeResolutionOracleReader, NativeResolutionOracleWriter,
    verify_native_resolution_oracle_bundle, write_native_resolution_oracle_manifest,
};
pub use resolution_survey::{
    NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT, NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT,
    NATIVE_RESOLUTION_SURVEY_SCHEMA_V2, NativeResolutionSurveyAlpmConflictV1,
    NativeResolutionSurveyAlpmMissingV1, NativeResolutionSurveyAlpmPackageV1,
    NativeResolutionSurveyAlpmResultV1, NativeResolutionSurveyCountsV1,
    NativeResolutionSurveyDebianMissingV1, NativeResolutionSurveyDebianPackageV1,
    NativeResolutionSurveyDebianResultV1, NativeResolutionSurveyErrorCountV1,
    NativeResolutionSurveyErrorKindV1, NativeResolutionSurveyErrorReasonV1,
    NativeResolutionSurveyErrorVariantV1, NativeResolutionSurveyEvidenceWithheldReasonV1,
    NativeResolutionSurveyFailureV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyRpmPackageV1, NativeResolutionSurveyRpmProblemV1,
    NativeResolutionSurveyRpmRuleV1, NativeResolutionSurveyV1, write_native_resolution_survey,
};

#[cfg(test)]
mod tests;
