// conary-core/src/corpus/mod.rs

//! Typed result vocabulary for the just-works corpus gate.
//!
//! The corpus proves that ordinary packages complete an ordinary user journey
//! across the supported source/target matrix. This module owns how that proof
//! is *reported*, not how it is run: the stages a case passes through, the
//! typed reason it stopped, and the identities that make a result attributable.
//!
//! It exists so gate results aggregate by typed stage and category rather than
//! by error-message text. Aggregating by wording makes a rephrased error look
//! like a new failure class and two distinct defects look like one, which is
//! not a basis for release decisions.
//!
//! The vocabulary lives in `conary-core` because it is shared by the CLI, Remi,
//! and the `conary-test` runner. See [`failure`] for the classification rule.

mod aggregate;
mod failure;

pub use aggregate::{CorpusAggregate, FailureTally, StageTally, aggregate_cases};
pub use failure::{ConversionFailure, FailureKind};

use serde::{Deserialize, Serialize};

/// One stage of the user journey a corpus case walks.
///
/// Ordering matches execution order, so `StageResult` sequences are directly
/// comparable and the first failing stage is the earliest one recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionStage {
    RepositoryAuthentication,
    ArtifactDownload,
    PackageSignatureVerification,
    NativeParse,
    CcsProjection,
    CcsSigning,
    CcsVerification,
    Publication,
    Resolution,
    TransactionPreflight,
    Installation,
    Update,
    Rollback,
    Removal,
}

impl ConversionStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConversionStage::RepositoryAuthentication => "repository_authentication",
            ConversionStage::ArtifactDownload => "artifact_download",
            ConversionStage::PackageSignatureVerification => "package_signature_verification",
            ConversionStage::NativeParse => "native_parse",
            ConversionStage::CcsProjection => "ccs_projection",
            ConversionStage::CcsSigning => "ccs_signing",
            ConversionStage::CcsVerification => "ccs_verification",
            ConversionStage::Publication => "publication",
            ConversionStage::Resolution => "resolution",
            ConversionStage::TransactionPreflight => "transaction_preflight",
            ConversionStage::Installation => "installation",
            ConversionStage::Update => "update",
            ConversionStage::Rollback => "rollback",
            ConversionStage::Removal => "removal",
        }
    }

    /// Every stage in execution order.
    pub const ALL: [ConversionStage; 14] = [
        ConversionStage::RepositoryAuthentication,
        ConversionStage::ArtifactDownload,
        ConversionStage::PackageSignatureVerification,
        ConversionStage::NativeParse,
        ConversionStage::CcsProjection,
        ConversionStage::CcsSigning,
        ConversionStage::CcsVerification,
        ConversionStage::Publication,
        ConversionStage::Resolution,
        ConversionStage::TransactionPreflight,
        ConversionStage::Installation,
        ConversionStage::Update,
        ConversionStage::Rollback,
        ConversionStage::Removal,
    ];
}

/// Which configured upstream feed the artifact came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceProfileIdentity {
    /// Exact configured profile id, e.g. `fedora-44`.
    pub profile: String,
    /// Source package format the profile's parser owns.
    pub format: String,
}

/// The exact artifact under test, pinned by digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArtifactIdentity {
    /// How this artifact participates in the declared journey.
    pub role: SourceArtifactRole,
    /// Typed authority from which the exact artifact digest was obtained.
    pub digest_source: SourceArtifactDigestSource,
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    /// SHA-256 of the source artifact. A result without this is not evidence.
    pub digest: String,
}

/// The lifecycle role of one exact source artifact.
///
/// A case may consume more than one artifact: an install followed by an update
/// must identify both inputs rather than attributing the complete journey to
/// the first package digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceArtifactRole {
    InstallRequest,
    InstallDependency,
    UpdateRequest,
    UpdateDependency,
}

/// Authority that produced an artifact's pinned digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceArtifactDigestSource {
    /// The fixture builder's versioned output manifest, computed from the
    /// completed native package bytes.
    FixtureBuildManifest,
}

/// Typed host capabilities the case ran against.
///
/// Deliberately capability facts rather than a distro name, matching the
/// lifecycle contract: the target exposes capabilities, it does not select
/// behavior by identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetCapabilitySnapshot {
    /// Target profile id used for reporting attribution.
    pub profile: String,
    pub architecture: String,
    pub init_system: String,
    /// Additional typed capability facts recorded for this run.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Outcome of one stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum StageOutcome {
    Passed,
    Failed {
        failure: ConversionFailure,
    },
    /// Not reached because an earlier stage failed.
    NotReached,
    /// Deliberately not part of this case's declared journey.
    NotApplicable {
        reason: String,
    },
}

/// One stage's recorded result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: ConversionStage,
    #[serde(flatten)]
    pub outcome: StageOutcome,
}

impl StageResult {
    pub fn passed(stage: ConversionStage) -> Self {
        Self {
            stage,
            outcome: StageOutcome::Passed,
        }
    }

    pub fn failed(stage: ConversionStage, failure: ConversionFailure) -> Self {
        Self {
            stage,
            outcome: StageOutcome::Failed { failure },
        }
    }

    pub fn not_reached(stage: ConversionStage) -> Self {
        Self {
            stage,
            outcome: StageOutcome::NotReached,
        }
    }

    pub fn failure(&self) -> Option<&ConversionFailure> {
        match &self.outcome {
            StageOutcome::Failed { failure } => Some(failure),
            _ => None,
        }
    }
}

/// Final disposition of a case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CorpusOutcome {
    /// Every declared stage passed.
    Completed,
    /// A stage failed. The stage and category are the aggregation key.
    Failed {
        stage: ConversionStage,
        failure: ConversionFailure,
    },
    /// The case did not run. Not a pass and not a failure of the code.
    Skipped { reason: String },
}

/// One corpus case: an exact artifact against an exact target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusCaseResult {
    /// Stable manifest-owned identity for this case.
    pub case_id: String,
    pub source_profile: SourceProfileIdentity,
    pub source_artifacts: Vec<SourceArtifactIdentity>,
    pub target_profile: TargetCapabilitySnapshot,
    pub stage_results: Vec<StageResult>,
    pub final_outcome: CorpusOutcome,
}

impl CorpusCaseResult {
    /// Build a result from the stages actually walked.
    ///
    /// The outcome is derived from the first failing stage rather than passed
    /// in separately, so a result cannot claim success while carrying a failed
    /// stage.
    pub fn from_stages(
        case_id: impl Into<String>,
        source_profile: SourceProfileIdentity,
        source_artifacts: Vec<SourceArtifactIdentity>,
        target_profile: TargetCapabilitySnapshot,
        stage_results: Vec<StageResult>,
    ) -> Self {
        let first_failure = stage_results.iter().find_map(|result| {
            result.failure().map(|failure| CorpusOutcome::Failed {
                stage: result.stage,
                failure: failure.clone(),
            })
        });
        let final_outcome = first_failure.unwrap_or_else(|| {
            if stage_results
                .iter()
                .any(|result| matches!(&result.outcome, StageOutcome::NotReached))
            {
                CorpusOutcome::Skipped {
                    reason: "one or more declared stages were not reached".into(),
                }
            } else {
                CorpusOutcome::Completed
            }
        });

        Self {
            case_id: case_id.into(),
            source_profile,
            source_artifacts,
            target_profile,
            stage_results,
            final_outcome,
        }
    }

    pub fn completed(&self) -> bool {
        matches!(self.final_outcome, CorpusOutcome::Completed)
    }

    /// The stage and category this case failed at, if any.
    pub fn failure_key(&self) -> Option<(ConversionStage, FailureKind)> {
        match &self.final_outcome {
            CorpusOutcome::Failed { stage, failure } => Some((*stage, failure.kind())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> SourceProfileIdentity {
        SourceProfileIdentity {
            profile: "fedora-44".into(),
            format: "rpm".into(),
        }
    }

    fn artifact() -> SourceArtifactIdentity {
        SourceArtifactIdentity {
            role: SourceArtifactRole::InstallRequest,
            digest_source: SourceArtifactDigestSource::FixtureBuildManifest,
            name: "2ping".into(),
            version: "4.5.1-24.fc44".into(),
            architecture: Some("noarch".into()),
            digest: "cf48a9380416daf02e934cbddd15d5356b3b6ea6b5f0824187074b66dd0fe14a".into(),
        }
    }

    fn target() -> TargetCapabilitySnapshot {
        TargetCapabilitySnapshot {
            profile: "arch".into(),
            architecture: "x86_64".into(),
            init_system: "systemd".into(),
            capabilities: vec!["posix-acl".into()],
        }
    }

    #[test]
    fn a_case_with_every_stage_passing_completes() {
        let stages = vec![
            StageResult::passed(ConversionStage::NativeParse),
            StageResult::passed(ConversionStage::CcsProjection),
            StageResult::passed(ConversionStage::Installation),
        ];

        let case = CorpusCaseResult::from_stages(
            "rpm-on-fedora",
            profile(),
            vec![artifact()],
            target(),
            stages,
        );

        assert!(case.completed());
        assert_eq!(case.failure_key(), None);
    }

    #[test]
    fn a_case_with_unreached_stages_cannot_complete() {
        let case = CorpusCaseResult::from_stages(
            "rpm-on-fedora",
            profile(),
            vec![artifact()],
            target(),
            vec![StageResult::not_reached(ConversionStage::Installation)],
        );

        assert!(!case.completed());
        assert!(matches!(case.final_outcome, CorpusOutcome::Skipped { .. }));
    }

    /// A result must not be able to claim success while carrying a failed
    /// stage, so the outcome is derived rather than supplied.
    #[test]
    fn a_failed_stage_forces_a_failed_outcome() {
        let stages = vec![
            StageResult::passed(ConversionStage::NativeParse),
            StageResult::failed(
                ConversionStage::CcsVerification,
                ConversionFailure::CcsAuthority {
                    detail: "manifest exceeds declared budget".into(),
                },
            ),
            StageResult::not_reached(ConversionStage::Installation),
        ];

        let case = CorpusCaseResult::from_stages(
            "rpm-on-fedora",
            profile(),
            vec![artifact()],
            target(),
            stages,
        );

        assert!(!case.completed());
        assert_eq!(
            case.failure_key(),
            Some((ConversionStage::CcsVerification, FailureKind::CcsAuthority))
        );
    }

    #[test]
    fn the_earliest_failing_stage_owns_the_outcome() {
        let stages = vec![
            StageResult::failed(
                ConversionStage::PackageSignatureVerification,
                ConversionFailure::Trust {
                    detail: "unknown signing subkey".into(),
                },
            ),
            StageResult::failed(
                ConversionStage::NativeParse,
                ConversionFailure::SourceArchive {
                    detail: "truncated".into(),
                },
            ),
        ];

        let case = CorpusCaseResult::from_stages(
            "rpm-on-fedora",
            profile(),
            vec![artifact()],
            target(),
            stages,
        );

        assert_eq!(
            case.failure_key(),
            Some((
                ConversionStage::PackageSignatureVerification,
                FailureKind::Trust
            )),
            "the first failure in execution order is the attributable one"
        );
    }

    #[test]
    fn stage_codes_are_stable_and_unique() {
        let mut codes: Vec<&str> = ConversionStage::ALL.iter().map(|s| s.as_str()).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "stage codes must be unique");
        assert_eq!(
            ConversionStage::ALL.len(),
            14,
            "the journey has fourteen stages"
        );
    }

    #[test]
    fn stages_order_by_execution_sequence() {
        assert!(ConversionStage::RepositoryAuthentication < ConversionStage::NativeParse);
        assert!(ConversionStage::Installation < ConversionStage::Rollback);
        assert!(ConversionStage::Rollback < ConversionStage::Removal);
    }

    #[test]
    fn a_case_round_trips_through_serde() {
        let case = CorpusCaseResult::from_stages(
            "rpm-on-fedora",
            profile(),
            vec![artifact()],
            target(),
            vec![StageResult::failed(
                ConversionStage::NativeParse,
                ConversionFailure::SourceMetadata {
                    detail: "inode metadata disagreement".into(),
                },
            )],
        );

        let json = serde_json::to_string(&case).expect("serialize");
        let restored: CorpusCaseResult = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored, case);
        assert_eq!(
            restored.failure_key(),
            Some((ConversionStage::NativeParse, FailureKind::SourceMetadata))
        );
    }
}
