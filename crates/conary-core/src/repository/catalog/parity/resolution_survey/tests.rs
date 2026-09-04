// crates/conary-core/src/repository/catalog/parity/resolution_survey/tests.rs

use std::collections::BTreeMap;

use super::*;
use crate::repository::catalog::parity::{
    NativeParityEcosystemV1, NativeParityPackageV1, NativeResolutionArchitectureAdmissionV1,
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
        total_diagnostic_outcomes: 0,
        diagnostic_outcomes: Vec::new(),
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
    for index in 0..=NATIVE_RESOLUTION_SURVEY_FAILURE_LIMIT {
        let mut root = root();
        root.package_key_sha256 = format!("{index:064x}");
        collector
            .failure(
                &root,
                *NativeRootResolutionError::new(
                    Error::ConfigError("diagnostic failure".to_string()),
                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                    NativeResolutionSurveyNativeExplanationV1::Rpm {
                        result: NativeResolutionSurveyRpmResultV1::Problems {
                            problems: Vec::new(),
                        },
                    },
                ),
            )
            .unwrap();
    }

    assert!(collector.remaining_evidence_bytes() > 0);
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

    let mut drifted_limit = survey.clone();
    drifted_limit.failure_record_limit += 1;
    let error = drifted_limit.validate().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failure record limit 5001 is unsupported")
    );

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
fn survey_caps_diagnostic_outcomes_and_reports_uncapped_truth() {
    let mut collector = collector(NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT);
    for index in 0..=NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT {
        let mut root = root();
        root.package_key_sha256 = format!("{index:064x}");
        collector
            .success(
                &root,
                NativeRootResolutionSuccess::explained(
                    NativeResolutionOutcomeV1::NotInstallable {
                        reason: super::super::resolution_contract::NativeResolutionNotInstallableReasonV1::ConflictingClosure,
                    },
                    NativeResolutionSurveyNativeExplanationV1::Rpm {
                        result: NativeResolutionSurveyRpmResultV1::Problems {
                            problems: Vec::new(),
                        },
                    },
                ),
            )
            .unwrap();
    }

    let limits = collector.explanation_limits();
    assert_eq!(limits.diagnostic_outcome_bytes(), 0);
    assert!(limits.failure_bytes() > 0);
    let mut failure_root = root();
    failure_root.package_key_sha256 = format!(
        "{:064x}",
        NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT + 1
    );
    collector
        .failure(
            &failure_root,
            *NativeRootResolutionError::new(
                Error::ConfigError("later diagnostic failure".to_string()),
                NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                NativeResolutionSurveyNativeExplanationV1::Rpm {
                    result: NativeResolutionSurveyRpmResultV1::Problems {
                        problems: Vec::new(),
                    },
                },
            ),
        )
        .unwrap();
    let survey = collector.finish().unwrap();
    assert_eq!(
        survey.total_diagnostic_outcomes,
        NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT as u64 + 1
    );
    assert_eq!(
        survey.retained_diagnostic_outcomes,
        NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT as u64
    );
    assert!(survey.diagnostic_outcomes_truncated);
    assert_eq!(
        survey.retained_explanations,
        NATIVE_RESOLUTION_SURVEY_DIAGNOSTIC_OUTCOME_LIMIT as u64 + 1
    );
    assert_eq!(survey.withheld_explanations, 0);
    assert_eq!(survey.retained_failures, 1);
    assert!(matches!(
        survey.failures[0].native_explanation,
        NativeResolutionSurveyNativeExplanationV1::Rpm { .. }
    ));

    let mut drifted_limit = survey.clone();
    drifted_limit.diagnostic_outcome_record_limit += 1;
    let error = drifted_limit.validate().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("diagnostic outcome record limit 5001 is unsupported")
    );
}

#[test]
fn survey_rejects_noncanonical_evidence_limit_before_writing() {
    let mut survey = collector(NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT)
        .finish()
        .unwrap();
    survey.evidence_byte_limit /= 2;
    let error = survey.validate().unwrap_err();
    assert!(matches!(error, Error::ConfigError(_)));
    assert!(
        error
            .to_string()
            .contains("evidence byte limit 16777216 is unsupported; expected 33554432")
    );
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("survey.json");
    assert!(matches!(
        write_native_resolution_survey(&output, &survey),
        Err(Error::ConfigError(_))
    ));
    assert!(!output.exists());
}

#[test]
fn collector_withholds_explanations_after_canonical_byte_budget_is_exhausted() {
    let explanation = NativeResolutionSurveyNativeExplanationV1::Rpm {
        result: NativeResolutionSurveyRpmResultV1::Problems {
            problems: Vec::new(),
        },
    };
    let explanation_bytes = canonical_value_size_with_limit(&explanation, u64::MAX)
        .unwrap()
        .unwrap();
    assert_eq!(
        explanation_bytes,
        crate::json::canonical_json(&explanation).unwrap().len() as u64
    );
    let mut collector = collector(explanation_bytes);
    for index in 0..3 {
        let mut root = root();
        root.package_key_sha256 = format!("{index:064x}");
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

    // Exercise retention with a tiny internal budget, but never publish
    // that test budget as a valid hard-cut survey contract.
    assert_eq!(collector.failures.len(), 3);
    assert_eq!(collector.retained_evidence_bytes, explanation_bytes);
    assert_eq!(collector.retained_explanations, 1);
    assert_eq!(collector.withheld_explanations, 2);
    assert!(collector.evidence_budget_exhausted);
    assert!(matches!(
        collector.failures[0].native_explanation,
        NativeResolutionSurveyNativeExplanationV1::Rpm { .. }
    ));
    assert!(collector.failures[1..].iter().all(|failure| matches!(
        failure.native_explanation,
        NativeResolutionSurveyNativeExplanationV1::Withheld {
            reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted
        }
    )));
    assert!(matches!(collector.finish(), Err(Error::ConfigError(_))));
}

#[test]
fn survey_counts_explanation_withheld_during_native_projection() {
    let mut collector = collector(NATIVE_RESOLUTION_SURVEY_EVIDENCE_BYTE_LIMIT);
    collector
        .failure(
            &root(),
            *NativeRootResolutionError::new(
                Error::ConfigError("diagnostic failure".to_string()),
                NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                NativeResolutionSurveyNativeExplanationV1::Withheld {
                    reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
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
