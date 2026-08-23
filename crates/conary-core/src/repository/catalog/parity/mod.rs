// crates/conary-core/src/repository/catalog/parity/mod.rs

//! Strict native full-catalog parity oracle artifacts and comparison.

mod compare;
mod contract;
mod io;

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

#[cfg(test)]
mod tests;
