use super::*;
use crate::db::models::{RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode};
use crate::repository::catalog::{
    CatalogPackageOriginV1, CatalogScopeV1, CatalogSourceEvidenceV1, SourceMetadataObjectRoleV1,
    write_catalog_candidate,
};
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::parsers::PackageMetadata;
use crate::repository::sync::synced_package_row;
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

fn repository() -> Repository {
    let mut repository = Repository::new(
        "fedora-everything".to_string(),
        "https://example.test/fedora/44/everything/x86_64".to_string(),
    );
    repository.id = Some(7);
    repository.source_profile = Some("fedora-44".to_string());
    repository
        .set_parser_config(RepositoryParserConfig::Rpm {
            architecture: "x86_64".to_string(),
        })
        .unwrap();
    repository
        .set_trust_policy(RepositoryTrustPolicy::Rpm {
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
        })
        .unwrap();
    repository
        .set_native_source_policy(
            RepositorySourcePolicy::new(
                "fedora-project",
                RepositoryPolicyScope::repository("fedora-everything-x86_64").unwrap(),
                NativeSourceEcosystem::Rpm,
                NativeSourceStream::release("44").unwrap(),
                RepositoryUpdateMode::Follow,
            )
            .unwrap(),
            "fedora-everything-x86_64",
            None,
        )
        .unwrap();
    repository
}

fn package(repository: &Repository) -> SyncedPackageRow {
    let mut package = PackageMetadata::new(
        "bash".to_string(),
        "5.2.37-1".to_string(),
        "c".repeat(64),
        4096,
        "packages/bash-5.2.37-1.x86_64.rpm".to_string(),
        RepositoryDependencyFlavor::Rpm,
        VersionScheme::Rpm,
    );
    package.architecture = Some("x86_64".to_string());
    synced_package_row(
        repository.id.unwrap(),
        repository.source_profile.as_deref(),
        &repository.url,
        repository.content_url.as_deref(),
        package,
    )
}

fn authenticated_object() -> SourceMetadataObjectV1 {
    SourceMetadataObjectV1 {
        role: SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.zst".to_string(),
        sha256: "d".repeat(64),
        size: 2048,
    }
}

#[test]
fn exact_native_evidence_becomes_a_bound_source_snapshot_without_operational_ids() {
    let repository = repository();
    let candidate = source_catalog_candidate(
        &repository,
        vec![package(&repository)],
        AuthenticatedSnapshotIdentity::for_bytes(b"signed repomd.xml"),
        vec![authenticated_object()],
    )
    .unwrap();

    assert_eq!(
        candidate.content().scope,
        CatalogScopeV1::Source {
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        }
    );
    assert_eq!(
        candidate.content().source_evidence,
        vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.zst".to_string(),
            sha256: "d".repeat(64),
            size: 2048,
        }]
    );
    assert!(matches!(
        candidate.content().packages[0].origin,
        CatalogPackageOriginV1::Source { .. }
    ));

    let directory = tempfile::tempdir().unwrap();
    let binding =
        write_catalog_candidate(directory.path().join("catalog.sqlite"), candidate.content())
            .unwrap();
    let manifest = candidate.bind(&binding).unwrap();
    assert_eq!(manifest.authenticated_objects, vec![authenticated_object()]);
    assert_eq!(manifest.catalog, binding.artifact);
    assert_eq!(manifest.counts.packages, 1);
}

#[test]
fn legacy_sha_only_root_cannot_claim_byte_complete_source_authority() {
    let repository = repository();
    let error = source_catalog_candidate(
        &repository,
        vec![package(&repository)],
        AuthenticatedSnapshotIdentity::from_sha256("a".repeat(64)).unwrap(),
        vec![authenticated_object()],
    )
    .err()
    .expect("legacy SHA-only identity must fail");
    assert!(error.to_string().contains("no exact byte length"));
}
