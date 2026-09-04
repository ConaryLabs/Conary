// crates/conary-core/src/repository/catalog/parity/resolution_survey.rs

//! Diagnostics-only native resolution survey contracts and collection.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::super::contract::{validate_identity, validate_sha256};
use super::contract::NativeParityImplementationV1;
use super::resolution_contract::{
    NativeResolutionOutcomeV1, NativeResolutionPolicyV1, NativeResolutionRootV1,
};
#[allow(unused_imports)] // Native producers consume these behind ecosystem features.
pub(super) use super::resolution_root::{
    NativeRootResolutionError, NativeRootResolutionResult, NativeRootResolutionSuccess,
};
use super::support::{Counter, checked_increment};
use crate::error::{Error, Result};

#[allow(unused_imports)] // Consumed by feature-gated native explanation builders.
pub(super) use super::survey_support::SurveyEvidenceBudget as NativeExplanationBudget;
use super::survey_support::{canonical_value_size_with_limit, write_private_canonical_json};

mod collector;

#[allow(unused_imports)] // Native producers consume these behind ecosystem features.
pub(super) use collector::{NativeResolutionSurveyCollector, RootOutcomeSink};

pub const NATIVE_RESOLUTION_SURVEY_SCHEMA_V3: u32 = 3;
pub const NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT: usize = 5_000;
pub const NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT: usize = 5_000;
pub const NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT: u64 = 32 * 1024 * 1024;
pub const NATIVE_RESOLUTION_SURVEY_DOCUMENT_BYTE_LIMIT: u64 = 64 * 1024 * 1024;

/// Diagnostics-only inventory of all per-root native projection failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyV1 {
    pub schema_version: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub package_oracle_manifest_sha256: String,
    pub implementation: NativeParityImplementationV1,
    pub policy: NativeResolutionPolicyV1,
    pub target_architecture: String,
    pub counts: NativeResolutionSurveyCountsV1,
    pub failure_record_limit: u64,
    pub total_failures: u64,
    pub retained_failures: u64,
    pub truncated: bool,
    pub evidence_byte_limit: u64,
    pub retained_evidence_bytes: u64,
    pub retained_explanations: u64,
    pub withheld_explanations: u64,
    pub truncated_evidence: bool,
    pub diagnostic_outcome_record_limit: u64,
    pub total_diagnostic_outcomes: u64,
    pub retained_diagnostic_outcomes: u64,
    pub diagnostic_outcomes_truncated: bool,
    pub diagnostic_outcomes: Vec<NativeResolutionSurveyRootOutcomeV1>,
    pub failures: Vec<NativeResolutionSurveyFailureV1>,
}

impl NativeResolutionSurveyV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != NATIVE_RESOLUTION_SURVEY_SCHEMA_V3 {
            return Err(Error::ConfigError(format!(
                "native resolution survey schema {} is unsupported; expected {}",
                self.schema_version, NATIVE_RESOLUTION_SURVEY_SCHEMA_V3
            )));
        }
        validate_identity(&self.profile, "native resolution survey profile")?;
        validate_sha256(
            &self.profile_revision_sha256,
            "native resolution survey profile revision SHA-256",
        )?;
        validate_sha256(
            &self.package_oracle_manifest_sha256,
            "native resolution survey package oracle manifest SHA-256",
        )?;
        self.implementation.validate()?;
        self.policy.validate()?;
        validate_identity(
            &self.target_architecture,
            "native resolution survey target architecture",
        )?;
        if self.target_architecture != self.policy.architecture {
            return Err(Error::ConfigError(
                "native resolution survey target architecture disagrees with policy".to_string(),
            ));
        }
        self.counts.validate()?;
        if self.evidence_byte_limit != NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT {
            return Err(Error::ConfigError(format!(
                "native resolution survey evidence byte limit {} is unsupported; expected {}",
                self.evidence_byte_limit, NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT
            )));
        }
        if self.failure_record_limit != NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT as u64 {
            return Err(Error::ConfigError(format!(
                "native resolution survey failure record limit {} is unsupported; expected {}",
                self.failure_record_limit, NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT
            )));
        }
        if self.diagnostic_outcome_record_limit
            != NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT as u64
        {
            return Err(Error::ConfigError(format!(
                "native resolution survey diagnostic outcome record limit {} is unsupported; expected {}",
                self.diagnostic_outcome_record_limit,
                NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT
            )));
        }
        if self.total_failures != self.counts.failed_roots
            || self.retained_failures != self.failures.len() as u64
            || self.retained_failures > self.total_failures
            || self.retained_failures > self.failure_record_limit
            || self.truncated != (self.retained_failures < self.total_failures)
        {
            return Err(Error::ConfigError(
                "native resolution survey failure counts are inconsistent".to_string(),
            ));
        }
        if self.total_diagnostic_outcomes > self.counts.not_installable_roots
            || self.retained_diagnostic_outcomes != self.diagnostic_outcomes.len() as u64
            || self.retained_diagnostic_outcomes > self.total_diagnostic_outcomes
            || self.retained_diagnostic_outcomes > self.diagnostic_outcome_record_limit
            || self.diagnostic_outcomes_truncated
                != (self.retained_diagnostic_outcomes < self.total_diagnostic_outcomes)
            || self
                .diagnostic_outcomes
                .windows(2)
                .any(|pair| pair[0].root_package_key_sha256 >= pair[1].root_package_key_sha256)
        {
            return Err(Error::ConfigError(
                "native resolution survey diagnostic outcomes are inconsistent".to_string(),
            ));
        }
        self.validate_evidence()?;
        if canonical_value_size_with_limit(self, NATIVE_RESOLUTION_SURVEY_DOCUMENT_BYTE_LIMIT)?
            .is_none()
        {
            return Err(Error::ConfigError(format!(
                "native resolution survey exceeds its {} byte document limit",
                NATIVE_RESOLUTION_SURVEY_DOCUMENT_BYTE_LIMIT
            )));
        }
        Ok(())
    }

    fn validate_evidence(&self) -> Result<()> {
        let mut retained_evidence_bytes = 0_u64;
        let mut retained_explanations = 0_u64;
        let mut withheld_explanations = 0_u64;
        let mut evidence_withholding_started = false;
        let mut explanations = Vec::with_capacity(
            self.diagnostic_outcomes
                .len()
                .saturating_add(self.failures.len()),
        );
        for outcome in &self.diagnostic_outcomes {
            outcome.validate()?;
            explanations.push((
                outcome.root_package_key_sha256.as_str(),
                &outcome.native_explanation,
            ));
        }
        for failure in &self.failures {
            failure.validate()?;
            explanations.push((
                failure.root_package_key_sha256.as_str(),
                &failure.native_explanation,
            ));
        }
        explanations.sort_unstable_by_key(|(root, _)| *root);
        if explanations.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Error::ConfigError(
                "native resolution survey repeats a diagnostic root".to_string(),
            ));
        }
        for (_, explanation) in explanations {
            match explanation {
                NativeResolutionSurveyNativeExplanationV1::Withheld {
                    reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
                } => {
                    evidence_withholding_started = true;
                    withheld_explanations =
                        checked_increment(withheld_explanations, Counter::Survey("native"))?;
                }
                explanation => {
                    if evidence_withholding_started {
                        return Err(Error::ConfigError(
                            "native resolution survey retained evidence after withholding began"
                                .to_string(),
                        ));
                    }
                    retained_explanations =
                        checked_increment(retained_explanations, Counter::Survey("native"))?;
                    let remaining_bytes = self
                        .evidence_byte_limit
                        .checked_sub(retained_evidence_bytes)
                        .ok_or_else(|| {
                            Error::ConfigError(
                                "native resolution survey retained evidence exceeds its byte limit"
                                    .to_string(),
                            )
                        })?;
                    let Some(explanation_bytes) =
                        canonical_value_size_with_limit(explanation, remaining_bytes)?
                    else {
                        return Err(Error::ConfigError(
                            "native resolution survey retained evidence exceeds its byte limit"
                                .to_string(),
                        ));
                    };
                    retained_evidence_bytes = retained_evidence_bytes
                        .checked_add(explanation_bytes)
                        .ok_or_else(|| {
                            Error::ConfigError(
                                "native resolution survey evidence bytes exceed u64".to_string(),
                            )
                        })?;
                }
            }
        }
        let retained_records = retained_explanations
            .checked_add(withheld_explanations)
            .ok_or_else(|| {
                Error::ConfigError(
                    "native resolution survey explanation counts exceed u64".to_string(),
                )
            })?;
        let expected_records = self
            .retained_failures
            .checked_add(self.retained_diagnostic_outcomes)
            .ok_or_else(|| {
                Error::ConfigError(
                    "native resolution survey diagnostic records exceed u64".to_string(),
                )
            })?;
        if retained_records != expected_records
            || retained_evidence_bytes != self.retained_evidence_bytes
            || retained_evidence_bytes > self.evidence_byte_limit
            || retained_explanations != self.retained_explanations
            || withheld_explanations != self.withheld_explanations
            || self.truncated_evidence != (withheld_explanations > 0)
        {
            return Err(Error::ConfigError(
                "native resolution survey evidence counts are inconsistent".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyCountsV1 {
    pub roots_walked: u64,
    pub resolved_roots: u64,
    pub unresolved_roots: u64,
    pub not_installable_roots: u64,
    pub failed_roots: u64,
    pub error_kinds: Vec<NativeResolutionSurveyErrorCountV1>,
}

impl NativeResolutionSurveyCountsV1 {
    fn validate(&self) -> Result<()> {
        let outcomes = self
            .resolved_roots
            .checked_add(self.unresolved_roots)
            .and_then(|count| count.checked_add(self.not_installable_roots))
            .and_then(|count| count.checked_add(self.failed_roots))
            .ok_or_else(|| {
                Error::ConfigError("native resolution survey counts exceed u64".to_string())
            })?;
        if self.roots_walked != outcomes {
            return Err(Error::ConfigError(
                "native resolution survey root counts do not match outcomes".to_string(),
            ));
        }
        let histogram_total = self.error_kinds.iter().try_fold(0_u64, |total, entry| {
            total.checked_add(entry.count).ok_or_else(|| {
                Error::ConfigError(
                    "native resolution survey error histogram exceeds u64".to_string(),
                )
            })
        })?;
        if histogram_total != self.failed_roots {
            return Err(Error::ConfigError(
                "native resolution survey error histogram does not match failures".to_string(),
            ));
        }
        if self
            .error_kinds
            .windows(2)
            .any(|pair| pair[0].kind >= pair[1].kind)
        {
            return Err(Error::ConfigError(
                "native resolution survey error histogram is noncanonical".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyErrorCountV1 {
    pub kind: NativeResolutionSurveyErrorKindV1,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyErrorKindV1 {
    pub error_variant: NativeResolutionSurveyErrorVariantV1,
    pub reason: NativeResolutionSurveyErrorReasonV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionSurveyErrorReasonV1 {
    NativeSolverFailed,
    ExactRootProjectionFailed,
    ResolvedClosureProjectionFailed,
    ResolvedClosureOmittedRoot,
    UnresolvedProjectionFailed,
    TransactionInitializationFailed,
    TransactionAddRootFailed,
    TransactionReleaseFailed,
    NativeArchitectureRejected,
    UnknownArchitectureToken,
    NativePackageConflict,
    NativeSolverUnexpectedFailure,
    ProviderSearchBudgetExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionSurveyErrorVariantV1 {
    Database,
    Io,
    IoError,
    InitError,
    SchemaRebuildRequired,
    MissingId,
    VersionParse,
    VersionComparison,
    HashError,
    ConfigError,
    DatabaseNotFound,
    DownloadError,
    RepositoryResponseBody,
    DurableChunkUnavailable,
    HttpStatus,
    ConflictError,
    ProviderSearchBudgetExceeded,
    ProfileArchitectureMismatch,
    UnknownArchitectureToken,
    UnsupportedNativeHostTarget,
    AmbiguousPackageSelection,
    ChecksumMismatch,
    ParseError,
    Budget,
    CatalogScratchCapacity,
    DeltaError,
    GpgVerificationFailed,
    ScriptletExecution,
    TriggerError,
    AlreadyExists,
    InvalidPath,
    PathTraversal,
    NotFound,
    RecoveryFailed,
    TimeoutError,
    ResolutionError,
    NotImplemented,
    Json,
    Capability,
    Federation,
    Cancelled,
    InternalError,
    TrustError,
    PoolOverflow,
}

#[allow(dead_code)] // Used by feature-gated native producers and default-feature contract tests.
impl NativeResolutionSurveyErrorVariantV1 {
    pub(super) fn from_error(error: &Error) -> Self {
        match error {
            Error::Database(_) => Self::Database,
            Error::Io(_) => Self::Io,
            Error::IoError(_) => Self::IoError,
            Error::InitError(_) => Self::InitError,
            Error::SchemaRebuildRequired { .. } => Self::SchemaRebuildRequired,
            Error::ResolutionBundleRebuildRequired { .. } => Self::SchemaRebuildRequired,
            Error::MissingId(_) => Self::MissingId,
            Error::VersionParse(_) => Self::VersionParse,
            Error::VersionComparison(_) => Self::VersionComparison,
            Error::HashError(_) => Self::HashError,
            Error::ConfigError(_) => Self::ConfigError,
            Error::DatabaseNotFound(_) => Self::DatabaseNotFound,
            Error::DownloadError(_) => Self::DownloadError,
            Error::RepositoryResponseBody { .. } => Self::RepositoryResponseBody,
            Error::DurableChunkUnavailable { .. } => Self::DurableChunkUnavailable,
            Error::HttpStatus { .. } => Self::HttpStatus,
            Error::ConflictError(_) => Self::ConflictError,
            Error::ProviderSearchBudgetExceeded { .. } => Self::ProviderSearchBudgetExceeded,
            Error::ProfileArchitectureMismatch { .. } => Self::ProfileArchitectureMismatch,
            Error::UnknownArchitectureToken { .. } => Self::UnknownArchitectureToken,
            Error::UnsupportedNativeHostTarget { .. } => Self::UnsupportedNativeHostTarget,
            Error::AmbiguousPackageSelection { .. } => Self::AmbiguousPackageSelection,
            Error::ChecksumMismatch { .. } => Self::ChecksumMismatch,
            Error::ParseError(_) => Self::ParseError,
            Error::Budget(_) => Self::Budget,
            // The existing survey budget class retains the concrete root,
            // re-solve count, and elapsed duration in its failure diagnostic.
            Error::HiddenConflictProbeBudgetExceeded { .. } => Self::Budget,
            Error::CatalogScratchCapacity(_) => Self::CatalogScratchCapacity,
            Error::DeltaError(_) => Self::DeltaError,
            Error::GpgVerificationFailed(_) => Self::GpgVerificationFailed,
            Error::ScriptletExecution { .. } => Self::ScriptletExecution,
            Error::TriggerError(_) => Self::TriggerError,
            Error::AlreadyExists(_) => Self::AlreadyExists,
            Error::InvalidPath(_) => Self::InvalidPath,
            Error::PathTraversal(_) => Self::PathTraversal,
            Error::NotFound(_) => Self::NotFound,
            Error::RecoveryFailed(_) | Error::BootVerity(_) => Self::RecoveryFailed,
            Error::TimeoutError(_) => Self::TimeoutError,
            Error::ResolutionError(_) => Self::ResolutionError,
            Error::NotImplemented(_) => Self::NotImplemented,
            Error::Json(_) => Self::Json,
            Error::Capability(_) => Self::Capability,
            Error::Federation(_) => Self::Federation,
            Error::Cancelled(_) => Self::Cancelled,
            Error::InternalError(_) => Self::InternalError,
            Error::TrustError(_) => Self::TrustError,
            Error::PoolOverflow(_) => Self::PoolOverflow,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyRootOutcomeV1 {
    pub root_package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub release: String,
    pub architecture: Option<String>,
    pub outcome: NativeResolutionOutcomeV1,
    pub native_explanation: NativeResolutionSurveyNativeExplanationV1,
}

impl NativeResolutionSurveyRootOutcomeV1 {
    fn validate(&self) -> Result<()> {
        validate_identity(&self.name, "native resolution survey package name")?;
        NativeResolutionRootV1 {
            root_package_key_sha256: self.root_package_key_sha256.clone(),
            outcome: self.outcome.clone(),
        }
        .validate()?;
        if !matches!(
            self.outcome,
            NativeResolutionOutcomeV1::NotInstallable {
                reason: super::resolution_contract::NativeResolutionNotInstallableReasonV1::ConflictingClosure
            }
        ) {
            return Err(Error::ConfigError(
                "native resolution survey retained diagnostics for a non-conflict outcome"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyFailureV1 {
    pub root_package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub release: String,
    pub architecture: Option<String>,
    pub error_kind: NativeResolutionSurveyErrorKindV1,
    pub error_message: String,
    pub native_explanation: NativeResolutionSurveyNativeExplanationV1,
}

impl NativeResolutionSurveyFailureV1 {
    fn validate(&self) -> Result<()> {
        validate_sha256(
            &self.root_package_key_sha256,
            "native resolution survey root package key",
        )?;
        validate_identity(&self.name, "native resolution survey package name")?;
        if self.error_message.is_empty() {
            return Err(Error::ConfigError(
                "native resolution survey error message is empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ecosystem", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeResolutionSurveyNativeExplanationV1 {
    Withheld {
        reason: NativeResolutionSurveyEvidenceWithheldReasonV1,
    },
    Rpm {
        result: NativeResolutionSurveyRpmResultV1,
    },
    Debian {
        result: NativeResolutionSurveyDebianResultV1,
    },
    Alpm {
        result: NativeResolutionSurveyAlpmResultV1,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeResolutionSurveyRpmResultV1 {
    Problems {
        problems: Vec<NativeResolutionSurveyRpmProblemV1>,
    },
    Resolved {
        packages: Vec<NativeResolutionSurveyRpmPackageV1>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionSurveyEvidenceWithheldReasonV1 {
    EvidenceBudgetExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyRpmProblemV1 {
    pub problem: i32,
    pub rules: Vec<NativeResolutionSurveyRpmRuleV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyRpmRuleV1 {
    pub rule_type_numeric: i32,
    pub rule_type_symbolic: String,
    pub from_native_index: Option<u64>,
    pub from: Option<NativeResolutionSurveyRpmPackageV1>,
    pub from_unavailable_reason: Option<String>,
    pub to_native_index: Option<u64>,
    pub to: Option<NativeResolutionSurveyRpmPackageV1>,
    pub to_unavailable_reason: Option<String>,
    pub dependency_id: Option<i32>,
    pub dependency: Option<String>,
    pub dependency_unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyRpmPackageV1 {
    pub package_key_sha256: String,
    pub name: String,
    pub evr: String,
    pub architecture: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeResolutionSurveyDebianResultV1 {
    Resolved {
        packages: Vec<NativeResolutionSurveyDebianPackageV1>,
    },
    Unresolved {
        missing: Vec<NativeResolutionSurveyDebianMissingV1>,
    },
    Conflicts {
        detail_unavailable_reason: Option<String>,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyDebianPackageV1 {
    pub name: String,
    pub version: String,
    pub architecture: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyDebianMissingV1 {
    pub requiring: NativeResolutionSurveyDebianPackageV1,
    pub relation_kind: String,
    pub dependency: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeResolutionSurveyAlpmResultV1 {
    ProviderSearchBudgetExceeded {
        root: String,
        checks: u32,
    },
    Prepared {
        packages: Vec<NativeResolutionSurveyAlpmPackageV1>,
    },
    Unsatisfied {
        missing: Vec<NativeResolutionSurveyAlpmMissingV1>,
    },
    InvalidArchitecture {
        packages: Vec<NativeResolutionSurveyAlpmPackageV1>,
        detail_unavailable_reason: Option<String>,
    },
    Conflicts {
        conflicts: Vec<NativeResolutionSurveyAlpmConflictV1>,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyAlpmPackageV1 {
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyAlpmMissingV1 {
    pub target: String,
    pub dependency: String,
    pub causing_package: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionSurveyAlpmConflictV1 {
    pub package1: NativeResolutionSurveyAlpmPackageV1,
    pub package2: NativeResolutionSurveyAlpmPackageV1,
    pub reason: String,
}

/// Write one diagnostics-only survey as a create-only canonical JSON file.
pub fn write_native_resolution_survey(
    path: &Path,
    survey: &NativeResolutionSurveyV1,
) -> Result<()> {
    survey.validate()?;
    write_private_canonical_json(path, survey, "native resolution survey")
}

#[cfg(test)]
mod tests;
