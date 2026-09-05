// crates/conary-core/src/repository/catalog/parity/tests/resolution_producer.rs

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::error::{Error, Result};
use crate::repository::catalog::parity::resolution_parallel::ResolutionExplanationLimits;
use crate::repository::catalog::parity::resolution_producer::{
    Both, NativeResolutionEcosystem, Oracle, ResolutionContext, Survey, produce_resolution,
};
use crate::repository::catalog::parity::resolution_survey::{
    NativeRootResolutionError, NativeRootResolutionResult, NativeRootResolutionSuccess,
};

struct Input {
    calls: AtomicUsize,
    fail_key: Option<String>,
}

struct FixtureEcosystem;

impl NativeResolutionEcosystem<'_> for FixtureEcosystem {
    type Input = Input;
    type Prepared = ();
    type Worker = ();
    const LABEL: &'static str = "fixture";

    fn prepare(context: &ResolutionContext<'_, Input>, _: &NativeParityOracleReader) -> Result<()> {
        context.policy.validate_for_profile(context.profile)
    }

    fn implementation() -> NativeParityImplementationV1 {
        implementation(NativeParityEcosystemV1::Rpm)
    }

    fn open_worker(_: &ResolutionContext<'_, Input>, _: &()) -> Result<()> {
        Ok(())
    }

    fn resolve_root(
        context: &ResolutionContext<'_, Input>,
        _: &mut (),
        root: &NativeParityPackageV1,
        limits: ResolutionExplanationLimits,
    ) -> Result<NativeRootResolutionResult> {
        context.inputs[0].calls.fetch_add(1, Ordering::SeqCst);
        if context.inputs[0].fail_key.as_ref() == Some(&root.package_key_sha256) {
            return Ok(Err(NativeRootResolutionError::new(
                Error::ConflictError("fixture failed root".to_string()),
                NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
                NativeResolutionSurveyNativeExplanationV1::Rpm {
                    result: NativeResolutionSurveyRpmResultV1::Problems { problems: vec![] },
                },
            )));
        }
        let outcome = NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
        };
        let explanation = if limits.diagnostic_outcome_bytes() == 0 {
            NativeResolutionSurveyNativeExplanationV1::Withheld {
                reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
            }
        } else {
            NativeResolutionSurveyNativeExplanationV1::Rpm {
                result: NativeResolutionSurveyRpmResultV1::Problems { problems: vec![] },
            }
        };
        Ok(Ok(NativeRootResolutionSuccess::explained(
            outcome,
            explanation,
        )))
    }
}

#[test]
fn combined_walk_matches_both_single_products_and_visits_each_root_once() {
    check_combined_walk(false);
}

#[test]
fn combined_walk_keeps_full_survey_and_no_strict_directory_after_failed_root() {
    check_combined_walk(true);
}

fn check_combined_walk(fail: bool) {
    let _capacity = super::super::resolution_parallel::resolution_test_capacity(2);
    let candidate = candidate(NativeParityEcosystemV1::Rpm);
    let mut packages = rows(&candidate);
    packages.sort_by(|a, b| a.package_key_sha256.cmp(&b.package_key_sha256));
    let oracle = oracle(&candidate, NativeParityEcosystemV1::Rpm, packages.clone());
    let inputs = [Input {
        calls: AtomicUsize::new(0),
        fail_key: fail.then(|| packages[0].package_key_sha256.clone()),
    }];
    let directory = tempfile::tempdir().unwrap();
    let survey_path = directory.path().join("survey.json");
    let strict_path = directory.path().join("strict");
    let ((survey, manifest), evidence) = produce_resolution::<FixtureEcosystem, _>(
        &candidate.profile,
        &inputs,
        oracle._directory.path(),
        "x86_64",
        Both {
            survey: &survey_path,
            oracle: &strict_path,
        },
        ResolutionWorkerRequest::Automatic,
    )
    .unwrap();
    assert_eq!(inputs[0].calls.load(Ordering::SeqCst), packages.len());
    assert_eq!(survey.counts.roots_walked, packages.len() as u64);
    assert_eq!(survey.total_failures, u64::from(fail));
    assert_eq!(manifest.is_some(), !fail);
    assert_eq!(strict_path.exists(), !fail);
    let standalone_survey = directory.path().join("standalone.json");
    produce_resolution::<FixtureEcosystem, _>(
        &candidate.profile,
        &inputs,
        oracle._directory.path(),
        "x86_64",
        Survey(&standalone_survey),
        ResolutionWorkerRequest::Automatic,
    )
    .unwrap();
    assert_eq!(
        fs::read(&survey_path).unwrap(),
        fs::read(&standalone_survey).unwrap()
    );
    assert_eq!(evidence.workers, 2);
    if !fail {
        assert!(survey.retained_explanations > 0);
        assert_eq!(survey.withheld_explanations, 0);
        let standalone_strict = directory.path().join("standalone-strict");
        let (standalone_manifest, _) = produce_resolution::<FixtureEcosystem, _>(
            &candidate.profile,
            &inputs,
            oracle._directory.path(),
            "x86_64",
            Oracle(&standalone_strict),
            ResolutionWorkerRequest::Automatic,
        )
        .unwrap();
        assert_eq!(manifest.unwrap(), standalone_manifest);
        for file in ["manifest.json", "roots.jsonl"] {
            assert_eq!(
                fs::read(strict_path.join(file)).unwrap(),
                fs::read(standalone_strict.join(file)).unwrap()
            );
        }
    }
    // Temporary strict staging is removed on both success and failure.
    assert!(fs::read_dir(directory.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".conary-resolution-")
    }));
}
