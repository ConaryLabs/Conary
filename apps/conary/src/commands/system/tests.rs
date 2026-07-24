// conary/src/commands/system/tests.rs

use super::{
    HOST_PROFILE_SETTING, NATIVE_REPOSITORY_SEEDS, cmd_init, cmd_rollback,
    paths_refer_to_same_location, restore_snapshots_to_live_root, rollback_claim_statuses,
    validate_init_privileges,
};
use crate::commands::{FileSnapshot, RevertMetadata, TroveSnapshot, parse_rollback_snapshots};
use conary_core::ccs::legacy_scriptlets::{
    DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
    LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy, PublicationStatus,
    ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder,
    VersionScheme,
};
use conary_core::db::models::{
    Changeset, ChangesetStatus, FileEntry, InstallSource, InstalledLegacyScriptletBundle,
    PackageResolution, Repository, RepositoryPackage, Trove, TroveType, settings,
};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use rusqlite::params;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[tokio::test]
async fn init_seeds_only_the_selected_profile_repositories() {
    let cases: [(&str, &[&str]); 3] = [
        ("fedora-44", &["fedora-44"]),
        ("ubuntu-26.04", &["ubuntu-26.04"]),
        ("arch", &["arch-core", "arch-extra", "arch-multilib"]),
    ];

    for (profile_id, expected_native_repos) in cases {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        cmd_init(db_path_str, profile_id).await.unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let remi = Repository::find_by_name(&conn, "remi").unwrap().unwrap();
        assert_eq!(remi.url, "https://remi.conary.io");
        assert_eq!(remi.default_strategy.as_deref(), Some("remi"));
        assert_eq!(
            remi.default_strategy_endpoint.as_deref(),
            Some("https://remi.conary.io")
        );
        assert_eq!(remi.default_strategy_distro.as_deref(), Some(profile_id));

        let native_repos = Repository::list_all(&conn)
            .unwrap()
            .into_iter()
            .filter(|repo| repo.name != "remi")
            .map(|repo| {
                assert!(!repo.enabled, "{} should start disabled", repo.name);
                repo.name
            })
            .collect::<std::collections::BTreeSet<_>>();
        let expected = expected_native_repos
            .iter()
            .map(|name| (*name).to_string())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(native_repos, expected, "profile {profile_id}");
        assert_eq!(
            settings::get(&conn, HOST_PROFILE_SETTING)
                .unwrap()
                .as_deref(),
            Some(profile_id)
        );
    }
}

#[tokio::test]
async fn init_reconciles_the_legacy_all_distro_seed_to_the_selected_profile() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str, "fedora-44").await.unwrap();
    {
        let conn = conary_core::db::open(db_path_str).unwrap();
        for seed in NATIVE_REPOSITORY_SEEDS {
            if Repository::find_by_name(&conn, seed.name)
                .unwrap()
                .is_none()
            {
                conary_core::repository::add_repository(
                    &conn,
                    seed.name.to_string(),
                    seed.legacy_urls
                        .first()
                        .copied()
                        .unwrap_or(seed.url)
                        .to_string(),
                    true,
                    seed.priority,
                )
                .unwrap();
            }
        }

        let mut ubuntu = Repository::find_by_name(&conn, "ubuntu-26.04")
            .unwrap()
            .unwrap();
        ubuntu.last_sync = Some("2026-07-18T00:00:00Z".to_string());
        ubuntu.update(&conn).unwrap();
        let ubuntu_id = ubuntu.id.unwrap();
        let mut package = RepositoryPackage::new(
            ubuntu_id,
            "legacy-http-package".to_string(),
            "1".to_string(),
            "sha256:test".to_string(),
            1,
            "http://archive.ubuntu.com/ubuntu/pool/legacy-http-package.deb".to_string(),
        );
        package.insert(&conn).unwrap();
        let mut resolution = PackageResolution::binary(
            ubuntu_id,
            "legacy-http-package".to_string(),
            "http://archive.ubuntu.com/ubuntu/pool/legacy-http-package.deb".to_string(),
            "sha256:test".to_string(),
        );
        resolution.insert(&conn).unwrap();
        settings::delete(&conn, HOST_PROFILE_SETTING).unwrap();

        let mut remi = Repository::find_by_name(&conn, "remi").unwrap().unwrap();
        remi.last_sync = Some("2026-07-17T00:00:00Z".to_string());
        remi.update(&conn).unwrap();
        let mut package = RepositoryPackage::new(
            remi.id.unwrap(),
            "fedora-only".to_string(),
            "1".to_string(),
            "sha256:test".to_string(),
            1,
            "https://example.invalid/fedora-only".to_string(),
        );
        package.insert(&conn).unwrap();
        let mut resolution = PackageResolution::remi(
            remi.id.unwrap(),
            "fedora-only".to_string(),
            "https://remi.conary.io".to_string(),
            "fedora-44".to_string(),
        );
        resolution.insert(&conn).unwrap();
        conn.execute(
            "INSERT INTO repository_package_keys
                    (repository_id, public_key, key_id, status)
                 VALUES (?1, 'stale-fedora-key', 'stale-fedora-key-id', 'active')",
            [remi.id.unwrap()],
        )
        .unwrap();
    }

    cmd_init(db_path_str, "ubuntu-26.04").await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let remi = Repository::find_by_name(&conn, "remi").unwrap().unwrap();
    assert_eq!(
        remi.default_strategy_distro.as_deref(),
        Some("ubuntu-26.04")
    );
    assert!(remi.last_sync.is_none());
    assert!(
        RepositoryPackage::find_by_repository(&conn, remi.id.unwrap())
            .unwrap()
            .is_empty()
    );
    assert!(
        PackageResolution::find_by_repository(&conn, remi.id.unwrap())
            .unwrap()
            .is_empty()
    );
    let package_key_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM repository_package_keys WHERE repository_id = ?1",
            [remi.id.unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(package_key_count, 0);

    let enabled_native_repos = Repository::list_all(&conn)
        .unwrap()
        .into_iter()
        .filter(|repo| repo.name != "remi" && repo.enabled)
        .map(|repo| repo.name)
        .collect::<Vec<_>>();
    assert!(enabled_native_repos.is_empty());
    assert_eq!(
        Repository::find_by_name(&conn, "ubuntu-26.04")
            .unwrap()
            .unwrap()
            .url,
        "https://archive.ubuntu.com/ubuntu"
    );
    let ubuntu = Repository::find_by_name(&conn, "ubuntu-26.04")
        .unwrap()
        .unwrap();
    assert!(ubuntu.last_sync.is_none());
    assert!(
        RepositoryPackage::find_by_repository(&conn, ubuntu.id.unwrap())
            .unwrap()
            .is_empty()
    );
    assert!(
        PackageResolution::find_by_repository(&conn, ubuntu.id.unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        settings::get(&conn, HOST_PROFILE_SETTING)
            .unwrap()
            .as_deref(),
        Some("ubuntu-26.04")
    );
}

#[tokio::test]
async fn init_rerun_preserves_same_profile_repository_choices() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str, "arch").await.unwrap();
    {
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut arch_core = Repository::find_by_name(&conn, "arch-core")
            .unwrap()
            .unwrap();
        arch_core.enabled = false;
        arch_core.update(&conn).unwrap();

        let mut arch_extra = Repository::find_by_name(&conn, "arch-extra")
            .unwrap()
            .unwrap();
        arch_extra.url = "https://mirror.example.invalid/arch-extra".to_string();
        arch_extra.update(&conn).unwrap();

        let mut remi = Repository::find_by_name(&conn, "remi").unwrap().unwrap();
        remi.url = "https://mirror.example.invalid/remi".to_string();
        remi.default_strategy_endpoint = Some(remi.url.clone());
        remi.default_strategy_distro = Some("fedora-44".to_string());
        remi.update(&conn).unwrap();
    }

    cmd_init(db_path_str, "arch").await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let arch_core = Repository::find_by_name(&conn, "arch-core")
        .unwrap()
        .unwrap();
    assert!(!arch_core.enabled);
    let arch_extra = Repository::find_by_name(&conn, "arch-extra")
        .unwrap()
        .unwrap();
    assert_eq!(arch_extra.url, "https://mirror.example.invalid/arch-extra");
    let remi = Repository::find_by_name(&conn, "remi").unwrap().unwrap();
    assert_eq!(remi.url, "https://mirror.example.invalid/remi");
    assert_eq!(remi.default_strategy_distro.as_deref(), Some("fedora-44"));
}

#[tokio::test]
async fn init_rejects_non_public_profile_before_creating_the_database() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");

    let err = cmd_init(db_path.to_str().unwrap(), "fedora")
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("unsupported host profile 'fedora'"));
    assert!(!db_path.exists());
}

#[test]
fn system_database_init_requires_root_but_custom_test_databases_do_not() {
    let default_db = Path::new("/var/lib/conary/conary.db");
    let error = validate_init_privileges(default_db, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("requires root privileges"));
    assert!(error.contains("re-run with sudo"));
    assert!(validate_init_privileges(default_db, true).is_ok());
    assert!(
        validate_init_privileges(Path::new("/var/lib/conary/../conary/conary.db"), false).is_err()
    );
    assert!(validate_init_privileges(Path::new("/tmp/conary-test.db"), false).is_ok());
}

#[cfg(unix)]
#[test]
fn privilege_path_comparison_resolves_a_dangling_database_symlink() {
    let temp_dir = tempfile::tempdir().unwrap();
    let actual_dir = temp_dir.path().join("actual");
    std::fs::create_dir(&actual_dir).unwrap();
    let actual_db = actual_dir.join("conary.db");
    let alias_db = temp_dir.path().join("database-alias");
    std::os::unix::fs::symlink("actual/conary.db", &alias_db).unwrap();

    assert!(paths_refer_to_same_location(&alias_db, &actual_db).unwrap());
}

#[cfg(unix)]
#[test]
fn privilege_check_rejects_deep_aliases_and_root_runtime_aliases() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut target = PathBuf::from("/var/lib/conary/conary.db");
    for index in (0..10).rev() {
        let alias = temp_dir.path().join(format!("alias-{index}"));
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        target = alias;
    }

    let non_root_error = validate_init_privileges(&target, false)
        .unwrap_err()
        .to_string();
    assert!(non_root_error.contains("canonical path"));

    let root_error = validate_init_privileges(&target, true)
        .unwrap_err()
        .to_string();
    assert!(root_error.contains("runtime state cannot diverge"));
}

#[tokio::test]
async fn init_error_names_unusable_database_parent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let parent_file = temp_dir.path().join("not-a-directory");
    std::fs::write(&parent_file, b"not a directory").unwrap();
    let db_path = parent_file.join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    let err = cmd_init(db_path_str, "fedora-44")
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains(&parent_file.display().to_string()));
    assert!(err.contains("safe next step"));
}

#[test]
fn rollback_claim_statuses_include_post_hooks_failed() {
    assert_eq!(rollback_claim_statuses(), ["applied", "post_hooks_failed"]);
}

#[test]
fn rollback_requires_active_generation_before_live_root_mutation() {
    let source = std::fs::read_to_string("apps/conary/src/commands/system.rs")
        .unwrap_or_else(|_| include_str!("../system.rs").to_string());
    let forbidden_restore = ["restore_snapshots_to_live_root", "(root_path"].concat();
    let forbidden_remove = ["remove_snapshots_from_live_root", "(root_path"].concat();
    assert!(
        !source.contains(&forbidden_restore),
        "rollback must not restore package payloads directly into the live root"
    );
    assert!(
        !source.contains(&forbidden_remove),
        "rollback must not remove package payloads directly from the live root"
    );
    let helper_start = source
        .find("fn rollback_changeset_with_snapshots")
        .expect("rollback snapshot helper missing");
    let helper = &source[helper_start..];
    let tx_start = helper
        .find("conary_core::db::transaction(conn")
        .expect("rollback helper transaction missing");
    let rebuild_start = helper
        .find("rebuild_and_mount")
        .expect("rollback helper rebuild missing");
    let guard_start = helper
        .find("require_active_generation_for_rollback")
        .expect("rollback helper active-generation guard missing");
    assert!(
        guard_start < tx_start,
        "rollback snapshot helper must check active generation before mutating the DB"
    );
    assert!(
        !helper[tx_start..rebuild_start].contains("require_active_generation_for_rollback"),
        "rollback snapshot helper must not fail its active-generation guard after DB mutation"
    );
}

#[test]
fn parse_rollback_snapshots_accepts_legacy_and_revert_wrapper_formats() {
    let single = TroveSnapshot {
        name: "nginx".to_string(),
        version: "1.24.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("web server".to_string()),
        install_source: "repository".to_string(),
        installed_from_repository_id: Some(7),
        files: vec![FileSnapshot {
            path: "/usr/sbin/nginx".to_string(),
            sha256_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            size: 1024,
            permissions: 0o755,
            symlink_target: None,
        }],
    };

    let parsed_single = parse_rollback_snapshots(&serde_json::to_string(&single).unwrap()).unwrap();
    assert_eq!(parsed_single.len(), 1);
    assert_eq!(parsed_single[0].name, "nginx");

    let wrapper = RevertMetadata {
        removed_troves: vec![
            single.clone(),
            TroveSnapshot {
                name: "vim".to_string(),
                version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                description: Some("editor".to_string()),
                install_source: "repository".to_string(),
                installed_from_repository_id: None,
                files: Vec::new(),
            },
        ],
    };

    let parsed_wrapper =
        parse_rollback_snapshots(&serde_json::to_string(&wrapper).unwrap()).unwrap();
    assert_eq!(parsed_wrapper.len(), 2);
    assert_eq!(parsed_wrapper[0].name, "nginx");
    assert_eq!(parsed_wrapper[1].name, "vim");
}

fn store_test_object(conn: &rusqlite::Connection, db_path: &Path, content: &[u8]) -> String {
    let cas = CasStore::new(objects_dir(&db_path.to_string_lossy())).unwrap();
    let hash = cas.store(content).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
             VALUES (?1, ?2, ?3)",
        params![
            &hash,
            format!("objects/{}/{}", &hash[0..2], &hash[2..]),
            content.len() as i64
        ],
    )
    .unwrap();
    hash
}

fn insert_test_trove(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    name: &str,
    version: &str,
    files: &[(&str, &str, i64)],
) -> i64 {
    let mut trove = Trove::new_with_source(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        InstallSource::File,
    );
    trove.installed_by_changeset_id = Some(changeset_id);
    let trove_id = trove.insert(conn).unwrap();

    for (path, hash, size) in files {
        let mut file = FileEntry::new(
            (*path).to_string(),
            (*hash).to_string(),
            *size,
            0o100644,
            trove_id,
        );
        file.insert(conn).unwrap();
    }

    trove_id
}

fn create_active_generation_link(runtime_root: &Path) {
    std::fs::create_dir_all(runtime_root.join("generations/1")).unwrap();
    std::os::unix::fs::symlink("generations/1", runtime_root.join("current")).unwrap();
}

fn rollback_legacy_entry() -> LegacyScriptletEntry {
    let body = "systemctl daemon-reload\n";
    LegacyScriptletEntry {
        id: "rpm:%post".to_string(),
        native_slot: "%post".to_string(),
        phase: LifecyclePath::PostInstall,
        lifecycle_paths: vec!["install:last".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: Vec::new(),
            after: vec!["payload".to_string()],
            extra: BTreeMap::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision: ScriptletDecision::Legacy,
        reason_code: "legacy-replay-required".to_string(),
        human_reason: Some("fixture legacy entry".to_string()),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        effects: Vec::new(),
        unknown_command_evidence: Vec::new(),
        blocked_classes: Vec::new(),
        boot_security_intents: Vec::new(),
        security_policy_intents: Vec::new(),
        rpm_trigger: None,
        deb_maintainer: None,
        arch_install: None,
        residual_replay: None,
        extra: BTreeMap::new(),
    }
}

fn rollback_legacy_bundle() -> LegacyScriptletBundle {
    LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 2,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "rollback-legacy-fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "goal6-test".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: Some(conary_core::hash::sha256_prefixed(b"rollback-evidence")),
        target_compatibility: TargetCompatibility::SourceNative,
        allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::LocalOnly,
        publication_status: PublicationStatus::LocalOnly,
        scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
        decision_counts: DecisionCounts {
            replaced: 0,
            legacy: 1,
            blocked: 0,
            review: 0,
            extra: BTreeMap::new(),
        },
        unsupported_class_counts: BTreeMap::new(),
        security_policy_intents: Vec::new(),
        entries: vec![rollback_legacy_entry()],
        extra: BTreeMap::new(),
    }
}

#[tokio::test]
async fn rollback_refuses_installed_legacy_bundle_before_deleting_trove() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    conary_core::db::init(&db_path_str).unwrap();
    create_active_generation_link(temp_dir.path());
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut changeset = Changeset::new("Install rollback legacy fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let trove_id = insert_test_trove(&conn, changeset_id, "rollback-legacy-fixture", "1.0-1", &[]);
    let bundle = rollback_legacy_bundle();
    let mut installed = InstalledLegacyScriptletBundle::new(
        trove_id,
        Some(changeset_id),
        "rpm/fedora/44/x86_64".to_string(),
        "allow-legacy-replay".to_string(),
        true,
        &bundle,
    )
    .unwrap();
    installed.insert_or_replace(&conn).unwrap();
    drop(conn);

    let err = cmd_rollback(
        changeset_id,
        &db_path_str,
        temp_dir.path().join("root").to_string_lossy().as_ref(),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        err.contains("RollbackReplayUnavailable"),
        "unexpected rollback error: {err}"
    );

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_some(),
        "rollback refusal must happen before deleting the installed trove"
    );
    assert!(
        InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_some(),
        "rollback refusal must preserve the installed legacy bundle row"
    );
    let reversed_by: Option<i64> = conn
        .query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reversed_by, None);
}

#[tokio::test]
async fn rollback_snapshot_path_refuses_installed_legacy_bundle_before_deleting_trove() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    conary_core::db::init(&db_path_str).unwrap();
    create_active_generation_link(temp_dir.path());
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let old_snapshot = TroveSnapshot {
        name: "rollback-legacy-fixture".to_string(),
        version: "0.9-1".to_string(),
        architecture: Some("x86_64".to_string()),
        description: None,
        install_source: InstallSource::File.as_str().to_string(),
        installed_from_repository_id: None,
        files: Vec::new(),
    };

    let mut changeset = Changeset::new("Upgrade rollback legacy fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![serde_json::to_string(&old_snapshot).unwrap(), changeset_id],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let trove_id = insert_test_trove(&conn, changeset_id, "rollback-legacy-fixture", "1.0-1", &[]);
    let bundle = rollback_legacy_bundle();
    let mut installed = InstalledLegacyScriptletBundle::new(
        trove_id,
        Some(changeset_id),
        "rpm/fedora/44/x86_64".to_string(),
        "allow-legacy-replay".to_string(),
        true,
        &bundle,
    )
    .unwrap();
    installed.insert_or_replace(&conn).unwrap();
    drop(conn);

    let err = cmd_rollback(
        changeset_id,
        &db_path_str,
        temp_dir.path().join("root").to_string_lossy().as_ref(),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        err.contains("RollbackReplayUnavailable"),
        "unexpected rollback error: {err}"
    );

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_some(),
        "snapshot rollback refusal must happen before deleting the installed trove"
    );
    assert!(
        InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_some(),
        "snapshot rollback refusal must preserve the installed legacy bundle row"
    );
    let reversed_by: Option<i64> = conn
        .query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reversed_by, None);
}

#[tokio::test]
async fn rollback_update_without_active_generation_fails_closed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    conary_core::db::init(&db_path_str).unwrap();
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let root = temp_dir.path().join("root");
    std::fs::create_dir_all(root.join("usr/share/conary-test")).unwrap();
    let hello_path = root.join("usr/share/conary-test/hello.txt");
    let added_path = root.join("usr/share/conary-test/added.txt");
    std::fs::write(&hello_path, b"hello from v2\n").unwrap();
    std::fs::write(&added_path, b"added in v2\n").unwrap();

    let v1_hash = store_test_object(&conn, &db_path, b"hello from v1\n");
    let v2_hash = store_test_object(&conn, &db_path, b"hello from v2\n");
    let added_hash = store_test_object(&conn, &db_path, b"added in v2\n");

    let old_snapshot = TroveSnapshot {
        name: "conary-test-fixture".to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: None,
        install_source: InstallSource::File.as_str().to_string(),
        installed_from_repository_id: None,
        files: vec![FileSnapshot {
            path: "/usr/share/conary-test/hello.txt".to_string(),
            sha256_hash: v1_hash,
            size: "hello from v1\n".len() as i64,
            permissions: 0o100644,
            symlink_target: None,
        }],
    };

    let mut update_changeset =
        Changeset::new("CCS upgrade conary-test-fixture 1.0.0 -> 2.0.0".to_string());
    let update_changeset_id = update_changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            serde_json::to_string(&old_snapshot).unwrap(),
            update_changeset_id
        ],
    )
    .unwrap();
    update_changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    insert_test_trove(
        &conn,
        update_changeset_id,
        "conary-test-fixture",
        "2.0.0",
        &[
            (
                "/usr/share/conary-test/hello.txt",
                &v2_hash,
                "hello from v2\n".len() as i64,
            ),
            (
                "/usr/share/conary-test/added.txt",
                &added_hash,
                "added in v2\n".len() as i64,
            ),
        ],
    );
    drop(conn);

    let err = cmd_rollback(
        update_changeset_id,
        &db_path_str,
        root.to_string_lossy().as_ref(),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(err.contains(&format!(
        "Cannot roll back changeset {update_changeset_id} without an active composefs generation"
    )));
    assert_eq!(
        std::fs::read_to_string(&hello_path).unwrap(),
        "hello from v2\n"
    );
    assert!(added_path.exists());

    let conn = conary_core::db::open(&db_path_str).unwrap();
    let troves = Trove::find_by_name(&conn, "conary-test-fixture").unwrap();
    assert_eq!(troves.len(), 1);
    assert_eq!(troves[0].version, "2.0.0");
    let status: String = conn
        .query_row(
            "SELECT status FROM changesets WHERE id = ?1",
            [update_changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "applied");
}

#[test]
fn direct_live_root_restore_recreates_regular_files_and_symlinks() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    conary_core::db::init(&db_path_str).unwrap();
    let conn = conary_core::db::open(&db_path_str).unwrap();
    let file_hash = store_test_object(&conn, &db_path, b"restored\n");
    let link_hash = {
        let cas = CasStore::new(objects_dir(&db_path_str)).unwrap();
        cas.store_symlink("tool").unwrap()
    };
    conn.execute(
        "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
             VALUES (?1, ?2, ?3)",
        params![
            &link_hash,
            format!("objects/{}/{}", &link_hash[0..2], &link_hash[2..]),
            "tool".len() as i64
        ],
    )
    .unwrap();

    let root = temp_dir.path().join("root");
    let snapshot = TroveSnapshot {
        name: "fixture".to_string(),
        version: "1.0.0".to_string(),
        architecture: None,
        description: None,
        install_source: InstallSource::File.as_str().to_string(),
        installed_from_repository_id: None,
        files: vec![
            FileSnapshot {
                path: "/usr/bin/tool".to_string(),
                sha256_hash: file_hash,
                size: "restored\n".len() as i64,
                permissions: 0o100755,
                symlink_target: None,
            },
            FileSnapshot {
                path: "/usr/bin/tool-link".to_string(),
                sha256_hash: link_hash,
                size: "tool".len() as i64,
                permissions: 0o120777,
                symlink_target: Some("tool".to_string()),
            },
        ],
    };

    let stats = restore_snapshots_to_live_root(&root, &db_path_str, &[snapshot]).unwrap();

    assert_eq!(stats.files_restored, 2);
    assert_eq!(
        std::fs::read_to_string(root.join("usr/bin/tool")).unwrap(),
        "restored\n"
    );
    assert_eq!(
        std::fs::read_link(root.join("usr/bin/tool-link")).unwrap(),
        Path::new("tool")
    );
}
