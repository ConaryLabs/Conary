// crates/conary-core/src/repository/catalog/parity/tests/candidate_resolution.rs

use std::collections::BTreeSet;
use std::fs;

use super::*;

fn candidate_fixture() -> CandidateFixture {
    let ecosystem = NativeParityEcosystemV1::Rpm;
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
    let mut dependency = package(ecosystem, &members, "dependency", "x86_64", 0, 'c');
    dependency.requirement_groups.clear();
    dependency.provides[0].version = Some("1.0-1".to_string());
    dependency.provides[0].version_relation =
        Some(crate::repository::dependency_model::ProvideVersionRelation::Equal);
    let mut resolved = package(ecosystem, &members, "resolved", "x86_64", 1, 'd');
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
    let mut unresolved = package(ecosystem, &members, "unresolved", "x86_64", 1, 'e');
    unresolved.requirement_groups = vec![requirement("pre_depends", "absent")];
    let content =
        CatalogContentV1::new(scope, evidence, vec![dependency, resolved, unresolved]).unwrap();
    let path = directory.path().join("catalog.sqlite");
    let binding = write_catalog_candidate(&path, &content).unwrap();
    let profile = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile: profile_name(ecosystem).to_string(),
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

fn expected_roots(candidate: &CandidateFixture) -> Vec<NativeResolutionRootV1> {
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

fn write_native_resolution(
    candidate: &CandidateFixture,
    package_oracle: &OracleFixture,
    roots: &[NativeResolutionRootV1],
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = NativeResolutionOracleWriter::create(
        directory.path().join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        &candidate.profile,
        package_oracle.reader.manifest(),
        NativeParityImplementationV1 {
            ecosystem: NativeParityEcosystemV1::Rpm,
            name: "libsolv".to_string(),
            version: "fixture-1.0".to_string(),
            projection_schema: 1,
        },
        NativeResolutionPolicyV1 {
            architecture: "x86_64".to_string(),
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
    let candidate = candidate_fixture();
    let package_oracle = oracle(&candidate, NativeParityEcosystemV1::Rpm, rows(&candidate));
    let roots = expected_roots(&candidate);
    let native = write_native_resolution(&candidate, &package_oracle, &roots);
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

    assert_eq!(produced.manifest.artifact.counts.roots, 3);
    assert_eq!(produced.manifest.artifact.counts.resolved_roots, 2);
    assert_eq!(produced.manifest.artifact.counts.unresolved_roots, 1);
    assert_eq!(
        produced.comparison.counts,
        produced.manifest.artifact.counts
    );
    let reopened =
        verify_native_resolution_oracle_bundle(&output, &candidate.profile, &package_oracle.reader)
            .unwrap();
    assert_eq!(reopened.manifest(), &produced.manifest);
}

#[test]
fn candidate_crawl_rejects_native_closure_drift() {
    let candidate = candidate_fixture();
    let package_oracle = oracle(&candidate, NativeParityEcosystemV1::Rpm, rows(&candidate));
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
    let native = write_native_resolution(&candidate, &package_oracle, &roots);
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
