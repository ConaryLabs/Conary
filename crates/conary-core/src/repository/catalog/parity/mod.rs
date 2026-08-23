// crates/conary-core/src/repository/catalog/parity/mod.rs

//! Strict native full-catalog parity oracle artifacts and comparison.

mod compare;
mod contract;
mod io;
mod resolution_compare;
mod resolution_contract;
mod resolution_io;

#[cfg(feature = "native-alpm-oracle")]
mod alpm;

#[cfg(feature = "native-rpm-oracle")]
mod rpm;

#[cfg(feature = "native-alpm-oracle")]
pub use alpm::{
    ALPM_PARITY_PROJECTION_SCHEMA_V1, ALPM_RESOLUTION_PROJECTION_SCHEMA_V1, AlpmParityMemberInput,
    produce_alpm_parity_oracle, produce_alpm_resolution_oracle,
};

#[cfg(feature = "native-rpm-oracle")]
pub use rpm::{RPM_PARITY_PROJECTION_SCHEMA_V1, RpmParityMemberInput, produce_rpm_parity_oracle};

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
    NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1, NativeResolutionComparisonError,
    NativeResolutionComparisonV1, NativeResolutionMismatchV1, NativeResolutionOutcomeKindV1,
    compare_native_resolution_oracle,
};
pub use resolution_contract::{
    NATIVE_RESOLUTION_ORACLE_SCHEMA_V1, NativeResolutionArtifactV1, NativeResolutionCountsV1,
    NativeResolutionInstalledStateV1, NativeResolutionOracleV1, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    NativeUnresolvedDependencyV1, native_requirement_group_sha256,
};
pub use resolution_io::{
    NATIVE_RESOLUTION_MANIFEST_FILE_NAME, NATIVE_RESOLUTION_ROOT_FILE_NAME,
    NativeResolutionOracleReader, NativeResolutionOracleWriter,
    verify_native_resolution_oracle_bundle, write_native_resolution_oracle_manifest,
};

#[cfg(test)]
mod tests;
