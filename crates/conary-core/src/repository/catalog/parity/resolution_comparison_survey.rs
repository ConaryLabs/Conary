// crates/conary-core/src/repository/catalog/parity/resolution_comparison_survey.rs

//! Diagnostics-only collect-all native/candidate resolution comparison.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::contract::NativeParityPackageV1;
use super::io::NativeParityOracleReader;
use super::resolution_compare::{NativeResolutionOutcomeKindV1, outcome_mismatch};
use super::resolution_contract::{NativeResolutionOutcomeV1, NativeResolutionRootV1};
use super::resolution_io::NativeResolutionOracleReader;
use super::survey_support::write_private_canonical_json;
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::contract::{validate_identity, validate_sha256};

pub const NATIVE_RESOLUTION_COMPARISON_SURVEY_SCHEMA_V2: u32 = 2;
pub const NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyV1 {
    pub schema_version: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub package_oracle_manifest_sha256: String,
    pub oracle_manifest_sha256: String,
    pub candidate_manifest_sha256: String,
    pub counts: NativeResolutionComparisonSurveyCountsV1,
    pub mismatch_record_limit: u64,
    pub total_mismatches: u64,
    pub retained_mismatches: u64,
    pub truncated: bool,
    pub mismatches: Vec<NativeResolutionComparisonSurveyMismatchV1>,
}

impl NativeResolutionComparisonSurveyV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != NATIVE_RESOLUTION_COMPARISON_SURVEY_SCHEMA_V2 {
            return Err(Error::ConfigError(format!(
                "native resolution comparison survey schema {} is unsupported; expected {}",
                self.schema_version, NATIVE_RESOLUTION_COMPARISON_SURVEY_SCHEMA_V2
            )));
        }
        validate_identity(&self.profile, "resolution comparison survey profile")?;
        for (value, label) in [
            (
                &self.profile_revision_sha256,
                "comparison survey profile revision",
            ),
            (
                &self.package_oracle_manifest_sha256,
                "comparison survey package oracle",
            ),
            (
                &self.oracle_manifest_sha256,
                "comparison survey native oracle",
            ),
            (
                &self.candidate_manifest_sha256,
                "comparison survey candidate",
            ),
        ] {
            validate_sha256(value, label)?;
        }
        self.counts.validate()?;
        if self.total_mismatches != self.counts.mismatched_roots
            || self.retained_mismatches != self.mismatches.len() as u64
            || self.retained_mismatches > self.total_mismatches
            || self.retained_mismatches > self.mismatch_record_limit
            || self.truncated != (self.retained_mismatches < self.total_mismatches)
        {
            return Err(Error::ConfigError(
                "native resolution comparison survey mismatch counts are inconsistent".to_string(),
            ));
        }
        for mismatch in &self.mismatches {
            mismatch.validate(self)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyCountsV1 {
    pub roots_walked: u64,
    pub matching_roots: u64,
    pub mismatched_roots: u64,
    pub mismatch_kinds: Vec<NativeResolutionComparisonSurveyMismatchCountV1>,
    pub outcome_kind_pairs: Vec<NativeResolutionComparisonSurveyOutcomePairCountV1>,
}

impl NativeResolutionComparisonSurveyCountsV1 {
    fn validate(&self) -> Result<()> {
        if self.matching_roots.checked_add(self.mismatched_roots) != Some(self.roots_walked) {
            return Err(Error::ConfigError(
                "native resolution comparison survey root counts are inconsistent".to_string(),
            ));
        }
        validate_histogram(
            &self.mismatch_kinds,
            self.mismatched_roots,
            |entry| &entry.kind,
            |entry| entry.count,
            "mismatch-kind",
        )?;
        validate_histogram(
            &self.outcome_kind_pairs,
            self.mismatched_roots,
            |entry| &entry.pair,
            |entry| entry.count,
            "outcome-pair",
        )
    }
}

fn validate_histogram<T, K: Ord>(
    entries: &[T],
    expected: u64,
    key: impl Fn(&T) -> &K,
    count: impl Fn(&T) -> u64,
    label: &str,
) -> Result<()> {
    let total = entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(count(entry)).ok_or_else(|| {
            Error::ConfigError(format!(
                "resolution comparison survey {label} histogram exceeds u64"
            ))
        })
    })?;
    if total != expected
        || entries
            .windows(2)
            .any(|pair| key(&pair[0]) >= key(&pair[1]))
    {
        return Err(Error::ConfigError(format!(
            "resolution comparison survey {label} histogram is inconsistent"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionComparisonSurveyMismatchKindV1 {
    ResolutionOutcome,
    DependencyClosure,
    UnresolvedDependencies,
    NotInstallableReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyMismatchCountV1 {
    pub kind: NativeResolutionComparisonSurveyMismatchKindV1,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyOutcomePairV1 {
    pub oracle: NativeResolutionOutcomeKindV1,
    pub candidate: NativeResolutionOutcomeKindV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyOutcomePairCountV1 {
    pub pair: NativeResolutionComparisonSurveyOutcomePairV1,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyRootIdentityV1 {
    pub package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub release: String,
    pub architecture: Option<String>,
}

impl NativeResolutionComparisonSurveyRootIdentityV1 {
    fn from_package(package: &NativeParityPackageV1) -> Self {
        Self {
            package_key_sha256: package.package_key_sha256.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            release: package.package_release.clone(),
            architecture: package.architecture.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyOutcomeEvidenceV1 {
    pub manifest_sha256: String,
    pub outcome: NativeResolutionOutcomeV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionComparisonSurveyMismatchV1 {
    pub root: NativeResolutionComparisonSurveyRootIdentityV1,
    pub kind: NativeResolutionComparisonSurveyMismatchKindV1,
    pub oracle: NativeResolutionComparisonSurveyOutcomeEvidenceV1,
    pub candidate: NativeResolutionComparisonSurveyOutcomeEvidenceV1,
}

impl NativeResolutionComparisonSurveyMismatchV1 {
    fn validate(&self, survey: &NativeResolutionComparisonSurveyV1) -> Result<()> {
        validate_sha256(&self.root.package_key_sha256, "comparison survey root key")?;
        validate_identity(&self.root.name, "comparison survey root package name")?;
        validate_identity(&self.root.version, "comparison survey root package version")?;
        validate_identity(&self.root.release, "comparison survey root package release")?;
        NativeResolutionRootV1 {
            root_package_key_sha256: self.root.package_key_sha256.clone(),
            outcome: self.oracle.outcome.clone(),
        }
        .validate()?;
        NativeResolutionRootV1 {
            root_package_key_sha256: self.root.package_key_sha256.clone(),
            outcome: self.candidate.outcome.clone(),
        }
        .validate()?;
        if self.oracle.outcome == self.candidate.outcome {
            return Err(Error::ConfigError(
                "native resolution comparison survey retained an equal outcome pair".to_string(),
            ));
        }
        if self.oracle.manifest_sha256 != survey.oracle_manifest_sha256
            || self.candidate.manifest_sha256 != survey.candidate_manifest_sha256
            || mismatch_kind(&self.oracle.outcome, &self.candidate.outcome) != self.kind
        {
            return Err(Error::ConfigError(
                "native resolution comparison survey mismatch evidence is inconsistent".to_string(),
            ));
        }
        Ok(())
    }
}

pub fn compare_native_resolution_oracle_survey(
    profile: &ProfileRevisionV2,
    package_oracle: &NativeParityOracleReader,
    oracle: &NativeResolutionOracleReader,
    candidate: &NativeResolutionOracleReader,
) -> Result<NativeResolutionComparisonSurveyV1> {
    oracle
        .manifest()
        .validate_binding(profile, package_oracle.manifest())?;
    candidate
        .manifest()
        .validate_binding(profile, package_oracle.manifest())?;
    if oracle.manifest().policy != candidate.manifest().policy {
        return Err(Error::ConflictError(
            "native resolution candidate uses a different resolution policy".to_string(),
        ));
    }
    oracle.verify_package_oracle(package_oracle)?;
    candidate.verify_package_oracle(package_oracle)?;

    let package_oracle_manifest_sha256 = package_oracle.manifest().manifest_sha256()?;
    let oracle_manifest_sha256 = oracle.manifest().manifest_sha256()?;
    let candidate_manifest_sha256 = candidate.manifest().manifest_sha256()?;
    let mut identities = BTreeMap::new();
    package_oracle.for_each_package(|package| {
        identities.insert(
            package.package_key_sha256.clone(),
            NativeResolutionComparisonSurveyRootIdentityV1::from_package(&package),
        );
        Ok(())
    })?;

    let mut collector = ComparisonSurveyCollector::new(
        oracle_manifest_sha256.clone(),
        candidate_manifest_sha256.clone(),
        identities,
    );
    let mut oracle_cursor = oracle.cursor()?;
    let mut candidate_cursor = candidate.cursor()?;
    loop {
        match (oracle_cursor.next_root()?, candidate_cursor.next_root()?) {
            (Some(oracle_root), Some(candidate_root)) => {
                if oracle_root.root_package_key_sha256 != candidate_root.root_package_key_sha256 {
                    return Err(Error::InternalError(
                        "verified resolution inputs lost canonical root alignment".to_string(),
                    ));
                }
                collector.root(&oracle_root, &candidate_root)?;
            }
            (None, None) => break,
            _ => {
                return Err(Error::InternalError(
                    "verified resolution inputs lost complete root coverage".to_string(),
                ));
            }
        }
    }
    let survey = collector.finish(NativeResolutionComparisonSurveyV1 {
        schema_version: NATIVE_RESOLUTION_COMPARISON_SURVEY_SCHEMA_V2,
        profile: profile.profile.clone(),
        profile_revision_sha256: profile.manifest_sha256()?,
        package_oracle_manifest_sha256,
        oracle_manifest_sha256,
        candidate_manifest_sha256,
        counts: NativeResolutionComparisonSurveyCountsV1::default(),
        mismatch_record_limit: NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT as u64,
        total_mismatches: 0,
        retained_mismatches: 0,
        truncated: false,
        mismatches: Vec::new(),
    })?;
    Ok(survey)
}

pub fn write_native_resolution_comparison_survey(
    path: &Path,
    survey: &NativeResolutionComparisonSurveyV1,
) -> Result<()> {
    survey.validate()?;
    write_private_canonical_json(path, survey, "native resolution comparison survey")
}

struct ComparisonSurveyCollector {
    oracle_manifest_sha256: String,
    candidate_manifest_sha256: String,
    identities: BTreeMap<String, NativeResolutionComparisonSurveyRootIdentityV1>,
    counts: NativeResolutionComparisonSurveyCountsV1,
    mismatch_kinds: BTreeMap<NativeResolutionComparisonSurveyMismatchKindV1, u64>,
    outcome_pairs: BTreeMap<NativeResolutionComparisonSurveyOutcomePairV1, u64>,
    mismatches: Vec<NativeResolutionComparisonSurveyMismatchV1>,
}

impl ComparisonSurveyCollector {
    fn new(
        oracle_manifest_sha256: String,
        candidate_manifest_sha256: String,
        identities: BTreeMap<String, NativeResolutionComparisonSurveyRootIdentityV1>,
    ) -> Self {
        Self {
            oracle_manifest_sha256,
            candidate_manifest_sha256,
            identities,
            counts: NativeResolutionComparisonSurveyCountsV1::default(),
            mismatch_kinds: BTreeMap::new(),
            outcome_pairs: BTreeMap::new(),
            mismatches: Vec::new(),
        }
    }

    fn root(
        &mut self,
        oracle: &NativeResolutionRootV1,
        candidate: &NativeResolutionRootV1,
    ) -> Result<()> {
        self.counts.roots_walked = increment(self.counts.roots_walked)?;
        if oracle == candidate {
            self.counts.matching_roots = increment(self.counts.matching_roots)?;
            return Ok(());
        }
        self.counts.mismatched_roots = increment(self.counts.mismatched_roots)?;
        let oracle_outcome = oracle.outcome.clone();
        let candidate_outcome = candidate.outcome.clone();
        let kind = mismatch_kind(&oracle_outcome, &candidate_outcome);
        let pair = NativeResolutionComparisonSurveyOutcomePairV1 {
            oracle: NativeResolutionOutcomeKindV1::from_outcome(&oracle_outcome),
            candidate: NativeResolutionOutcomeKindV1::from_outcome(&candidate_outcome),
        };
        increment_entry(&mut self.mismatch_kinds, kind)?;
        increment_entry(&mut self.outcome_pairs, pair.clone())?;
        if self.mismatches.len() < NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT {
            let key = &oracle.root_package_key_sha256;
            let root = self.identities.get(key).cloned().ok_or_else(|| {
                Error::InternalError(format!(
                    "verified package oracle omitted comparison root {key}"
                ))
            })?;
            self.mismatches
                .push(NativeResolutionComparisonSurveyMismatchV1 {
                    root,
                    kind,
                    oracle: NativeResolutionComparisonSurveyOutcomeEvidenceV1 {
                        manifest_sha256: self.oracle_manifest_sha256.clone(),
                        outcome: oracle_outcome,
                    },
                    candidate: NativeResolutionComparisonSurveyOutcomeEvidenceV1 {
                        manifest_sha256: self.candidate_manifest_sha256.clone(),
                        outcome: candidate_outcome,
                    },
                });
        }
        Ok(())
    }

    fn finish(
        self,
        mut survey: NativeResolutionComparisonSurveyV1,
    ) -> Result<NativeResolutionComparisonSurveyV1> {
        survey.counts = self.counts;
        survey.counts.mismatch_kinds = self
            .mismatch_kinds
            .into_iter()
            .map(|(kind, count)| NativeResolutionComparisonSurveyMismatchCountV1 { kind, count })
            .collect();
        survey.counts.outcome_kind_pairs = self
            .outcome_pairs
            .into_iter()
            .map(|(pair, count)| NativeResolutionComparisonSurveyOutcomePairCountV1 { pair, count })
            .collect();
        survey.total_mismatches = survey.counts.mismatched_roots;
        survey.retained_mismatches = self.mismatches.len() as u64;
        survey.truncated = survey.retained_mismatches < survey.total_mismatches;
        survey.mismatches = self.mismatches;
        survey.validate()?;
        Ok(survey)
    }
}

fn mismatch_kind(
    oracle: &NativeResolutionOutcomeV1,
    candidate: &NativeResolutionOutcomeV1,
) -> NativeResolutionComparisonSurveyMismatchKindV1 {
    match outcome_mismatch("survey", oracle, candidate) {
        super::resolution_compare::NativeResolutionMismatchV1::ResolutionOutcome { .. } => {
            NativeResolutionComparisonSurveyMismatchKindV1::ResolutionOutcome
        }
        super::resolution_compare::NativeResolutionMismatchV1::DependencyClosure { .. } => {
            NativeResolutionComparisonSurveyMismatchKindV1::DependencyClosure
        }
        super::resolution_compare::NativeResolutionMismatchV1::UnresolvedDependencies {
            ..
        } => NativeResolutionComparisonSurveyMismatchKindV1::UnresolvedDependencies,
        super::resolution_compare::NativeResolutionMismatchV1::NotInstallableReason { .. } => {
            NativeResolutionComparisonSurveyMismatchKindV1::NotInstallableReason
        }
        _ => unreachable!("two complete outcomes cannot produce a missing-root mismatch"),
    }
}

fn increment_entry<K: Ord>(map: &mut BTreeMap<K, u64>, key: K) -> Result<()> {
    let count = map.entry(key).or_default();
    *count = increment(*count)?;
    Ok(())
}

fn increment(value: u64) -> Result<u64> {
    value.checked_add(1).ok_or_else(|| {
        Error::ConfigError("native resolution comparison survey count exceeds u64".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(key: &str) -> NativeResolutionRootV1 {
        NativeResolutionRootV1 {
            root_package_key_sha256: key.to_string(),
            outcome: NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: vec![key.to_string()],
            },
        }
    }

    fn unresolved(key: &str) -> NativeResolutionRootV1 {
        NativeResolutionRootV1 {
            root_package_key_sha256: key.to_string(),
            outcome: NativeResolutionOutcomeV1::Unresolved {
                dependencies: vec![
                    super::super::resolution_contract::NativeUnresolvedDependencyV1 {
                        requiring_package_key_sha256: key.to_string(),
                        requirement_group_sha256: "e".repeat(64),
                    },
                ],
            },
        }
    }

    #[test]
    fn collector_caps_records_but_preserves_uncapped_histograms() {
        let identities = (0..=NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT)
            .map(|index| {
                let key = format!("{index:064x}");
                (
                    key.clone(),
                    NativeResolutionComparisonSurveyRootIdentityV1 {
                        package_key_sha256: key,
                        name: format!("package-{index:05}"),
                        version: "1".to_string(),
                        release: "1".to_string(),
                        architecture: Some("x86_64".to_string()),
                    },
                )
            })
            .collect();
        let mut collector =
            ComparisonSurveyCollector::new("c".repeat(64), "d".repeat(64), identities);
        for index in 0..=NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT {
            let key = format!("{index:064x}");
            collector.root(&resolved(&key), &unresolved(&key)).unwrap();
        }
        let survey = collector
            .finish(NativeResolutionComparisonSurveyV1 {
                schema_version: NATIVE_RESOLUTION_COMPARISON_SURVEY_SCHEMA_V2,
                profile: "fedora-44".to_string(),
                profile_revision_sha256: "a".repeat(64),
                package_oracle_manifest_sha256: "b".repeat(64),
                oracle_manifest_sha256: "c".repeat(64),
                candidate_manifest_sha256: "d".repeat(64),
                counts: NativeResolutionComparisonSurveyCountsV1::default(),
                mismatch_record_limit: NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT as u64,
                total_mismatches: 0,
                retained_mismatches: 0,
                truncated: false,
                mismatches: Vec::new(),
            })
            .unwrap();

        assert_eq!(
            survey.total_mismatches,
            NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT as u64 + 1
        );
        assert_eq!(
            survey.retained_mismatches,
            NATIVE_RESOLUTION_COMPARISON_SURVEY_MISMATCH_LIMIT as u64
        );
        assert!(survey.truncated);
        assert_eq!(survey.counts.mismatch_kinds.len(), 1);
        assert_eq!(
            survey.counts.mismatch_kinds[0].count,
            survey.total_mismatches
        );
        assert_eq!(survey.counts.outcome_kind_pairs.len(), 1);
        assert_eq!(
            survey.counts.outcome_kind_pairs[0].count,
            survey.total_mismatches
        );

        let mut obsolete = survey;
        obsolete.schema_version = 1;
        assert!(obsolete.validate().is_err());
    }
}
