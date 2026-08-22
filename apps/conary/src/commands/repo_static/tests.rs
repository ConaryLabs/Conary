// apps/conary/src/commands/repo_static/tests.rs

use std::collections::BTreeSet;
use std::future::Future;
use std::path::Path;

use clap::Parser;
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::db::models::{
    Repository, RepositoryPackage, RepositoryPackageKey, RepositoryPackageKeyStatus,
    RepositoryProvide, SecurityAdvisorySupport,
};
use conary_core::repository::dependency_model::CapabilityProvenance;
use conary_core::trust::ceremony::{create_initial_root, create_initial_root_single_key};
use conary_core::trust::keys::{sign_tuf_metadata, signing_keypair_to_tuf_key};
use conary_core::trust::metadata::{RootMetadata, Signed};
use rusqlite::{Connection, params};

use super::test_support::with_static_repo_prompt_override;
use super::*;
use crate::cli::{Cli, Commands, RepoCommands};
use crate::commands::{RepoAddOptions, cmd_repo_add};

const OTHER_VALID_KEY_ID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn parse_cli<const N: usize>(args: [&str; N]) -> Result<Cli, clap::Error> {
    let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || <Cli as Parser>::try_parse_from(args))
        .expect("parser thread should spawn")
        .join()
        .expect("parser thread should not panic")
}

struct TestDb {
    _tempdir: tempfile::TempDir,
    db_path: String,
}

impl TestDb {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let db_path = tempdir.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        Self {
            _tempdir: tempdir,
            db_path: db_path.to_string_lossy().to_string(),
        }
    }

    fn conn(&self) -> Connection {
        conary_core::db::open(&self.db_path).unwrap()
    }
}

struct StaticRepoFixture {
    _tempdir: tempfile::TempDir,
    root_key_ids: Vec<String>,
    base_url: String,
}

impl StaticRepoFixture {
    fn single_key(name: &str) -> Self {
        let root_key = SigningKeyPair::generate();
        let root = create_initial_root_single_key(&root_key, 365).unwrap();
        let root_key_ids = root_role_key_ids(&root);
        Self::from_root(name, Some("fixture static repo"), root_key_ids, root)
    }

    fn multi_root_key(name: &str) -> Self {
        let root_key = SigningKeyPair::generate();
        let second_root_key = SigningKeyPair::generate();
        let targets_key = SigningKeyPair::generate();
        let snapshot_key = SigningKeyPair::generate();
        let timestamp_key = SigningKeyPair::generate();

        let mut root =
            create_initial_root(&root_key, &targets_key, &snapshot_key, &timestamp_key, 365)
                .unwrap();
        let (second_key_id, second_tuf_key) = signing_keypair_to_tuf_key(&second_root_key).unwrap();
        root.signed
            .keys
            .insert(second_key_id.clone(), second_tuf_key);
        root.signed
            .roles
            .get_mut("root")
            .unwrap()
            .keyids
            .push(second_key_id);
        root.signatures = vec![sign_tuf_metadata(&root_key, &root.signed).unwrap()];

        let root_key_ids = root_role_key_ids(&root);
        Self::from_root(name, Some("fixture static repo"), root_key_ids, root)
    }

    fn with_identity_root_ids(name: &str, identity_root_key_ids: Vec<String>) -> Self {
        let root_key = SigningKeyPair::generate();
        let root = create_initial_root_single_key(&root_key, 365).unwrap();
        Self::from_root(
            name,
            Some("fixture static repo"),
            identity_root_key_ids,
            root,
        )
    }

    fn with_relabelled_root_key(name: &str) -> Self {
        let victim_key = SigningKeyPair::generate();
        let attacker_key = SigningKeyPair::generate();
        let mut root = create_initial_root_single_key(&victim_key, 365).unwrap();
        let victim_key_id = root_role_key_ids(&root)[0].clone();
        let (_, attacker_tuf_key) = signing_keypair_to_tuf_key(&attacker_key).unwrap();
        root.signed
            .keys
            .insert(victim_key_id.clone(), attacker_tuf_key);
        let mut attacker_signature = sign_tuf_metadata(&attacker_key, &root.signed).unwrap();
        attacker_signature.keyid = victim_key_id.clone();
        root.signatures = vec![attacker_signature];

        Self::from_root(name, Some("fixture static repo"), vec![victim_key_id], root)
    }

    fn with_zero_root_threshold(name: &str) -> Self {
        let root_key = SigningKeyPair::generate();
        let mut root = create_initial_root_single_key(&root_key, 365).unwrap();
        root.signed.roles.get_mut("root").unwrap().threshold = 0;
        let root_key_ids = root_role_key_ids(&root);
        Self::from_root(name, Some("fixture static repo"), root_key_ids, root)
    }

    fn from_root(
        name: &str,
        description: Option<&str>,
        identity_root_key_ids: Vec<String>,
        root: Signed<RootMetadata>,
    ) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let metadata_dir = tempdir.path().join("metadata");
        std::fs::create_dir_all(&metadata_dir).unwrap();
        std::fs::write(
            tempdir.path().join("conary-repo.toml"),
            repo_identity_toml(name, description, &identity_root_key_ids),
        )
        .unwrap();
        std::fs::write(
            metadata_dir.join("root.json"),
            serde_json::to_vec_pretty(&root).unwrap(),
        )
        .unwrap();
        let base_url = format!("file://{}", tempdir.path().display());
        Self {
            _tempdir: tempdir,
            root_key_ids: root_role_key_ids(&root),
            base_url,
        }
    }

    fn metadata_url(&self) -> String {
        format!("{}/metadata", self.base_url)
    }
}

fn repo_identity_toml(name: &str, description: Option<&str>, root_key_ids: &[String]) -> String {
    let root_keys = root_key_ids
        .iter()
        .map(|key_id| format!("\"{key_id}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let description = description
        .map(|value| format!("description = \"{value}\"\n"))
        .unwrap_or_default();
    let schema = conary_core::repository::static_repo::SCHEMA_VERSION;
    format!(
        "schema = {schema}\n[repo]\nname = \"{name}\"\n{description}[trust]\nroot_key_ids = [{root_keys}]\n"
    )
}

fn root_role_key_ids(root: &Signed<RootMetadata>) -> Vec<String> {
    root.signed.roles["root"].keyids.clone()
}

async fn add_static_repo(
    db: &TestDb,
    fixture: &StaticRepoFixture,
    fingerprints: Vec<String>,
) -> anyhow::Result<()> {
    add_static_repo_with(db, fixture, fingerprints, false, false, false, None).await
}

async fn add_static_repo_with(
    db: &TestDb,
    fixture: &StaticRepoFixture,
    fingerprints: Vec<String>,
    replace: bool,
    yes: bool,
    native_trust: bool,
    source_profile: Option<&str>,
) -> anyhow::Result<()> {
    let debian_release_keys = if native_trust {
        vec![
            conary_core::repository::OpenPgpTrustRoot::new(
                "https://keys.example.test/archive.asc".to_string(),
                "A".repeat(40),
            )
            .unwrap(),
        ]
    } else {
        Vec::new()
    };
    cmd_repo_add(RepoAddOptions {
        name: "acme".to_string(),
        url: fixture.base_url.clone(),
        package_format: None,
        distribution: None,
        component: None,
        architecture: None,
        database: None,
        db_path: db.db_path.clone(),
        content_url: None,
        priority: 50,
        disabled: false,
        debian_release_keys,
        rpm_metadata_keys: Vec::new(),
        rpm_metalink: None,
        rpm_package_keys: Vec::new(),
        arch_keyring: None,
        arch_keyring_format: None,
        arch_master_keys: Vec::new(),
        arch_packager_key_threshold: None,
        arch_database_signature: None,
        default_strategy: None,
        remi_endpoint: None,
        remi_metadata_root: None,
        ccs_package_keys: Vec::new(),
        source_profile: source_profile.map(str::to_string),
        source_id: None,
        repository_id: None,
        stream_kind: None,
        stream_id: None,
        policy_group: None,
        follow: false,
        pin_snapshot_sha256: None,
        security_advisory_support: SecurityAdvisorySupport::Unknown,
        fingerprints,
        yes,
        replace,
    })
    .await
}

/// Add a repository whose format the caller declares, the shape every Remi
/// consumer uses (`--package-format json --source-profile <profile>`).
async fn add_repo_with_declared_format(
    db: &TestDb,
    url: &str,
    source_profile: &str,
) -> anyhow::Result<()> {
    let authority_dir = tempfile::tempdir().unwrap();
    let private_key = authority_dir.path().join("targets.private");
    let public_key = authority_dir.path().join("targets.public");
    conary_core::ccs::signing::SigningKeyPair::generate()
        .with_key_id("targets")
        .save_to_files(&private_key, &public_key)
        .unwrap();
    let root_path = authority_dir.path().join("universe-root.json");
    let root_key = conary_core::ccs::signing::SigningKeyPair::generate();
    let targets_key = conary_core::ccs::signing::SigningKeyPair::generate();
    let snapshot_key = conary_core::ccs::signing::SigningKeyPair::generate();
    let timestamp_key = conary_core::ccs::signing::SigningKeyPair::generate();
    let root = conary_core::trust::ceremony::create_initial_root(
        &root_key,
        &targets_key,
        &snapshot_key,
        &timestamp_key,
        30,
    )
    .unwrap();
    std::fs::write(
        &root_path,
        conary_core::json::canonical_json(&root).unwrap(),
    )
    .unwrap();
    cmd_repo_add(RepoAddOptions {
        name: "acme".to_string(),
        url: url.to_string(),
        package_format: Some(conary_core::repository::RepositoryFormat::Json),
        distribution: None,
        component: None,
        architecture: None,
        database: None,
        db_path: db.db_path.clone(),
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
        default_strategy: Some("remi".to_string()),
        remi_endpoint: Some(url.to_string()),
        remi_metadata_root: Some(root_path),
        ccs_package_keys: vec![public_key],
        source_profile: Some(source_profile.to_string()),
        source_id: None,
        repository_id: None,
        stream_kind: None,
        stream_id: None,
        policy_group: None,
        follow: false,
        pin_snapshot_sha256: None,
        security_advisory_support: SecurityAdvisorySupport::Unknown,
        fingerprints: Vec::new(),
        yes: true,
        replace: false,
    })
    .await
}

fn assert_no_repo(conn: &Connection, name: &str) {
    assert!(
        Repository::find_by_name(conn, name).unwrap().is_none(),
        "repository should not have been persisted"
    );
}

fn repo(conn: &Connection) -> Repository {
    Repository::find_by_name(conn, "acme").unwrap().unwrap()
}

fn stored_tuf_key_ids(conn: &Connection, repo_id: i64) -> BTreeSet<String> {
    conn.prepare("SELECT id FROM tuf_keys WHERE repository_id = ?1 ORDER BY id")
        .unwrap()
        .query_map([repo_id], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<BTreeSet<_>>>()
        .unwrap()
}

fn count_rows(conn: &Connection, table: &str, repo_id: i64) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE repository_id = ?1");
    conn.query_row(&sql, [repo_id], |row| row.get(0)).unwrap()
}

fn insert_synced_visibility(conn: &Connection, repo_id: i64) {
    let mut package = RepositoryPackage::new(
        repo_id,
        "acme-widget".to_string(),
        "1.0-1".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "abc".to_string(),
        42,
        "packages/acme-widget/acme-widget-1.0-1-x86_64.ccs".to_string(),
    );
    package.architecture = Some("x86_64".to_string());
    let package_id = package.insert(conn).unwrap();
    RepositoryProvide::new(
        package_id,
        "acme-widget".to_string(),
        None,
        "package".to_string(),
        None,
        conary_core::repository::versioning::VersionScheme::Rpm,
    )
    .with_provenance(CapabilityProvenance::ExactIdentity)
    .insert(conn)
    .unwrap();
    conn.execute(
        "INSERT INTO tuf_targets
         (repository_id, target_path, sha256, length, custom_json, targets_version)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![repo_id, "packages/acme-widget.ccs", "abc", 42, 1],
    )
    .unwrap();
    RepositoryPackageKey::replace_for_repository(
        conn,
        repo_id,
        &[RepositoryPackageKey {
            repository_id: repo_id,
            public_key: "package-key".to_string(),
            key_id: Some("package-key-id".to_string()),
            status: RepositoryPackageKeyStatus::Active,
            synced_at: None,
        }],
    )
    .unwrap();
}

async fn with_prompt_override<F, Fut, T>(
    interactive: bool,
    accept: bool,
    f: F,
) -> (T, Option<String>)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    with_static_repo_prompt_override(interactive, accept, f).await
}

#[tokio::test]
async fn fingerprint_mismatch_fails_before_insert() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    let err = add_static_repo(&db, &fixture, vec![OTHER_VALID_KEY_ID.to_string()])
        .await
        .unwrap_err();

    assert!(err.to_string().contains("fingerprint"));
    assert_no_repo(&db.conn(), "acme");
}

#[tokio::test]
async fn exact_fingerprint_set_inserts_tuf_enabled_repo() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap();

    let conn = db.conn();
    let repo = repo(&conn);
    assert!(repo.tuf_enabled);
    assert_eq!(
        repo.tuf_root_url.as_deref(),
        Some(fixture.metadata_url().as_str())
    );
}

#[tokio::test]
async fn multi_key_root_exact_set_fingerprint_match_inserts_tuf_enabled_repo() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::multi_root_key("acme-static");

    add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap();

    assert!(repo(&db.conn()).tuf_enabled);
}

#[tokio::test]
async fn fingerprint_subset_fails_when_root_role_has_extra_key_ids() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::multi_root_key("acme-static");

    let err = add_static_repo(&db, &fixture, vec![fixture.root_key_ids[0].clone()])
        .await
        .unwrap_err();

    assert!(err.to_string().contains("fingerprint"));
    assert_no_repo(&db.conn(), "acme");
}

#[tokio::test]
async fn fingerprint_superset_fails_when_supplied_set_contains_unserved_key_id() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");
    let mut fingerprints = fixture.root_key_ids.clone();
    fingerprints.push(OTHER_VALID_KEY_ID.to_string());

    let err = add_static_repo(&db, &fixture, fingerprints)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("fingerprint"));
    assert_no_repo(&db.conn(), "acme");
}

#[tokio::test]
async fn duplicate_fingerprints_after_normalization_fail_as_ambiguous_input() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");
    let lower = fixture.root_key_ids[0].clone();
    let upper = lower.to_ascii_uppercase();

    let err = add_static_repo(&db, &fixture, vec![lower, upper])
        .await
        .unwrap_err();

    assert!(err.to_string().contains("duplicate"));
    assert_no_repo(&db.conn(), "acme");
}

#[tokio::test]
async fn inserted_static_repo_has_metadata_url_and_static_strategy() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap();

    let repo = repo(&db.conn());
    assert_eq!(
        repo.tuf_root_url.as_deref(),
        Some(fixture.metadata_url().as_str())
    );
    assert_eq!(repo.default_strategy.as_deref(), Some("static"));
}

#[tokio::test]
async fn distro_specific_static_repo_preserves_its_exact_source_profile() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    add_static_repo_with(
        &db,
        &fixture,
        fixture.root_key_ids.clone(),
        false,
        false,
        false,
        Some("arch"),
    )
    .await
    .unwrap();

    let repo = repo(&db.conn());
    assert_eq!(repo.default_strategy.as_deref(), Some("static"));
    assert_eq!(repo.source_profile.as_deref(), Some("arch"));
    assert_eq!(
        repo.resolution_source_profile().unwrap().unwrap().id(),
        "arch"
    );
}

#[tokio::test]
async fn static_repo_add_rejects_native_trust_flags_after_probe_without_fingerprint() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    let err = add_static_repo_with(&db, &fixture, Vec::new(), false, false, true, None)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("native repository trust flags"));
    assert_no_repo(&db.conn(), "acme");
}

/// A host whose identity probe would fail must not decide a repository whose
/// package format was already declared. The reserved `.invalid` origin also
/// proves this path performs no discovery request.
#[tokio::test]
async fn declared_package_format_adds_a_native_repo_whatever_the_identity_probe_would_find() {
    let db = TestDb::new();
    let base_url = "http://identity-probe-must-not-run.invalid";

    add_repo_with_declared_format(&db, base_url, "fedora-44")
        .await
        .unwrap();

    let repo = repo(&db.conn());
    assert_eq!(repo.url, base_url);
    assert_eq!(repo.source_profile.as_deref(), Some("fedora-44"));
    assert_ne!(repo.default_strategy.as_deref(), Some("static"));
    assert_eq!(repo.tuf_root_url, None);
}

/// The probe still owns the undeclared case, so the test above cannot pass by
/// the probe having been removed.
#[tokio::test]
async fn undeclared_package_format_still_reaches_the_identity_probe() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap();

    assert_eq!(repo(&db.conn()).default_strategy.as_deref(), Some("static"));
}

#[test]
fn manual_default_strategy_static_is_rejected_at_parse_time() {
    assert!(
        parse_cli([
            "conary",
            "repo",
            "add",
            "acme",
            "file:///tmp/repo",
            "--default-strategy",
            "static",
        ])
        .is_err()
    );
}

#[tokio::test]
async fn non_interactive_tofu_fails_when_no_fingerprint_is_supplied() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    let (result, _prompt) = with_prompt_override(false, true, || async {
        add_static_repo(&db, &fixture, Vec::new()).await
    })
    .await;

    let err = result.unwrap_err();
    assert!(err.to_string().contains("non-interactive"));
    assert_no_repo(&db.conn(), "acme");
}

#[test]
fn conary_non_interactive_env_one_is_non_interactive() {
    assert!(conary_non_interactive_env_is_enabled_for_value(Some("1")));
}

#[tokio::test]
async fn interactive_tofu_prompt_includes_stale_root_replay_caveat() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");

    let (result, prompt) = with_prompt_override(true, true, || async {
        add_static_repo(&db, &fixture, Vec::new()).await
    })
    .await;

    result.unwrap();
    let prompt = prompt.expect("interactive TOFU should render a prompt");
    assert!(prompt.contains("TOFU cannot detect a replayed old root"));
    assert!(prompt.contains("on-path attacker can pin a stale identity"));
}

#[tokio::test]
async fn reset_trust_removes_trust_material_and_synced_package_visibility() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::single_key("acme-static");
    add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap();
    let conn = db.conn();
    let added_repo = repo(&conn);
    let repo_id = added_repo.id.unwrap();
    insert_synced_visibility(&conn, repo_id);
    drop(conn);

    cmd_repo_reset_trust("acme", &db.db_path).await.unwrap();

    let conn = db.conn();
    let repo = repo(&conn);
    assert!(!repo.enabled);
    assert!(!repo.tuf_enabled);
    assert_eq!(repo.tuf_root_version, None);
    assert_eq!(repo.default_strategy.as_deref(), Some("static"));
    assert_eq!(count_rows(&conn, "tuf_roots", repo_id), 0);
    assert_eq!(count_rows(&conn, "tuf_keys", repo_id), 0);
    assert_eq!(count_rows(&conn, "tuf_metadata", repo_id), 0);
    assert_eq!(count_rows(&conn, "tuf_targets", repo_id), 0);
    assert_eq!(count_rows(&conn, "repository_package_keys", repo_id), 0);
    assert_eq!(
        RepositoryPackage::find_by_repository(&conn, repo_id)
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn reset_trust_rejects_non_static_repositories_without_changing_visibility() {
    let db = TestDb::new();
    let conn = db.conn();
    let mut native = Repository::new(
        "acme".to_string(),
        "https://example.invalid/repo".to_string(),
    );
    native.default_strategy = Some("binary".to_string());
    let repo_id = native.insert(&conn).unwrap();
    insert_synced_visibility(&conn, repo_id);
    drop(conn);

    let err = cmd_repo_reset_trust("acme", &db.db_path).await.unwrap_err();

    let conn = db.conn();
    let repo = repo(&conn);
    assert!(err.to_string().contains("static repositories"));
    assert!(repo.enabled);
    assert_eq!(repo.default_strategy.as_deref(), Some("binary"));
    assert_eq!(
        RepositoryPackage::find_by_repository(&conn, repo_id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(count_rows(&conn, "repository_package_keys", repo_id), 1);
}

#[tokio::test]
async fn duplicate_name_without_replace_preserves_existing_trust() {
    let db = TestDb::new();
    let first = StaticRepoFixture::single_key("acme-static");
    let second = StaticRepoFixture::single_key("acme-static");
    add_static_repo(&db, &first, first.root_key_ids.clone())
        .await
        .unwrap();
    let conn = db.conn();
    let repo_id = repo(&conn).id.unwrap();
    let before = stored_tuf_key_ids(&conn, repo_id);
    drop(conn);

    let err = add_static_repo(&db, &second, second.root_key_ids.clone())
        .await
        .unwrap_err();

    let conn = db.conn();
    assert!(err.to_string().contains("already exists"));
    assert_eq!(stored_tuf_key_ids(&conn, repo_id), before);
}

#[tokio::test]
async fn replace_updates_existing_row_and_bootstraps_new_root() {
    let db = TestDb::new();
    let first = StaticRepoFixture::single_key("acme-static");
    let second = StaticRepoFixture::single_key("acme-static");
    add_static_repo(&db, &first, first.root_key_ids.clone())
        .await
        .unwrap();
    let conn = db.conn();
    let repo_id = repo(&conn).id.unwrap();
    drop(conn);

    add_static_repo_with(
        &db,
        &second,
        second.root_key_ids.clone(),
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    let conn = db.conn();
    let repo = repo(&conn);
    assert_eq!(repo.id, Some(repo_id));
    assert_eq!(repo.url, second.base_url);
    assert_eq!(
        stored_tuf_key_ids(&conn, repo_id),
        second.root_key_ids.iter().cloned().collect()
    );
}

#[tokio::test]
async fn reset_then_repin_reestablishes_trust_and_reenables_sync() {
    let db = TestDb::new();
    let first = StaticRepoFixture::single_key("acme-static");
    let second = StaticRepoFixture::single_key("acme-static");
    add_static_repo(&db, &first, first.root_key_ids.clone())
        .await
        .unwrap();
    cmd_repo_reset_trust("acme", &db.db_path).await.unwrap();

    add_static_repo_with(
        &db,
        &second,
        second.root_key_ids.clone(),
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    let conn = db.conn();
    let repo = repo(&conn);
    assert!(repo.enabled);
    assert!(repo.tuf_enabled);
    assert_eq!(repo.default_strategy.as_deref(), Some("static"));
    assert_eq!(
        repo.tuf_root_url.as_deref(),
        Some(second.metadata_url().as_str())
    );
    assert_eq!(
        stored_tuf_key_ids(&conn, repo.id.unwrap()),
        second.root_key_ids.iter().cloned().collect()
    );
}

#[tokio::test]
async fn identity_root_key_mismatch_fails_before_insert() {
    let db = TestDb::new();
    let fixture =
        StaticRepoFixture::with_identity_root_ids("acme-static", vec![OTHER_VALID_KEY_ID.into()]);

    let err = add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap_err();

    assert!(err.to_string().contains("conary-repo.toml"));
    assert_no_repo(&db.conn(), "acme");
}

#[tokio::test]
async fn relabelled_root_key_id_fails_before_insert() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::with_relabelled_root_key("acme-static");

    let err = add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap_err();

    assert!(err.to_string().contains("Root key ID"));
    assert_no_repo(&db.conn(), "acme");
}

#[tokio::test]
async fn zero_root_threshold_fails_before_insert() {
    let db = TestDb::new();
    let fixture = StaticRepoFixture::with_zero_root_threshold("acme-static");

    let err = add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
        .await
        .unwrap_err();

    assert!(err.to_string().contains("threshold"));
    assert_no_repo(&db.conn(), "acme");
}

#[tokio::test]
async fn static_identity_probe_error_does_not_fall_back_to_native_add() {
    let db = TestDb::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("conary-repo.toml")).unwrap();
    let fixture = StaticRepoFixture {
        _tempdir: dir,
        root_key_ids: Vec::new(),
        base_url: String::new(),
    };
    let base_url = format!("file://{}", fixture._tempdir.path().display());

    let result = cmd_repo_add(RepoAddOptions {
        name: "acme".to_string(),
        url: base_url,
        package_format: None,
        distribution: None,
        component: None,
        architecture: None,
        database: None,
        db_path: db.db_path.clone(),
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
        default_strategy: None,
        remi_endpoint: None,
        remi_metadata_root: None,
        ccs_package_keys: Vec::new(),
        source_profile: None,
        source_id: None,
        repository_id: None,
        stream_kind: None,
        stream_id: None,
        policy_group: None,
        follow: false,
        pin_snapshot_sha256: None,
        security_advisory_support: SecurityAdvisorySupport::Unknown,
        fingerprints: Vec::new(),
        yes: false,
        replace: false,
    })
    .await;

    let err = result.unwrap_err();
    assert!(err.to_string().contains("probe static repository identity"));
    assert_no_repo(&db.conn(), "acme");
}

#[test]
fn repo_reset_trust_parse_shape_stays_routed_to_repo_command() {
    let cli = parse_cli(["conary", "repo", "reset-trust", "acme"]).unwrap();

    assert!(matches!(
        cli.command,
        Some(Commands::Repo(RepoCommands::ResetTrust { .. }))
    ));
}

#[test]
fn normalizes_bare_static_repo_path_to_absolute_storage_path() {
    let tempdir = tempfile::tempdir().unwrap();
    let normalized = normalize_static_repo_base_path(Path::new("."), tempdir.path()).unwrap();

    assert!(Path::new(&normalized).is_absolute());
}
