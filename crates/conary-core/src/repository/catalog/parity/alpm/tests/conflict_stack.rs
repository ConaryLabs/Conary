// crates/conary-core/src/repository/catalog/parity/alpm/tests/conflict_stack.rs

//! Newly exposed native questions descend before ancestor alternatives resume.

use super::*;
use crate::repository::catalog::parity::{
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyErrorVariantV1,
};

#[derive(Clone, Copy)]
enum Shape {
    Healthy,
    Blocked,
    ThreeDeep,
    Backtrack,
    Budget,
}

#[test]
fn alternative_exposes_a_relevant_question_with_a_healthy_provider() {
    check_shape(Shape::Healthy, 3);
}

#[test]
fn exposed_question_exhausts_all_conflicting_providers() {
    check_shape(Shape::Blocked, 3);
}

#[test]
fn three_deep_exposed_questions_resolve_with_ancestor_answers() {
    check_shape(Shape::ThreeDeep, 4);
}

#[test]
fn exhausted_child_backtracks_to_the_next_ancestor_provider() {
    check_shape(Shape::Backtrack, 4);
}

#[test]
fn exposed_question_budget_failure_never_classifies_the_root() {
    check_shape(Shape::Budget, 256);
}

fn check_shape(shape: Shape, checks: u32) {
    let root_name = match shape {
        Shape::Healthy => "stack-healthy-root",
        Shape::Blocked => "stack-blocked-root",
        Shape::ThreeDeep => "stack-deep-root",
        Shape::Backtrack => "stack-backtrack-root",
        Shape::Budget => "stack-budget-root",
    };
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("core.db");
    let b_count = if matches!(shape, Shape::Budget) {
        256
    } else {
        2
    };
    let b_names = (1..=b_count)
        .map(|i| format!("b{i:03}"))
        .collect::<Vec<_>>();
    let checksums = (0..b_count + 6)
        .map(|i| format!("{i:064x}"))
        .collect::<Vec<_>>();
    let conflict = [root_name];
    let mut root = PackageFixture::new(root_name, &checksums[0]);
    root.depends = &["virtual-a"];
    let mut a1 = PackageFixture::new("a1", &checksums[1]);
    a1.provides = &["virtual-a"];
    a1.conflicts = &conflict;
    let mut a2 = PackageFixture::new("a2", &checksums[2]);
    a2.provides = &["virtual-a"];
    a2.depends = &["virtual-b"];
    let mut packages = vec![root, a1, a2];
    for (index, name) in b_names.iter().enumerate() {
        let mut provider = PackageFixture::new(name, &checksums[index + 3]);
        provider.provides = &["virtual-b"];
        if index == 0 || matches!(shape, Shape::Blocked | Shape::Backtrack | Shape::Budget) {
            provider.conflicts = &conflict;
        } else if matches!(shape, Shape::ThreeDeep) {
            provider.depends = &["virtual-c"];
        }
        packages.push(provider);
    }
    if matches!(shape, Shape::ThreeDeep) {
        let mut c1 = PackageFixture::new("c1", &checksums[b_count + 3]);
        c1.provides = &["virtual-c"];
        c1.conflicts = &conflict;
        let mut c2 = PackageFixture::new("c2", &checksums[b_count + 4]);
        c2.provides = &["virtual-c"];
        packages.extend([c1, c2]);
    }
    if matches!(shape, Shape::Backtrack) {
        let mut a3 = PackageFixture::new("a3", &checksums[b_count + 5]);
        a3.provides = &["virtual-a"];
        packages.push(a3);
    }
    write_database(&database, &packages);
    let snapshots = vec![source_snapshot("arch-core-x86_64", &database)];
    let databases = vec![database];
    let mut profile = profile(&snapshots);
    profile.counts.packages = packages.len() as u64;
    profile.counts.source_evidence = 1;
    let member_inputs = inputs(&snapshots, &databases);
    let package_output = directory.path().join("packages");
    produce_alpm_parity_oracle(&profile, &member_inputs, &package_output).unwrap();
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut by_name = std::collections::BTreeMap::new();
    package_reader
        .for_each_package(|package| {
            by_name.insert(package.name, package.package_key_sha256);
            Ok(())
        })
        .unwrap();
    let root_key = &by_name[root_name];
    let resolution_output = directory.path().join("resolution");
    let strict = produce_alpm_resolution_oracle(
        &profile,
        &member_inputs,
        &package_output,
        "x86_64",
        &resolution_output,
    );
    let expected = match shape {
        Shape::Budget => None,
        Shape::Blocked => Some(NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
        }),
        _ => {
            let names = match shape {
                Shape::ThreeDeep => vec![root_name, "a2", "b002", "c2"],
                Shape::Backtrack => vec![root_name, "a3"],
                _ => vec![root_name, "a2", "b002"],
            };
            let mut closure = names
                .iter()
                .map(|name| by_name[*name].clone())
                .collect::<Vec<_>>();
            closure.sort();
            Some(NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: closure,
            })
        }
    };
    if matches!(shape, Shape::Budget) {
        assert!(
            matches!(strict, Err(Error::ProviderSearchBudgetExceeded { ref root, checks: 256 }) if root == root_name)
        );
        assert!(
            !resolution_output
                .join(NATIVE_RESOLUTION_MANIFEST_FILE_NAME)
                .exists()
        );
    } else {
        strict.unwrap();
        let resolution =
            verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
                .unwrap();
        let mut outcome = None;
        resolution
            .for_each_root(|row| {
                if row.root_package_key_sha256 == *root_key {
                    outcome = Some(row.outcome);
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(outcome, expected);
    }
    assert_eq!(
        resolution::native_probe_checks(root_key),
        checks,
        "strict checks"
    );
    let survey = produce_alpm_resolution_survey(
        &profile,
        &member_inputs,
        &package_output,
        "x86_64",
        &directory.path().join("survey.json"),
    )
    .unwrap();
    assert_eq!(
        resolution::native_probe_checks(root_key),
        checks,
        "survey checks"
    );
    assert_eq!(survey.counts.roots_walked, packages.len() as u64);
    if matches!(shape, Shape::Budget) {
        assert_eq!(survey.counts.failed_roots, 1);
        assert_eq!(survey.counts.resolved_roots, packages.len() as u64 - 1);
        assert_eq!(survey.counts.not_installable_roots, 0);
        assert_eq!(survey.counts.unresolved_roots, 0);
        assert!(survey.diagnostic_outcomes.is_empty());
        assert_eq!(survey.failures.len(), 1);
        let failure = &survey.failures[0];
        assert_eq!(failure.root_package_key_sha256, *root_key);
        assert_eq!(
            failure.error_kind.reason,
            NativeResolutionSurveyErrorReasonV1::ProviderSearchBudgetExceeded
        );
        assert_eq!(
            failure.error_kind.error_variant,
            NativeResolutionSurveyErrorVariantV1::ProviderSearchBudgetExceeded
        );
        assert_eq!(
            failure.native_explanation,
            NativeResolutionSurveyNativeExplanationV1::Alpm {
                result: NativeResolutionSurveyAlpmResultV1::ProviderSearchBudgetExceeded {
                    root: root_name.to_string(),
                    checks: 256,
                },
            }
        );
    } else {
        assert_eq!(survey.counts.failed_roots, 0);
        assert!(survey.failures.is_empty());
        if matches!(shape, Shape::Blocked) {
            let diagnostic = survey
                .diagnostic_outcomes
                .iter()
                .find(|row| row.root_package_key_sha256 == *root_key)
                .unwrap();
            assert_eq!(Some(diagnostic.outcome.clone()), expected);
        } else {
            assert_eq!(survey.counts.not_installable_roots, 0);
        }
    }
    println!("{root_name}: strict and survey each performed {checks} native checks");
}
