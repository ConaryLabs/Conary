// conary-core/src/corpus/aggregate.rs

//! Aggregation of corpus case results.
//!
//! Every tally is keyed by a typed discriminant: a [`ConversionStage`] and a
//! [`FailureKind`], plus an `incident_id` for unclassified failures that is
//! itself derived from the error's type chain. No key is derived from a
//! failure's human-readable detail, so rephrasing a message cannot split one
//! bucket into two or merge two distinct defects into one.
//!
//! This is enforced structurally rather than by convention: the key types are
//! `Copy` enums and the aggregation functions never read `detail()`.

use super::failure::FailureKind;
use super::{ConversionStage, CorpusCaseResult, CorpusOutcome};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Failures grouped by category within one stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureTally {
    pub kind: FailureKind,
    pub count: usize,
    /// Distinct incident ids seen for unclassified failures, sorted.
    ///
    /// Empty for every classified category. A growing list here means the
    /// underlying errors are too coarse to report on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incident_ids: Vec<String>,
}

/// One stage's failure profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageTally {
    pub stage: ConversionStage,
    pub failures: Vec<FailureTally>,
    pub total: usize,
}

/// Aggregate report over a set of cases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusAggregate {
    pub cases: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Stages that recorded at least one failure, in execution order.
    pub stages: Vec<StageTally>,
    /// Cases whose failure could not be attributed to a typed category.
    ///
    /// This is a gate signal in its own right: the corpus cannot claim a clean
    /// typed result while any case lands here.
    pub unclassified: usize,
}

impl CorpusAggregate {
    /// Whether every case completed its declared journey.
    pub fn all_completed(&self) -> bool {
        self.failed == 0 && self.completed == self.cases
    }
}

/// Aggregate cases by typed stage and failure category.
///
/// Keys come from enum discriminants only. Case ordering does not affect the
/// result: stages sort by execution order and categories by their stable code.
pub fn aggregate_cases(cases: &[CorpusCaseResult]) -> CorpusAggregate {
    let mut completed = 0usize;
    let mut skipped = 0usize;
    let mut unclassified = 0usize;

    // Keyed by typed discriminants, never by any human-readable text.
    let mut buckets: BTreeMap<ConversionStage, BTreeMap<FailureKind, (usize, Vec<String>)>> =
        BTreeMap::new();

    for case in cases {
        match &case.final_outcome {
            CorpusOutcome::Completed => completed += 1,
            CorpusOutcome::Skipped { .. } => skipped += 1,
            CorpusOutcome::Failed { stage, failure } => {
                let kind = failure.kind();
                if kind == FailureKind::InternalUnclassified {
                    unclassified += 1;
                }

                let entry = buckets
                    .entry(*stage)
                    .or_default()
                    .entry(kind)
                    .or_insert_with(|| (0, Vec::new()));
                entry.0 += 1;

                if let super::ConversionFailure::InternalUnclassified { incident_id, .. } = failure
                    && !entry.1.contains(incident_id)
                {
                    entry.1.push(incident_id.clone());
                }
            }
        }
    }

    let stages = buckets
        .into_iter()
        .map(|(stage, kinds)| {
            let mut total = 0usize;
            let failures = kinds
                .into_iter()
                .map(|(kind, (count, mut incident_ids))| {
                    total += count;
                    incident_ids.sort();
                    FailureTally {
                        kind,
                        count,
                        incident_ids,
                    }
                })
                .collect();
            StageTally {
                stage,
                failures,
                total,
            }
        })
        .collect::<Vec<_>>();

    let failed = stages.iter().map(|stage| stage.total).sum();

    CorpusAggregate {
        cases: cases.len(),
        completed,
        failed,
        skipped,
        stages,
        unclassified,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        ConversionFailure, SourceArtifactIdentity, SourceProfileIdentity, StageResult,
        TargetCapabilitySnapshot,
    };
    use super::*;

    fn case_failing_with(stage: ConversionStage, failure: ConversionFailure) -> CorpusCaseResult {
        CorpusCaseResult::from_stages(
            "arch-on-fedora",
            SourceProfileIdentity {
                profile: "arch".into(),
                format: "alpm".into(),
            },
            vec![SourceArtifactIdentity {
                role: super::super::SourceArtifactRole::InstallRequest,
                digest_source: super::super::SourceArtifactDigestSource::FixtureBuildManifest,
                name: "btop".into(),
                version: "1.4.0-1".into(),
                architecture: Some("x86_64".into()),
                digest: "d3983ae27b3e6e470bc33fd060a29c2e9a4a0c5a363ea76a07dd4ce300709c9c".into(),
            }],
            TargetCapabilitySnapshot {
                profile: "fedora-44".into(),
                architecture: "x86_64".into(),
                init_system: "systemd".into(),
                capabilities: Vec::new(),
            },
            Vec::new(),
            vec![StageResult::failed(stage, failure)],
        )
    }

    fn passing_case() -> CorpusCaseResult {
        CorpusCaseResult::from_stages(
            "arch-on-fedora",
            SourceProfileIdentity {
                profile: "arch".into(),
                format: "alpm".into(),
            },
            vec![SourceArtifactIdentity {
                role: super::super::SourceArtifactRole::InstallRequest,
                digest_source: super::super::SourceArtifactDigestSource::FixtureBuildManifest,
                name: "curl".into(),
                version: "8.0.0-1".into(),
                architecture: Some("x86_64".into()),
                digest: "abc".into(),
            }],
            TargetCapabilitySnapshot {
                profile: "fedora-44".into(),
                architecture: "x86_64".into(),
                init_system: "systemd".into(),
                capabilities: Vec::new(),
            },
            Vec::new(),
            vec![StageResult::passed(ConversionStage::Installation)],
        )
    }

    #[test]
    fn cases_tally_by_stage_and_category() {
        let cases = vec![
            passing_case(),
            case_failing_with(
                ConversionStage::NativeParse,
                ConversionFailure::SourceMetadata {
                    detail: "inode metadata disagreement".into(),
                },
            ),
            case_failing_with(
                ConversionStage::NativeParse,
                ConversionFailure::SourceMetadata {
                    detail: "a completely different sentence".into(),
                },
            ),
            case_failing_with(
                ConversionStage::CcsVerification,
                ConversionFailure::CcsAuthority {
                    detail: "manifest exceeds budget".into(),
                },
            ),
        ];

        let report = aggregate_cases(&cases);

        assert_eq!(report.cases, 4);
        assert_eq!(report.completed, 1);
        assert_eq!(report.failed, 3);
        assert!(!report.all_completed());
        assert_eq!(report.stages.len(), 2);

        let parse = &report.stages[0];
        assert_eq!(parse.stage, ConversionStage::NativeParse);
        assert_eq!(parse.total, 2);
        assert_eq!(parse.failures[0].kind, FailureKind::SourceMetadata);
        assert_eq!(
            parse.failures[0].count, 2,
            "two differently worded failures of one category are one bucket"
        );
    }

    /// The rule this module exists to enforce.
    #[test]
    fn differing_detail_text_does_not_split_a_bucket() {
        let cases = vec![
            case_failing_with(
                ConversionStage::NativeParse,
                ConversionFailure::SourceArchive {
                    detail: "truncated stream at entry 41".into(),
                },
            ),
            case_failing_with(
                ConversionStage::NativeParse,
                ConversionFailure::SourceArchive {
                    detail: "wholly unrelated phrasing".into(),
                },
            ),
        ];

        let report = aggregate_cases(&cases);

        assert_eq!(report.stages.len(), 1);
        assert_eq!(report.stages[0].failures.len(), 1);
        assert_eq!(report.stages[0].failures[0].count, 2);
    }

    /// The converse: identical wording must not merge distinct categories.
    #[test]
    fn identical_detail_text_does_not_merge_distinct_categories() {
        let shared = "conversion failed".to_string();
        let cases = vec![
            case_failing_with(
                ConversionStage::NativeParse,
                ConversionFailure::SourceArchive {
                    detail: shared.clone(),
                },
            ),
            case_failing_with(
                ConversionStage::NativeParse,
                ConversionFailure::ResourceBudget { detail: shared },
            ),
        ];

        let report = aggregate_cases(&cases);

        assert_eq!(
            report.stages[0].failures.len(),
            2,
            "same wording across two categories must stay two buckets"
        );
    }

    #[test]
    fn unclassified_failures_are_counted_and_carry_their_incident_ids() {
        let cases = vec![
            case_failing_with(
                ConversionStage::Publication,
                ConversionFailure::InternalUnclassified {
                    incident_id: "a1b2c3d4e5f60718".into(),
                    detail: "opaque".into(),
                },
            ),
            case_failing_with(
                ConversionStage::Publication,
                ConversionFailure::InternalUnclassified {
                    incident_id: "a1b2c3d4e5f60718".into(),
                    detail: "opaque but worded differently".into(),
                },
            ),
        ];

        let report = aggregate_cases(&cases);

        assert_eq!(report.unclassified, 2);
        let tally = &report.stages[0].failures[0];
        assert_eq!(tally.kind, FailureKind::InternalUnclassified);
        assert_eq!(tally.count, 2);
        assert_eq!(
            tally.incident_ids,
            vec!["a1b2c3d4e5f60718".to_string()],
            "one shared incident id is recorded once"
        );
    }

    #[test]
    fn skipped_cases_are_neither_passes_nor_failures() {
        let mut skipped = passing_case();
        skipped.final_outcome = CorpusOutcome::Skipped {
            reason: "fixture unavailable on this target".into(),
        };

        let report = aggregate_cases(&[skipped]);

        assert_eq!(report.skipped, 1);
        assert_eq!(report.completed, 0);
        assert_eq!(report.failed, 0);
        assert!(
            !report.all_completed(),
            "a skipped case must not satisfy the gate"
        );
    }

    #[test]
    fn stages_report_in_execution_order_regardless_of_case_order() {
        let cases = vec![
            case_failing_with(
                ConversionStage::Rollback,
                ConversionFailure::Transaction { detail: "x".into() },
            ),
            case_failing_with(
                ConversionStage::ArtifactDownload,
                ConversionFailure::Publication { detail: "y".into() },
            ),
        ];

        let report = aggregate_cases(&cases);

        assert_eq!(report.stages[0].stage, ConversionStage::ArtifactDownload);
        assert_eq!(report.stages[1].stage, ConversionStage::Rollback);
    }

    #[test]
    fn an_all_passing_corpus_reports_clean() {
        let report = aggregate_cases(&[passing_case(), passing_case()]);

        assert!(report.all_completed());
        assert_eq!(report.unclassified, 0);
        assert!(report.stages.is_empty());
    }
}
