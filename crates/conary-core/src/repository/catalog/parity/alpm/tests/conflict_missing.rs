// crates/conary-core/src/repository/catalog/parity/alpm/tests/conflict_missing.rs

//! Prepared paths beat deterministic missing fallbacks; all answers are replayed.

use super::*;

#[derive(Clone, Copy)]
enum Shape {
    LaterPrepared,
    FirstMissing,
    IndependentMissing,
    NestedReplay,
}

#[test]
fn later_prepared_provider_beats_first_missing_alternative() {
    check(Shape::LaterPrepared, 4);
}

#[test]
fn first_conflict_free_missing_edges_survive_later_missing_alternatives() {
    check(Shape::FirstMissing, 5);
}

#[test]
fn independent_missing_closure_replays_the_override() {
    check(Shape::IndependentMissing, 4);
}

#[test]
fn independent_missing_closure_replays_every_active_answer() {
    check(Shape::NestedReplay, 6);
}

fn check(shape: Shape, checks: u32) {
    let root_name = match shape {
        Shape::LaterPrepared => "missing-later-prepared-root",
        Shape::FirstMissing => "missing-first-fallback-root",
        Shape::IndependentMissing => "missing-override-root",
        Shape::NestedReplay => "missing-nested-replay-root",
    };
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("core.db");
    let checksums = ['a', 'b', 'c', 'd', 'e', 'f'].map(digest);
    let conflict = [root_name];
    let independent = matches!(shape, Shape::IndependentMissing | Shape::NestedReplay);
    let mut root = PackageFixture::new(root_name, &checksums[0]);
    root.depends = if independent {
        &["virtual-a", "independent-missing"]
    } else {
        &["virtual-a"]
    };
    let mut a1 = PackageFixture::new("a1", &checksums[1]);
    a1.provides = &["virtual-a"];
    a1.conflicts = &conflict;
    let mut a2 = PackageFixture::new("a2", &checksums[2]);
    a2.provides = &["virtual-a"];
    a2.depends = match shape {
        Shape::LaterPrepared | Shape::FirstMissing => &["first-missing"],
        Shape::NestedReplay => &["virtual-b"],
        _ => &[],
    };
    let mut packages = vec![root, a1, a2];
    if !independent {
        let mut a3 = PackageFixture::new("a3", &checksums[3]);
        a3.provides = &["virtual-a"];
        if matches!(shape, Shape::FirstMissing) {
            a3.depends = &["later-missing"];
        }
        packages.push(a3);
    }
    if matches!(shape, Shape::NestedReplay) {
        let mut b1 = PackageFixture::new("b1", &checksums[4]);
        b1.provides = &["virtual-b"];
        b1.conflicts = &conflict;
        let mut b2 = PackageFixture::new("b2", &checksums[5]);
        b2.provides = &["virtual-b"];
        packages.extend([b1, b2]);
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
        .for_each_package(|p| {
            by_name.insert(p.name.clone(), p);
            Ok(())
        })
        .unwrap();
    let root_key = &by_name[root_name].package_key_sha256;
    let output = directory.path().join("resolution");
    produce_alpm_resolution_oracle(&profile, &member_inputs, &package_output, "x86_64", &output)
        .unwrap();
    let reader =
        verify_native_resolution_oracle_bundle(&output, &profile, &package_reader).unwrap();
    let mut outcome = None;
    reader
        .for_each_root(|row| {
            if row.root_package_key_sha256 == *root_key {
                outcome = Some(row.outcome);
            }
            Ok(())
        })
        .unwrap();
    let expected = if matches!(shape, Shape::LaterPrepared) {
        let mut closure = vec![root_key.clone(), by_name["a3"].package_key_sha256.clone()];
        closure.sort();
        NativeResolutionOutcomeV1::Resolved {
            closure_package_keys_sha256: closure,
        }
    } else {
        let requiring = &by_name[if independent { root_name } else { "a2" }];
        let requirement = if independent {
            "independent-missing"
        } else {
            "first-missing"
        };
        let group = requiring
            .requirement_groups
            .iter()
            .find(|g| g.atoms.iter().any(|a| a.capability == requirement))
            .unwrap();
        let mut dependencies = vec![NativeUnresolvedDependencyV1 {
            requiring_package_key_sha256: requiring.package_key_sha256.clone(),
            requirement_group_sha256: native_requirement_group_sha256(group).unwrap(),
        }];
        if !independent {
            // Native resolvedeps also reports the root's virtual dependency
            // when the selected provider's recursive resolution fails.
            let group = by_name[root_name]
                .requirement_groups
                .iter()
                .find(|g| g.atoms.iter().any(|a| a.capability == "virtual-a"))
                .unwrap();
            dependencies.push(NativeUnresolvedDependencyV1 {
                requiring_package_key_sha256: root_key.clone(),
                requirement_group_sha256: native_requirement_group_sha256(group).unwrap(),
            });
        }
        dependencies.sort();
        NativeResolutionOutcomeV1::Unresolved { dependencies }
    };
    assert_eq!(outcome, Some(expected));
    assert_eq!(
        resolution::native_probe_checks(root_key),
        checks,
        "strict checks"
    );
    let assert_replay = || {
        if independent {
            let closure = resolution::native_probe_missing_closure(root_key);
            assert!(closure.contains(&"a2".to_string()), "{closure:?}");
            assert!(!closure.contains(&"a1".to_string()), "{closure:?}");
            if matches!(shape, Shape::NestedReplay) {
                assert!(closure.contains(&"b2".to_string()), "{closure:?}");
                assert!(!closure.contains(&"b1".to_string()), "{closure:?}");
            }
        }
    };
    assert_replay();
    let survey = produce_alpm_resolution_survey(
        &profile,
        &member_inputs,
        &package_output,
        "x86_64",
        &directory.path().join("survey.json"),
    )
    .unwrap();
    assert_eq!(survey.counts.failed_roots, 0);
    assert_eq!(
        resolution::native_probe_checks(root_key),
        checks,
        "survey checks"
    );
    assert_replay();
    println!("{root_name}: strict and survey each performed {checks} native checks");
}
