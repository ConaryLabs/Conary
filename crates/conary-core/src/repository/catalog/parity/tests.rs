// crates/conary-core/src/repository/catalog/parity/tests.rs

use std::fs;

use super::*;
use crate::repository::catalog::{
    CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogProvideRecordV1,
    CatalogReader, CatalogRequirementAtomV1, CatalogRequirementGroupV1, CatalogScopeV1,
    CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V2, ProfileRevisionV2, ProfileSourceMemberV2,
    SourceStreamKindV1, SourceStreamV1, write_catalog_candidate,
};
use crate::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, RepositoryRequirementClause,
    RepositoryRequirementExpression,
};
use crate::repository::dependency_source::CapabilityProvenance;
use crate::repository::supported_profiles::ProfileSourceRole;
use crate::repository::versioning::VersionScheme;

struct CandidateFixture {
    _directory: tempfile::TempDir,
    profile: ProfileRevisionV2,
    reader: CatalogReader,
}

struct OracleFixture {
    _directory: tempfile::TempDir,
    reader: NativeParityOracleReader,
}

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn profile_name(ecosystem: NativeParityEcosystemV1) -> &'static str {
    match ecosystem {
        NativeParityEcosystemV1::Rpm => "fedora-44",
        NativeParityEcosystemV1::Debian => "ubuntu-26.04",
        NativeParityEcosystemV1::Alpm => "arch",
    }
}

fn implementation(ecosystem: NativeParityEcosystemV1) -> NativeParityImplementationV1 {
    let name = match ecosystem {
        NativeParityEcosystemV1::Rpm => "libdnf5",
        NativeParityEcosystemV1::Debian => "apt-pkg",
        NativeParityEcosystemV1::Alpm => "libalpm",
    };
    NativeParityImplementationV1 {
        ecosystem,
        name: name.to_string(),
        version: "fixture-1.0".to_string(),
        projection_schema: 1,
    }
}

fn members(ecosystem: NativeParityEcosystemV1) -> Vec<ProfileSourceMemberV2> {
    let source = match ecosystem {
        NativeParityEcosystemV1::Rpm => "fedora-project",
        NativeParityEcosystemV1::Debian => "ubuntu-archive",
        NativeParityEcosystemV1::Alpm => "archlinux",
    };
    [
        (ProfileSourceRole::Base, 100, "base", 'a'),
        (ProfileSourceRole::Updates, 90, "updates", 'b'),
    ]
    .into_iter()
    .enumerate()
    .map(
        |(ordinal, (role, precedence, repository, snapshot))| ProfileSourceMemberV2 {
            ordinal: u32::try_from(ordinal).unwrap(),
            role,
            source_identity: source.to_string(),
            repository_identity: format!("{}-{repository}", profile_name(ecosystem)),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "fixture".to_string(),
            },
            precedence,
            required: true,
            source_snapshot_sha256: digest(snapshot),
        },
    )
    .collect()
}

fn requirement(kind: &str, capability: &str) -> CatalogRequirementGroupV1 {
    let clause = RepositoryRequirementClause::name_only(capability.to_string());
    CatalogRequirementGroupV1 {
        kind: kind.to_string(),
        behavior: "hard".to_string(),
        description: Some(format!("{kind} fixture")),
        native_text: Some(capability.to_string()),
        expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(clause))
            .unwrap(),
        atoms: vec![CatalogRequirementAtomV1 {
            capability: capability.to_string(),
            version_constraint: None,
            kind: "package".to_string(),
            dependency_type: "runtime".to_string(),
            raw: Some(capability.to_string()),
        }],
    }
}

fn package(
    ecosystem: NativeParityEcosystemV1,
    members: &[ProfileSourceMemberV2],
    name: &str,
    architecture: &str,
    member_ordinal: usize,
    checksum: char,
) -> CatalogPackageRecordV1 {
    let member = &members[member_ordinal];
    let extension = match ecosystem {
        NativeParityEcosystemV1::Rpm => "rpm",
        NativeParityEcosystemV1::Debian => "deb",
        NativeParityEcosystemV1::Alpm => "pkg.tar.zst",
    };
    let scheme = ecosystem.version_scheme();
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Profile {
            member_ordinal: member.ordinal,
            source_identity: member.source_identity.clone(),
            repository_identity: member.repository_identity.clone(),
            source_snapshot_sha256: member.source_snapshot_sha256.clone(),
        },
        source_profile: profile_name(ecosystem).to_string(),
        name: name.to_string(),
        version: "1.0-1".to_string(),
        package_release: "1".to_string(),
        architecture: Some(architecture.to_string()),
        debian_multi_arch: (scheme == VersionScheme::Debian).then_some(DebianMultiArch::No),
        description: Some(format!("{name} presentation text excluded from parity")),
        checksum: digest(checksum),
        size: 4096,
        download_url: format!("https://packages.example/{name}.{extension}"),
        metadata: Some("{}".to_string()),
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: scheme,
        provides: vec![CatalogProvideRecordV1 {
            capability: format!("virtual-{name}"),
            version: None,
            version_relation: None,
            kind: "virtual".to_string(),
            raw: Some(format!("virtual-{name}")),
            version_scheme: scheme,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::AuthorDeclared,
        }],
        requirement_groups: vec![
            requirement("depends", &format!("runtime-{name}")),
            requirement("conflict", &format!("conflict-{name}")),
            requirement("breaks", &format!("breaks-{name}")),
            requirement("replace", &format!("replace-{name}")),
            requirement("obsolete", &format!("obsolete-{name}")),
        ],
    }
}

fn candidate(ecosystem: NativeParityEcosystemV1) -> CandidateFixture {
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
    let architectures = match ecosystem {
        NativeParityEcosystemV1::Rpm => ("x86_64", "noarch"),
        NativeParityEcosystemV1::Debian => ("amd64", "all"),
        NativeParityEcosystemV1::Alpm => ("x86_64", "any"),
    };
    let content = CatalogContentV1::new(
        scope.clone(),
        evidence,
        vec![
            package(ecosystem, &members, "tool", architectures.0, 1, 'c'),
            package(ecosystem, &members, "library", architectures.1, 0, 'd'),
        ],
    )
    .unwrap();
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

fn oracle(
    candidate: &CandidateFixture,
    ecosystem: NativeParityEcosystemV1,
    mut rows: Vec<NativeParityPackageV1>,
) -> OracleFixture {
    rows.sort_by(|left, right| left.package_key_sha256.cmp(&right.package_key_sha256));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(NATIVE_PARITY_PACKAGE_FILE_NAME);
    let mut writer =
        NativeParityOracleWriter::create(&path, &candidate.profile, implementation(ecosystem))
            .unwrap();
    for row in &rows {
        writer.package(row).unwrap();
    }
    let manifest = writer.finish().unwrap();
    write_native_parity_oracle_manifest(directory.path(), &manifest).unwrap();
    let reader = verify_native_parity_oracle_bundle(directory.path(), &candidate.profile).unwrap();
    OracleFixture {
        _directory: directory,
        reader,
    }
}

fn rows(candidate: &CandidateFixture) -> Vec<NativeParityPackageV1> {
    candidate
        .reader
        .packages()
        .unwrap()
        .iter()
        .map(NativeParityPackageV1::from_catalog)
        .collect::<crate::error::Result<Vec<_>>>()
        .unwrap()
}

fn assert_fact_mismatch(
    candidate: &CandidateFixture,
    rows: Vec<NativeParityPackageV1>,
    fact: NativeParityFactV1,
) {
    let oracle = oracle(candidate, NativeParityEcosystemV1::Debian, rows);
    let error = compare_native_parity_oracle(&candidate.profile, &candidate.reader, &oracle.reader)
        .expect_err("changed native fact must refuse parity");
    let NativeParityComparisonError::Mismatch(mismatch) = &error else {
        panic!("expected typed package fact mismatch: {error:?}");
    };
    let NativeParityMismatchV1::PackageFacts { facts, .. } = mismatch.as_ref() else {
        panic!("expected typed package fact mismatch: {error:?}");
    };
    assert!(facts.contains(&fact), "missing fact {fact:?} in {facts:?}");
}

#[test]
fn rpm_debian_and_alpm_complete_catalog_oracles_compare_exactly() {
    for ecosystem in [
        NativeParityEcosystemV1::Rpm,
        NativeParityEcosystemV1::Debian,
        NativeParityEcosystemV1::Alpm,
    ] {
        let candidate = candidate(ecosystem);
        let oracle = oracle(&candidate, ecosystem, rows(&candidate));
        let comparison =
            compare_native_parity_oracle(&candidate.profile, &candidate.reader, &oracle.reader)
                .unwrap();
        assert_eq!(comparison.profile, profile_name(ecosystem));
        assert_eq!(
            comparison.counts,
            NativeParityCountsV1::from(candidate.profile.counts)
        );
    }
}

#[test]
fn payload_provider_grouped_negative_and_precedence_changes_are_typed() {
    let candidate = candidate(NativeParityEcosystemV1::Debian);

    let mut changed = rows(&candidate);
    changed[0].checksum = digest('e');
    assert_fact_mismatch(&candidate, changed, NativeParityFactV1::PayloadAuthority);

    let mut changed = rows(&candidate);
    changed[0].provides[0].raw = Some("native-provider-drift".to_string());
    assert_fact_mismatch(&candidate, changed, NativeParityFactV1::Providers);

    let mut changed = rows(&candidate);
    let positive = changed[0]
        .requirement_groups
        .iter_mut()
        .find(|group| group.kind == "depends")
        .unwrap();
    positive.description = Some("native grouped requirement drift".to_string());
    assert_fact_mismatch(&candidate, changed, NativeParityFactV1::GroupedRequirements);

    let mut changed = rows(&candidate);
    let negative = changed[0]
        .requirement_groups
        .iter_mut()
        .find(|group| group.kind == "obsolete")
        .unwrap();
    negative.description = Some("native negative relation drift".to_string());
    assert_fact_mismatch(&candidate, changed, NativeParityFactV1::NegativeRelations);

    let mut changed = rows(&candidate);
    let selected = &candidate.profile.members[0];
    let updates_owned = changed
        .iter_mut()
        .find(|package| package.member_ordinal == 1)
        .unwrap();
    updates_owned.member_ordinal = selected.ordinal;
    updates_owned.source_identity = selected.source_identity.clone();
    updates_owned.repository_identity = selected.repository_identity.clone();
    updates_owned.source_snapshot_sha256 = selected.source_snapshot_sha256.clone();
    assert_fact_mismatch(&candidate, changed, NativeParityFactV1::OriginPrecedence);

    let mut changed = rows(&candidate);
    changed[0].debian_multi_arch = Some(DebianMultiArch::Same);
    assert_fact_mismatch(&candidate, changed, NativeParityFactV1::IdentityVariant);
}

#[test]
fn missing_and_extra_packages_are_distinct_typed_mismatches() {
    let candidate = candidate(NativeParityEcosystemV1::Rpm);
    let mut missing = rows(&candidate);
    missing.remove(0);
    let missing_oracle = oracle(&candidate, NativeParityEcosystemV1::Rpm, missing);
    let error = compare_native_parity_oracle(
        &candidate.profile,
        &candidate.reader,
        &missing_oracle.reader,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        NativeParityComparisonError::Mismatch(mismatch)
            if matches!(mismatch.as_ref(), NativeParityMismatchV1::CandidateOnlyPackage { .. })
    ));

    let mut extra = rows(&candidate);
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: candidate.profile.profile.clone(),
        },
        candidate.reader.source_evidence().unwrap(),
        vec![package(
            NativeParityEcosystemV1::Rpm,
            &candidate.profile.members,
            "extra-native-package",
            "x86_64",
            0,
            'e',
        )],
    )
    .unwrap();
    extra.push(NativeParityPackageV1::from_catalog(&content.packages[0]).unwrap());
    let extra_oracle = oracle(&candidate, NativeParityEcosystemV1::Rpm, extra);
    let error =
        compare_native_parity_oracle(&candidate.profile, &candidate.reader, &extra_oracle.reader)
            .unwrap_err();
    assert!(matches!(
        error,
        NativeParityComparisonError::Mismatch(mismatch)
            if matches!(mismatch.as_ref(), NativeParityMismatchV1::OracleOnlyPackage { .. })
    ));
}

#[test]
fn bundle_reopen_rejects_tamper_unknown_fields_and_noncanonical_rows() {
    let candidate = candidate(NativeParityEcosystemV1::Alpm);
    let oracle = oracle(&candidate, NativeParityEcosystemV1::Alpm, rows(&candidate));
    fs::write(
        oracle.reader.path(),
        [fs::read(oracle.reader.path()).unwrap(), b"tamper".to_vec()].concat(),
    )
    .unwrap();
    assert!(
        NativeParityOracleReader::open_verified(oracle.reader.path(), oracle.reader.manifest())
            .is_err()
    );

    let unknown = serde_json::json!({
        "schema_version": 1,
        "profile": "arch",
        "extra": true
    });
    assert!(serde_json::from_value::<NativeParityOracleV1>(unknown).is_err());
}

#[test]
fn writer_rejects_duplicate_or_reordered_package_keys() {
    let candidate = candidate(NativeParityEcosystemV1::Rpm);
    let rows = rows(&candidate);
    let directory = tempfile::tempdir().unwrap();
    let mut writer = NativeParityOracleWriter::create(
        directory.path().join(NATIVE_PARITY_PACKAGE_FILE_NAME),
        &candidate.profile,
        implementation(NativeParityEcosystemV1::Rpm),
    )
    .unwrap();
    writer.package(&rows[1]).unwrap();
    assert!(writer.package(&rows[0]).is_err());
    assert!(writer.package(&rows[1]).is_err());
}
