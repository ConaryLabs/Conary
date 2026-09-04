// crates/conary-core/src/repository/catalog/parity/debian/tests/sibling_conflict.rs

use super::*;

#[test]
fn transitive_sibling_conflicts_are_typed_outcomes() {
    check("Conflicts: right\n", "", true);
}
#[test]
fn reverse_transitive_sibling_conflicts_are_typed_outcomes() {
    check("", "Conflicts: left\n", true);
}
#[test]
fn transitive_sibling_breaks_are_typed_outcomes() {
    check("Breaks: right\n", "", true);
}
#[test]
fn compatible_transitive_siblings_resolve() {
    check("", "", false);
}

fn check(left: &str, right: &str, conflicting: bool) {
    let directory = tempfile::tempdir().unwrap();
    let text = [
        resolution_stanza("root", "1", "amd64", 'a', "Depends: parent\n"),
        resolution_stanza("parent", "1", "amd64", 'b', "Depends: left, right\n"),
        resolution_stanza("left", "1", "amd64", 'c', left),
        resolution_stanza("right", "1", "amd64", 'd', right),
    ]
    .concat();
    let packages = vec![write_resolution_packages(
        directory.path(),
        "ubuntu-main",
        &text,
    )];
    let snapshots = vec![source_snapshot("ubuntu-main", &packages[0])];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 4;
    let package_output = directory.path().join("packages");
    produce_debian_parity_oracle(&profile, &inputs(&snapshots, &packages), &package_output)
        .unwrap();
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut keys = std::collections::BTreeMap::new();
    package_reader
        .for_each_package(|p| {
            keys.insert(p.name.clone(), p.package_key_sha256);
            Ok(())
        })
        .unwrap();
    let output = directory.path().join("resolution");
    produce_debian_resolution_oracle(
        &profile,
        &inputs(&snapshots, &packages),
        &package_output,
        "amd64",
        &output,
    )
    .unwrap();
    let reader =
        verify_native_resolution_oracle_bundle(&output, &profile, &package_reader).unwrap();
    let mut root = None;
    reader
        .for_each_root(|row| {
            if row.root_package_key_sha256 == keys["root"] {
                root = Some(row.outcome);
            }
            Ok(())
        })
        .unwrap();
    let expected = if conflicting {
        NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
        }
    } else {
        let mut closure = keys.values().cloned().collect::<Vec<_>>();
        closure.sort();
        NativeResolutionOutcomeV1::Resolved {
            closure_package_keys_sha256: closure,
        }
    };
    assert_eq!(root, Some(expected));
    let survey = produce_debian_resolution_survey(
        &profile,
        &inputs(&snapshots, &packages),
        &package_output,
        "amd64",
        &directory.path().join("survey.json"),
    )
    .unwrap();
    assert_eq!(survey.counts.roots_walked, 4);
    assert_eq!(survey.counts.failed_roots, 0);
    assert_eq!(survey.counts.unresolved_roots, 0);
    assert_eq!(
        survey.counts.resolved_roots,
        if conflicting { 2 } else { 4 }
    );
    assert_eq!(
        survey.counts.not_installable_roots,
        if conflicting { 2 } else { 0 }
    );
    assert!(survey.failures.is_empty());
    println!(
        "siblings conflicting={conflicting}: 4 roots, {} resolved, {} conflicting_closure, 0 failed",
        survey.counts.resolved_roots, survey.counts.not_installable_roots
    );
}
