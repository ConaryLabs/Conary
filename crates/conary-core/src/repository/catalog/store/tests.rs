// crates/conary-core/src/repository/catalog/store/tests.rs

use std::io::{Seek, SeekFrom, Write};

use super::*;
use crate::repository::catalog::SourceMetadataObjectRoleV1;
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::dependency_source::CapabilityProvenance;

fn source_scope() -> CatalogScopeV1 {
    CatalogScopeV1::Source {
        source_profile: "fedora-44".to_string(),
        source_identity: "fedora-project".to_string(),
        repository_identity: "fedora-everything-x86_64".to_string(),
    }
}

fn evidence() -> Vec<CatalogSourceEvidenceV1> {
    vec![CatalogSourceEvidenceV1::AuthenticatedObject {
        role: SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.zst".to_string(),
        sha256: "a".repeat(64),
        size: 4096,
    }]
}

fn package(name: &str, checksum: &str) -> CatalogPackageRecordV1 {
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Source {
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        },
        source_profile: "fedora-44".to_string(),
        name: name.to_string(),
        version: "1.0-1".to_string(),
        package_release: "1".to_string(),
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some(format!("{name} package")),
        checksum: checksum.to_string(),
        size: 128,
        download_url: format!("https://example.test/{name}.rpm"),
        metadata: Some("{}".to_string()),
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Rpm,
        provides: vec![CatalogProvideRecordV1 {
            capability: name.to_string(),
            version: Some("1.0-1".to_string()),
            version_relation: Some(ProvideVersionRelation::Equal),
            kind: "package".to_string(),
            raw: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }],
        requirement_groups: vec![CatalogRequirementGroupV1 {
            kind: "depends".to_string(),
            behavior: "hard".to_string(),
            description: None,
            native_text: Some("glibc >= 2.39".to_string()),
            expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(
                RepositoryRequirementClause::versioned("glibc".to_string(), ">= 2.39".to_string()),
            ))
            .unwrap(),
            atoms: vec![CatalogRequirementAtomV1 {
                capability: "glibc".to_string(),
                version_constraint: Some(">= 2.39".to_string()),
                kind: "package".to_string(),
                dependency_type: "runtime".to_string(),
                raw: Some("glibc >= 2.39".to_string()),
            }],
        }],
    }
}

#[test]
fn catalog_artifact_is_independent_of_input_order() {
    let directory = tempfile::tempdir().unwrap();
    let left_content = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![package("zlib", "b"), package("bash", "a")],
    )
    .unwrap();
    let right_content = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![package("bash", "a"), package("zlib", "b")],
    )
    .unwrap();
    assert_eq!(left_content, right_content);
    let left_path = directory.path().join("left.sqlite");
    let right_path = directory.path().join("right.sqlite");
    let left = write_catalog_candidate(&left_path, &left_content).unwrap();
    let right = write_catalog_candidate(&right_path, &right_content).unwrap();
    assert_eq!(left.artifact, right.artifact);
    assert_eq!(left.logical_digest_sha256, right.logical_digest_sha256);
    let reader = CatalogReader::open_verified(&left_path, &left).unwrap();
    assert_eq!(reader.packages().unwrap(), left_content.packages);
    assert_eq!(reader.source_evidence().unwrap(), evidence());
    assert_eq!(reader.find_packages_by_name("bash").unwrap().len(), 1);
}

#[test]
fn duplicate_package_identity_is_rejected_even_when_checksum_changes() {
    let error = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![package("bash", "a"), package("bash", "b")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("repeats exact package key"));
}

#[test]
fn verified_reader_rejects_tamper_and_manifest_count_drift() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.sqlite");
    let content =
        CatalogContentV1::new(source_scope(), evidence(), vec![package("bash", "a")]).unwrap();
    let binding = write_catalog_candidate(&path, &content).unwrap();
    let mut wrong_counts = binding.clone();
    wrong_counts.counts.packages += 1;
    assert!(CatalogReader::open_verified(&path, &wrong_counts).is_err());
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(128)).unwrap();
    file.write_all(&[0xff]).unwrap();
    file.sync_all().unwrap();
    let error = CatalogReader::open_verified(&path, &binding)
        .err()
        .expect("tampered catalog must fail");
    assert!(error.to_string().contains("Checksum mismatch"));
}

#[test]
fn candidate_build_does_not_touch_adjacent_operational_database() {
    let directory = tempfile::tempdir().unwrap();
    let operational = directory.path().join("conary.db");
    fs::write(&operational, b"operational sentinel").unwrap();
    let before = hash_file(&operational).unwrap();
    let content =
        CatalogContentV1::new(source_scope(), evidence(), vec![package("bash", "a")]).unwrap();
    write_catalog_candidate(directory.path().join("catalog.sqlite"), &content).unwrap();
    assert_eq!(hash_file(&operational).unwrap(), before);
}
