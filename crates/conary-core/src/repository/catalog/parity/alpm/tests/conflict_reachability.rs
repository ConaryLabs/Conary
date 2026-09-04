// crates/conary-core/src/repository/catalog/parity/alpm/tests/conflict_reachability.rs

//! Conflict relevance follows the native-selected provider's dependency closure.

use super::*;

#[derive(Clone, Copy)]
enum Shape {
    HealthyAlternative,
    BothBlocked,
    TwoLevels,
    MissingFirst,
    NativeSelectedSatisfier,
}

#[test]
fn selected_provider_reaches_transitive_conflict_and_healthy_alternative_resolves() {
    check_shape(Shape::HealthyAlternative);
}

#[test]
fn both_providers_reach_transitive_conflict() {
    check_shape(Shape::BothBlocked);
}

#[test]
fn selected_provider_reaches_conflict_through_two_levels() {
    check_shape(Shape::TwoLevels);
}

#[test]
fn early_prepare_failure_uses_sync_closure_for_transitive_relevance() {
    check_shape(Shape::MissingFirst);
}

#[test]
fn populated_transaction_relevance_uses_chosen_not_sync_default_satisfier() {
    check_shape(Shape::NativeSelectedSatisfier);
}

fn check_shape(shape: Shape) {
    let root_name = match shape {
        Shape::HealthyAlternative => "transitive-healthy-root",
        Shape::BothBlocked => "transitive-blocked-root",
        Shape::TwoLevels => "transitive-deep-root",
        Shape::MissingFirst => "transitive-early-root",
        Shape::NativeSelectedSatisfier => "transitive-chosen-root",
    };
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("core.db");
    let checksums = ['a', 'b', 'c', 'd', 'e', 'f'].map(digest);
    let mut root = PackageFixture::new(root_name, &checksums[0]);
    root.depends = &["virtual-a"];
    if matches!(shape, Shape::NativeSelectedSatisfier) {
        root.depends = &["blocker", "virtual-a"];
    }
    let mut a1 = PackageFixture::new("a1", &checksums[1]);
    a1.provides = &["virtual-a"];
    a1.depends = match shape {
        Shape::TwoLevels => &["mid"],
        Shape::NativeSelectedSatisfier => &["virtual-b"],
        _ => &["blocker"],
    };
    let mut a2 = PackageFixture::new("a2", &checksums[2]);
    a2.provides = &["virtual-a"];
    if matches!(shape, Shape::BothBlocked) {
        a2.depends = &["blocker"];
    }
    let root_conflict = [root_name];
    let mut blocker = PackageFixture::new("blocker", &checksums[3]);
    blocker.conflicts = &root_conflict;
    if matches!(shape, Shape::NativeSelectedSatisfier) {
        blocker.provides = &["virtual-b"];
    }
    if matches!(shape, Shape::MissingFirst) {
        // Native resolvedeps restores the unpopulated add set on missing-first
        // failure. The alternative is healthy, so its real prepare can resolve.
        blocker.depends = &["missing-leaf"];
    }
    let mut packages = vec![root, a1, a2, blocker];
    if matches!(shape, Shape::NativeSelectedSatisfier) {
        // b0 is the sync-db default for virtual-b, but root added blocker
        // first. Native resolvedeps therefore binds a1 -> blocker within
        // its chosen set. A sync-db reconstruction would miss this edge.
        let mut b0 = PackageFixture::new("b0", &checksums[5]);
        b0.provides = &["virtual-b"];
        packages.push(b0);
    }
    if matches!(shape, Shape::TwoLevels) {
        let mut mid = PackageFixture::new("mid", &checksums[4]);
        mid.depends = &["blocker"];
        packages.push(mid);
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
    produce_alpm_resolution_oracle(
        &profile,
        &member_inputs,
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();
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
    let blocked = matches!(shape, Shape::BothBlocked | Shape::NativeSelectedSatisfier);
    let expected = if blocked {
        NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
        }
    } else {
        let mut closure = vec![root_key.clone(), by_name["a2"].clone()];
        closure.sort();
        NativeResolutionOutcomeV1::Resolved {
            closure_package_keys_sha256: closure,
        }
    };
    assert_eq!(outcome, Some(expected.clone()));
    let checks = if matches!(shape, Shape::MissingFirst) {
        3
    } else {
        2
    };
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
    assert_eq!(survey.counts.roots_walked, packages.len() as u64);
    assert_eq!(survey.counts.failed_roots, 0);
    assert!(survey.failures.is_empty());
    assert_eq!(
        resolution::native_probe_checks(root_key),
        checks,
        "survey checks"
    );
    if blocked {
        let diagnostic = survey
            .diagnostic_outcomes
            .iter()
            .find(|row| row.root_package_key_sha256 == *root_key)
            .unwrap();
        assert_eq!(diagnostic.outcome, expected);
    } else {
        assert_eq!(survey.counts.not_installable_roots, 0);
    }
    println!("{root_name}: strict and survey each performed {checks} native checks");
}
