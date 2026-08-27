// crates/conary-core/src/repository/catalog/profile/tests/debian_pockets.rs

//! Production-shaped Debian pocket composition and conflict proofs.

use super::{debian_source_content, debian_source_manifest, digest};
use crate::error::Error;
use crate::repository::catalog::{
    CatalogBindingV1, CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogReader,
    CatalogScopeV1, DebianSourcePocketV1, write_catalog_candidate,
};
use crate::repository::supported_profiles::ProfileSourceRole;

use super::super::{ProfileCatalogMemberInputV2, write_profile_catalog_candidate};

type SemanticMutation = (&'static str, fn(&mut CatalogPackageRecordV1));

#[test]
fn ubuntu_pocket_duplicate_keeps_typed_precedence_origin_under_input_reordering() {
    let directory = tempfile::tempdir().unwrap();
    let security_path = directory.path().join("security.sqlite");
    let updates_path = directory.path().join("updates.sqlite");
    let security_content = debian_source_content(
        "ubuntu-resolute-security-main-amd64",
        "resolute-security",
        'a',
        |_| {},
    );
    let updates_content = debian_source_content(
        "ubuntu-resolute-updates-main-amd64",
        "resolute-updates",
        'b',
        |_| {},
    );
    let security_binding = write_catalog_candidate(&security_path, &security_content).unwrap();
    let updates_binding = write_catalog_candidate(&updates_path, &updates_content).unwrap();
    let security_manifest = debian_source_manifest(
        "ubuntu-resolute-security-main-amd64",
        "resolute-security",
        'a',
        &security_binding,
    );
    let updates_manifest = debian_source_manifest(
        "ubuntu-resolute-updates-main-amd64",
        "resolute-updates",
        'b',
        &updates_binding,
    );
    let security_reader = CatalogReader::open_verified(&security_path, &security_binding).unwrap();
    let updates_reader = CatalogReader::open_verified(&updates_path, &updates_binding).unwrap();

    let inputs = |reverse: bool| {
        let security = ProfileCatalogMemberInputV2 {
            ordinal: 0,
            role: ProfileSourceRole::Security,
            precedence: 200,
            required: true,
            manifest: &security_manifest,
            reader: &security_reader,
        };
        let updates = ProfileCatalogMemberInputV2 {
            ordinal: 1,
            role: ProfileSourceRole::Updates,
            precedence: 100,
            required: true,
            manifest: &updates_manifest,
            reader: &updates_reader,
        };
        if reverse {
            vec![updates, security]
        } else {
            vec![security, updates]
        }
    };

    let forward_path = directory.path().join("profile-forward.sqlite");
    let reverse_path = directory.path().join("profile-reverse.sqlite");
    let forward =
        write_profile_catalog_candidate(&forward_path, "ubuntu-26.04", 2, inputs(false)).unwrap();
    let reversed =
        write_profile_catalog_candidate(&reverse_path, "ubuntu-26.04", 2, inputs(true)).unwrap();

    assert_eq!(forward, reversed);
    assert_eq!(forward.counts.packages, 1);
    assert_eq!(forward.counts.source_evidence, 2);
    assert_eq!(
        forward.members[0].source_snapshot_sha256,
        security_manifest.manifest_sha256().unwrap()
    );
    assert_eq!(
        forward.members[1].source_snapshot_sha256,
        updates_manifest.manifest_sha256().unwrap()
    );

    let binding = CatalogBindingV1 {
        scope: CatalogScopeV1::Profile {
            profile: forward.profile.clone(),
        },
        artifact: forward.catalog.clone(),
        logical_digest_sha256: forward.logical_digest_sha256.clone(),
        counts: forward.counts,
    };
    let reader = CatalogReader::open_verified(&forward_path, &binding).unwrap();
    let packages = reader.packages().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(
        packages[0].origin,
        CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: "ubuntu".to_string(),
            repository_identity: "ubuntu-resolute-security-main-amd64".to_string(),
            source_snapshot_sha256: security_manifest.manifest_sha256().unwrap(),
        }
    );
    assert_eq!(
        packages[0].debian_source_pocket().unwrap(),
        Some(DebianSourcePocketV1 {
            distribution: "resolute-security".to_string(),
            component: "main".to_string(),
        })
    );
}

#[test]
fn ubuntu_pocket_duplicate_rejects_streamed_semantic_disagreement() {
    let mutations: Vec<SemanticMutation> = vec![
        ("payload digest", |package| package.checksum = digest('e')),
        ("payload size", |package| package.size += 1),
        ("provider", |package| {
            package.provides[0].capability = "linux-headers-virtual-7.1".to_string();
        }),
        ("dependency", |package| {
            package
                .requirement_groups
                .iter_mut()
                .find(|group| group.kind == "depends")
                .unwrap()
                .atoms[0]
                .capability
                .push_str("-different");
        }),
        ("conflict", |package| {
            package
                .requirement_groups
                .iter_mut()
                .find(|group| group.kind == "conflict")
                .unwrap()
                .atoms[0]
                .capability
                .push_str("-different");
        }),
        ("replacement", |package| {
            package
                .requirement_groups
                .iter_mut()
                .find(|group| group.kind == "replace")
                .unwrap()
                .atoms[0]
                .capability
                .push_str("-different");
        }),
        ("intrinsic metadata", |package| {
            let mut metadata =
                serde_json::from_str::<serde_json::Value>(package.metadata.as_deref().unwrap())
                    .unwrap();
            metadata["section"] = serde_json::Value::String("admin".to_string());
            package.metadata = Some(metadata.to_string());
        }),
    ];

    for (index, (label, mutate)) in mutations.into_iter().enumerate() {
        let directory = tempfile::tempdir().unwrap();
        let security_path = directory.path().join("security.sqlite");
        let updates_path = directory.path().join("updates.sqlite");
        let security_content = debian_source_content(
            "ubuntu-resolute-security-main-amd64",
            "resolute-security",
            'a',
            |_| {},
        );
        let updates_content = debian_source_content(
            "ubuntu-resolute-updates-main-amd64",
            "resolute-updates",
            'b',
            mutate,
        );
        let security_binding = write_catalog_candidate(&security_path, &security_content).unwrap();
        let updates_binding = write_catalog_candidate(&updates_path, &updates_content).unwrap();
        let security_manifest = debian_source_manifest(
            "ubuntu-resolute-security-main-amd64",
            "resolute-security",
            'a',
            &security_binding,
        );
        let updates_manifest = debian_source_manifest(
            "ubuntu-resolute-updates-main-amd64",
            "resolute-updates",
            'b',
            &updates_binding,
        );
        let security_reader =
            CatalogReader::open_verified(&security_path, &security_binding).unwrap();
        let updates_reader = CatalogReader::open_verified(&updates_path, &updates_binding).unwrap();

        let error = write_profile_catalog_candidate(
            directory.path().join(format!("profile-{index}.sqlite")),
            "ubuntu-26.04",
            2,
            vec![
                ProfileCatalogMemberInputV2 {
                    ordinal: 0,
                    role: ProfileSourceRole::Security,
                    precedence: 200,
                    required: true,
                    manifest: &security_manifest,
                    reader: &security_reader,
                },
                ProfileCatalogMemberInputV2 {
                    ordinal: 1,
                    role: ProfileSourceRole::Updates,
                    precedence: 100,
                    required: true,
                    manifest: &updates_manifest,
                    reader: &updates_reader,
                },
            ],
        )
        .unwrap_err();

        assert!(
            matches!(error, Error::ConflictError(_))
                && error.to_string().contains("disagrees between repositories"),
            "{label} disagreement produced unexpected error: {error}"
        );
    }
}
