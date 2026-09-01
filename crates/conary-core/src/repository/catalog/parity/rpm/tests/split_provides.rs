// crates/conary-core/src/repository/catalog/parity/rpm/tests/split_provides.rs

use super::*;

#[test]
fn producer_accepts_split_provide_with_directory_descendant() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, snapshots) = fixture(
        directory.path(),
        false,
        SplitProvideFileCoverage::Descendant,
    );
    let profile = profile(&snapshots);
    let output = directory.path().join("oracle");

    let manifest =
        produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &output).unwrap();

    assert_eq!(manifest.artifact.counts.packages, 3);
    let reader = verify_native_parity_oracle_bundle(&output, &profile).unwrap();
    let mut alpha = None;
    reader
        .for_each_package(|package| {
            if package.name == "alpha" {
                alpha = Some(package);
            }
            Ok(())
        })
        .unwrap();
    let alpha = alpha.unwrap();
    assert!(alpha.provides.iter().any(|provide| {
        provide.capability == "/usr/lib/cmake/cmocka/cmocka-config.cmake" && provide.kind == "file"
    }));
    assert_eq!(
        alpha
            .requirement_groups
            .iter()
            .filter(|group| group.kind == "supplements")
            .count(),
        1,
        "directory split-provides machinery must not become a source supplement"
    );
}

#[test]
fn producer_rejects_split_provide_without_matching_file() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, snapshots) = fixture(directory.path(), false, SplitProvideFileCoverage::Missing);
    let profile = profile(&snapshots);

    let error = produce_rpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ConflictError(_)));
    assert!(
        error.to_string().contains(
            "split-provides supplement 'cmocka-test:/usr/lib/cmake/cmocka' has no matching"
        )
    );
}

#[test]
fn producer_rejects_lexical_prefix_and_foreign_split_provide_files() {
    for coverage in [
        SplitProvideFileCoverage::LexicalPrefix,
        SplitProvideFileCoverage::ForeignDescendant,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (metadata, snapshots) = fixture(directory.path(), false, coverage);
        let profile = profile(&snapshots);

        let error = produce_rpm_parity_oracle(
            &profile,
            &inputs(&snapshots, &metadata),
            &directory.path().join("oracle"),
        )
        .unwrap_err();

        assert!(matches!(error, Error::ConflictError(_)));
        assert!(error.to_string().contains(
            "split-provides supplement 'cmocka-test:/usr/lib/cmake/cmocka' has no matching"
        ));
    }
}

#[test]
fn path_coverage_requires_a_path_component_boundary() {
    let path = "/usr/lib/cmake/cmocka";

    assert!(split_provides_path_is_covered(path, path));
    assert!(split_provides_path_is_covered(
        "/usr/lib/cmake/cmocka/cmocka-config.cmake",
        path,
    ));
    assert!(!split_provides_path_is_covered(
        "/usr/lib/cmake/cmockable/cmocka-config.cmake",
        path,
    ));
    assert!(!split_provides_path_is_covered(
        "/usr/lib/cmake/cmocka/",
        path,
    ));
    for candidate in [
        "/usr/lib/cmake/cmocka/../elsewhere",
        "/usr/lib/cmake/cmocka/./config.cmake",
        "/usr/lib/cmake/cmocka//config.cmake",
    ] {
        assert!(!split_provides_path_is_covered(candidate, path));
    }
}

#[test]
fn shape_rejects_namespace_tree_and_path_mutations() {
    assert_eq!(
        validate_split_provides_shape(
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph-test"),
            Some("/usr/bin/ceph-kvstore-tool"),
        )
        .unwrap(),
        (
            "ceph-test:/usr/bin/ceph-kvstore-tool".to_string(),
            "/usr/bin/ceph-kvstore-tool".to_string(),
        )
    );
    for (namespace, flags, prefix, path) in [
        (
            "namespace:language",
            ffi::REL_WITH,
            Some("ceph-test"),
            Some("/usr/bin/ceph-kvstore-tool"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_AND,
            Some("ceph-test"),
            Some("/usr/bin/ceph-kvstore-tool"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            None,
            Some("/usr/bin/ceph-kvstore-tool"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph-test"),
            None,
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph:test"),
            Some("/usr/bin/ceph-kvstore-tool"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph-test"),
            Some("usr/bin/ceph-kvstore-tool"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph-test"),
            Some("/"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph-test"),
            Some("/usr/lib/cmake/"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph-test"),
            Some("/usr//lib/cmake"),
        ),
        (
            "namespace:splitprovides",
            ffi::REL_WITH,
            Some("ceph-test"),
            Some("/usr/../lib/cmake"),
        ),
    ] {
        assert!(validate_split_provides_shape(namespace, flags, prefix, path).is_err());
    }
}
