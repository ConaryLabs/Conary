// crates/conary-core/src/repository/catalog/bundle/tests.rs

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogContentV1, CatalogPackageRecordV1, CatalogScopeV1,
    CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V1, ProfileSourceMemberV1,
    SOURCE_SNAPSHOT_SCHEMA_V1, SourceEcosystemV1, SourceMetadataObjectRoleV1,
    SourceMetadataObjectV1, SourceProvenanceV1, SourceStreamKindV1, SourceStreamV1,
    write_catalog_candidate,
};
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn artifact(byte: char, size: u64) -> CatalogArtifactV1 {
    CatalogArtifactV1 {
        sha256: digest(byte),
        size,
    }
}

fn package(origin: CatalogPackageOriginV1) -> CatalogPackageRecordV1 {
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin,
        source_profile: "fedora-44".to_string(),
        name: "bash".to_string(),
        version: "5.2.37-1".to_string(),
        package_release: "1".to_string(),
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some("shell".to_string()),
        checksum: digest('c'),
        size: 2048,
        download_url: "https://example.test/bash.rpm".to_string(),
        metadata: Some("{}".to_string()),
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Rpm,
        provides: Vec::new(),
        requirement_groups: Vec::new(),
    }
}

fn source_object() -> SourceMetadataObjectV1 {
    SourceMetadataObjectV1 {
        role: SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.zst".to_string(),
        sha256: digest('d'),
        size: 1024,
    }
}

fn source_content() -> CatalogContentV1 {
    let object = source_object();
    CatalogContentV1::new(
        CatalogScopeV1::Source {
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        },
        vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: object.role,
            source_path: object.source_path,
            sha256: object.sha256,
            size: object.size,
        }],
        vec![package(CatalogPackageOriginV1::Source {
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        })],
    )
    .unwrap()
}

fn source_manifest(binding: &CatalogBindingV1) -> SourceSnapshotV1 {
    let parser_config = RepositoryParserConfig::Rpm {
        architecture: "x86_64".to_string(),
    };
    let trust_policy = RepositoryTrustPolicy::Rpm {
        metadata: RpmMetadataAuthority::Metalink {
            url: "https://example.test/metalink".to_string(),
        },
        package_keys: vec![
            OpenPgpTrustRoot::new(
                "https://example.test/fedora.gpg".to_string(),
                "A".repeat(40),
            )
            .unwrap(),
        ],
    };
    SourceSnapshotV1 {
        schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
        source_profile: "fedora-44".to_string(),
        source_identity: "fedora-project".to_string(),
        repository_identity: "fedora-everything-x86_64".to_string(),
        stream: SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: "44".to_string(),
        },
        stream_binding_sha256: digest('e'),
        parser_projection_version: 1,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Rpm,
            metadata_url: "https://example.test/repository".to_string(),
            content_url: Some("https://content.example.test/repository".to_string()),
            parser_config_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&parser_config).unwrap(),
            ),
            parser_config,
            trust_policy_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&trust_policy).unwrap(),
            ),
            trust_policy,
        },
        authenticated_root: artifact('2', 512),
        authenticated_objects: vec![source_object()],
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    }
}

#[test]
fn source_bundle_is_verified_before_atomic_content_addressed_publication() {
    let directory = tempfile::tempdir().unwrap();
    let candidates = directory.path().join("candidates");
    let candidate = candidates.join("source");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidates).unwrap();
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    write_source_catalog_manifest(&candidate, &manifest).unwrap();
    verify_source_catalog_bundle(&candidate, &manifest).unwrap();

    let expected_identity = manifest.manifest_sha256().unwrap();
    let published = publish_source_catalog_bundle(&candidate, &catalogs, &manifest).unwrap();
    assert_eq!(published.file_name().unwrap(), expected_identity.as_str());
    assert!(!candidate.exists());
    verify_source_catalog_bundle(&published, &manifest).unwrap();
}

#[test]
fn malformed_or_extra_candidate_content_is_never_published() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("candidate");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    write_source_catalog_manifest(&candidate, &manifest).unwrap();
    fs::write(candidate.join("unowned.tmp"), b"nope").unwrap();

    assert!(publish_source_catalog_bundle(&candidate, &catalogs, &manifest).is_err());
    assert!(candidate.exists());
    assert!(!catalogs.join("sources").exists());
}

#[test]
fn profile_bundle_rejects_mixed_member_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("profile");
    fs::create_dir(&candidate).unwrap();
    let source_snapshot_sha256 = digest('3');
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "fedora-44".to_string(),
        },
        vec![CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        }],
        vec![package(CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        })],
    )
    .unwrap();
    let binding = write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &content).unwrap();
    let manifest = ProfileRevisionV1 {
        schema_version: PROFILE_REVISION_SCHEMA_V1,
        profile: "fedora-44".to_string(),
        projection_version: 1,
        members: vec![ProfileSourceMemberV1 {
            ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            priority: 10,
            required: true,
            source_snapshot_sha256: digest('4'),
        }],
        catalog: binding.artifact,
        logical_digest_sha256: binding.logical_digest_sha256,
        counts: binding.counts,
    };
    let error = write_profile_catalog_manifest(&candidate, &manifest).unwrap_err();
    assert!(error.to_string().contains("members do not match"));
    assert!(!candidate.join(CATALOG_MANIFEST_FILE_NAME).exists());
}
