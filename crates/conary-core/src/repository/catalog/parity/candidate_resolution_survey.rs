// crates/conary-core/src/repository/catalog/parity/candidate_resolution_survey.rs

//! Diagnostics-only Conary exact-root resolution survey contract.

use std::collections::BTreeMap;
use std::path::Path;

use petgraph::Direction;
use petgraph::visit::EdgeRef;
use resolvo::conflict::{ConflictCause, ConflictEdge, ConflictGraph, ConflictNode};
use resolvo::{Interner, Requirement, SolvableId, VersionSetId};
use serde::{Deserialize, Serialize};

use super::contract::{NativeParityImplementationV1, NativeParityOracleV1, NativeParityPackageV1};
use super::resolution_contract::{NativeResolutionOutcomeV1, NativeResolutionPolicyV1};
use super::resolution_survey::{
    NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT, NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT,
    NativeResolutionSurveyErrorVariantV1,
};
use super::support::{Counter, checked_increment};
use super::survey_support::{canonical_value_size_with_limit, write_private_canonical_json};
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::contract::{validate_identity, validate_sha256};
use crate::resolver::identity::PackageIdentity;
use crate::resolver::provider::ConaryProvider;

pub const CONARY_RESOLUTION_SURVEY_SCHEMA_V2: u32 = 2;
pub const CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT: usize = NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT;
pub const CONARY_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT: u64 =
    NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyV1 {
    pub schema_version: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub package_oracle_manifest_sha256: String,
    pub implementation: NativeParityImplementationV1,
    pub policy: NativeResolutionPolicyV1,
    pub target_architecture: String,
    pub counts: ConaryResolutionSurveyCountsV1,
    pub outcomes: Vec<ConaryResolutionSurveyRootOutcomeV1>,
    pub failure_record_limit: u64,
    pub total_failures: u64,
    pub retained_failures: u64,
    pub truncated: bool,
    pub evidence_byte_limit: u64,
    pub retained_evidence_bytes: u64,
    pub retained_explanations: u64,
    pub withheld_explanations: u64,
    pub truncated_evidence: bool,
    pub failures: Vec<ConaryResolutionSurveyFailureV1>,
}

impl ConaryResolutionSurveyV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CONARY_RESOLUTION_SURVEY_SCHEMA_V2 {
            return Err(Error::ConfigError(format!(
                "Conary resolution survey schema {} is unsupported; expected {}",
                self.schema_version, CONARY_RESOLUTION_SURVEY_SCHEMA_V2
            )));
        }
        validate_identity(&self.profile, "Conary resolution survey profile")?;
        validate_sha256(
            &self.profile_revision_sha256,
            "Conary resolution survey profile revision SHA-256",
        )?;
        validate_sha256(
            &self.package_oracle_manifest_sha256,
            "Conary resolution survey package oracle manifest SHA-256",
        )?;
        self.implementation.validate()?;
        self.policy.validate()?;
        validate_identity(
            &self.target_architecture,
            "Conary resolution survey target architecture",
        )?;
        if self.target_architecture != self.policy.architecture {
            return Err(Error::ConfigError(
                "Conary resolution survey target architecture disagrees with policy".to_string(),
            ));
        }
        self.counts.validate()?;
        self.validate_outcomes()?;
        if self.total_failures != self.counts.failed_roots
            || self.retained_failures != self.failures.len() as u64
            || self.retained_failures > self.total_failures
            || self.retained_failures > self.failure_record_limit
            || self.truncated != (self.retained_failures < self.total_failures)
        {
            return Err(Error::ConfigError(
                "Conary resolution survey failure counts are inconsistent".to_string(),
            ));
        }
        self.validate_evidence()
    }

    fn validate_outcomes(&self) -> Result<()> {
        let successful_roots = self
            .counts
            .resolved_roots
            .checked_add(self.counts.unresolved_roots)
            .and_then(|value| value.checked_add(self.counts.not_installable_roots))
            .ok_or_else(|| {
                Error::ConfigError("Conary resolution survey counts exceed u64".to_string())
            })?;
        if self.outcomes.len() as u64 != successful_roots
            || self
                .outcomes
                .windows(2)
                .any(|pair| pair[0].root_package_key_sha256 >= pair[1].root_package_key_sha256)
            || self
                .failures
                .windows(2)
                .any(|pair| pair[0].root_package_key_sha256 >= pair[1].root_package_key_sha256)
            || self.outcomes.iter().any(|outcome| {
                self.failures
                    .binary_search_by(|failure| {
                        failure
                            .root_package_key_sha256
                            .cmp(&outcome.root_package_key_sha256)
                    })
                    .is_ok()
            })
        {
            return Err(Error::ConfigError(
                "Conary resolution survey root records are inconsistent".to_string(),
            ));
        }
        let mut resolved = 0_u64;
        let mut unresolved = 0_u64;
        let mut not_installable = 0_u64;
        for outcome in &self.outcomes {
            outcome.validate()?;
            match outcome.outcome {
                NativeResolutionOutcomeV1::Resolved { .. } => {
                    resolved = checked_increment(resolved, Counter::Survey("Conary"))?;
                }
                NativeResolutionOutcomeV1::Unresolved { .. } => {
                    unresolved = checked_increment(unresolved, Counter::Survey("Conary"))?;
                }
                NativeResolutionOutcomeV1::NotInstallable { .. } => {
                    not_installable =
                        checked_increment(not_installable, Counter::Survey("Conary"))?;
                }
            }
        }
        if resolved != self.counts.resolved_roots
            || unresolved != self.counts.unresolved_roots
            || not_installable != self.counts.not_installable_roots
        {
            return Err(Error::ConfigError(
                "Conary resolution survey typed outcomes disagree with counts".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_evidence(&self) -> Result<()> {
        let mut bytes = 0_u64;
        let mut retained = 0_u64;
        let mut withheld = 0_u64;
        let mut withholding_started = false;
        for failure in &self.failures {
            failure.validate()?;
            match &failure.native_explanation {
                ConaryResolutionSurveyNativeExplanationV1::Withheld {
                    reason: ConaryResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
                } => {
                    withholding_started = true;
                    withheld = checked_increment(withheld, Counter::Survey("Conary"))?;
                }
                explanation => {
                    if withholding_started {
                        return Err(Error::ConfigError(
                            "Conary resolution survey retained evidence after withholding began"
                                .to_string(),
                        ));
                    }
                    let remaining =
                        self.evidence_byte_limit.checked_sub(bytes).ok_or_else(|| {
                            Error::ConfigError(
                                "Conary resolution survey evidence exceeds its byte limit"
                                    .to_string(),
                            )
                        })?;
                    let Some(size) = canonical_value_size_with_limit(explanation, remaining)?
                    else {
                        return Err(Error::ConfigError(
                            "Conary resolution survey evidence exceeds its byte limit".to_string(),
                        ));
                    };
                    bytes = bytes.checked_add(size).ok_or_else(|| {
                        Error::ConfigError(
                            "Conary resolution survey evidence exceeds u64".to_string(),
                        )
                    })?;
                    retained = checked_increment(retained, Counter::Survey("Conary"))?;
                }
            }
        }
        if retained.checked_add(withheld) != Some(self.retained_failures)
            || bytes != self.retained_evidence_bytes
            || retained != self.retained_explanations
            || withheld != self.withheld_explanations
            || self.truncated_evidence != (withheld > 0)
        {
            return Err(Error::ConfigError(
                "Conary resolution survey evidence counts are inconsistent".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyCountsV1 {
    pub roots_walked: u64,
    pub resolved_roots: u64,
    pub unresolved_roots: u64,
    pub not_installable_roots: u64,
    pub failed_roots: u64,
    pub error_kinds: Vec<ConaryResolutionSurveyErrorCountV1>,
}

impl ConaryResolutionSurveyCountsV1 {
    fn validate(&self) -> Result<()> {
        let total = self
            .resolved_roots
            .checked_add(self.unresolved_roots)
            .and_then(|value| value.checked_add(self.not_installable_roots))
            .and_then(|value| value.checked_add(self.failed_roots))
            .ok_or_else(|| {
                Error::ConfigError("Conary resolution survey counts exceed u64".to_string())
            })?;
        let histogram = self.error_kinds.iter().try_fold(0_u64, |total, entry| {
            total.checked_add(entry.count).ok_or_else(|| {
                Error::ConfigError("Conary resolution survey histogram exceeds u64".to_string())
            })
        })?;
        if total != self.roots_walked || histogram != self.failed_roots {
            return Err(Error::ConfigError(
                "Conary resolution survey root counts are inconsistent".to_string(),
            ));
        }
        if self
            .error_kinds
            .windows(2)
            .any(|pair| pair[0].kind >= pair[1].kind)
        {
            return Err(Error::ConfigError(
                "Conary resolution survey error histogram is noncanonical".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyErrorCountV1 {
    pub kind: ConaryResolutionSurveyErrorKindV1,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyErrorKindV1 {
    pub error_variant: NativeResolutionSurveyErrorVariantV1,
    pub reason: ConaryResolutionSurveyErrorReasonV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConaryResolutionSurveyErrorReasonV1 {
    ExactRootProjectionFailed,
    ArchitectureAdmissionFailed,
    SolverFailed,
    ResolvedClosureProjectionFailed,
    ResolvedClosureOmittedRoot,
    UnresolvedProjectionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyFailureV1 {
    pub root_package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub release: String,
    pub architecture: Option<String>,
    pub error_kind: ConaryResolutionSurveyErrorKindV1,
    pub error_message: String,
    pub native_explanation: ConaryResolutionSurveyNativeExplanationV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyRootOutcomeV1 {
    pub root_package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub release: String,
    pub architecture: Option<String>,
    pub outcome: NativeResolutionOutcomeV1,
}

impl ConaryResolutionSurveyRootOutcomeV1 {
    fn validate(&self) -> Result<()> {
        validate_identity(&self.name, "Conary resolution survey package name")?;
        validate_identity(&self.version, "Conary resolution survey package version")?;
        validate_identity(&self.release, "Conary resolution survey package release")?;
        super::resolution_contract::NativeResolutionRootV1 {
            root_package_key_sha256: self.root_package_key_sha256.clone(),
            outcome: self.outcome.clone(),
        }
        .validate()
    }
}

impl ConaryResolutionSurveyFailureV1 {
    fn validate(&self) -> Result<()> {
        validate_sha256(
            &self.root_package_key_sha256,
            "Conary resolution survey root package key",
        )?;
        validate_identity(&self.name, "Conary resolution survey package name")?;
        validate_identity(&self.version, "Conary resolution survey package version")?;
        validate_identity(&self.release, "Conary resolution survey package release")?;
        if self.error_message.is_empty() {
            return Err(Error::ConfigError(
                "Conary resolution survey error message is empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConaryResolutionSurveyNativeExplanationV1 {
    Withheld {
        reason: ConaryResolutionSurveyEvidenceWithheldReasonV1,
    },
    ResolvoConflictGraph {
        unresolved_edges: Vec<ConaryResolutionSurveyUnresolvedEdgeV1>,
        conflict_edges: Vec<ConaryResolutionSurveyConflictEdgeV1>,
        excluded_nodes: Vec<ConaryResolutionSurveyExcludedNodeV1>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConaryResolutionSurveyEvidenceWithheldReasonV1 {
    EvidenceBudgetExhausted,
    ConflictGraphUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveySolvableV1 {
    pub package_key_sha256: Option<String>,
    pub name: String,
    pub version: String,
    pub release: Option<String>,
    pub architecture: Option<String>,
    pub repository_name: String,
    pub source_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyVersionSetV1 {
    pub name: String,
    pub constraint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyUnresolvedEdgeV1 {
    pub requiring: ConaryResolutionSurveySolvableV1,
    pub requirement: String,
    pub version_sets: Vec<ConaryResolutionSurveyVersionSetV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConaryResolutionSurveyConflictKindV1 {
    Locked,
    Constrains {
        version_set: ConaryResolutionSurveyVersionSetV1,
    },
    ForbidMultipleInstances,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyConflictEdgeV1 {
    pub from: ConaryResolutionSurveySolvableV1,
    pub to: ConaryResolutionSurveySolvableV1,
    pub conflict: ConaryResolutionSurveyConflictKindV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConaryResolutionSurveyExcludedReasonV1 {
    MissingDependencyAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConaryResolutionSurveyExcludedNodeV1 {
    pub solvable: ConaryResolutionSurveySolvableV1,
    pub reason: ConaryResolutionSurveyExcludedReasonV1,
    pub message: String,
}

pub(super) struct ConaryRootResolutionError {
    pub(super) error: Error,
    pub(super) reason: ConaryResolutionSurveyErrorReasonV1,
    pub(super) explanation: Option<ConaryResolutionSurveyNativeExplanationV1>,
}

pub(super) type ConaryRootResolutionResult =
    std::result::Result<NativeResolutionOutcomeV1, Box<ConaryRootResolutionError>>;

pub(super) struct ConaryResolutionSurveyCollector {
    profile: String,
    profile_revision_sha256: String,
    package_oracle_manifest_sha256: String,
    implementation: NativeParityImplementationV1,
    policy: NativeResolutionPolicyV1,
    counts: ConaryResolutionSurveyCountsV1,
    histogram: BTreeMap<ConaryResolutionSurveyErrorKindV1, u64>,
    outcomes: Vec<ConaryResolutionSurveyRootOutcomeV1>,
    failures: Vec<ConaryResolutionSurveyFailureV1>,
    evidence_byte_limit: u64,
    retained_evidence_bytes: u64,
    retained_explanations: u64,
    withheld_explanations: u64,
    evidence_budget_exhausted: bool,
}

impl ConaryResolutionSurveyCollector {
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
            counts: ConaryResolutionSurveyCountsV1::default(),
            histogram: BTreeMap::new(),
            outcomes: Vec::new(),
            failures: Vec::new(),
            evidence_byte_limit: CONARY_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT,
            retained_evidence_bytes: 0,
            retained_explanations: 0,
            withheld_explanations: 0,
            evidence_budget_exhausted: false,
        })
    }

    pub(super) fn explanation_byte_limit(&self) -> u64 {
        if self.evidence_budget_exhausted
            || self.failures.len() >= CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT
        {
            0
        } else {
            self.evidence_byte_limit
                .saturating_sub(self.retained_evidence_bytes)
        }
    }

    pub(super) fn root(
        &mut self,
        root: &NativeParityPackageV1,
        result: ConaryRootResolutionResult,
    ) -> Result<()> {
        self.counts.roots_walked =
            checked_increment(self.counts.roots_walked, Counter::Survey("Conary"))?;
        match result {
            Ok(outcome) => self.outcome(root, outcome)?,
            Err(failure) => self.failure(root, *failure)?,
        }
        Ok(())
    }

    fn outcome(
        &mut self,
        root: &NativeParityPackageV1,
        outcome: NativeResolutionOutcomeV1,
    ) -> Result<()> {
        match &outcome {
            NativeResolutionOutcomeV1::Resolved { .. } => {
                self.counts.resolved_roots =
                    checked_increment(self.counts.resolved_roots, Counter::Survey("Conary"))?;
            }
            NativeResolutionOutcomeV1::Unresolved { .. } => {
                self.counts.unresolved_roots =
                    checked_increment(self.counts.unresolved_roots, Counter::Survey("Conary"))?;
            }
            NativeResolutionOutcomeV1::NotInstallable { .. } => {
                self.counts.not_installable_roots = checked_increment(
                    self.counts.not_installable_roots,
                    Counter::Survey("Conary"),
                )?;
            }
        }
        self.outcomes.push(ConaryResolutionSurveyRootOutcomeV1 {
            root_package_key_sha256: root.package_key_sha256.clone(),
            name: root.name.clone(),
            version: root.version.clone(),
            release: root.package_release.clone(),
            architecture: root.architecture.clone(),
            outcome,
        });
        Ok(())
    }

    fn failure(
        &mut self,
        root: &NativeParityPackageV1,
        failure: ConaryRootResolutionError,
    ) -> Result<()> {
        self.counts.failed_roots =
            checked_increment(self.counts.failed_roots, Counter::Survey("Conary"))?;
        let kind = ConaryResolutionSurveyErrorKindV1 {
            error_variant: NativeResolutionSurveyErrorVariantV1::from_error(&failure.error),
            reason: failure.reason,
        };
        let count = self.histogram.entry(kind.clone()).or_default();
        *count = checked_increment(*count, Counter::Survey("Conary"))?;
        if self.failures.len() < CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT {
            let explanation = self.retain_explanation(failure.explanation)?;
            self.failures.push(ConaryResolutionSurveyFailureV1 {
                root_package_key_sha256: root.package_key_sha256.clone(),
                name: root.name.clone(),
                version: root.version.clone(),
                release: root.package_release.clone(),
                architecture: root.architecture.clone(),
                error_kind: kind,
                error_message: failure.error.to_string(),
                native_explanation: explanation,
            });
        }
        Ok(())
    }

    fn retain_explanation(
        &mut self,
        explanation: Option<ConaryResolutionSurveyNativeExplanationV1>,
    ) -> Result<ConaryResolutionSurveyNativeExplanationV1> {
        let explanation =
            explanation.unwrap_or(ConaryResolutionSurveyNativeExplanationV1::Withheld {
                reason: ConaryResolutionSurveyEvidenceWithheldReasonV1::ConflictGraphUnavailable,
            });
        if !self.evidence_budget_exhausted {
            let remaining = self
                .evidence_byte_limit
                .saturating_sub(self.retained_evidence_bytes);
            if let Some(size) = canonical_value_size_with_limit(&explanation, remaining)? {
                self.retained_evidence_bytes = self
                    .retained_evidence_bytes
                    .checked_add(size)
                    .ok_or_else(|| {
                        Error::ConfigError("Conary survey evidence exceeds u64".to_string())
                    })?;
                self.retained_explanations =
                    checked_increment(self.retained_explanations, Counter::Survey("Conary"))?;
                return Ok(explanation);
            }
            self.evidence_budget_exhausted = true;
        }
        self.withheld_explanations =
            checked_increment(self.withheld_explanations, Counter::Survey("Conary"))?;
        Ok(ConaryResolutionSurveyNativeExplanationV1::Withheld {
            reason: ConaryResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
        })
    }

    pub(super) fn finish(mut self) -> Result<ConaryResolutionSurveyV1> {
        self.counts.error_kinds = self
            .histogram
            .into_iter()
            .map(|(kind, count)| ConaryResolutionSurveyErrorCountV1 { kind, count })
            .collect();
        let retained_failures = self.failures.len() as u64;
        let total_failures = self.counts.failed_roots;
        let survey = ConaryResolutionSurveyV1 {
            schema_version: CONARY_RESOLUTION_SURVEY_SCHEMA_V2,
            profile: self.profile,
            profile_revision_sha256: self.profile_revision_sha256,
            package_oracle_manifest_sha256: self.package_oracle_manifest_sha256,
            implementation: self.implementation,
            target_architecture: self.policy.architecture.clone(),
            policy: self.policy,
            counts: self.counts,
            outcomes: self.outcomes,
            failure_record_limit: CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT as u64,
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

pub fn write_conary_resolution_survey(
    path: &Path,
    survey: &ConaryResolutionSurveyV1,
) -> Result<()> {
    survey.validate()?;
    write_private_canonical_json(path, survey, "Conary resolution survey")
}

pub(super) fn conflict_graph_explanation(
    graph: &ConflictGraph<SolvableId>,
    provider: &ConaryProvider<'_>,
    package_key: impl Fn(i64) -> Option<String>,
) -> ConaryResolutionSurveyNativeExplanationV1 {
    let mut unresolved_edges = Vec::new();
    if let Some(unresolved) = graph.unresolved_node {
        for edge in graph.graph.edges_directed(unresolved, Direction::Incoming) {
            if let (ConflictEdge::Requires(requirement), ConflictNode::Solvable(requiring)) =
                (*edge.weight(), graph.graph[edge.source()])
            {
                unresolved_edges.push(ConaryResolutionSurveyUnresolvedEdgeV1 {
                    requiring: solvable(provider.get_solvable(requiring), &package_key),
                    requirement: requirement.display(provider).to_string(),
                    version_sets: version_sets(requirement, provider),
                });
            }
        }
    }
    let mut conflict_edges = Vec::new();
    let mut excluded_nodes = Vec::new();
    for edge in graph.graph.edge_references() {
        let ConflictEdge::Conflict(cause) = *edge.weight() else {
            continue;
        };
        match cause {
            ConflictCause::Excluded => {
                let (ConflictNode::Solvable(source), ConflictNode::Excluded(reason)) =
                    (graph.graph[edge.source()], graph.graph[edge.target()])
                else {
                    continue;
                };
                if provider.is_missing_dependency_authority_reason(reason) {
                    excluded_nodes.push(ConaryResolutionSurveyExcludedNodeV1 {
                        solvable: solvable(provider.get_solvable(source), &package_key),
                        reason: ConaryResolutionSurveyExcludedReasonV1::MissingDependencyAuthority,
                        message: provider.display_string(reason).to_string(),
                    });
                }
            }
            ConflictCause::Locked(locked) => {
                if let ConflictNode::Solvable(target) = graph.graph[edge.target()] {
                    conflict_edges.push(ConaryResolutionSurveyConflictEdgeV1 {
                        from: solvable(provider.get_solvable(locked), &package_key),
                        to: solvable(provider.get_solvable(target), &package_key),
                        conflict: ConaryResolutionSurveyConflictKindV1::Locked,
                    });
                }
            }
            ConflictCause::Constrains(version_set) => {
                if let (ConflictNode::Solvable(source), ConflictNode::Solvable(target)) =
                    (graph.graph[edge.source()], graph.graph[edge.target()])
                {
                    conflict_edges.push(ConaryResolutionSurveyConflictEdgeV1 {
                        from: solvable(provider.get_solvable(source), &package_key),
                        to: solvable(provider.get_solvable(target), &package_key),
                        conflict: ConaryResolutionSurveyConflictKindV1::Constrains {
                            version_set: version_set_value(version_set, provider),
                        },
                    });
                }
            }
            ConflictCause::ForbidMultipleInstances => {
                if let (ConflictNode::Solvable(source), ConflictNode::Solvable(target)) =
                    (graph.graph[edge.source()], graph.graph[edge.target()])
                {
                    conflict_edges.push(ConaryResolutionSurveyConflictEdgeV1 {
                        from: solvable(provider.get_solvable(source), &package_key),
                        to: solvable(provider.get_solvable(target), &package_key),
                        conflict: ConaryResolutionSurveyConflictKindV1::ForbidMultipleInstances,
                    });
                }
            }
        }
    }
    unresolved_edges.sort();
    unresolved_edges.dedup();
    conflict_edges.sort();
    conflict_edges.dedup();
    excluded_nodes.sort();
    excluded_nodes.dedup();
    ConaryResolutionSurveyNativeExplanationV1::ResolvoConflictGraph {
        unresolved_edges,
        conflict_edges,
        excluded_nodes,
    }
}

fn version_sets(
    requirement: Requirement,
    provider: &ConaryProvider<'_>,
) -> Vec<ConaryResolutionSurveyVersionSetV1> {
    match requirement {
        Requirement::Single(version_set) => vec![version_set_value(version_set, provider)],
        Requirement::Union(union) => provider
            .version_sets_in_union(union)
            .map(|version_set| version_set_value(version_set, provider))
            .collect(),
    }
}

fn version_set_value(
    version_set: VersionSetId,
    provider: &ConaryProvider<'_>,
) -> ConaryResolutionSurveyVersionSetV1 {
    ConaryResolutionSurveyVersionSetV1 {
        name: provider
            .display_name(provider.version_set_name(version_set))
            .to_string(),
        constraint: provider.display_version_set(version_set).to_string(),
    }
}

fn solvable(
    package: &PackageIdentity,
    package_key: &impl Fn(i64) -> Option<String>,
) -> ConaryResolutionSurveySolvableV1 {
    ConaryResolutionSurveySolvableV1 {
        package_key_sha256: package.repo_package_id.and_then(package_key),
        name: package.name.clone(),
        version: package.version.clone(),
        release: package.package_release.clone(),
        architecture: package.architecture.clone(),
        repository_name: package.repository_name.clone(),
        source_profile: package.repository_profile.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::catalog::parity::{
        NativeParityEcosystemV1, NativeResolutionArchitectureAdmissionV1,
        NativeResolutionInstalledStateV1, NativeResolutionProviderPolicyV1,
        NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1,
    };

    fn collector(evidence_byte_limit: u64) -> ConaryResolutionSurveyCollector {
        ConaryResolutionSurveyCollector {
            profile: "fedora-44".to_string(),
            profile_revision_sha256: "a".repeat(64),
            package_oracle_manifest_sha256: "b".repeat(64),
            implementation: NativeParityImplementationV1 {
                ecosystem: NativeParityEcosystemV1::Rpm,
                name: "conary-sat".to_string(),
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
            counts: ConaryResolutionSurveyCountsV1::default(),
            histogram: BTreeMap::new(),
            outcomes: Vec::new(),
            failures: Vec::new(),
            evidence_byte_limit,
            retained_evidence_bytes: 0,
            retained_explanations: 0,
            withheld_explanations: 0,
            evidence_budget_exhausted: false,
        }
    }

    fn root(index: usize) -> NativeParityPackageV1 {
        serde_json::from_value(serde_json::json!({
            "package_key_sha256": format!("{index:064x}"),
            "member_ordinal": 0,
            "source_identity": "fedora",
            "repository_identity": "fedora-base",
            "source_snapshot_sha256": "c".repeat(64),
            "source_profile": "fedora-44",
            "name": format!("package-{index:05}"),
            "version": "1",
            "package_release": "1",
            "architecture": "x86_64",
            "debian_multi_arch": null,
            "checksum": format!("sha256:{}", "d".repeat(64)),
            "size": 1,
            "download_url": "https://example.test/package.rpm",
            "version_scheme": "rpm",
            "provides": [],
            "requirement_groups": []
        }))
        .unwrap()
    }

    fn failure(
        explanation: ConaryResolutionSurveyNativeExplanationV1,
    ) -> Box<ConaryRootResolutionError> {
        Box::new(ConaryRootResolutionError {
            error: Error::ConflictError("typed candidate failure".to_string()),
            reason: ConaryResolutionSurveyErrorReasonV1::SolverFailed,
            explanation: Some(explanation),
        })
    }

    fn empty_graph() -> ConaryResolutionSurveyNativeExplanationV1 {
        ConaryResolutionSurveyNativeExplanationV1::ResolvoConflictGraph {
            unresolved_edges: Vec::new(),
            conflict_edges: Vec::new(),
            excluded_nodes: Vec::new(),
        }
    }

    #[test]
    fn collector_caps_records_but_preserves_uncapped_failure_truth() {
        let mut collector = collector(CONARY_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT);
        for index in 0..=CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT {
            collector
                .root(&root(index), Err(failure(empty_graph())))
                .unwrap();
        }

        let survey = collector.finish().unwrap();
        assert_eq!(
            survey.total_failures,
            CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT as u64 + 1
        );
        assert_eq!(
            survey.retained_failures,
            CONARY_RESOLUTION_SURVEY_FAILURE_LIMIT as u64
        );
        assert!(survey.truncated);
        assert_eq!(survey.counts.error_kinds[0].count, survey.total_failures);

        let mut obsolete = survey.clone();
        obsolete.schema_version = 1;
        assert!(obsolete.validate().is_err());
    }

    #[test]
    fn hidden_conflict_budget_is_a_counted_failure_never_an_outcome() {
        let mut collector = collector(CONARY_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT);
        collector
            .root(
                &root(0),
                Err(Box::new(ConaryRootResolutionError {
                    error: Error::HiddenConflictProbeBudgetExceeded {
                        root: "package-00000".to_string(),
                        resolves: 64,
                        elapsed: std::time::Duration::from_millis(1234),
                    },
                    reason: ConaryResolutionSurveyErrorReasonV1::SolverFailed,
                    explanation: None,
                })),
            )
            .unwrap();
        let survey = collector.finish().unwrap();
        assert_eq!(survey.counts.failed_roots, 1);
        assert_eq!(survey.total_failures, 1);
        assert!(survey.outcomes.is_empty());
        assert_eq!(
            survey.counts.error_kinds[0].kind.error_variant,
            NativeResolutionSurveyErrorVariantV1::Budget
        );
        assert_eq!(survey.counts.error_kinds[0].count, 1);
        assert!(
            survey.failures[0]
                .error_message
                .contains("64 re-solves; elapsed=1.234s")
        );
        let bytes = serde_json::to_vec(&survey).unwrap();
        let reopened: ConaryResolutionSurveyV1 = serde_json::from_slice(&bytes).unwrap();
        reopened.validate().unwrap();
        assert_eq!(reopened, survey);
    }

    #[test]
    fn collector_withholds_every_later_explanation_after_byte_budget() {
        let explanation = empty_graph();
        let explanation_bytes = canonical_value_size_with_limit(&explanation, u64::MAX)
            .unwrap()
            .unwrap();
        let mut collector = collector(explanation_bytes);
        for index in 0..3 {
            collector
                .root(&root(index), Err(failure(explanation.clone())))
                .unwrap();
        }

        let survey = collector.finish().unwrap();
        assert_eq!(survey.retained_evidence_bytes, explanation_bytes);
        assert_eq!(survey.retained_explanations, 1);
        assert_eq!(survey.withheld_explanations, 2);
        assert!(survey.truncated_evidence);
        assert!(survey.failures[1..].iter().all(|failure| matches!(
            failure.native_explanation,
            ConaryResolutionSurveyNativeExplanationV1::Withheld {
                reason: ConaryResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted
            }
        )));
    }
}
