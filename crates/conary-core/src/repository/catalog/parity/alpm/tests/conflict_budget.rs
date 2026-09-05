// crates/conary-core/src/repository/catalog/parity/alpm/tests/conflict_budget.rs

//! Native check counts, independent of provider Cartesian-product size.

use super::*;
use crate::repository::catalog::parity::{
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyErrorVariantV1,
};

#[derive(Clone, Copy)]
enum Shape {
    AllConflicting,
    HealthyAlternative,
    MissingAndConflicting,
    BudgetExceeded,
}

#[test]
fn thirty_provider_pairs_only_check_the_conflict_party() {
    check_shape(Shape::AllConflicting, 30, 2, 2);
}

#[test]
fn thirty_provider_pairs_accept_one_native_prepared_alternative() {
    check_shape(Shape::HealthyAlternative, 30, 2, 2);
}

#[test]
fn missing_first_thirty_provider_pairs_do_not_expand_a_cartesian_product() {
    check_shape(Shape::MissingAndConflicting, 30, 2, 4);
}

#[test]
fn native_provider_budget_is_a_typed_strict_and_survey_failure() {
    check_shape(Shape::BudgetExceeded, 1, 257, 256);
}

fn check_shape(shape: Shape, dependencies: usize, providers: usize, expected_checks: u32) {
    let root_name = match shape {
        Shape::AllConflicting => "bounded-conflict-root",
        Shape::HealthyAlternative => "bounded-resolved-root",
        Shape::MissingAndConflicting => "bounded-mixed-root",
        Shape::BudgetExceeded => "bounded-budget-root",
    };
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("core.db");
    let requirements = (0..dependencies)
        .map(|i| format!("bounded-cap-{i:02}>=1"))
        .collect::<Vec<_>>();
    let provides = (0..dependencies)
        .map(|i| format!("bounded-cap-{i:02}=1"))
        .collect::<Vec<_>>();
    let provide_refs = provides
        .iter()
        .map(|name| vec![name.as_str()])
        .collect::<Vec<_>>();
    let names = (0..dependencies)
        .flat_map(|i| (0..providers).map(move |j| format!("provider-{i:02}-{j:03}")))
        .collect::<Vec<_>>();
    let checksums = (1..=names.len())
        .map(|i| format!("{i:064x}"))
        .collect::<Vec<_>>();
    let root_checksum = digest('f');
    let mut requires = requirements.iter().map(String::as_str).collect::<Vec<_>>();
    if matches!(shape, Shape::MissingAndConflicting) {
        requires.push("absent-independent-capability");
    }
    let conflict_names = names[(dependencies - 1) * providers..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let root_conflict = [root_name];
    let mut root = PackageFixture::new(root_name, &root_checksum);
    root.depends = &requires;
    // Exercise root-side conflicts as well as provider-side ones. Put the
    // conflict-party dependency last, behind 29 irrelevant binary choices.
    if matches!(shape, Shape::AllConflicting | Shape::MissingAndConflicting) {
        root.conflicts = &conflict_names;
    }
    let mut packages = vec![root];
    for (index, name) in names.iter().enumerate() {
        let dependency = index / providers;
        let provider = index % providers;
        let mut package = PackageFixture::new(name, &checksums[index]);
        package.provides = &provide_refs[dependency];
        if dependency == dependencies - 1 && matches!(shape, Shape::BudgetExceeded)
            || dependency == dependencies - 1
                && provider == 0
                && matches!(shape, Shape::HealthyAlternative)
        {
            package.conflicts = &root_conflict;
        }
        packages.push(package);
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
    if matches!(shape, Shape::BudgetExceeded) {
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
        if matches!(shape, Shape::HealthyAlternative) {
            let mut expected = vec![root_key.clone()];
            for i in 0..dependencies {
                let provider = usize::from(i == dependencies - 1);
                expected.push(by_name[&format!("provider-{i:02}-{provider:03}")].clone());
            }
            expected.sort();
            assert_eq!(
                outcome,
                Some(NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256: expected
                })
            );
        } else {
            assert_eq!(
                outcome,
                Some(NativeResolutionOutcomeV1::NotInstallable {
                    reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure
                })
            );
        }
    }
    assert_eq!(
        resolution::native_probe_checks(root_key),
        expected_checks,
        "strict native check count"
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
        expected_checks,
        "survey native check count"
    );
    assert_eq!(survey.counts.roots_walked, packages.len() as u64);
    if matches!(shape, Shape::BudgetExceeded) {
        assert_eq!(survey.counts.failed_roots, 1);
        assert_eq!(survey.counts.resolved_roots, names.len() as u64);
        assert_eq!(survey.counts.unresolved_roots, 0);
        assert_eq!(survey.counts.not_installable_roots, 0);
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
                    checks: 256
                },
            }
        );
    } else {
        assert_eq!(survey.counts.failed_roots, 0);
        assert!(survey.failures.is_empty());
    }
    println!("{root_name}: strict and survey each performed {expected_checks} native checks");
}
