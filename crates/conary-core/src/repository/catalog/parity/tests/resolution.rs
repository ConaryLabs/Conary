// crates/conary-core/src/repository/catalog/parity/tests/resolution.rs

use std::fs;

use super::*;

struct ResolutionFixture {
    _directory: tempfile::TempDir,
    reader: NativeResolutionOracleReader,
}

fn policy(ecosystem: NativeParityEcosystemV1) -> NativeResolutionPolicyV1 {
    NativeResolutionPolicyV1 {
        architecture: match ecosystem {
            NativeParityEcosystemV1::Rpm | NativeParityEcosystemV1::Alpm => "x86_64",
            NativeParityEcosystemV1::Debian => "amd64",
        }
        .to_string(),
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    }
}

fn solver_implementation(
    ecosystem: NativeParityEcosystemV1,
    name: &str,
) -> NativeParityImplementationV1 {
    NativeParityImplementationV1 {
        ecosystem,
        name: name.to_string(),
        version: "fixture-1.0".to_string(),
        projection_schema: 1,
    }
}

fn root_rows(packages: &[NativeParityPackageV1]) -> Vec<NativeResolutionRootV1> {
    let mut packages = packages.to_vec();
    packages.sort_by(|left, right| left.package_key_sha256.cmp(&right.package_key_sha256));
    let all_keys = packages
        .iter()
        .map(|package| package.package_key_sha256.clone())
        .collect::<Vec<_>>();
    let unresolved = packages.last().unwrap();
    let group = unresolved
        .requirement_groups
        .iter()
        .find(|group| group.kind == "depends")
        .unwrap();
    vec![
        NativeResolutionRootV1 {
            root_package_key_sha256: packages[0].package_key_sha256.clone(),
            outcome: NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: all_keys,
            },
        },
        NativeResolutionRootV1 {
            root_package_key_sha256: unresolved.package_key_sha256.clone(),
            outcome: NativeResolutionOutcomeV1::Unresolved {
                dependencies: vec![NativeUnresolvedDependencyV1 {
                    requiring_package_key_sha256: unresolved.package_key_sha256.clone(),
                    requirement_group_sha256: native_requirement_group_sha256(group).unwrap(),
                }],
            },
        },
    ]
}

fn write_resolution(
    candidate: &CandidateFixture,
    package_oracle: &OracleFixture,
    ecosystem: NativeParityEcosystemV1,
    implementation_name: &str,
    roots: &[NativeResolutionRootV1],
    verify_complete: bool,
) -> ResolutionFixture {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(NATIVE_RESOLUTION_ROOT_FILE_NAME);
    let mut writer = NativeResolutionOracleWriter::create(
        &path,
        &candidate.profile,
        package_oracle.reader.manifest(),
        solver_implementation(ecosystem, implementation_name),
        policy(ecosystem),
    )
    .unwrap();
    for root in roots {
        writer.root(root).unwrap();
    }
    let manifest = writer.finish().unwrap();
    write_native_resolution_oracle_manifest(directory.path(), &manifest).unwrap();
    let reader = if verify_complete {
        verify_native_resolution_oracle_bundle(
            directory.path(),
            &candidate.profile,
            &package_oracle.reader,
        )
        .unwrap()
    } else {
        NativeResolutionOracleReader::open_verified(&path, &manifest).unwrap()
    };
    ResolutionFixture {
        _directory: directory,
        reader,
    }
}

fn fixtures(
    ecosystem: NativeParityEcosystemV1,
) -> (CandidateFixture, OracleFixture, Vec<NativeResolutionRootV1>) {
    let candidate = candidate(ecosystem);
    let packages = rows(&candidate);
    let roots = root_rows(&packages);
    let package_oracle = oracle(&candidate, ecosystem, packages);
    (candidate, package_oracle, roots)
}

#[test]
fn all_ecosystems_reopen_and_compare_exact_resolution_evidence() {
    for ecosystem in [
        NativeParityEcosystemV1::Rpm,
        NativeParityEcosystemV1::Debian,
        NativeParityEcosystemV1::Alpm,
    ] {
        let (candidate, package_oracle, roots) = fixtures(ecosystem);
        let native = write_resolution(
            &candidate,
            &package_oracle,
            ecosystem,
            "native-solver",
            &roots,
            true,
        );
        let conary = write_resolution(
            &candidate,
            &package_oracle,
            ecosystem,
            "conary-resolvo",
            &roots,
            true,
        );
        let comparison = compare_native_resolution_oracle(
            &candidate.profile,
            &package_oracle.reader,
            &native.reader,
            &conary.reader,
        )
        .unwrap();
        assert_eq!(comparison.profile, profile_name(ecosystem));
        assert_eq!(comparison.counts.roots, 2);
        assert_eq!(comparison.counts.resolved_roots, 1);
        assert_eq!(comparison.counts.unresolved_roots, 1);
    }
}

#[test]
fn closure_unresolved_and_outcome_drift_are_typed() {
    let ecosystem = NativeParityEcosystemV1::Debian;
    let (candidate, package_oracle, roots) = fixtures(ecosystem);
    let native = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "apt-pkg",
        &roots,
        true,
    );

    let mut changed = roots.clone();
    let changed_root_key = changed[0].root_package_key_sha256.clone();
    let NativeResolutionOutcomeV1::Resolved {
        closure_package_keys_sha256,
    } = &mut changed[0].outcome
    else {
        unreachable!()
    };
    closure_package_keys_sha256.retain(|key| key == &changed_root_key);
    let conary = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "conary-resolvo",
        &changed,
        true,
    );
    let error = compare_native_resolution_oracle(
        &candidate.profile,
        &package_oracle.reader,
        &native.reader,
        &conary.reader,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        NativeResolutionComparisonError::Mismatch(mismatch)
            if matches!(mismatch.as_ref(), NativeResolutionMismatchV1::DependencyClosure { .. })
    ));

    let mut changed = roots.clone();
    let first_package = rows(&candidate)
        .into_iter()
        .min_by(|left, right| left.package_key_sha256.cmp(&right.package_key_sha256))
        .unwrap();
    let first_group = first_package
        .requirement_groups
        .iter()
        .find(|group| group.kind == "depends")
        .unwrap();
    changed[1].outcome = NativeResolutionOutcomeV1::Unresolved {
        dependencies: vec![NativeUnresolvedDependencyV1 {
            requiring_package_key_sha256: first_package.package_key_sha256,
            requirement_group_sha256: native_requirement_group_sha256(first_group).unwrap(),
        }],
    };
    let conary = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "conary-resolvo",
        &changed,
        true,
    );
    let error = compare_native_resolution_oracle(
        &candidate.profile,
        &package_oracle.reader,
        &native.reader,
        &conary.reader,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        NativeResolutionComparisonError::Mismatch(mismatch)
            if matches!(
                mismatch.as_ref(),
                NativeResolutionMismatchV1::UnresolvedDependencies { .. }
            )
    ));

    let mut changed = roots.clone();
    changed[1].outcome = NativeResolutionOutcomeV1::Resolved {
        closure_package_keys_sha256: vec![changed[1].root_package_key_sha256.clone()],
    };
    let conary = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "conary-resolvo",
        &changed,
        true,
    );
    let error = compare_native_resolution_oracle(
        &candidate.profile,
        &package_oracle.reader,
        &native.reader,
        &conary.reader,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        NativeResolutionComparisonError::Mismatch(mismatch)
            if matches!(mismatch.as_ref(), NativeResolutionMismatchV1::ResolutionOutcome { .. })
    ));
}

#[test]
fn missing_and_extra_roots_are_typed_before_complete_coverage_rejection() {
    let ecosystem = NativeParityEcosystemV1::Alpm;
    let (candidate, package_oracle, roots) = fixtures(ecosystem);
    let native = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "libalpm",
        &roots,
        true,
    );

    let missing = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "conary-resolvo",
        &roots[..1],
        false,
    );
    let error = compare_native_resolution_oracle(
        &candidate.profile,
        &package_oracle.reader,
        &native.reader,
        &missing.reader,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        NativeResolutionComparisonError::Mismatch(mismatch)
            if matches!(mismatch.as_ref(), NativeResolutionMismatchV1::OracleOnlyRoot { .. })
    ));

    let mut extra_roots = roots.clone();
    extra_roots.push(NativeResolutionRootV1 {
        root_package_key_sha256: digest('f'),
        outcome: NativeResolutionOutcomeV1::Resolved {
            closure_package_keys_sha256: vec![digest('f')],
        },
    });
    extra_roots.sort_by(|left, right| {
        left.root_package_key_sha256
            .cmp(&right.root_package_key_sha256)
    });
    let extra = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "conary-resolvo",
        &extra_roots,
        false,
    );
    let error = compare_native_resolution_oracle(
        &candidate.profile,
        &package_oracle.reader,
        &native.reader,
        &extra.reader,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        NativeResolutionComparisonError::Mismatch(mismatch)
            if matches!(mismatch.as_ref(), NativeResolutionMismatchV1::CandidateOnlyRoot { .. })
    ));
}

#[test]
fn comparison_rejects_policy_drift_before_accepting_equal_rows() {
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let (candidate, package_oracle, roots) = fixtures(ecosystem);
    let native = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "libdnf5",
        &roots,
        true,
    );

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(NATIVE_RESOLUTION_ROOT_FILE_NAME);
    let mut changed_policy = policy(ecosystem);
    changed_policy.architecture = "aarch64".to_string();
    let mut writer = NativeResolutionOracleWriter::create(
        &path,
        &candidate.profile,
        package_oracle.reader.manifest(),
        solver_implementation(ecosystem, "conary-resolvo"),
        changed_policy,
    )
    .unwrap();
    for root in &roots {
        writer.root(root).unwrap();
    }
    let manifest = writer.finish().unwrap();
    let changed = NativeResolutionOracleReader::open_verified(&path, &manifest).unwrap();
    let error = compare_native_resolution_oracle(
        &candidate.profile,
        &package_oracle.reader,
        &native.reader,
        &changed,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        NativeResolutionComparisonError::Candidate(_)
    ));
}

#[test]
fn reopen_rejects_tamper_count_drift_unknown_references_and_package_oracle_drift() {
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let (candidate, package_oracle, roots) = fixtures(ecosystem);
    let resolution = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "libdnf5",
        &roots,
        true,
    );
    fs::write(
        resolution.reader.path(),
        [
            fs::read(resolution.reader.path()).unwrap(),
            b"tamper".to_vec(),
        ]
        .concat(),
    )
    .unwrap();
    assert!(
        NativeResolutionOracleReader::open_verified(
            resolution.reader.path(),
            resolution.reader.manifest()
        )
        .is_err()
    );

    let bad_counts = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "libdnf5",
        &roots,
        true,
    );
    let mut bad_manifest = bad_counts.reader.manifest().clone();
    bad_manifest.artifact.counts.closure_package_references += 1;
    let bad_reader =
        NativeResolutionOracleReader::open_verified(bad_counts.reader.path(), &bad_manifest)
            .unwrap();
    assert!(bad_reader.verify_contents().is_err());

    let mut absent_reference = roots.clone();
    absent_reference[0].outcome = NativeResolutionOutcomeV1::Resolved {
        closure_package_keys_sha256: vec![
            absent_reference[0].root_package_key_sha256.clone(),
            digest('f'),
        ],
    };
    if let NativeResolutionOutcomeV1::Resolved {
        closure_package_keys_sha256,
    } = &mut absent_reference[0].outcome
    {
        closure_package_keys_sha256.sort();
    }
    let absent = write_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        "libdnf5",
        &absent_reference,
        false,
    );
    assert!(
        absent
            .reader
            .verify_package_oracle(&package_oracle.reader)
            .is_err()
    );

    let mut other_rows = rows(&candidate);
    other_rows[0].checksum = digest('e');
    let other_oracle = oracle(&candidate, ecosystem, other_rows);
    assert!(
        resolution
            .reader
            .verify_package_oracle(&other_oracle.reader)
            .is_err()
    );

    let unknown = serde_json::json!({
        "root_package_key_sha256": digest('a'),
        "outcome": {"status": "resolved", "closure_package_keys_sha256": [digest('a')]},
        "extra": true
    });
    assert!(serde_json::from_value::<NativeResolutionRootV1>(unknown).is_err());
    let mixed = serde_json::json!({
        "root_package_key_sha256": digest('a'),
        "outcome": {
            "status": "resolved",
            "closure_package_keys_sha256": [digest('a')],
            "dependencies": []
        }
    });
    assert!(serde_json::from_value::<NativeResolutionRootV1>(mixed).is_err());
}

#[test]
fn row_validation_rejects_duplicates_mixed_empty_and_reordered_authority() {
    let root = digest('a');
    assert!(
        NativeResolutionRootV1 {
            root_package_key_sha256: root.clone(),
            outcome: NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: Vec::new(),
            },
        }
        .validate()
        .is_err()
    );
    assert!(
        NativeResolutionRootV1 {
            root_package_key_sha256: root.clone(),
            outcome: NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: vec![root.clone(), root.clone()],
            },
        }
        .validate()
        .is_err()
    );
    assert!(
        NativeResolutionRootV1 {
            root_package_key_sha256: root.clone(),
            outcome: NativeResolutionOutcomeV1::Unresolved {
                dependencies: Vec::new(),
            },
        }
        .validate()
        .is_err()
    );

    let (candidate, package_oracle, roots) = fixtures(NativeParityEcosystemV1::Debian);
    let directory = tempfile::tempdir().unwrap();
    let mut writer = NativeResolutionOracleWriter::create(
        directory.path().join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        &candidate.profile,
        package_oracle.reader.manifest(),
        solver_implementation(NativeParityEcosystemV1::Debian, "apt-pkg"),
        policy(NativeParityEcosystemV1::Debian),
    )
    .unwrap();
    writer.root(&roots[1]).unwrap();
    assert!(writer.root(&roots[0]).is_err());
    assert!(writer.root(&roots[1]).is_err());
}

#[test]
fn canonical_writer_and_reopener_scale_by_rows_without_retaining_all_roots() {
    const ROOTS: u64 = 16_384;

    let ecosystem = NativeParityEcosystemV1::Alpm;
    let (candidate, package_oracle, _) = fixtures(ecosystem);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(NATIVE_RESOLUTION_ROOT_FILE_NAME);
    let mut writer = NativeResolutionOracleWriter::create(
        &path,
        &candidate.profile,
        package_oracle.reader.manifest(),
        solver_implementation(ecosystem, "libalpm"),
        policy(ecosystem),
    )
    .unwrap();
    for index in 0..ROOTS {
        let key = format!("{index:064x}");
        writer
            .root(&NativeResolutionRootV1 {
                root_package_key_sha256: key.clone(),
                outcome: NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256: vec![key],
                },
            })
            .unwrap();
    }
    let manifest = writer.finish().unwrap();
    assert_eq!(manifest.artifact.counts.roots, ROOTS);
    assert_eq!(manifest.artifact.counts.closure_package_references, ROOTS);
    let reader = NativeResolutionOracleReader::open_verified(&path, &manifest).unwrap();
    let mut visited = 0_u64;
    reader
        .for_each_root(|_| {
            visited += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(visited, ROOTS);
}
