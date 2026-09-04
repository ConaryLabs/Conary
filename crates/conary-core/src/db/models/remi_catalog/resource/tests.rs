// crates/conary-core/src/db/models/remi_catalog/resource/tests.rs

use super::*;
use crate::db::schema::ensure_current;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V3, SOURCE_SNAPSHOT_SCHEMA_V1,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1,
    SourceStreamV1,
};
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn physical_attestation(catalog_size: u64, digest_byte: char) -> RemiCatalogPhysicalAttestation {
    let chunk_count = portable_chunk_count_v1(catalog_size).unwrap();
    RemiCatalogPhysicalAttestation::new(
        PortableManifestAttestationV1 {
            sha256: digest(digest_byte),
            size: portable_manifest_size_v1(chunk_count).unwrap(),
        },
        catalog_size,
    )
    .unwrap()
}

fn source_manifest() -> SourceSnapshotV1 {
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
        stream_binding_sha256: digest('a'),
        parser_projection_version: crate::repository::catalog::SOURCE_CATALOG_PROJECTION_VERSION_V2,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Rpm,
            metadata_url: "https://example.test/repository".to_string(),
            content_url: None,
            parser_config_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&parser_config).unwrap(),
            ),
            parser_config,
            trust_policy_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&trust_policy).unwrap(),
            ),
            trust_policy,
        },
        authenticated_root: CatalogArtifactV1 {
            sha256: digest('b'),
            size: 1024,
        },
        authenticated_objects: vec![SourceMetadataObjectV1 {
            role: SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.zst".to_string(),
            sha256: digest('c'),
            size: 2048,
        }],
        catalog: CatalogArtifactV1 {
            sha256: digest('d'),
            size: 4096,
        },
        logical_digest_sha256: digest('e'),
        counts: CatalogCountsV1 {
            source_evidence: 1,
            ..CatalogCountsV1::default()
        },
    }
}

fn profile_manifest(source: &SourceSnapshotV1) -> ProfileRevisionV2 {
    ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V3,
        profile: "fedora-44".to_string(),
        target_architecture:
            crate::repository::supported_profiles::ProfileTargetArchitecture::X86_64,
        projection_version: 1,
        members: vec![ProfileSourceMemberV2 {
            ordinal: 0,
            source_identity: source.source_identity.clone(),
            repository_identity: source.repository_identity.clone(),
            stream: source.stream.clone(),
            role: crate::repository::supported_profiles::ProfileSourceRole::Base,
            precedence: 10,
            required: true,
            source_snapshot_sha256: source.manifest_sha256().unwrap(),
        }],
        catalog: CatalogArtifactV1 {
            sha256: digest('f'),
            size: 8192,
        },
        logical_digest_sha256: digest('0'),
        counts: CatalogCountsV1 {
            source_evidence: 1,
            ..CatalogCountsV1::default()
        },
    }
}

fn source_attestations(
    sources: &[SourceSnapshotV1],
) -> BTreeMap<String, RemiCatalogPhysicalAttestation> {
    sources
        .iter()
        .map(|source| {
            (
                source.catalog.sha256.clone(),
                physical_attestation(source.catalog.size, '8'),
            )
        })
        .collect()
}

fn register_fixture(
    conn: &Connection,
    sources: &[SourceSnapshotV1],
    profile: &ProfileRevisionV2,
    created_at: i64,
) -> Result<()> {
    register_profile_catalog_revision(
        conn,
        sources,
        &source_attestations(sources),
        profile,
        physical_attestation(profile.catalog.size, '9'),
        created_at,
    )
}

#[test]
fn exact_published_manifests_register_atomically_and_replay_idempotently() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let source = source_manifest();
    let profile = profile_manifest(&source);

    register_fixture(&conn, std::slice::from_ref(&source), &profile, 100).unwrap();
    register_fixture(&conn, std::slice::from_ref(&source), &profile, 200).unwrap();

    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM remi_profile_revision_members",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
    let stored_source =
        RemiCatalogResource::find_by_sha256(&conn, &source.manifest_sha256().unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(stored_source.created_at, 100);
    assert_eq!(
        stored_source.physical_attestation,
        physical_attestation(source.catalog.size, '8')
    );
    let stored_profile =
        RemiCatalogResource::find_by_sha256(&conn, &profile.manifest_sha256().unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(
        stored_profile.physical_attestation,
        physical_attestation(profile.catalog.size, '9')
    );
}

#[test]
fn authenticated_root_change_can_register_same_projection_artifact() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let source = source_manifest();
    let profile = profile_manifest(&source);
    register_fixture(&conn, std::slice::from_ref(&source), &profile, 100).unwrap();

    let mut alias = source.clone();
    alias.authenticated_root.sha256 = digest('1');
    assert_ne!(
        alias.manifest_sha256().unwrap(),
        source.manifest_sha256().unwrap()
    );
    assert_eq!(alias.catalog, source.catalog);
    let mut alias_profile = profile_manifest(&alias);
    alias_profile.catalog.sha256 = digest('2');
    alias_profile.logical_digest_sha256 = digest('3');

    register_fixture(&conn, std::slice::from_ref(&alias), &alias_profile, 200).unwrap();

    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM remi_catalog_resources
                 WHERE resource_kind = 'source_snapshot' AND artifact_sha256 = ?1",
            [&source.catalog.sha256],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        4
    );
    assert!(
        RemiCatalogResource::find_by_sha256(&conn, &alias.manifest_sha256().unwrap())
            .unwrap()
            .is_some()
    );
}

#[test]
fn mixed_member_registration_writes_nothing() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let source = source_manifest();
    let profile = profile_manifest(&source);
    let mut mixed = source.clone();
    mixed.repository_identity = "fedora-updates-x86_64".to_string();

    assert!(register_fixture(&conn, &[mixed], &profile, 100).is_err());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[test]
fn registration_rejects_missing_extra_and_noncanonical_attestation_keys() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let source = source_manifest();
    let profile = profile_manifest(&source);
    let profile_attestation = physical_attestation(profile.catalog.size, '9');

    for source_attestations in [
        BTreeMap::new(),
        BTreeMap::from([(digest('1'), physical_attestation(1, '7'))]),
        BTreeMap::from([("A".repeat(64), physical_attestation(1, '7'))]),
    ] {
        assert!(
            register_profile_catalog_revision(
                &conn,
                std::slice::from_ref(&source),
                &source_attestations,
                &profile,
                profile_attestation.clone(),
                100,
            )
            .is_err()
        );
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[test]
fn invalid_portable_manifest_digest_is_rejected_before_persistence() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let source = source_manifest();
    let error = RemiCatalogResource::from_source_snapshot(
        &source,
        RemiCatalogPhysicalAttestation {
            portable_manifest: PortableManifestAttestationV1 {
                sha256: "A".repeat(64),
                size: 96,
            },
            chunk_size: PORTABLE_CHUNK_SIZE_V1,
            chunk_count: 1,
        },
        100,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("exactly 64 lowercase hexadecimal characters"),
        "{error}"
    );
}
