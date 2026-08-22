// apps/conary/src/commands/repo/authority_tests.rs
//! Repository command tests for CCS package-authority persistence.

use super::*;
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::db::models::{Repository, RepositoryPackageKey, SecurityAdvisorySupport};
use std::path::PathBuf;

fn universe_root(db_path: &std::path::Path) -> PathBuf {
    let path = db_path
        .parent()
        .unwrap()
        .join("test-remi-universe-root.json");
    if !path.exists() {
        let root = SigningKeyPair::generate();
        let targets = SigningKeyPair::generate();
        let snapshot = SigningKeyPair::generate();
        let timestamp = SigningKeyPair::generate();
        let signed = conary_core::trust::ceremony::create_initial_root(
            &root, &targets, &snapshot, &timestamp, 30,
        )
        .unwrap();
        std::fs::write(&path, conary_core::json::canonical_json(&signed).unwrap()).unwrap();
    }
    path
}

fn remi_repo_options(
    name: &str,
    db_path: &std::path::Path,
    endpoint: &str,
    profile: &str,
    ccs_package_keys: Vec<PathBuf>,
) -> RepoAddOptions {
    RepoAddOptions {
        name: name.to_string(),
        url: endpoint.to_string(),
        package_format: Some(RepositoryFormat::Json),
        distribution: None,
        component: None,
        architecture: None,
        database: None,
        db_path: db_path.to_string_lossy().to_string(),
        content_url: None,
        priority: 50,
        disabled: false,
        debian_release_keys: Vec::new(),
        rpm_metadata_keys: Vec::new(),
        rpm_metalink: None,
        rpm_package_keys: Vec::new(),
        arch_keyring: None,
        arch_keyring_format: None,
        arch_master_keys: Vec::new(),
        arch_packager_key_threshold: None,
        arch_database_signature: None,
        fingerprints: Vec::new(),
        yes: false,
        replace: false,
        default_strategy: Some("remi".to_string()),
        remi_endpoint: Some(endpoint.to_string()),
        remi_metadata_root: Some(universe_root(db_path)),
        ccs_package_keys,
        source_profile: Some(profile.to_string()),
        source_id: None,
        repository_id: None,
        stream_kind: None,
        stream_id: None,
        policy_group: None,
        follow: false,
        pin_snapshot_sha256: None,
        security_advisory_support: SecurityAdvisorySupport::Unknown,
    }
}

fn binary_repo_options(
    name: &str,
    db_path: &std::path::Path,
    endpoint: &str,
    profile: &str,
    ccs_package_keys: Vec<PathBuf>,
) -> RepoAddOptions {
    let mut options = remi_repo_options(name, db_path, endpoint, profile, ccs_package_keys);
    options.default_strategy = Some("binary".to_string());
    options.remi_endpoint = None;
    options.remi_metadata_root = None;
    options
}

#[tokio::test]
async fn repo_add_seeds_exact_canonical_remi_package_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    cmd_repo_add(remi_repo_options(
        "canonical-fedora",
        &db_path,
        "https://remi.conary.io/",
        "fedora-44",
        Vec::new(),
    ))
    .await
    .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let repo = Repository::find_by_name(&conn, "canonical-fedora")
        .unwrap()
        .unwrap();
    let expected = conary_core::repository::remi_authority::canonical_remi_package_keys(
        "https://remi.conary.io",
        "fedora-44",
    )
    .unwrap()[0]
        .public_key
        .clone();
    assert_eq!(
        RepositoryPackageKey::trusted_keys_for_repository(&conn, repo.id.unwrap()).unwrap(),
        vec![expected]
    );
}

#[tokio::test]
async fn repo_add_requires_explicit_authority_for_self_hosted_remi_without_mutation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let error = cmd_repo_add(remi_repo_options(
        "self-hosted",
        &db_path,
        "https://remi.example.invalid",
        "fedora-44",
        Vec::new(),
    ))
    .await
    .unwrap_err();

    assert!(error.to_string().contains("--ccs-package-key"), "{error:#}");
    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        Repository::find_by_name(&conn, "self-hosted")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn repo_add_rejects_canonical_authority_override_without_mutation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let private_key = temp_dir.path().join("targets.private");
    let public_key = temp_dir.path().join("targets.public");
    SigningKeyPair::generate()
        .with_key_id("targets")
        .save_to_files(&private_key, &public_key)
        .unwrap();

    let error = cmd_repo_add(remi_repo_options(
        "canonical-override",
        &db_path,
        "https://remi.conary.io",
        "fedora-44",
        vec![public_key],
    ))
    .await
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("cannot override release-tracked CCS package authority"),
        "{error:#}"
    );
    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        Repository::find_by_name(&conn, "canonical-override")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn repo_add_rolls_back_repository_when_authority_persistence_fails() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    {
        let conn = conary_core::db::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_test_package_authority
             BEFORE INSERT ON repository_package_keys
             BEGIN
                 SELECT RAISE(ABORT, 'simulated package authority persistence failure');
             END;",
        )
        .unwrap();
    }

    let error = cmd_repo_add(remi_repo_options(
        "canonical-rollback",
        &db_path,
        "https://remi.conary.io",
        "fedora-44",
        Vec::new(),
    ))
    .await
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("persist verified CCS package authority"),
        "{error:#}"
    );
    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        Repository::find_by_name(&conn, "canonical-rollback")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn repo_add_persists_explicit_self_hosted_authority_and_rejects_duplicates() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let private_key = temp_dir.path().join("targets.private");
    let public_key = temp_dir.path().join("targets.public");
    let signing_key = SigningKeyPair::generate().with_key_id("targets");
    signing_key
        .save_to_files(&private_key, &public_key)
        .unwrap();

    let duplicate_error = cmd_repo_add(remi_repo_options(
        "duplicate-authority",
        &db_path,
        "https://remi.example.invalid",
        "fedora-44",
        vec![public_key.clone(), public_key.clone()],
    ))
    .await
    .unwrap_err();
    assert!(
        duplicate_error.to_string().contains("repeats public key"),
        "{duplicate_error:#}"
    );
    {
        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            Repository::find_by_name(&conn, "duplicate-authority")
                .unwrap()
                .is_none()
        );
    }

    cmd_repo_add(remi_repo_options(
        "self-hosted",
        &db_path,
        "https://remi.example.invalid",
        "fedora-44",
        vec![public_key],
    ))
    .await
    .unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let repo = Repository::find_by_name(&conn, "self-hosted")
        .unwrap()
        .unwrap();
    assert_eq!(
        RepositoryPackageKey::trusted_keys_for_repository(&conn, repo.id.unwrap()).unwrap(),
        vec![signing_key.public_key_base64()]
    );
}

#[tokio::test]
async fn repo_add_persists_explicit_binary_ccs_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let private_key = temp_dir.path().join("binary.private");
    let public_key = temp_dir.path().join("binary.public");
    let signing_key = SigningKeyPair::generate().with_key_id("binary");
    signing_key
        .save_to_files(&private_key, &public_key)
        .unwrap();

    cmd_repo_add(binary_repo_options(
        "binary-ccs",
        &db_path,
        "https://packages.example.invalid",
        "arch",
        vec![public_key],
    ))
    .await
    .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let repo = Repository::find_by_name(&conn, "binary-ccs")
        .unwrap()
        .unwrap();
    assert_eq!(repo.default_strategy.as_deref(), Some("binary"));
    assert_eq!(repo.source_profile.as_deref(), Some("arch"));
    assert_eq!(
        RepositoryPackageKey::trusted_keys_for_repository(&conn, repo.id.unwrap()).unwrap(),
        vec![signing_key.public_key_base64()]
    );
}
