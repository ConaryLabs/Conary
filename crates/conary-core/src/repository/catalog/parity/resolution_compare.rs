// crates/conary-core/src/repository/catalog/parity/resolution_compare.rs

//! Bounded comparison of native and candidate dependency-resolution evidence.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::super::ProfileRevisionV2;
use super::io::NativeParityOracleReader;
use super::resolution_contract::{NativeResolutionCountsV1, NativeResolutionOutcomeV1};
use super::resolution_io::NativeResolutionOracleReader;
use crate::error::Error as ConaryError;

pub const NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionOutcomeKindV1 {
    Resolved,
    Unresolved,
}

impl NativeResolutionOutcomeKindV1 {
    fn from_outcome(outcome: &NativeResolutionOutcomeV1) -> Self {
        match outcome {
            NativeResolutionOutcomeV1::Resolved { .. } => Self::Resolved,
            NativeResolutionOutcomeV1::Unresolved { .. } => Self::Unresolved,
        }
    }
}

/// First exact divergence in the canonical per-root merge walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeResolutionMismatchV1 {
    OracleOnlyRoot {
        root_package_key_sha256: String,
    },
    CandidateOnlyRoot {
        root_package_key_sha256: String,
    },
    ResolutionOutcome {
        root_package_key_sha256: String,
        oracle: NativeResolutionOutcomeKindV1,
        candidate: NativeResolutionOutcomeKindV1,
    },
    DependencyClosure {
        root_package_key_sha256: String,
    },
    UnresolvedDependencies {
        root_package_key_sha256: String,
    },
}

/// Complete successful comparison record for later promotion-proof binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonV1 {
    pub schema_version: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub package_oracle_manifest_sha256: String,
    pub oracle_manifest_sha256: String,
    pub candidate_manifest_sha256: String,
    pub counts: NativeResolutionCountsV1,
}

#[derive(Debug, Error)]
pub enum NativeResolutionComparisonError {
    #[error("native resolution oracle is invalid: {0}")]
    Oracle(#[source] ConaryError),
    #[error("native resolution candidate is invalid: {0}")]
    Candidate(#[source] ConaryError),
    #[error("native resolution candidate diverges from the pinned oracle: {0:?}")]
    Mismatch(Box<NativeResolutionMismatchV1>),
}

pub fn compare_native_resolution_oracle(
    profile: &ProfileRevisionV2,
    package_oracle: &NativeParityOracleReader,
    oracle: &NativeResolutionOracleReader,
    candidate: &NativeResolutionOracleReader,
) -> std::result::Result<NativeResolutionComparisonV1, NativeResolutionComparisonError> {
    oracle
        .manifest()
        .validate_binding(profile, package_oracle.manifest())
        .map_err(NativeResolutionComparisonError::Oracle)?;
    candidate
        .manifest()
        .validate_binding(profile, package_oracle.manifest())
        .map_err(NativeResolutionComparisonError::Candidate)?;
    if oracle.manifest().policy != candidate.manifest().policy {
        return Err(NativeResolutionComparisonError::Candidate(
            ConaryError::ConflictError(
                "native resolution candidate uses a different resolution policy".to_string(),
            ),
        ));
    }

    let mut oracle_cursor = oracle
        .cursor()
        .map_err(NativeResolutionComparisonError::Oracle)?;
    let mut candidate_cursor = candidate
        .cursor()
        .map_err(NativeResolutionComparisonError::Candidate)?;
    let mut expected = oracle_cursor
        .next_root()
        .map_err(NativeResolutionComparisonError::Oracle)?;
    let mut actual = candidate_cursor
        .next_root()
        .map_err(NativeResolutionComparisonError::Candidate)?;
    loop {
        match (expected.as_ref(), actual.as_ref()) {
            (Some(oracle_root), Some(candidate_root)) => {
                match oracle_root
                    .root_package_key_sha256
                    .cmp(&candidate_root.root_package_key_sha256)
                {
                    std::cmp::Ordering::Less => {
                        return Err(mismatch(NativeResolutionMismatchV1::OracleOnlyRoot {
                            root_package_key_sha256: oracle_root.root_package_key_sha256.clone(),
                        }));
                    }
                    std::cmp::Ordering::Greater => {
                        return Err(mismatch(NativeResolutionMismatchV1::CandidateOnlyRoot {
                            root_package_key_sha256: candidate_root.root_package_key_sha256.clone(),
                        }));
                    }
                    std::cmp::Ordering::Equal => {
                        if oracle_root.outcome != candidate_root.outcome {
                            return Err(mismatch(outcome_mismatch(
                                &oracle_root.root_package_key_sha256,
                                &oracle_root.outcome,
                                &candidate_root.outcome,
                            )));
                        }
                        expected = oracle_cursor
                            .next_root()
                            .map_err(NativeResolutionComparisonError::Oracle)?;
                        actual = candidate_cursor
                            .next_root()
                            .map_err(NativeResolutionComparisonError::Candidate)?;
                    }
                }
            }
            (Some(oracle_root), None) => {
                return Err(mismatch(NativeResolutionMismatchV1::OracleOnlyRoot {
                    root_package_key_sha256: oracle_root.root_package_key_sha256.clone(),
                }));
            }
            (None, Some(candidate_root)) => {
                return Err(mismatch(NativeResolutionMismatchV1::CandidateOnlyRoot {
                    root_package_key_sha256: candidate_root.root_package_key_sha256.clone(),
                }));
            }
            (None, None) => break,
        }
    }

    oracle
        .verify_package_oracle(package_oracle)
        .map_err(NativeResolutionComparisonError::Oracle)?;
    candidate
        .verify_package_oracle(package_oracle)
        .map_err(NativeResolutionComparisonError::Candidate)?;

    Ok(NativeResolutionComparisonV1 {
        schema_version: NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1,
        profile: profile.profile.clone(),
        profile_revision_sha256: profile
            .manifest_sha256()
            .map_err(NativeResolutionComparisonError::Candidate)?,
        package_oracle_manifest_sha256: package_oracle
            .manifest()
            .manifest_sha256()
            .map_err(NativeResolutionComparisonError::Oracle)?,
        oracle_manifest_sha256: oracle
            .manifest()
            .manifest_sha256()
            .map_err(NativeResolutionComparisonError::Oracle)?,
        candidate_manifest_sha256: candidate
            .manifest()
            .manifest_sha256()
            .map_err(NativeResolutionComparisonError::Candidate)?,
        counts: oracle.manifest().artifact.counts,
    })
}

fn outcome_mismatch(
    root_package_key_sha256: &str,
    oracle: &NativeResolutionOutcomeV1,
    candidate: &NativeResolutionOutcomeV1,
) -> NativeResolutionMismatchV1 {
    match (oracle, candidate) {
        (
            NativeResolutionOutcomeV1::Resolved { .. },
            NativeResolutionOutcomeV1::Resolved { .. },
        ) => NativeResolutionMismatchV1::DependencyClosure {
            root_package_key_sha256: root_package_key_sha256.to_string(),
        },
        (
            NativeResolutionOutcomeV1::Unresolved { .. },
            NativeResolutionOutcomeV1::Unresolved { .. },
        ) => NativeResolutionMismatchV1::UnresolvedDependencies {
            root_package_key_sha256: root_package_key_sha256.to_string(),
        },
        _ => NativeResolutionMismatchV1::ResolutionOutcome {
            root_package_key_sha256: root_package_key_sha256.to_string(),
            oracle: NativeResolutionOutcomeKindV1::from_outcome(oracle),
            candidate: NativeResolutionOutcomeKindV1::from_outcome(candidate),
        },
    }
}

fn mismatch(mismatch: NativeResolutionMismatchV1) -> NativeResolutionComparisonError {
    NativeResolutionComparisonError::Mismatch(Box::new(mismatch))
}
