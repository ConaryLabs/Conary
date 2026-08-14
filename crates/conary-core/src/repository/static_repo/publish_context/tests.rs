// conary-core/src/repository/static_repo/publish_context/tests.rs

use super::*;
use crate::repository::static_repo::publish::{StaticPublishOptions, publish_static_repo};
use crate::repository::static_repo::{PackageKeyEntry, PackageKeyStatus, PackageKeysFile};

#[test]
fn new_repo_requires_explicit_key_dir_for_artifact_form() {
    let err = match StaticPublishPrepareOptions::artifact_form_new_repo_without_key_dir_for_tests()
        .prepare()
    {
        Ok(_) => panic!("expected missing artifact-form key dir to fail"),
        Err(error) => error,
    };

    assert!(
        err.to_string()
            .contains("static publish requires --key-dir")
    );
}

#[test]
fn new_artifact_repo_with_key_dir_prepares_initial_publish_key() {
    let temp = tempfile::tempdir().unwrap();
    let key_dir = temp.path().join("keys-local");
    let context = StaticPublishPrepareOptions {
        destination: RepoLocation::File {
            root: temp.path().join("repo"),
        },
        key_dir: Some(key_dir.clone()),
        publish_form: StaticPublishForm::Artifact,
    }
    .prepare()
    .unwrap();

    assert!(key_dir.join("publish.private").exists());
    assert!(
        context
            .accepted_signers
            .accepts_key_id(&context.active_publish_key_id)
    );
}

#[test]
fn project_form_attestation_reemits_v3_package_as_v3() {
    let temp = tempfile::tempdir().unwrap();
    let key_dir = temp.path().join("keys-local");
    let context = prepare_project_form_static_context(
        &RepoLocation::File {
            root: temp.path().join("repo"),
        },
        &key_dir,
    )
    .unwrap();
    let evidence = crate::ccs::attestation::test_support::sample_hermetic_evidence_for_tests();
    let evidence_hash = crate::ccs::attestation::canonical_json_hash(&evidence).unwrap();
    let provenance = ManifestProvenance {
        origin_class: Some("native-built".to_string()),
        hardening_level: Some("hermetic".to_string()),
        hermetic_evidence: Some(evidence),
        ..Default::default()
    };
    let package_path = temp.path().join("project-v3.ccs");
    let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("project-v3");
    authority.provenance.hermetic_evidence_hash = Some(evidence_hash);
    crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &crate::ccs::v3::test_support::one_file_payloads_for_tests(),
        &package_path,
        &context.active_publish_key,
        None,
        None,
        None,
    )
    .unwrap();

    let attested = attach_project_form_attestation(ProjectFormAttestationInput {
        package_path: &package_path,
        provenance: &provenance,
        context: &context,
        conary_version: "test",
    })
    .unwrap();

    let archive = crate::ccs::archive_reader::inspect_untrusted_ccs_archive(
        fs::File::open(attested).unwrap(),
    )
    .unwrap();
    assert_eq!(archive.v3_authority.identity.name, "project-v3");
    assert!(archive.v3_build_attestation.is_some());
}

#[test]
fn existing_repo_uses_verified_active_package_keys() {
    let temp = tempfile::tempdir().unwrap();
    let context = prepare_existing_repo_context_for_tests(temp.path()).unwrap();

    assert!(
        context
            .accepted_signers
            .accepts_key_id(&context.active_publish_key_id)
    );
}

#[test]
fn new_repo_does_not_trust_stray_package_keys_without_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let key_dir = temp.path().join("keys-local");
    std::fs::create_dir_all(&key_dir).unwrap();
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publish");
    key.save_to_files(
        &key_dir.join("publish.private"),
        &key_dir.join("publish.public"),
    )
    .unwrap();
    let stray_key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("stray");
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(repo_root.join("keys")).unwrap();
    let keys = PackageKeysFile {
        schema: crate::repository::static_repo::SCHEMA_VERSION,
        keys: vec![PackageKeyEntry {
            algorithm: "ed25519".to_string(),
            public_key: stray_key.public_key_base64(),
            key_id: Some("stray".to_string()),
            status: PackageKeyStatus::Active,
            comment: Some("unverified stray key".to_string()),
        }],
    };
    std::fs::write(
        repo_root.join("keys/package-keys.json"),
        serde_json::to_string_pretty(&keys).unwrap(),
    )
    .unwrap();

    let context = StaticPublishPrepareOptions {
        destination: RepoLocation::File { root: repo_root },
        key_dir: Some(key_dir),
        publish_form: StaticPublishForm::Artifact,
    }
    .prepare()
    .unwrap();

    assert!(
        context
            .accepted_signers
            .accepts_key_id(&context.active_publish_key_id)
    );
    assert!(!context.accepted_signers.accepts_key_id("stray"));
}

#[test]
fn artifact_destination_snapshot_is_read_only_for_missing_repo() {
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let destination = RepoLocation::File { root: repo.clone() };

    let snapshot = inspect_artifact_form_static_destination(&destination).unwrap();

    assert!(snapshot.initial);
    assert!(snapshot.root_key_fingerprint.is_none());
    assert!(
        !repo.exists(),
        "read-only snapshot must not create repository directories"
    );
}

#[test]
fn artifact_destination_snapshot_reports_existing_trust_state() {
    let temp = tempfile::TempDir::new().unwrap();
    let context = prepare_existing_repo_context_for_tests(temp.path()).unwrap();
    let destination = context.destination.clone();

    let snapshot = inspect_artifact_form_static_destination(&destination).unwrap();

    assert!(!snapshot.initial);
    assert!(
        snapshot
            .root_key_fingerprint
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(
        snapshot
            .package_keys_sha256
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(
        snapshot
            .accepted_signer_set_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(
        snapshot.publish_policy_digest,
        STATIC_PUBLISH_POLICY_DIGEST_V1
    );
    let versions = snapshot.metadata_versions.expect("metadata versions");
    assert!(versions.root_version >= 1);
    assert!(versions.targets_version >= 1);
    assert!(versions.snapshot_version >= 1);
    assert!(versions.timestamp_version >= 1);
}

impl StaticPublishPrepareOptions {
    fn artifact_form_new_repo_without_key_dir_for_tests() -> Self {
        Self {
            destination: RepoLocation::File {
                root: std::env::temp_dir().join("conary-m2-new-repo-without-key-dir"),
            },
            key_dir: None,
            publish_form: StaticPublishForm::Artifact,
        }
    }
}

fn prepare_existing_repo_context_for_tests(
    root: &std::path::Path,
) -> anyhow::Result<PreparedStaticPublishContext> {
    let key_dir = root.join("keys-local");
    std::fs::create_dir_all(&key_dir)?;
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publish");
    key.save_to_files(
        &key_dir.join("publish.private"),
        &key_dir.join("publish.public"),
    )?;
    let repo_root = root.join("repo");
    publish_static_repo(StaticPublishOptions {
        repo_name: "test-repo".to_string(),
        repo_description: None,
        destination: RepoLocation::File {
            root: repo_root.clone(),
        },
        key_dir: key_dir.clone(),
        state_file: root.join("last-published.toml"),
        package_paths: Vec::new(),
        refresh: false,
        rotate_publish_key: false,
        rotate_root_key: false,
        artifact_gate_context: None,
    })?;
    StaticPublishPrepareOptions {
        destination: RepoLocation::File { root: repo_root },
        key_dir: Some(key_dir),
        publish_form: StaticPublishForm::Artifact,
    }
    .prepare()
}
