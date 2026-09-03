// crates/conary-core/src/repository/catalog/parity/resolution_survey.rs

//! Diagnostics-only native resolution survey contracts and collection.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::super::ProfileRevisionV2;
use super::super::contract::{validate_identity, validate_sha256};
use super::contract::{NativeParityImplementationV1, NativeParityOracleV1, NativeParityPackageV1};
use super::resolution_contract::{
    NativeResolutionOutcomeV1, NativeResolutionPolicyV1, NativeResolutionRootV1,
};
use super::resolution_io::NativeResolutionOracleWriter;
use crate::error::{Error, Result};

#[allow(unused_imports)] // Consumed by feature-gated native explanation builders.
pub(super) use super::survey_support::SurveyEvidenceBudget as NativeExplanationBudget;
use super::survey_support::{canonical_value_size_with_limit, write_private_canonical_json};

pub const NATIVE_RESOLUTION_SURVEY_SCHEMA_V2: u32 = 2;
pub const NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT: usize = 5_000;
pub const NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT: u64 = 64 * 1024 * 1024;

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
    pub failures: Vec<NativeResolutionSurveyFailureV1>,
}

impl NativeResolutionSurveyV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != NATIVE_RESOLUTION_SURVEY_SCHEMA_V2 {
            return Err(Error::ConfigError(format!(
                "native resolution survey schema {} is unsupported; expected {}",
                self.schema_version, NATIVE_RESOLUTION_SURVEY_SCHEMA_V2
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
        self.validate_evidence()?;
        Ok(())
    }

    fn validate_evidence(&self) -> Result<()> {
        let mut retained_evidence_bytes = 0_u64;
        let mut retained_explanations = 0_u64;
        let mut withheld_explanations = 0_u64;
        let mut evidence_withholding_started = false;
        for failure in &self.failures {
            failure.validate()?;
            match &failure.native_explanation {
                NativeResolutionSurveyNativeExplanationV1::Withheld {
                    reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
                } => {
                    evidence_withholding_started = true;
                    withheld_explanations = checked_increment(withheld_explanations)?;
                }
                explanation => {
                    if evidence_withholding_started {
                        return Err(Error::ConfigError(
                            "native resolution survey retained evidence after withholding began"
                                .to_string(),
                        ));
                    }
                    retained_explanations = checked_increment(retained_explanations)?;
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
        if retained_records != self.retained_failures
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
            Error::ProfileArchitectureMismatch { .. } => Self::ProfileArchitectureMismatch,
            Error::UnknownArchitectureToken { .. } => Self::UnknownArchitectureToken,
            Error::UnsupportedNativeHostTarget { .. } => Self::UnsupportedNativeHostTarget,
            Error::AmbiguousPackageSelection { .. } => Self::AmbiguousPackageSelection,
            Error::ChecksumMismatch { .. } => Self::ChecksumMismatch,
            Error::ParseError(_) => Self::ParseError,
            Error::Budget(_) => Self::Budget,
            Error::CatalogScratchCapacity(_) => Self::CatalogScratchCapacity,
            Error::DeltaError(_) => Self::DeltaError,
            Error::GpgVerificationFailed(_) => Self::GpgVerificationFailed,
            Error::ScriptletExecution { .. } => Self::ScriptletExecution,
            Error::TriggerError(_) => Self::TriggerError,
            Error::AlreadyExists(_) => Self::AlreadyExists,
            Error::InvalidPath(_) => Self::InvalidPath,
            Error::PathTraversal(_) => Self::PathTraversal,
            Error::NotFound(_) => Self::NotFound,
            Error::RecoveryFailed(_) => Self::RecoveryFailed,
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
        problems: Vec<NativeResolutionSurveyRpmProblemV1>,
    },
    Debian {
        result: NativeResolutionSurveyDebianResultV1,
    },
    Alpm {
        result: NativeResolutionSurveyAlpmResultV1,
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

#[allow(dead_code)] // Shared by feature-gated native producer root loops.
pub(super) struct NativeRootResolutionError {
    error: Error,
    reason: NativeResolutionSurveyErrorReasonV1,
    explanation: NativeResolutionSurveyNativeExplanationV1,
    wire_identity: Option<(NativeResolutionSurveyErrorVariantV1, String)>,
}

#[allow(dead_code)]
impl NativeRootResolutionError {
    pub(super) fn new(
        error: Error,
        reason: NativeResolutionSurveyErrorReasonV1,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    ) -> Box<Self> {
        Box::new(Self {
            error,
            reason,
            explanation,
            wire_identity: None,
        })
    }

    pub(super) fn into_wire(
        self,
    ) -> (
        NativeResolutionSurveyErrorVariantV1,
        String,
        NativeResolutionSurveyErrorReasonV1,
        NativeResolutionSurveyNativeExplanationV1,
    ) {
        let variant = NativeResolutionSurveyErrorVariantV1::from_error(&self.error);
        (
            variant,
            self.error.to_string(),
            self.reason,
            self.explanation,
        )
    }

    pub(super) fn from_wire(
        variant: NativeResolutionSurveyErrorVariantV1,
        message: String,
        reason: NativeResolutionSurveyErrorReasonV1,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    ) -> Box<Self> {
        Box::new(Self {
            error: Error::ResolutionError(message.clone()),
            reason,
            explanation,
            wire_identity: Some((variant, message)),
        })
    }

    #[cfg(feature = "native-alpm-oracle")]
    pub(super) fn replace_error(
        &mut self,
        error: Error,
        reason: NativeResolutionSurveyErrorReasonV1,
    ) {
        self.error = error;
        self.reason = reason;
        self.wire_identity = None;
    }

    #[cfg(feature = "native-alpm-oracle")]
    pub(super) fn error_message(&self) -> String {
        self.error.to_string()
    }
}

#[allow(dead_code)]
pub(super) type NativeRootResolutionResult =
    std::result::Result<NativeResolutionOutcomeV1, Box<NativeRootResolutionError>>;

#[allow(dead_code)]
pub(super) enum RootOutcomeSink<'a> {
    Strict(&'a mut NativeResolutionOracleWriter),
    Survey(&'a mut NativeResolutionSurveyCollector),
}

#[allow(dead_code)]
impl RootOutcomeSink<'_> {
    pub(super) fn explanation_byte_limit(&self) -> u64 {
        match self {
            Self::Strict(_) => 0,
            Self::Survey(collector) => collector.remaining_evidence_bytes(),
        }
    }

    pub(super) fn root(
        &mut self,
        root: &NativeParityPackageV1,
        result: NativeRootResolutionResult,
    ) -> Result<()> {
        match (self, result) {
            (Self::Strict(writer), Ok(outcome)) => writer.root(&NativeResolutionRootV1 {
                root_package_key_sha256: root.package_key_sha256.clone(),
                outcome,
            }),
            (Self::Strict(_), Err(failure)) => Err(failure.error),
            (Self::Survey(collector), Ok(outcome)) => {
                collector.success(outcome)?;
                Ok(())
            }
            (Self::Survey(collector), Err(failure)) => {
                collector.failure(root, *failure)?;
                Ok(())
            }
        }
    }
}

#[allow(dead_code)]
pub(super) struct NativeResolutionSurveyCollector {
    profile: String,
    profile_revision_sha256: String,
    package_oracle_manifest_sha256: String,
    implementation: NativeParityImplementationV1,
    policy: NativeResolutionPolicyV1,
    counts: NativeResolutionSurveyCountsV1,
    histogram: BTreeMap<NativeResolutionSurveyErrorKindV1, u64>,
    failures: Vec<NativeResolutionSurveyFailureV1>,
    evidence_byte_limit: u64,
    retained_evidence_bytes: u64,
    retained_explanations: u64,
    withheld_explanations: u64,
    evidence_budget_exhausted: bool,
}

#[allow(dead_code)]
impl NativeResolutionSurveyCollector {
    pub(super) fn new(
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleV1,
        implementation: NativeParityImplementationV1,
        policy: NativeResolutionPolicyV1,
    ) -> Result<Self> {
        Ok(Self {
            profile: profile.profile.clone(),
            profile_revision_sha256: profile.manifest_sha256()?,
            package_oracle_manifest_sha256: package_oracle.manifest_sha256()?,
            implementation,
            policy,
            counts: NativeResolutionSurveyCountsV1::default(),
            histogram: BTreeMap::new(),
            failures: Vec::new(),
            evidence_byte_limit: NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT,
            retained_evidence_bytes: 0,
            retained_explanations: 0,
            withheld_explanations: 0,
            evidence_budget_exhausted: false,
        })
    }

    fn success(&mut self, outcome: NativeResolutionOutcomeV1) -> Result<()> {
        self.counts.roots_walked = checked_increment(self.counts.roots_walked)?;
        match outcome {
            NativeResolutionOutcomeV1::Resolved { .. } => {
                self.counts.resolved_roots = checked_increment(self.counts.resolved_roots)?;
            }
            NativeResolutionOutcomeV1::Unresolved { .. } => {
                self.counts.unresolved_roots = checked_increment(self.counts.unresolved_roots)?;
            }
            NativeResolutionOutcomeV1::NotInstallable { .. } => {
                self.counts.not_installable_roots =
                    checked_increment(self.counts.not_installable_roots)?;
            }
        }
        Ok(())
    }

    fn remaining_evidence_bytes(&self) -> u64 {
        if self.evidence_budget_exhausted
            || self.failures.len() >= NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT
        {
            0
        } else {
            self.evidence_byte_limit
                .saturating_sub(self.retained_evidence_bytes)
        }
    }

    fn failure(
        &mut self,
        root: &NativeParityPackageV1,
        failure: NativeRootResolutionError,
    ) -> Result<()> {
        let NativeRootResolutionError {
            error,
            reason,
            explanation,
            wire_identity,
        } = failure;
        self.counts.roots_walked = checked_increment(self.counts.roots_walked)?;
        self.counts.failed_roots = checked_increment(self.counts.failed_roots)?;
        let (error_variant, error_message) = wire_identity.unwrap_or_else(|| {
            (
                NativeResolutionSurveyErrorVariantV1::from_error(&error),
                error.to_string(),
            )
        });
        let kind = NativeResolutionSurveyErrorKindV1 {
            error_variant,
            reason,
        };
        let count = self.histogram.entry(kind.clone()).or_default();
        *count = checked_increment(*count)?;
        if self.failures.len() < NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT {
            let native_explanation = self.retain_explanation(explanation)?;
            self.failures.push(NativeResolutionSurveyFailureV1 {
                root_package_key_sha256: root.package_key_sha256.clone(),
                name: root.name.clone(),
                version: root.version.clone(),
                release: root.package_release.clone(),
                architecture: root.architecture.clone(),
                error_kind: kind,
                error_message,
                native_explanation,
            });
        }
        Ok(())
    }

    fn retain_explanation(
        &mut self,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    ) -> Result<NativeResolutionSurveyNativeExplanationV1> {
        if matches!(
            explanation,
            NativeResolutionSurveyNativeExplanationV1::Withheld {
                reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted
            }
        ) {
            self.evidence_budget_exhausted = true;
            self.withheld_explanations = checked_increment(self.withheld_explanations)?;
            return Ok(explanation);
        }
        if !self.evidence_budget_exhausted {
            let remaining_bytes = self
                .evidence_byte_limit
                .saturating_sub(self.retained_evidence_bytes);
            if let Some(explanation_bytes) =
                canonical_value_size_with_limit(&explanation, remaining_bytes)?
            {
                let retained_evidence_bytes = self
                    .retained_evidence_bytes
                    .checked_add(explanation_bytes)
                    .ok_or_else(|| {
                        Error::ConfigError(
                            "native resolution survey evidence bytes exceed u64".to_string(),
                        )
                    })?;
                self.retained_evidence_bytes = retained_evidence_bytes;
                self.retained_explanations = checked_increment(self.retained_explanations)?;
                return Ok(explanation);
            }
            self.evidence_budget_exhausted = true;
        }
        self.withheld_explanations = checked_increment(self.withheld_explanations)?;
        Ok(NativeResolutionSurveyNativeExplanationV1::Withheld {
            reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
        })
    }

    pub(super) fn finish(mut self) -> Result<NativeResolutionSurveyV1> {
        self.counts.error_kinds = self
            .histogram
            .into_iter()
            .map(|(kind, count)| NativeResolutionSurveyErrorCountV1 { kind, count })
            .collect();
        let retained_failures = self.failures.len() as u64;
        let total_failures = self.counts.failed_roots;
        let survey = NativeResolutionSurveyV1 {
            schema_version: NATIVE_RESOLUTION_SURVEY_SCHEMA_V2,
            profile: self.profile,
            profile_revision_sha256: self.profile_revision_sha256,
            package_oracle_manifest_sha256: self.package_oracle_manifest_sha256,
            implementation: self.implementation,
            target_architecture: self.policy.architecture.clone(),
            policy: self.policy,
            counts: self.counts,
            failure_record_limit: NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT as u64,
            total_failures,
            retained_failures,
            truncated: retained_failures < total_failures,
            evidence_byte_limit: self.evidence_byte_limit,
            retained_evidence_bytes: self.retained_evidence_bytes,
            retained_explanations: self.retained_explanations,
            withheld_explanations: self.withheld_explanations,
            truncated_evidence: self.withheld_explanations > 0,
            failures: self.failures,
        };
        survey.validate()?;
        Ok(survey)
    }
}

/// Write one diagnostics-only survey as a create-only canonical JSON file.
pub fn write_native_resolution_survey(
    path: &Path,
    survey: &NativeResolutionSurveyV1,
) -> Result<()> {
    survey.validate()?;
    write_private_canonical_json(path, survey, "native resolution survey")
}

#[allow(dead_code)]
fn checked_increment(value: u64) -> Result<u64> {
    value
        .checked_add(1)
        .ok_or_else(|| Error::ConfigError("native resolution survey count exceeds u64".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::catalog::parity::{
        NativeParityEcosystemV1, NativeResolutionArchitectureAdmissionV1,
        NativeResolutionInstalledStateV1, NativeResolutionProviderPolicyV1,
        NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1,
    };

    fn collector(evidence_byte_limit: u64) -> NativeResolutionSurveyCollector {
        NativeResolutionSurveyCollector {
            profile: "test-profile".to_string(),
            profile_revision_sha256: "a".repeat(64),
            package_oracle_manifest_sha256: "b".repeat(64),
            implementation: NativeParityImplementationV1 {
                ecosystem: NativeParityEcosystemV1::Rpm,
                name: "test-solver".to_string(),
                version: "1".to_string(),
                projection_schema: 1,
            },
            policy: NativeResolutionPolicyV1 {
                architecture: "x86_64".to_string(),
                architecture_admission: NativeResolutionArchitectureAdmissionV1::NativeOnly,
                installed_state: NativeResolutionInstalledStateV1::Empty,
                roots: NativeResolutionRootPolicyV1::EveryExactPackage,
                positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
                provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
            },
            counts: NativeResolutionSurveyCountsV1::default(),
            histogram: BTreeMap::new(),
            failures: Vec::new(),
            evidence_byte_limit,
            retained_evidence_bytes: 0,
            retained_explanations: 0,
            withheld_explanations: 0,
            evidence_budget_exhausted: false,
        }
    }

    fn root() -> NativeParityPackageV1 {
        serde_json::from_value(serde_json::json!({
            "package_key_sha256": "c".repeat(64),
            "member_ordinal": 0,
            "source_identity": "test-source",
            "repository_identity": "test-repository",
            "source_snapshot_sha256": "d".repeat(64),
            "source_profile": "test-profile",
            "name": "test-package",
            "version": "1",
            "package_release": "1",
            "architecture": "x86_64",
            "debian_multi_arch": null,
            "checksum": format!("sha256:{}", "e".repeat(64)),
            "size": 1,
            "download_url": "https://example.test/test-package.rpm",
            "version_scheme": "rpm",
            "provides": [],
            "requirement_groups": []
        }))
        .unwrap()
    }

    #[test]
    fn survey_retains_bounded_failures_and_reports_uncapped_truth() {
        let mut collector = collector(NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT);
        let root = root();

        for _ in 0..=NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT {
            collector
                .failure(
                    &root,
                    *NativeRootResolutionError::new(
                        Error::ConfigError("diagnostic failure".to_string()),
                        NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                        NativeResolutionSurveyNativeExplanationV1::Rpm {
                            problems: Vec::new(),
                        },
                    ),
                )
                .unwrap();
        }

        let survey = collector.finish().unwrap();
        assert_eq!(
            survey.total_failures,
            NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT as u64 + 1
        );
        assert_eq!(
            survey.retained_failures,
            NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT as u64
        );
        assert!(survey.truncated);
        assert_eq!(
            survey.evidence_byte_limit,
            NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT
        );
        assert_eq!(
            survey.retained_explanations,
            NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT as u64
        );
        assert_eq!(survey.withheld_explanations, 0);
        assert!(!survey.truncated_evidence);
        assert_eq!(survey.counts.error_kinds.len(), 1);
        assert_eq!(survey.counts.error_kinds[0].count, survey.total_failures);

        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("survey.json");
        write_native_resolution_survey(&output, &survey).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let error = write_native_resolution_survey(&output, &survey).unwrap_err();
        assert!(matches!(error, Error::Io(_)));
    }

    #[test]
    fn survey_withholds_explanations_after_canonical_byte_budget_is_exhausted() {
        let explanation = NativeResolutionSurveyNativeExplanationV1::Rpm {
            problems: Vec::new(),
        };
        let explanation_bytes = canonical_value_size_with_limit(&explanation, u64::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(
            explanation_bytes,
            crate::json::canonical_json(&explanation).unwrap().len() as u64
        );
        let mut collector = collector(explanation_bytes);
        let root = root();

        for _ in 0..3 {
            collector
                .failure(
                    &root,
                    *NativeRootResolutionError::new(
                        Error::ConfigError("diagnostic failure".to_string()),
                        NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                        explanation.clone(),
                    ),
                )
                .unwrap();
        }

        let survey = collector.finish().unwrap();
        assert_eq!(survey.retained_failures, 3);
        assert_eq!(survey.evidence_byte_limit, explanation_bytes);
        assert_eq!(survey.retained_evidence_bytes, explanation_bytes);
        assert_eq!(survey.retained_explanations, 1);
        assert_eq!(survey.withheld_explanations, 2);
        assert!(survey.truncated_evidence);
        assert!(matches!(
            survey.failures[0].native_explanation,
            NativeResolutionSurveyNativeExplanationV1::Rpm { .. }
        ));
        assert!(survey.failures[1..].iter().all(|failure| matches!(
            failure.native_explanation,
            NativeResolutionSurveyNativeExplanationV1::Withheld {
                reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted
            }
        )));
        survey.validate().unwrap();
    }

    #[test]
    fn survey_counts_explanation_withheld_during_native_projection() {
        let mut collector = collector(1);
        collector
            .failure(
                &root(),
                *NativeRootResolutionError::new(
                    Error::ConfigError("diagnostic failure".to_string()),
                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                    NativeResolutionSurveyNativeExplanationV1::Withheld {
                        reason:
                            NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
                    },
                ),
            )
            .unwrap();

        let survey = collector.finish().unwrap();
        assert_eq!(survey.retained_evidence_bytes, 0);
        assert_eq!(survey.retained_explanations, 0);
        assert_eq!(survey.withheld_explanations, 1);
        assert!(survey.truncated_evidence);
        survey.validate().unwrap();
    }
}
