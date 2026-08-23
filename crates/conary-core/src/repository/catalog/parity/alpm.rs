// crates/conary-core/src/repository/catalog/parity/alpm.rs

//! Independent libalpm-backed native parity production.

use std::path::{Path, PathBuf};

use super::NativeParityOracleV1;
use crate::error::Result;
use crate::repository::catalog::ProfileRevisionV2;

pub const ALPM_PARITY_PROJECTION_SCHEMA_V1: u32 = 1;

/// Produce and independently reopen one strict ALPM parity bundle.
///
/// The implementation deliberately accepts source database artifacts rather
/// than a Conary catalog. Each path corresponds to the profile member at the
/// same ordinal.
pub fn produce_alpm_parity_oracle(
    _profile: &ProfileRevisionV2,
    _databases: &[PathBuf],
    _output: &Path,
) -> Result<NativeParityOracleV1> {
    todo!("project exact libalpm facts into the strict parity artifact")
}
