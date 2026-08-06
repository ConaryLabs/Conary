// conary-test/src/engine/corpus.rs

//! Projection of typed runtime evidence into just-works corpus results.

use crate::config::corpus::CorpusCaseDef;
use crate::container::backend::{ContainerBackend, ContainerId};
use crate::engine::suite::TestStatus;
use anyhow::{Result, bail};
use conary_core::corpus::{
    ConversionFailure, ConversionStage, CorpusCaseResult, CorpusOutcome, SourceArtifactIdentity,
    SourceProfileIdentity, StageResult, TargetCapabilitySnapshot,
};
use serde::Deserialize;
use std::collections::HashSet;

const EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Versioned evidence written by the in-container lifecycle fixture.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusRuntimeEvidence {
    schema_version: u32,
    source_profile: String,
    source_format: String,
    source_artifacts: Vec<SourceArtifactIdentity>,
    completed_stages: Vec<ConversionStage>,
    active_stage: Option<ConversionStage>,
}

/// Concrete owner for malformed, absent, or contradictory evidence.
struct CorpusEvidenceFailure;

/// Concrete owner for a test command or assertion failure.
struct CorpusTestExecutionFailure;

pub async fn capture_case(
    definition: &CorpusCaseDef,
    test_id: &str,
    target_profile: &str,
    status: TestStatus,
    diagnostic: Option<&str>,
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
) -> CorpusCaseResult {
    if matches!(status, TestStatus::Skipped | TestStatus::Cancelled) {
        return skipped_case(
            definition,
            test_id,
            target_profile,
            diagnostic.unwrap_or("corpus case did not run"),
        );
    }

    let evidence = match backend
        .copy_from(container_id, &definition.evidence_path)
        .await
        .and_then(|bytes| Ok(serde_json::from_slice::<CorpusRuntimeEvidence>(&bytes)?))
    {
        Ok(evidence) => evidence,
        Err(error) => {
            return evidence_failure_case(
                definition,
                test_id,
                target_profile,
                Vec::new(),
                format!("failed to read typed corpus evidence: {error}"),
            );
        }
    };

    if let Err(error) = validate_evidence(definition, status, &evidence) {
        return evidence_failure_case(
            definition,
            test_id,
            target_profile,
            evidence.source_artifacts,
            error.to_string(),
        );
    }

    let mut stages = evidence
        .completed_stages
        .iter()
        .copied()
        .map(StageResult::passed)
        .collect::<Vec<_>>();

    if status == TestStatus::Failed {
        let active = evidence
            .active_stage
            .expect("validated failed evidence has an active stage");
        stages.push(StageResult::failed(
            active,
            ConversionFailure::internal_for::<CorpusTestExecutionFailure>(
                diagnostic.unwrap_or("corpus test failed"),
            ),
        ));
        stages.extend(
            definition.stages[stages.len()..]
                .iter()
                .copied()
                .map(StageResult::not_reached),
        );
    }

    CorpusCaseResult::from_stages(
        test_id,
        source_profile(definition),
        evidence.source_artifacts,
        target_snapshot(definition, target_profile),
        stages,
    )
}

fn validate_evidence(
    definition: &CorpusCaseDef,
    status: TestStatus,
    evidence: &CorpusRuntimeEvidence,
) -> Result<()> {
    if evidence.schema_version != EVIDENCE_SCHEMA_VERSION {
        bail!(
            "unsupported corpus evidence schema version {}",
            evidence.schema_version
        );
    }
    if evidence.source_profile != definition.source_profile
        || evidence.source_format != definition.source_format.as_str()
    {
        bail!("runtime source profile or format contradicts the corpus declaration");
    }
    if evidence.source_artifacts.is_empty() {
        bail!("corpus evidence carries no source artifacts");
    }

    let mut request_roles = HashSet::new();
    let mut artifact_identities = HashSet::new();
    for artifact in &evidence.source_artifacts {
        if !artifact_identities.insert((
            artifact.role,
            artifact.name.as_str(),
            artifact.version.as_str(),
            artifact.architecture.as_deref(),
            artifact.digest.as_str(),
        )) {
            bail!("corpus evidence repeats an exact source artifact identity");
        }
        if matches!(
            artifact.role,
            conary_core::corpus::SourceArtifactRole::InstallRequest
                | conary_core::corpus::SourceArtifactRole::UpdateRequest
        ) && !request_roles.insert(artifact.role)
        {
            bail!(
                "corpus evidence repeats requested artifact role {:?}",
                artifact.role
            );
        }
        if artifact.name.trim().is_empty()
            || artifact.version.trim().is_empty()
            || artifact.digest.len() != 64
            || !artifact
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!(
                "corpus source artifact {:?} lacks exact name, version, or lowercase SHA-256 identity",
                artifact.role
            );
        }
        if artifact.digest_source != definition.digest_source {
            bail!("runtime artifact digest source contradicts the corpus declaration");
        }
    }
    if definition.stages.contains(&ConversionStage::Installation)
        && !request_roles.contains(&conary_core::corpus::SourceArtifactRole::InstallRequest)
    {
        bail!("installation journey lacks its requested source artifact");
    }
    if definition.stages.contains(&ConversionStage::Update)
        && !request_roles.contains(&conary_core::corpus::SourceArtifactRole::UpdateRequest)
    {
        bail!("update journey lacks its requested source artifact");
    }

    if !definition.stages.starts_with(&evidence.completed_stages) {
        bail!("completed corpus stages are not a prefix of the declared journey");
    }

    match status {
        TestStatus::Passed => {
            if evidence.completed_stages != definition.stages || evidence.active_stage.is_some() {
                bail!("passed corpus test did not complete its exact declared journey");
            }
        }
        TestStatus::Failed => {
            let expected_active = definition
                .stages
                .get(evidence.completed_stages.len())
                .copied();
            if evidence.active_stage != expected_active {
                bail!("failed corpus test does not identify the next declared active stage");
            }
        }
        TestStatus::Skipped | TestStatus::Cancelled => unreachable!("handled before evidence read"),
    }
    Ok(())
}

fn evidence_failure_case(
    definition: &CorpusCaseDef,
    test_id: &str,
    target_profile: &str,
    source_artifacts: Vec<SourceArtifactIdentity>,
    detail: String,
) -> CorpusCaseResult {
    let failed_stage = definition.stages[0];
    let mut stages = vec![StageResult::failed(
        failed_stage,
        ConversionFailure::internal_for::<CorpusEvidenceFailure>(detail),
    )];
    stages.extend(
        definition.stages[1..]
            .iter()
            .copied()
            .map(StageResult::not_reached),
    );
    CorpusCaseResult::from_stages(
        test_id,
        source_profile(definition),
        source_artifacts,
        target_snapshot(definition, target_profile),
        stages,
    )
}

fn skipped_case(
    definition: &CorpusCaseDef,
    test_id: &str,
    target_profile: &str,
    reason: &str,
) -> CorpusCaseResult {
    let mut case = CorpusCaseResult::from_stages(
        test_id,
        source_profile(definition),
        Vec::new(),
        target_snapshot(definition, target_profile),
        definition
            .stages
            .iter()
            .copied()
            .map(StageResult::not_reached)
            .collect(),
    );
    case.final_outcome = CorpusOutcome::Skipped {
        reason: reason.to_string(),
    };
    case
}

pub fn case_did_not_run(
    definition: &CorpusCaseDef,
    test_id: &str,
    target_profile: &str,
    reason: &str,
) -> CorpusCaseResult {
    skipped_case(definition, test_id, target_profile, reason)
}

fn source_profile(definition: &CorpusCaseDef) -> SourceProfileIdentity {
    SourceProfileIdentity {
        profile: definition.source_profile.clone(),
        format: definition.source_format.as_str().to_string(),
    }
}

fn target_snapshot(definition: &CorpusCaseDef, target_profile: &str) -> TargetCapabilitySnapshot {
    TargetCapabilitySnapshot {
        profile: target_profile.to_string(),
        architecture: definition.target.architecture.clone(),
        init_system: definition.target.init_system.clone(),
        capabilities: definition.target.capabilities.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::corpus::{CorpusSourceFormat, CorpusTargetDef};
    use conary_core::corpus::{FailureKind, SourceArtifactDigestSource, SourceArtifactRole};

    fn definition() -> CorpusCaseDef {
        CorpusCaseDef {
            evidence_path: "/tmp/corpus.json".into(),
            source_profile: "fedora-44".into(),
            source_format: CorpusSourceFormat::Rpm,
            digest_source: SourceArtifactDigestSource::FixtureBuildManifest,
            target: CorpusTargetDef {
                architecture: "x86_64".into(),
                init_system: "systemd".into(),
                capabilities: vec!["native_lifecycle".into()],
            },
            stages: vec![
                ConversionStage::Installation,
                ConversionStage::Update,
                ConversionStage::Rollback,
                ConversionStage::Removal,
            ],
        }
    }

    fn artifacts() -> Vec<SourceArtifactIdentity> {
        vec![
            SourceArtifactIdentity {
                role: SourceArtifactRole::InstallRequest,
                digest_source: SourceArtifactDigestSource::FixtureBuildManifest,
                name: "fixture".into(),
                version: "1.0-1".into(),
                architecture: Some("x86_64".into()),
                digest: "a".repeat(64),
            },
            SourceArtifactIdentity {
                role: SourceArtifactRole::UpdateRequest,
                digest_source: SourceArtifactDigestSource::FixtureBuildManifest,
                name: "fixture".into(),
                version: "2.0-1".into(),
                architecture: Some("x86_64".into()),
                digest: "b".repeat(64),
            },
        ]
    }

    #[test]
    fn passing_evidence_requires_the_complete_declared_journey() {
        let evidence = CorpusRuntimeEvidence {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            source_profile: "fedora-44".into(),
            source_format: "rpm".into(),
            source_artifacts: artifacts(),
            completed_stages: definition().stages,
            active_stage: None,
        };
        assert!(validate_evidence(&definition(), TestStatus::Passed, &evidence).is_ok());

        let incomplete = CorpusRuntimeEvidence {
            completed_stages: vec![ConversionStage::Installation],
            ..evidence
        };
        assert!(validate_evidence(&definition(), TestStatus::Passed, &incomplete).is_err());
    }

    #[test]
    fn failed_evidence_names_the_exact_next_stage() {
        let evidence = CorpusRuntimeEvidence {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            source_profile: "fedora-44".into(),
            source_format: "rpm".into(),
            source_artifacts: artifacts(),
            completed_stages: vec![ConversionStage::Installation],
            active_stage: Some(ConversionStage::Update),
        };
        assert!(validate_evidence(&definition(), TestStatus::Failed, &evidence).is_ok());

        let contradictory = CorpusRuntimeEvidence {
            active_stage: Some(ConversionStage::Removal),
            ..evidence
        };
        assert!(validate_evidence(&definition(), TestStatus::Failed, &contradictory).is_err());
    }

    #[test]
    fn malformed_or_duplicate_artifact_authority_is_rejected() {
        let mut malformed = artifacts();
        malformed[1].role = SourceArtifactRole::InstallRequest;
        malformed[1].digest = "not-a-digest".into();
        let evidence = CorpusRuntimeEvidence {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            source_profile: "fedora-44".into(),
            source_format: "rpm".into(),
            source_artifacts: malformed,
            completed_stages: definition().stages,
            active_stage: None,
        };
        assert!(validate_evidence(&definition(), TestStatus::Passed, &evidence).is_err());
    }

    #[test]
    fn runtime_source_identity_cannot_contradict_the_manifest() {
        let evidence = CorpusRuntimeEvidence {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            source_profile: "arch".into(),
            source_format: "alpm".into(),
            source_artifacts: artifacts(),
            completed_stages: definition().stages,
            active_stage: None,
        };
        assert!(validate_evidence(&definition(), TestStatus::Passed, &evidence).is_err());
    }

    #[test]
    fn diagnostic_wording_cannot_change_evidence_failure_bucket() {
        let first = evidence_failure_case(
            &definition(),
            "TC01",
            "fedora44",
            Vec::new(),
            "first wording".into(),
        );
        let second = evidence_failure_case(
            &definition(),
            "TC02",
            "fedora44",
            Vec::new(),
            "unrelated sentence".into(),
        );

        assert_eq!(
            first.failure_key(),
            Some((
                ConversionStage::Installation,
                FailureKind::InternalUnclassified
            ))
        );
        assert_eq!(first.failure_key(), second.failure_key());
        let first_id = match &first.final_outcome {
            CorpusOutcome::Failed {
                failure: ConversionFailure::InternalUnclassified { incident_id, .. },
                ..
            } => incident_id,
            other => panic!("unexpected outcome: {other:?}"),
        };
        let second_id = match &second.final_outcome {
            CorpusOutcome::Failed {
                failure: ConversionFailure::InternalUnclassified { incident_id, .. },
                ..
            } => incident_id,
            other => panic!("unexpected outcome: {other:?}"),
        };
        assert_eq!(first_id, second_id);
    }

    #[test]
    fn an_unrun_declared_case_remains_visible_as_skipped() {
        let case = case_did_not_run(&definition(), "TC01", "fedora44", "suite timeout");

        assert!(matches!(
            case.final_outcome,
            CorpusOutcome::Skipped { ref reason } if reason == "suite timeout"
        ));
        assert_eq!(case.stage_results.len(), definition().stages.len());
        assert!(case.stage_results.iter().all(|stage| matches!(
            &stage.outcome,
            conary_core::corpus::StageOutcome::NotReached
        )));
    }
}
