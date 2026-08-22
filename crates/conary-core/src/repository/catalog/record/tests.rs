// crates/conary-core/src/repository/catalog/record/tests.rs

use super::*;
use crate::repository::dependency_model::{
    RepositoryRequirementClause, RepositoryRequirementExpression,
};

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn profile_origin(snapshot: &str) -> CatalogPackageOriginV1 {
    CatalogPackageOriginV1::Profile {
        member_ordinal: 0,
        source_identity: "archlinux".to_string(),
        repository_identity: "arch-core-x86_64".to_string(),
        source_snapshot_sha256: snapshot.to_string(),
    }
}

fn provide(capability: &str) -> CatalogProvideRecordV1 {
    CatalogProvideRecordV1 {
        capability: capability.to_string(),
        version: None,
        version_relation: None,
        kind: "virtual".to_string(),
        raw: Some(capability.to_string()),
        version_scheme: VersionScheme::Arch,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::AuthorDeclared,
    }
}

fn group(capability: &str) -> CatalogRequirementGroupV1 {
    let clause = RepositoryRequirementClause::name_only(capability.to_string());
    CatalogRequirementGroupV1 {
        kind: "depends".to_string(),
        behavior: "hard".to_string(),
        description: None,
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

fn package(snapshot: &str) -> CatalogPackageRecordV1 {
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: profile_origin(snapshot),
        source_profile: "arch".to_string(),
        name: "tool".to_string(),
        version: "1.0-1".to_string(),
        package_release: "1".to_string(),
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: None,
        checksum: digest('a'),
        size: 64,
        download_url: "https://example.test/tool.pkg.tar.zst".to_string(),
        metadata: Some("{ \"z\": 2, \"a\": 1 }".to_string()),
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Arch,
        provides: vec![provide("z-capability"), provide("a-capability")],
        requirement_groups: vec![group("z-dependency"), group("a-dependency")],
    }
}

fn evidence(snapshot: &str) -> Vec<CatalogSourceEvidenceV1> {
    vec![CatalogSourceEvidenceV1::SourceSnapshot {
        member_ordinal: 0,
        source_identity: "archlinux".to_string(),
        repository_identity: "arch-core-x86_64".to_string(),
        source_snapshot_sha256: snapshot.to_string(),
    }]
}

#[test]
fn profile_content_canonicalizes_nested_records_and_json() {
    let snapshot = digest('b');
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![package(&snapshot)],
    )
    .unwrap();
    let package = &content.packages[0];
    assert_eq!(package.metadata.as_deref(), Some(r#"{"a":1,"z":2}"#));
    assert_eq!(package.provides[0].capability, "a-capability");
    assert_eq!(
        package.requirement_groups[0].atoms[0].capability,
        "a-dependency"
    );
}

#[test]
fn profile_package_must_resolve_to_exact_member_evidence() {
    let snapshot = digest('b');
    let mut mismatched = package(&snapshot);
    mismatched.origin = profile_origin(&digest('c'));
    let error = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![mismatched],
    )
    .unwrap_err();
    assert!(error.to_string().contains("does not resolve"));

    let mut missing = package(&snapshot);
    let CatalogPackageOriginV1::Profile { member_ordinal, .. } = &mut missing.origin else {
        unreachable!();
    };
    *member_ordinal = 4;
    let error = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![missing],
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing source member ordinal 4")
    );
}

#[test]
fn typed_provide_invariants_fail_closed() {
    let snapshot = digest('b');
    let mut invalid = package(&snapshot);
    invalid.provides[0].version = Some("1".to_string());
    invalid.provides[0].version_relation = Some(ProvideVersionRelation::GreaterThan);
    let error = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![invalid],
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("may only carry an exact version relation")
    );
}

#[test]
fn source_native_unicode_capabilities_round_trip_without_weakening_catalog_ids() {
    let snapshot = digest('b');
    let capability = "font(源ノ角ゴシックcodejp)";
    let mut record = package(&snapshot);
    record.provides = vec![provide(capability)];
    record.requirement_groups = vec![group(capability)];

    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![record],
    )
    .unwrap();

    assert_eq!(content.packages[0].provides[0].capability, capability);
    assert_eq!(
        content.packages[0].requirement_groups[0].atoms[0].capability,
        capability
    );
}

#[test]
fn source_native_capabilities_reject_control_characters() {
    let snapshot = digest('b');
    let mut record = package(&snapshot);
    record.provides = vec![provide("bad\ncapability")];

    let error = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![record],
    )
    .unwrap_err();

    assert!(error.to_string().contains("control characters"));
}

#[test]
fn source_native_file_capabilities_preserve_significant_trailing_space() {
    let snapshot = digest('b');
    let capability = "/usr/share/doc/giac/fr ";
    let mut file = provide(capability);
    file.kind = "file".to_string();
    file.version_scheme = VersionScheme::Rpm;
    file.raw = None;
    file.provenance = CapabilityProvenance::SourceDerivedFile {
        format: crate::repository::dependency_source::SourcePackageFormat::Rpm,
    };
    let mut record = package(&snapshot);
    record.provides = vec![file];

    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![record],
    )
    .unwrap();

    assert_eq!(content.packages[0].provides[0].capability, capability);
}

#[test]
fn streamed_relations_preserve_exact_v1_logical_identity() {
    let snapshot = digest('b');
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "arch".to_string(),
        },
        evidence(&snapshot),
        vec![package(&snapshot)],
    )
    .unwrap();
    let expected_digest = content.logical_digest_sha256().unwrap();
    let expected_counts = content.counts().unwrap();
    let mut base = content.packages[0].clone();
    let provides = std::mem::take(&mut base.provides);
    let requirement_groups = std::mem::take(&mut base.requirement_groups);

    let mut streamed =
        CatalogLogicalDigestV1::new(&content.scope, &content.source_evidence).unwrap();
    let mut package = streamed.begin_package(&base).unwrap();
    for provide in &provides {
        package.provide(provide).unwrap();
    }
    for group in &requirement_groups {
        let mut base = group.clone();
        let atoms = std::mem::take(&mut base.atoms);
        let mut streamed_group = package.begin_requirement_group(&base).unwrap();
        for atom in atoms {
            streamed_group.atom(&atom).unwrap();
        }
        streamed_group.finish().unwrap();
    }
    package.finish().unwrap();
    let (actual_digest, actual_counts) = streamed.finish().unwrap();

    assert_eq!(actual_digest, expected_digest);
    assert_eq!(actual_counts, expected_counts);
}
