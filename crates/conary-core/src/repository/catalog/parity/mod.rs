// crates/conary-core/src/repository/catalog/parity/mod.rs

//! Strict native full-catalog parity oracle artifacts and comparison.

mod candidate_resolution;
mod candidate_resolution_survey;
mod compare;
mod contract;
mod io;
mod resolution_compare;
mod resolution_comparison_survey;
mod resolution_contract;
mod resolution_io;
mod resolution_parallel;
#[cfg(any(
    feature = "native-rpm-oracle",
    feature = "native-debian-oracle",
    feature = "native-alpm-oracle"
))]
mod resolution_producer;
mod resolution_root;
mod resolution_survey;
mod support;
mod survey_support;

#[cfg(feature = "native-alpm-oracle")]
mod alpm;

#[cfg(feature = "native-rpm-oracle")]
mod rpm;

#[cfg(feature = "native-debian-oracle")]
mod debian;

#[cfg(feature = "native-alpm-oracle")]
pub use alpm::{
    ALPM_PARITY_PROJECTION_SCHEMA_V1, ALPM_RESOLUTION_PROJECTION_SCHEMA_V3, AlpmParityMemberInput,
    produce_alpm_parity_oracle, produce_alpm_resolution_oracle,
    produce_alpm_resolution_oracle_with_workers, produce_alpm_resolution_survey,
    produce_alpm_resolution_survey_with_workers,
};

#[cfg(feature = "native-rpm-oracle")]
pub use rpm::{
    RPM_PARITY_PROJECTION_SCHEMA_V1, RPM_RESOLUTION_PROJECTION_SCHEMA_V5, RpmParityMemberInput,
    produce_rpm_parity_oracle, produce_rpm_resolution_oracle,
    produce_rpm_resolution_oracle_with_workers, produce_rpm_resolution_survey,
    produce_rpm_resolution_survey_with_workers,
};

#[cfg(feature = "native-debian-oracle")]
pub use debian::{
    DEBIAN_PARITY_PROJECTION_SCHEMA_V1, DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V3,
    DebianParityMemberInput, produce_debian_parity_oracle, produce_debian_resolution_oracle,
    produce_debian_resolution_oracle_with_workers, produce_debian_resolution_survey,
    produce_debian_resolution_survey_with_workers, run_debian_resolution_worker,
};

pub use crate::repository::architecture::{
    NativeMachineEndiannessV1, NativeMachineIdentityV1, NativeResolutionArchitectureDecisionV1,
};
pub use candidate_resolution::{
    CONARY_RESOLUTION_PROJECTION_SCHEMA_V3, ConaryResolutionCandidateV1,
    produce_conary_resolution_candidate, produce_conary_resolution_candidate_with_workers,
    produce_conary_resolution_comparison_survey,
    produce_conary_resolution_comparison_survey_with_workers, produce_conary_resolution_survey,
    produce_conary_resolution_survey_with_workers,
};
pub use candidate_resolution_survey::{
    CONARY_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT, CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT,
    CONARY_RESOLUTION_SURVEY_SCHEMA_V2, ConaryResolutionSurveyConflictEdgeV1,
    ConaryResolutionSurveyConflictKindV1, ConaryResolutionSurveyCountsV1,
    ConaryResolutionSurveyErrorCountV1, ConaryResolutionSurveyErrorKindV1,
    ConaryResolutionSurveyErrorReasonV1, ConaryResolutionSurveyEvidenceWithheldReasonV1,
    ConaryResolutionSurveyExcludedNodeV1, ConaryResolutionSurveyExcludedReasonV1,
    ConaryResolutionSurveyFailureV1, ConaryResolutionSurveyNativeExplanationV1,
    ConaryResolutionSurveyRootOutcomeV1, ConaryResolutionSurveySolvableV1,
    ConaryResolutionSurveyUnresolvedEdgeV1, ConaryResolutionSurveyV1,
    ConaryResolutionSurveyVersionSetV1, write_conary_resolution_survey,
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
    NATIVE_RESOLUTION_COMPARISON_SCHEMA_V3, NativeResolutionComparisonError,
    NativeResolutionComparisonV1, NativeResolutionMismatchV1, NativeResolutionOutcomeKindV1,
    compare_native_resolution_oracle,
};
pub use resolution_comparison_survey::{
    NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT,
    NATIVE_RESOLUTION_COMPARISON_SURVEY_SCHEMA_V2, NativeResolutionComparisonSurveyCountsV1,
    NativeResolutionComparisonSurveyMismatchCountV1,
    NativeResolutionComparisonSurveyMismatchKindV1, NativeResolutionComparisonSurveyMismatchV1,
    NativeResolutionComparisonSurveyOutcomeEvidenceV1,
    NativeResolutionComparisonSurveyOutcomePairCountV1,
    NativeResolutionComparisonSurveyOutcomePairV1, NativeResolutionComparisonSurveyRootIdentityV1,
    NativeResolutionComparisonSurveyV1, compare_native_resolution_oracle_survey,
    write_native_resolution_comparison_survey,
};
pub use resolution_contract::{
    NATIVE_RESOLUTION_ORACLE_SCHEMA_V3, NativeResolutionArchitectureAdmissionV1,
    NativeResolutionArtifactV1, NativeResolutionCountsV1, NativeResolutionInstalledStateV1,
    NativeResolutionNotInstallableReasonV1, NativeResolutionOracleV1, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    NativeUnresolvedDependencyV1, native_requirement_group_sha256,
};
pub use resolution_io::{
    NATIVE_RESOLUTION_MANIFEST_FILE_NAME, NATIVE_RESOLUTION_ROOT_FILE_NAME,
    NativeResolutionBundleState, NativeResolutionOracleReader, NativeResolutionOracleWriter,
    inspect_native_resolution_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest,
};
#[cfg(test)]
pub(crate) use resolution_parallel::resolution_test_capacity;
pub use resolution_parallel::{
    ResolutionWalkImplementationEvidenceV1, ResolutionWorkerCount, ResolutionWorkerRequest,
    ensure_resolution_walk_evidence_outside_bundle, write_resolution_walk_implementation_evidence,
};
pub use resolution_survey::{
    NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT,
    NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT, NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT,
    NATIVE_RESOLUTION_SURVEY_SCHEMA_V3, NativeResolutionSurveyAlpmConflictV1,
    NativeResolutionSurveyAlpmMissingV1, NativeResolutionSurveyAlpmPackageV1,
    NativeResolutionSurveyAlpmResultV1, NativeResolutionSurveyCountsV1,
    NativeResolutionSurveyDebianMissingV1, NativeResolutionSurveyDebianPackageV1,
    NativeResolutionSurveyDebianResultV1, NativeResolutionSurveyErrorCountV1,
    NativeResolutionSurveyErrorKindV1, NativeResolutionSurveyErrorReasonV1,
    NativeResolutionSurveyErrorVariantV1, NativeResolutionSurveyEvidenceWithheldReasonV1,
    NativeResolutionSurveyFailureV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyRootOutcomeV1, NativeResolutionSurveyRpmPackageV1,
    NativeResolutionSurveyRpmProblemV1, NativeResolutionSurveyRpmResultV1,
    NativeResolutionSurveyRpmRuleV1, NativeResolutionSurveyV1, write_native_resolution_survey,
};

#[cfg(test)]
mod tests;
