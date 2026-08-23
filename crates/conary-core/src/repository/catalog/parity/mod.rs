// crates/conary-core/src/repository/catalog/parity/mod.rs

//! Strict native full-catalog parity oracle artifacts and comparison.

mod contract;

pub use contract::{
    NATIVE_PARITY_ORACLE_SCHEMA_V1, NativeParityArtifactV1, NativeParityCountsV1,
    NativeParityEcosystemV1, NativeParityImplementationV1, NativeParityOracleV1,
    NativeParityPackageV1,
};
