// crates/conary-core/src/repository/catalog/parity/resolution_survey/collector.rs

//! Ordered collection of strict outcomes and bounded survey diagnostics.

use std::collections::BTreeMap;

use super::{
    NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT,
    NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT, NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT,
    NATIVE_RESOLUTION_SURVEY_SCHEMA_V3, NativeResolutionSurveyCountsV1,
    NativeResolutionSurveyErrorCountV1, NativeResolutionSurveyErrorKindV1,
    NativeResolutionSurveyErrorVariantV1, NativeResolutionSurveyEvidenceWithheldReasonV1,
    NativeResolutionSurveyFailureV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyRootOutcomeV1, NativeResolutionSurveyV1, checked_increment,
};
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::contract::{
    NativeParityImplementationV1, NativeParityOracleV1, NativeParityPackageV1,
};
use crate::repository::catalog::parity::resolution_contract::{
    NativeResolutionNotInstallableReasonV1, NativeResolutionOutcomeV1, NativeResolutionPolicyV1,
    NativeResolutionRootV1,
};
use crate::repository::catalog::parity::resolution_io::NativeResolutionOracleWriter;
use crate::repository::catalog::parity::resolution_root::{
    NativeRootResolutionError, NativeRootResolutionResult, NativeRootResolutionSuccess,
};
use crate::repository::catalog::parity::survey_support::canonical_value_size_with_limit;

#[allow(dead_code)]
pub(in crate::repository::catalog::parity) enum RootOutcomeSink<'a> {
    Strict(&'a mut NativeResolutionOracleWriter),
    Survey(&'a mut NativeResolutionSurveyCollector),
}

#[allow(dead_code)]
impl RootOutcomeSink<'_> {
    pub(in crate::repository::catalog::parity) fn explanation_byte_limit(&self) -> u64 {
        match self {
            Self::Strict(_) => 0,
            Self::Survey(collector) => collector.remaining_evidence_bytes(),
        }
    }

    pub(in crate::repository::catalog::parity) fn root(
        &mut self,
        root: &NativeParityPackageV1,
        result: NativeRootResolutionResult,
    ) -> Result<()> {
        match (self, result) {
            (Self::Strict(writer), Ok(success)) => writer.root(&NativeResolutionRootV1 {
                root_package_key_sha256: root.package_key_sha256.clone(),
                outcome: success.outcome,
            }),
            (Self::Strict(_), Err(failure)) => Err(failure.error),
            (Self::Survey(collector), Ok(success)) => {
                collector.success(root, success)?;
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
pub(in crate::repository::catalog::parity) struct NativeResolutionSurveyCollector {
    pub(super) profile: String,
    pub(super) profile_revision_sha256: String,
    pub(super) package_oracle_manifest_sha256: String,
    pub(super) implementation: NativeParityImplementationV1,
    pub(super) policy: NativeResolutionPolicyV1,
    pub(super) counts: NativeResolutionSurveyCountsV1,
    pub(super) histogram: BTreeMap<NativeResolutionSurveyErrorKindV1, u64>,
    pub(super) total_diagnostic_outcomes: u64,
    pub(super) diagnostic_outcomes: Vec<NativeResolutionSurveyRootOutcomeV1>,
    pub(super) failures: Vec<NativeResolutionSurveyFailureV1>,
    pub(super) evidence_byte_limit: u64,
    pub(super) retained_evidence_bytes: u64,
    pub(super) retained_explanations: u64,
    pub(super) withheld_explanations: u64,
    pub(super) evidence_budget_exhausted: bool,
}

#[allow(dead_code)]
impl NativeResolutionSurveyCollector {
    pub(in crate::repository::catalog::parity) fn new(
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
            total_diagnostic_outcomes: 0,
            diagnostic_outcomes: Vec::new(),
            failures: Vec::new(),
            evidence_byte_limit: NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT,
            retained_evidence_bytes: 0,
            retained_explanations: 0,
            withheld_explanations: 0,
            evidence_budget_exhausted: false,
        })
    }

    pub(super) fn success(
        &mut self,
        root: &NativeParityPackageV1,
        success: NativeRootResolutionSuccess,
    ) -> Result<()> {
        let NativeRootResolutionSuccess {
            outcome,
            explanation,
        } = success;
        self.counts.roots_walked = checked_increment(self.counts.roots_walked)?;
        match &outcome {
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
        let is_conflicting_closure = matches!(
            outcome,
            NativeResolutionOutcomeV1::NotInstallable {
                reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure
            }
        );
        match (is_conflicting_closure, explanation) {
            (true, Some(explanation)) => {
                self.total_diagnostic_outcomes = checked_increment(self.total_diagnostic_outcomes)?;
                if self.diagnostic_outcomes.len()
                    < NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT
                {
                    let native_explanation = self.retain_explanation(explanation)?;
                    self.diagnostic_outcomes
                        .push(NativeResolutionSurveyRootOutcomeV1 {
                            root_package_key_sha256: root.package_key_sha256.clone(),
                            name: root.name.clone(),
                            version: root.version.clone(),
                            release: root.package_release.clone(),
                            architecture: root.architecture.clone(),
                            outcome,
                            native_explanation,
                        });
                }
            }
            (true, None) => {
                return Err(Error::InternalError(
                    "conflicting native resolution outcome has no survey explanation".to_string(),
                ));
            }
            (false, Some(_)) => {
                return Err(Error::InternalError(
                    "non-conflict native resolution outcome carries survey explanation".to_string(),
                ));
            }
            (false, None) => {}
        }
        Ok(())
    }

    pub(in crate::repository::catalog::parity) fn remaining_evidence_bytes(&self) -> u64 {
        if self.evidence_budget_exhausted {
            0
        } else {
            self.evidence_byte_limit
                .saturating_sub(self.retained_evidence_bytes)
        }
    }

    pub(super) fn failure(
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

    pub(in crate::repository::catalog::parity) fn finish(
        mut self,
    ) -> Result<NativeResolutionSurveyV1> {
        self.counts.error_kinds = self
            .histogram
            .into_iter()
            .map(|(kind, count)| NativeResolutionSurveyErrorCountV1 { kind, count })
            .collect();
        let retained_failures = self.failures.len() as u64;
        let total_failures = self.counts.failed_roots;
        let retained_diagnostic_outcomes = self.diagnostic_outcomes.len() as u64;
        let survey = NativeResolutionSurveyV1 {
            schema_version: NATIVE_RESOLUTION_SURVEY_SCHEMA_V3,
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
            diagnostic_outcome_record_limit: NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT
                as u64,
            total_diagnostic_outcomes: self.total_diagnostic_outcomes,
            retained_diagnostic_outcomes,
            diagnostic_outcomes_truncated: retained_diagnostic_outcomes
                < self.total_diagnostic_outcomes,
            diagnostic_outcomes: self.diagnostic_outcomes,
            failures: self.failures,
        };
        survey.validate()?;
        Ok(survey)
    }
}
