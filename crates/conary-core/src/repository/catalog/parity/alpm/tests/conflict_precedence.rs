// crates/conary-core/src/repository/catalog/parity/alpm/tests/conflict_precedence.rs

//! Registered-database precedence in missing-first conflict probes.

use super::*;

#[test]
fn precedence_selected_conflict_is_not_avoided_by_shadowed_provider() {
    assert_precedence_outcome(true);
}

#[test]
fn precedence_selected_healthy_provider_resolves_despite_shadowed_conflict() {
    assert_precedence_outcome(false);
}

fn assert_precedence_outcome(selected_conflicts: bool) {
    let directory = tempfile::tempdir().unwrap();
    let databases = vec![
        directory.path().join("core.db"),
        directory.path().join("extra.db"),
    ];
    let checksums = ['a', 'b', 'c'].map(digest);
    let mut root = PackageFixture::new("precedence-root", &checksums[0]);
    root.depends = &["provider>=1"];
    let mut selected = PackageFixture::new("provider", &checksums[1]);
    let mut shadowed = PackageFixture::new("provider", &checksums[2]);
    // A newer version in the lower-precedence database must not create an
    // alternative to the satisfying literal selected from the first database.
    shadowed.version = "2.0-1";
    let conflicting = if selected_conflicts {
        &mut selected
    } else {
        &mut shadowed
    };
    conflicting.conflicts = &["precedence-root"];
    // Make native preparation fail on missing dependencies before checking
    // conflicts, so the regression exercises the reachable conflict probe.
    conflicting.depends = &["absent-provider-dependency"];
    write_database(&databases[0], &[root, selected]);
    write_database(&databases[1], &[shadowed]);
    let snapshots = vec![
        source_snapshot("arch-core-x86_64", &databases[0]),
        source_snapshot("arch-extra-x86_64", &databases[1]),
    ];
    let profile = profile(&snapshots);
    let member_inputs = inputs(&snapshots, &databases);
    let package_output = directory.path().join("packages");
    produce_alpm_parity_oracle(&profile, &member_inputs, &package_output).unwrap();
    let resolution_output = directory.path().join("resolution");
    produce_alpm_resolution_oracle(
        &profile,
        &member_inputs,
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();
    let packages = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut root_key = String::new();
    let mut selected_key = String::new();
    packages
        .for_each_package(|package| {
            if package.name == "precedence-root" {
                root_key = package.package_key_sha256;
            } else if package.member_ordinal == 0 {
                selected_key = package.package_key_sha256;
            }
            Ok(())
        })
        .unwrap();
    let resolution =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &packages).unwrap();
    let mut outcome = None;
    resolution
        .for_each_root(|row| {
            if row.root_package_key_sha256 == root_key {
                outcome = Some(row.outcome);
            }
            Ok(())
        })
        .unwrap();
    let expected = if selected_conflicts {
        NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
        }
    } else {
        let mut closure = vec![root_key.clone(), selected_key];
        closure.sort();
        NativeResolutionOutcomeV1::Resolved {
            closure_package_keys_sha256: closure,
        }
    };
    assert_eq!(outcome, Some(expected.clone()));

    let survey = produce_alpm_resolution_survey(
        &profile,
        &member_inputs,
        &package_output,
        "x86_64",
        &directory.path().join("survey.json"),
    )
    .unwrap();
    assert_eq!(survey.counts.roots_walked, 3);
    assert_eq!(survey.counts.failed_roots, 0);
    assert!(survey.failures.is_empty());
    if selected_conflicts {
        let diagnostic = survey
            .diagnostic_outcomes
            .iter()
            .find(|row| row.root_package_key_sha256 == root_key)
            .unwrap();
        assert_eq!(diagnostic.outcome, expected);
    } else {
        assert_eq!(survey.counts.resolved_roots, 2);
        assert_eq!(survey.counts.not_installable_roots, 0);
    }
}
