// crates/conary-core/src/repository/catalog/parity/tests/candidate_resolution.rs

use std::collections::BTreeSet;
use std::fs;

use super::*;

fn architecture(ecosystem: NativeParityEcosystemV1) -> &'static str {
    match ecosystem {
        NativeParityEcosystemV1::Rpm | NativeParityEcosystemV1::Alpm => "x86_64",
        NativeParityEcosystemV1::Debian => "amd64",
    }
}

pub(super) fn candidate_fixture(ecosystem: NativeParityEcosystemV1) -> CandidateFixture {
    candidate_fixture_with(ecosystem, |_, _| {})
}

pub(super) fn candidate_fixture_with(
    ecosystem: NativeParityEcosystemV1,
    customize: impl FnOnce(&[ProfileSourceMemberV2], &mut Vec<CatalogPackageRecordV1>),
) -> CandidateFixture {
    let directory = tempfile::tempdir().unwrap();
    let members = members(ecosystem);
    let scope = CatalogScopeV1::Profile {
        profile: profile_name(ecosystem).to_string(),
    };
    let evidence = members
        .iter()
        .map(|member| CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: member.ordinal,
            source_identity: member.source_identity.clone(),
            repository_identity: member.repository_identity.clone(),
            source_snapshot_sha256: member.source_snapshot_sha256.clone(),
        })
        .collect();
    let mut dependency = package(
        ecosystem,
        &members,
        "dependency",
        architecture(ecosystem),
        0,
        'c',
    );
    dependency.requirement_groups.clear();
    dependency.provides[0].version = Some("1.0-1".to_string());
    dependency.provides[0].version_relation =
        Some(crate::repository::dependency_model::ProvideVersionRelation::Equal);
    let mut resolved = package(
        ecosystem,
        &members,
        "resolved",
        architecture(ecosystem),
        1,
        'd',
    );
    let versioned = RepositoryRequirementClause::versioned(
        "virtual-dependency".to_string(),
        ">= 1.0-1".to_string(),
    );
    resolved.requirement_groups = vec![
        CatalogRequirementGroupV1 {
            kind: "depends".to_string(),
            behavior: "hard".to_string(),
            description: None,
            native_text: Some("virtual-dependency >= 1.0-1".to_string()),
            expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(
                versioned,
            ))
            .unwrap(),
            atoms: vec![CatalogRequirementAtomV1 {
                capability: "virtual-dependency".to_string(),
                version_constraint: Some(">= 1.0-1".to_string()),
                kind: "virtual".to_string(),
                dependency_type: "runtime".to_string(),
                raw: Some("virtual-dependency >= 1.0-1".to_string()),
            }],
        },
        requirement("optional", "absent-optional"),
        requirement("build", "absent-build"),
    ];
    let mut unresolved = package(
        ecosystem,
        &members,
        "unresolved",
        architecture(ecosystem),
        1,
        'e',
    );
    unresolved.requirement_groups = vec![requirement("pre_depends", "absent")];
    let mut packages = vec![dependency, resolved, unresolved];
    customize(&members, &mut packages);
    let content = CatalogContentV1::new(scope, evidence, packages).unwrap();
    let path = directory.path().join("catalog.sqlite");
    let binding = write_catalog_candidate(&path, &content).unwrap();
    let profile = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V3,
        profile: profile_name(ecosystem).to_string(),
        target_architecture: crate::repository::supported_profiles::profile_by_public_id(
            profile_name(ecosystem),
        )
        .unwrap()
        .target_architecture(),
        projection_version: 1,
        members,
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    };
    profile.validate().unwrap();
    let reader = CatalogReader::open_verified(&path, &binding).unwrap();
    CandidateFixture {
        _directory: directory,
        profile,
        reader,
    }
}

pub(super) fn expected_roots(candidate: &CandidateFixture) -> Vec<NativeResolutionRootV1> {
    let packages = rows(candidate);
    let dependency = packages
        .iter()
        .find(|package| package.name == "dependency")
        .unwrap();
    let resolved = packages
        .iter()
        .find(|package| package.name == "resolved")
        .unwrap();
    let unresolved = packages
        .iter()
        .find(|package| package.name == "unresolved")
        .unwrap();
    let missing_group = unresolved.requirement_groups.first().unwrap();
    let mut roots = vec![
        NativeResolutionRootV1 {
            root_package_key_sha256: dependency.package_key_sha256.clone(),
            outcome: NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: vec![dependency.package_key_sha256.clone()],
            },
        },
        NativeResolutionRootV1 {
            root_package_key_sha256: resolved.package_key_sha256.clone(),
            outcome: NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: BTreeSet::from([
                    dependency.package_key_sha256.clone(),
                    resolved.package_key_sha256.clone(),
                ])
                .into_iter()
                .collect(),
            },
        },
        NativeResolutionRootV1 {
            root_package_key_sha256: unresolved.package_key_sha256.clone(),
            outcome: NativeResolutionOutcomeV1::Unresolved {
                dependencies: vec![NativeUnresolvedDependencyV1 {
                    requiring_package_key_sha256: unresolved.package_key_sha256.clone(),
                    requirement_group_sha256: native_requirement_group_sha256(missing_group)
                        .unwrap(),
                }],
            },
        },
    ];
    roots.sort_by(|left, right| {
        left.root_package_key_sha256
            .cmp(&right.root_package_key_sha256)
    });
    roots
}

pub(super) fn write_native_resolution(
    candidate: &CandidateFixture,
    package_oracle: &OracleFixture,
    ecosystem: NativeParityEcosystemV1,
    roots: &[NativeResolutionRootV1],
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = NativeResolutionOracleWriter::create(
        directory.path().join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        &candidate.profile,
        package_oracle.reader.manifest(),
        NativeParityImplementationV1 {
            ecosystem,
            name: match ecosystem {
                NativeParityEcosystemV1::Rpm => "libsolv",
                NativeParityEcosystemV1::Debian => "apt-pkg",
                NativeParityEcosystemV1::Alpm => "libalpm",
            }
            .to_string(),
            version: "fixture-1.0".to_string(),
            projection_schema: 1,
        },
        NativeResolutionPolicyV1 {
            architecture: architecture(ecosystem).to_string(),
            architecture_admission: NativeResolutionArchitectureAdmissionV1::NativeOnly,
            installed_state: NativeResolutionInstalledStateV1::Empty,
            roots: NativeResolutionRootPolicyV1::EveryExactPackage,
            positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
            provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
        },
    )
    .unwrap();
    for root in roots {
        writer.root(root).unwrap();
    }
    let manifest = writer.finish().unwrap();
    write_native_resolution_oracle_manifest(directory.path(), &manifest).unwrap();
    verify_native_resolution_oracle_bundle(
        directory.path(),
        &candidate.profile,
        &package_oracle.reader,
    )
    .unwrap();
    directory
}

#[test]
fn complete_candidate_crawl_reopens_and_matches_native_resolution() {
    for ecosystem in [
        NativeParityEcosystemV1::Rpm,
        NativeParityEcosystemV1::Debian,
        NativeParityEcosystemV1::Alpm,
    ] {
        let candidate = candidate_fixture(ecosystem);
        let package_oracle = oracle(&candidate, ecosystem, rows(&candidate));
        let roots = expected_roots(&candidate);
        let native = write_native_resolution(&candidate, &package_oracle, ecosystem, &roots);
        let output_parent = tempfile::tempdir().unwrap();
        let output = output_parent.path().join("candidate-resolution");

        let produced = produce_conary_resolution_candidate(
            &candidate.profile,
            &candidate.reader,
            package_oracle._directory.path(),
            native.path(),
            architecture(ecosystem),
            &output,
        )
        .unwrap();

        assert_eq!(produced.manifest.artifact.counts.roots, 3);
        assert_eq!(produced.manifest.artifact.counts.resolved_roots, 2);
        assert_eq!(produced.manifest.artifact.counts.unresolved_roots, 1);
        assert_eq!(
            produced.comparison.counts,
            produced.manifest.artifact.counts
        );
        let reopened = verify_native_resolution_oracle_bundle(
            &output,
            &candidate.profile,
            &package_oracle.reader,
        )
        .unwrap();
        assert_eq!(reopened.manifest(), &produced.manifest);
    }
}

#[test]
fn serial_and_parallel_candidate_outputs_are_byte_identical() {
    let _capacity = crate::repository::catalog::parity::resolution_test_capacity(2);
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let candidate = candidate_fixture(ecosystem);
    let package_oracle = oracle(&candidate, ecosystem, rows(&candidate));
    let native = write_native_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        &expected_roots(&candidate),
    );
    let outputs = tempfile::tempdir().unwrap();
    let serial = outputs.path().join("serial");
    let parallel = outputs.path().join("parallel");
    let one = ResolutionWorkerRequest::explicit(ResolutionWorkerCount::new(1).unwrap());
    let two = ResolutionWorkerRequest::explicit(ResolutionWorkerCount::new(2).unwrap());

    produce_conary_resolution_candidate_with_workers(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        native.path(),
        architecture(ecosystem),
        &serial,
        one,
    )
    .unwrap();
    produce_conary_resolution_candidate_with_workers(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        native.path(),
        architecture(ecosystem),
        &parallel,
        two,
    )
    .unwrap();
    for name in [
        NATIVE_RESOLUTION_ROOT_FILE_NAME,
        NATIVE_RESOLUTION_MANIFEST_FILE_NAME,
    ] {
        assert_eq!(
            fs::read(serial.join(name)).unwrap(),
            fs::read(parallel.join(name)).unwrap(),
            "candidate bundle byte drift in {name}"
        );
    }

    let serial_survey = outputs.path().join("serial-survey.json");
    let parallel_survey = outputs.path().join("parallel-survey.json");
    produce_conary_resolution_survey_with_workers(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        architecture(ecosystem),
        &serial_survey,
        one,
    )
    .unwrap();
    produce_conary_resolution_survey_with_workers(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        architecture(ecosystem),
        &parallel_survey,
        two,
    )
    .unwrap();
    assert_eq!(
        fs::read(serial_survey).unwrap(),
        fs::read(parallel_survey).unwrap(),
        "candidate survey JSON changed with worker scheduling"
    );
}

#[test]
fn candidate_producer_rejects_operator_architecture_before_writing_roots() {
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let candidate = candidate_fixture(ecosystem);
    let package_oracle = oracle(&candidate, ecosystem, rows(&candidate));
    let native = write_native_resolution(
        &candidate,
        &package_oracle,
        ecosystem,
        &expected_roots(&candidate),
    );
    let output_parent = tempfile::tempdir().unwrap();
    let output = output_parent.path().join("candidate-resolution");

    let error = produce_conary_resolution_candidate(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        native.path(),
        "aarch64",
        &output,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        crate::Error::ProfileArchitectureMismatch { .. }
    ));
    assert!(!output.exists());
}

#[test]
fn candidate_crawl_rejects_native_closure_drift() {
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let candidate = candidate_fixture(ecosystem);
    let package_oracle = oracle(&candidate, ecosystem, rows(&candidate));
    let mut roots = expected_roots(&candidate);
    let resolved = roots
        .iter_mut()
        .find(|root| {
            matches!(
                &root.outcome,
                NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256
                } if closure_package_keys_sha256.len() == 2
            )
        })
        .unwrap();
    let NativeResolutionOutcomeV1::Resolved {
        closure_package_keys_sha256,
    } = &mut resolved.outcome
    else {
        unreachable!()
    };
    closure_package_keys_sha256.retain(|key| key == &resolved.root_package_key_sha256);
    let native = write_native_resolution(&candidate, &package_oracle, ecosystem, &roots);
    let output_parent = tempfile::tempdir().unwrap();
    let output = output_parent.path().join("candidate-resolution");

    let error = produce_conary_resolution_candidate(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        native.path(),
        "x86_64",
        &output,
    )
    .unwrap_err();
    assert!(error.to_string().contains("DependencyClosure"));
    assert!(fs::metadata(output.join(NATIVE_RESOLUTION_ROOT_FILE_NAME)).is_ok());
}

#[test]
fn candidate_crawl_excludes_foreign_root_and_projects_its_provider_edge() {
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let candidate = candidate_fixture_with(ecosystem, |members, packages| {
        let mut provider = package(ecosystem, members, "cross-provider", "i686", 0, 'f');
        provider.requirement_groups.clear();
        let mut root = package(ecosystem, members, "cross-provider-root", "x86_64", 1, 'g');
        root.requirement_groups = vec![requirement("depends", "virtual-cross-provider")];
        packages.extend([provider, root]);
    });
    let package_rows = rows(&candidate);
    let package_oracle = oracle(&candidate, ecosystem, package_rows.clone());
    let provider = package_rows
        .iter()
        .find(|package| package.name == "cross-provider")
        .unwrap();
    let root = package_rows
        .iter()
        .find(|package| package.name == "cross-provider-root")
        .unwrap();
    let mut expected = expected_roots(&candidate);
    expected.push(NativeResolutionRootV1 {
        root_package_key_sha256: provider.package_key_sha256.clone(),
        outcome: NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ArchitectureExcluded,
        },
    });
    expected.push(NativeResolutionRootV1 {
        root_package_key_sha256: root.package_key_sha256.clone(),
        outcome: NativeResolutionOutcomeV1::Unresolved {
            dependencies: vec![NativeUnresolvedDependencyV1 {
                requiring_package_key_sha256: root.package_key_sha256.clone(),
                requirement_group_sha256: native_requirement_group_sha256(
                    root.requirement_groups.first().unwrap(),
                )
                .unwrap(),
            }],
        },
    });
    expected.sort_by(|left, right| {
        left.root_package_key_sha256
            .cmp(&right.root_package_key_sha256)
    });
    let excluded_position = expected
        .iter()
        .position(|candidate| candidate.root_package_key_sha256 == provider.package_key_sha256)
        .unwrap();
    assert!(excluded_position + 1 < expected.len());
    let native = write_native_resolution(&candidate, &package_oracle, ecosystem, &expected);
    let output_parent = tempfile::tempdir().unwrap();
    let output = output_parent.path().join("candidate-resolution");

    let produced = produce_conary_resolution_candidate(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        native.path(),
        "x86_64",
        &output,
    )
    .unwrap();

    assert_eq!(produced.manifest.artifact.counts.roots, 5);
    assert_eq!(produced.manifest.artifact.counts.not_installable_roots, 1);
    assert_eq!(produced.manifest.artifact.counts.unresolved_roots, 2);
    assert_eq!(
        produced.comparison.counts,
        produced.manifest.artifact.counts
    );
}

#[test]
fn candidate_survey_walks_conflict_missing_architecture_and_healthy_roots() {
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let candidate = candidate_fixture_with(ecosystem, |members, packages| {
        let mut provider = package(ecosystem, members, "conflict-root", "x86_64", 0, 'f');
        provider.version = "2.0-1".to_string();
        provider.package_release = "2".to_string();
        provider.requirement_groups.clear();
        let capability = provider.provides[0].capability.clone();
        let mut conflict = package(ecosystem, members, "conflict-root", "x86_64", 1, 'g');
        conflict.provides[0].capability = "virtual-conflict-root-old".to_string();
        conflict.requirement_groups = vec![requirement("depends", &capability)];
        let mut excluded = package(ecosystem, members, "foreign-root", "i686", 1, 'h');
        excluded.requirement_groups.clear();
        packages.extend([provider, conflict, excluded]);
    });
    let package_rows = rows(&candidate);
    let package_oracle = oracle(&candidate, ecosystem, package_rows.clone());
    let output_parent = tempfile::tempdir().unwrap();
    let survey_path = output_parent.path().join("candidate-survey.json");

    let survey = produce_conary_resolution_survey(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        "x86_64",
        &survey_path,
    )
    .unwrap();

    assert_eq!(survey.counts.roots_walked, package_rows.len() as u64);
    assert_eq!(survey.counts.failed_roots, 1);
    assert_eq!(survey.counts.unresolved_roots, 1);
    assert_eq!(survey.counts.not_installable_roots, 1);
    assert_eq!(survey.counts.resolved_roots, 3);
    assert_eq!(survey.outcomes.len(), 5);
    assert_eq!(survey.failures.len(), 1);
    assert!(matches!(
        survey
            .outcomes
            .iter()
            .find(|root| root.name == "unresolved")
            .unwrap()
            .outcome,
        NativeResolutionOutcomeV1::Unresolved { .. }
    ));
    assert!(matches!(
        survey
            .outcomes
            .iter()
            .find(|root| root.name == "foreign-root")
            .unwrap()
            .outcome,
        NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ArchitectureExcluded
        }
    ));
    assert_eq!(survey.failures[0].name, "conflict-root");
    assert_eq!(
        survey.failures[0].error_kind.reason,
        ConaryResolutionSurveyErrorReasonV1::SolverFailed
    );
    let ConaryResolutionSurveyNativeExplanationV1::ResolvoConflictGraph {
        unresolved_edges,
        conflict_edges,
        excluded_nodes: _,
    } = &survey.failures[0].native_explanation
    else {
        panic!("SAT failure must retain typed resolvo graph evidence");
    };
    assert!(unresolved_edges.is_empty());
    assert!(!conflict_edges.is_empty());
    assert!(conflict_edges.iter().all(|edge| {
        edge.from.package_key_sha256.is_some() && edge.to.package_key_sha256.is_some()
    }));
    assert!(survey_path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&survey_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(write_conary_resolution_survey(&survey_path, &survey).is_err());
    assert!(!output_parent.path().join("manifest.json").exists());
    assert!(!output_parent.path().join("roots.jsonl").exists());

    let native_roots = package_rows
        .iter()
        .map(|package| NativeResolutionRootV1 {
            root_package_key_sha256: package.package_key_sha256.clone(),
            outcome: NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: vec![package.package_key_sha256.clone()],
            },
        })
        .collect::<Vec<_>>();
    let native = write_native_resolution(&candidate, &package_oracle, ecosystem, &native_roots);
    let strict_output = output_parent.path().join("strict-candidate");
    let error = produce_conary_resolution_candidate(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        native.path(),
        "x86_64",
        &strict_output,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        concat!(
            "Conflict: exact repository package 6 is unsatisfiable without a missing typed dependency: The following packages are incompatible\n",
            "└─ conflict-root repository package 6 cannot be installed because there are no viable options:\n",
            "   └─ conflict-root conflict-root=1.0-1 would require\n",
            "      └─ virtual-conflict-root Any, which cannot be installed because there are no viable options:\n",
            "         └─ conflict-root conflict-root=2.0-1, which conflicts with the versions reported above.\n"
        )
    );
    assert!(
        !strict_output
            .join(NATIVE_RESOLUTION_MANIFEST_FILE_NAME)
            .exists()
    );
}

#[test]
fn candidate_survey_rejects_architecture_before_creating_output() {
    let ecosystem = NativeParityEcosystemV1::Rpm;
    let candidate = candidate_fixture(ecosystem);
    let package_oracle = oracle(&candidate, ecosystem, rows(&candidate));
    let output = candidate._directory.path().join("survey.json");
    let error = produce_conary_resolution_survey(
        &candidate.profile,
        &candidate.reader,
        package_oracle._directory.path(),
        "aarch64",
        &output,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        crate::Error::ProfileArchitectureMismatch { .. }
    ));
    assert!(!output.exists());
}
