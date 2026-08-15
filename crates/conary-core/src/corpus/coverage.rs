// conary-core/src/corpus/coverage.rs

//! Typed semantic coverage for the just-works corpus gate.

use super::{CorpusCaseResult, SourceArtifactRole};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

/// One exact semantic property required by the W7 just-works corpus.
///
/// The serialized spellings are release-gate authority. Coverage must never be
/// inferred from fixture names, test identifiers, or diagnostic wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusSemantic {
    IdentityExactVersion,
    IdentityEpochRelease,
    IdentityArchitectureIndependent,
    IdentityNativeArchitecture,
    PayloadFiles,
    PayloadDirectories,
    PayloadSymlinks,
    PayloadHardlinks,
    PayloadSparseFiles,
    PayloadLargeFiles,
    MetadataXattrs,
    MetadataCapabilities,
    MetadataOwnership,
    MetadataTimestamps,
    RelationsVersionedDependency,
    RelationsVirtualProvide,
    RelationsSameNameCompatibilityProvide,
    RelationsConflict,
    RelationsReplacement,
    ConfigurationMatchedConfig,
    ConfigurationUnmatchedDeclaration,
    ConfigurationLocalModification,
    ConfigurationDeletion,
    ConfigurationUpgrade,
    ConfigurationRemoval,
    LifecycleNone,
    LifecycleShell,
    LifecycleNonShellInterpreter,
    LifecycleTrigger,
    LifecycleTransactionHook,
    TrustMultipleValidSigningKeys,
    TrustSigningSubkeys,
    TrustExpiredKey,
    TrustRevokedKey,
    TrustUnknownKey,
    RuntimeServiceActivation,
    RuntimeTargetHelper,
    RuntimeDeferredGenerationWork,
    FailureInterruptedDownload,
    FailureConversion,
    FailurePayloadMutation,
    FailureLifecycle,
    FailurePublication,
    FailureActivation,
}

impl CorpusSemantic {
    /// Complete W7 semantic-property vocabulary in stable report order.
    pub const ALL: [Self; 44] = [
        Self::IdentityExactVersion,
        Self::IdentityEpochRelease,
        Self::IdentityArchitectureIndependent,
        Self::IdentityNativeArchitecture,
        Self::PayloadFiles,
        Self::PayloadDirectories,
        Self::PayloadSymlinks,
        Self::PayloadHardlinks,
        Self::PayloadSparseFiles,
        Self::PayloadLargeFiles,
        Self::MetadataXattrs,
        Self::MetadataCapabilities,
        Self::MetadataOwnership,
        Self::MetadataTimestamps,
        Self::RelationsVersionedDependency,
        Self::RelationsVirtualProvide,
        Self::RelationsSameNameCompatibilityProvide,
        Self::RelationsConflict,
        Self::RelationsReplacement,
        Self::ConfigurationMatchedConfig,
        Self::ConfigurationUnmatchedDeclaration,
        Self::ConfigurationLocalModification,
        Self::ConfigurationDeletion,
        Self::ConfigurationUpgrade,
        Self::ConfigurationRemoval,
        Self::LifecycleNone,
        Self::LifecycleShell,
        Self::LifecycleNonShellInterpreter,
        Self::LifecycleTrigger,
        Self::LifecycleTransactionHook,
        Self::TrustMultipleValidSigningKeys,
        Self::TrustSigningSubkeys,
        Self::TrustExpiredKey,
        Self::TrustRevokedKey,
        Self::TrustUnknownKey,
        Self::RuntimeServiceActivation,
        Self::RuntimeTargetHelper,
        Self::RuntimeDeferredGenerationWork,
        Self::FailureInterruptedDownload,
        Self::FailureConversion,
        Self::FailurePayloadMutation,
        Self::FailureLifecycle,
        Self::FailurePublication,
        Self::FailureActivation,
    ];
}

/// A semantic claim and the exact source-artifact roles that must support it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusCoverageClaim {
    pub semantic: CorpusSemantic,
    pub artifact_roles: Vec<SourceArtifactRole>,
}

/// Coverage projected from completed, digest-attributable corpus cases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusCoverageAggregate {
    pub required: Vec<CorpusSemantic>,
    pub covered: Vec<CorpusSemantic>,
    pub missing: Vec<CorpusSemantic>,
    /// Claims that did not resolve every declared role to one exact artifact.
    pub unattributed_claims: usize,
}

impl CorpusCoverageAggregate {
    pub fn all_covered(&self) -> bool {
        self.missing.is_empty() && self.covered == self.required && self.unattributed_claims == 0
    }
}

/// Aggregate coverage without consulting free-form diagnostics.
pub fn aggregate_coverage(
    required: &[CorpusSemantic],
    cases: &[CorpusCaseResult],
) -> CorpusCoverageAggregate {
    let required = required.iter().copied().collect::<BTreeSet<_>>();
    let mut covered = BTreeSet::new();
    let mut unattributed_claims = 0;

    for case in cases.iter().filter(|case| case.completed()) {
        let mut seen = HashSet::new();
        for claim in &case.coverage_claims {
            if !seen.insert(claim.semantic) || !claim_is_attributable(case, claim) {
                unattributed_claims += 1;
                continue;
            }
            covered.insert(claim.semantic);
        }
    }

    let missing = required.difference(&covered).copied().collect();
    CorpusCoverageAggregate {
        required: required.into_iter().collect(),
        covered: covered.into_iter().collect(),
        missing,
        unattributed_claims,
    }
}

fn claim_is_attributable(case: &CorpusCaseResult, claim: &CorpusCoverageClaim) -> bool {
    if claim.artifact_roles.is_empty()
        || !claim
            .artifact_roles
            .windows(2)
            .all(|roles| roles[0] < roles[1])
    {
        return false;
    }

    claim.artifact_roles.iter().all(|role| {
        let matching = case
            .source_artifacts
            .iter()
            .filter(|artifact| artifact.role == *role)
            .collect::<Vec<_>>();
        matching.len() == 1
            && !matching[0].name.trim().is_empty()
            && !matching[0].version.trim().is_empty()
            && matching[0].digest.len() == 64
            && matching[0]
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{
        ConversionStage, SourceArtifactDigestSource, SourceArtifactIdentity, SourceProfileIdentity,
        StageResult, TargetCapabilitySnapshot,
    };

    fn case(completed: bool, digest: &str) -> CorpusCaseResult {
        CorpusCaseResult::from_stages(
            "rpm-fedora",
            SourceProfileIdentity {
                profile: "fedora-44".into(),
                format: "rpm".into(),
            },
            vec![SourceArtifactIdentity {
                role: SourceArtifactRole::InstallRequest,
                digest_source: SourceArtifactDigestSource::FixtureBuildManifest,
                name: "fixture".into(),
                version: "1-1".into(),
                architecture: Some("x86_64".into()),
                digest: digest.into(),
            }],
            TargetCapabilitySnapshot {
                profile: "fedora44".into(),
                architecture: "x86_64".into(),
                init_system: "systemd".into(),
                capabilities: vec!["native_lifecycle".into()],
            },
            vec![CorpusCoverageClaim {
                semantic: CorpusSemantic::IdentityExactVersion,
                artifact_roles: vec![SourceArtifactRole::InstallRequest],
            }],
            if completed {
                vec![StageResult::passed(ConversionStage::Installation)]
            } else {
                vec![StageResult::not_reached(ConversionStage::Installation)]
            },
        )
    }

    #[test]
    fn only_completed_attributable_cases_cover_requirements() {
        let required = [
            CorpusSemantic::IdentityExactVersion,
            CorpusSemantic::PayloadHardlinks,
        ];
        let digest = "a".repeat(64);
        let aggregate = aggregate_coverage(&required, &[case(true, &digest)]);

        assert_eq!(
            aggregate.covered,
            vec![CorpusSemantic::IdentityExactVersion]
        );
        assert_eq!(aggregate.missing, vec![CorpusSemantic::PayloadHardlinks]);
        assert_eq!(aggregate.unattributed_claims, 0);
        assert!(!aggregate.all_covered());

        let incomplete = aggregate_coverage(&required[..1], &[case(false, &digest)]);
        assert!(incomplete.covered.is_empty());
        assert_eq!(
            incomplete.missing,
            vec![CorpusSemantic::IdentityExactVersion]
        );
    }

    #[test]
    fn malformed_or_unbound_artifacts_do_not_satisfy_coverage() {
        let required = [CorpusSemantic::IdentityExactVersion];
        let aggregate = aggregate_coverage(&required, &[case(true, "short")]);
        assert_eq!(aggregate.unattributed_claims, 1);
        assert_eq!(aggregate.missing, required);

        let mut wrong_role = case(true, &"b".repeat(64));
        wrong_role.coverage_claims[0].artifact_roles = vec![SourceArtifactRole::UpdateRequest];
        let aggregate = aggregate_coverage(&required, &[wrong_role]);
        assert_eq!(aggregate.unattributed_claims, 1);
        assert_eq!(aggregate.missing, required);
    }

    #[test]
    fn undeclared_extra_coverage_cannot_pass_the_gate() {
        let required = [CorpusSemantic::IdentityExactVersion];
        let mut case = case(true, &"c".repeat(64));
        case.coverage_claims.push(CorpusCoverageClaim {
            semantic: CorpusSemantic::PayloadFiles,
            artifact_roles: vec![SourceArtifactRole::InstallRequest],
        });

        let aggregate = aggregate_coverage(&required, &[case]);
        assert!(aggregate.missing.is_empty());
        assert_eq!(aggregate.covered.len(), 2);
        assert!(!aggregate.all_covered());
    }

    #[test]
    fn vocabulary_round_trips_all_stable_spellings() {
        let mut spellings = Vec::new();
        for semantic in CorpusSemantic::ALL {
            let json = serde_json::to_string(&semantic).unwrap();
            let restored: CorpusSemantic = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, semantic);
            spellings.push(json);
        }
        spellings.sort();
        spellings.dedup();
        assert_eq!(spellings.len(), 44);
    }
}
