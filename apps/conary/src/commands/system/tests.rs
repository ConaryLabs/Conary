// conary/src/commands/system/tests.rs

use super::{
    NATIVE_REPOSITORY_SEEDS, cmd_init, cmd_rollback, paths_refer_to_same_location,
    restore_snapshot, restore_snapshots_to_live_root, rollback_claim_status,
    validate_init_privileges,
};
use crate::commands::{
    FileSnapshot, NativeLifecycleSnapshot, TroveSnapshot, metadata_with_removed_troves,
};
use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleBundle,
    NativeLifecycleEntry, NativeLifecycleEntryKind, RpmCriticality, RpmProgram, RpmRuntimeMetadata,
    ScriptletFidelity, SourceFormat, TransactionOrder, VersionScheme,
};
use conary_core::db::models::{
    Changeset, ChangesetStatus, FileEntry, InstallSource, InstalledNativeLifecycleBundle,
    PackageResolution, Repository, RepositoryPackage, Trove, TroveType,
};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use rusqlite::params;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn resolved_node(kind: PayloadNodeKind, mode: u32) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric { id: 0 },
        group: PayloadIdentity::Numeric { id: 0 },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    })
    .unwrap()
}

fn regular_snapshot(path: &str, sha256: String, size: u64, mode: u32) -> FileSnapshot {
    FileSnapshot {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            libc::S_IFREG | (mode & 0o7777),
        ),
        content: Some(PayloadContentAuthority { sha256, size }),
    }
}

fn symlink_snapshot(path: &str, target: &str) -> FileSnapshot {
    FileSnapshot {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Symlink {
                target: target.to_string(),
            },
            libc::S_IFLNK | 0o777,
        ),
        content: None,
    }
}

#[tokio::test]
async fn init_seeds_every_builtin_source_feed_without_a_host_distro() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str).await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let host_capabilities =
        conary_core::ccs::HostCapabilityInventory::load_required(&conn).unwrap();
    assert_eq!(
        host_capabilities.schema_version,
        conary_core::ccs::HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION
    );
    for feed in conary_core::repository::supported_profiles::public_profiles() {
        let name = format!("remi-{}", feed.id());
        let remi = Repository::find_by_name(&conn, &name).unwrap().unwrap();
        assert_eq!(remi.url, "https://remi.conary.io");
        assert_eq!(remi.default_strategy.as_deref(), Some("remi"));
        assert_eq!(
            remi.default_strategy_endpoint.as_deref(),
            Some("https://remi.conary.io")
        );
        assert_eq!(remi.default_strategy_distro.as_deref(), Some(feed.id()));
    }

    for seed in NATIVE_REPOSITORY_SEEDS {
        let repo = Repository::find_by_name(&conn, seed.name).unwrap().unwrap();
        assert!(!repo.enabled, "{} should start disabled", repo.name);
        assert_eq!(
            repo.default_strategy_distro.as_deref(),
            Some(seed.source_feed)
        );
        assert_eq!(repo.parser_config, Some(seed.parser.config()));
    }
}

#[tokio::test]
async fn init_repairs_one_source_contract_without_resetting_other_feeds() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str).await.unwrap();
    {
        let conn = conary_core::db::open(db_path_str).unwrap();
        let mut ubuntu = Repository::find_by_name(&conn, "ubuntu-26.04")
            .unwrap()
            .unwrap();
        ubuntu
            .set_parser_config(conary_core::repository::RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            })
            .unwrap();
        ubuntu.last_sync = Some("2026-07-18T00:00:00Z".to_string());
        ubuntu.update(&conn).unwrap();
        let ubuntu_id = ubuntu.id.unwrap();
        let mut package = RepositoryPackage::new(
            ubuntu_id,
            "ubuntu-package".to_string(),
            "1".to_string(),
            conary_core::repository::versioning::VersionScheme::Debian,
            "sha256:test".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/ubuntu-package.deb".to_string(),
        );
        package.distro = Some("ubuntu-26.04".to_string());
        package.insert(&conn).unwrap();
        let mut resolution = PackageResolution::binary(
            ubuntu_id,
            "ubuntu-package".to_string(),
            "https://archive.ubuntu.com/ubuntu/pool/ubuntu-package.deb".to_string(),
            "sha256:test".to_string(),
        );
        resolution.insert(&conn).unwrap();

        let mut remi = Repository::find_by_name(&conn, "remi-fedora-44")
            .unwrap()
            .unwrap();
        remi.last_sync = Some("2026-07-17T00:00:00Z".to_string());
        remi.update(&conn).unwrap();
        let mut package = RepositoryPackage::new(
            remi.id.unwrap(),
            "fedora-only".to_string(),
            "1".to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            "sha256:test".to_string(),
            1,
            "https://example.invalid/fedora-only".to_string(),
        );
        package.distro = Some("fedora-44".to_string());
        package.insert(&conn).unwrap();
        let mut resolution = PackageResolution::remi(
            remi.id.unwrap(),
            "fedora-only".to_string(),
            "https://remi.conary.io".to_string(),
            "fedora-44".to_string(),
        );
        resolution.insert(&conn).unwrap();
    }

    cmd_init(db_path_str).await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let remi = Repository::find_by_name(&conn, "remi-fedora-44")
        .unwrap()
        .unwrap();
    assert_eq!(remi.default_strategy_distro.as_deref(), Some("fedora-44"));
    assert_eq!(remi.last_sync.as_deref(), Some("2026-07-17T00:00:00Z"));
    assert_eq!(
        RepositoryPackage::find_by_repository(&conn, remi.id.unwrap())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        PackageResolution::find_by_repository(&conn, remi.id.unwrap())
            .unwrap()
            .len(),
        1
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
}

#[tokio::test]
async fn init_rerun_preserves_operator_repository_choices() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str).await.unwrap();
    {
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut arch_core = Repository::find_by_name(&conn, "arch-core")
            .unwrap()
            .unwrap();
        arch_core.enabled = true;
        arch_core.update(&conn).unwrap();

        let mut arch_extra = Repository::find_by_name(&conn, "arch-extra")
            .unwrap()
            .unwrap();
        arch_extra.url = "https://mirror.example.invalid/arch-extra".to_string();
        arch_extra.update(&conn).unwrap();

        let mut remi = Repository::find_by_name(&conn, "remi-arch")
            .unwrap()
            .unwrap();
        remi.url = "https://mirror.example.invalid/remi".to_string();
        remi.default_strategy_endpoint = Some(remi.url.clone());
        remi.default_strategy_distro = Some("fedora-44".to_string());
        remi.update(&conn).unwrap();
    }

    cmd_init(db_path_str).await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let arch_core = Repository::find_by_name(&conn, "arch-core")
        .unwrap()
        .unwrap();
    assert!(arch_core.enabled);
    let arch_extra = Repository::find_by_name(&conn, "arch-extra")
        .unwrap()
        .unwrap();
    assert_eq!(arch_extra.url, "https://mirror.example.invalid/arch-extra");
    let remi = Repository::find_by_name(&conn, "remi-arch")
        .unwrap()
        .unwrap();
    assert_eq!(remi.url, "https://mirror.example.invalid/remi");
    assert_eq!(remi.default_strategy_distro.as_deref(), Some("fedora-44"));
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

    let err = cmd_init(db_path_str).await.unwrap_err().to_string();

    assert!(err.contains(&parent_file.display().to_string()));
    assert!(err.contains("safe next step"));
}

#[test]
fn rollback_claim_status_is_applied() {
    assert_eq!(rollback_claim_status(), "applied");
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
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    trove.installed_by_changeset_id = Some(changeset_id);
    let trove_id = trove.insert(conn).unwrap();

    for (path, hash, size) in files {
        let mut file = FileEntry::new(
            (*path).to_string(),
            resolved_node(
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                },
                libc::S_IFREG | 0o644,
            ),
            Some(PayloadContentAuthority {
                sha256: (*hash).to_string(),
                size: u64::try_from(*size).unwrap(),
            }),
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

fn rollback_typed_rpm_entry() -> NativeLifecycleEntry {
    let body = "print('rollback-post-remove')\n";
    NativeLifecycleEntry {
        id: "rpm:%postun".to_string(),
        native_slot: "%postun".to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PostRemove,
        lifecycle_paths: vec!["remove:last".to_string()],
        interpreter: "<lua>".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: Vec::new(),
            after: vec!["payload".to_string()],
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            critical: true,
            criticality: RpmCriticality::Header,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: Default::default(),
            header_context: Default::default(),
            package_rpm_version: None,
        }),
        rpm_sysusers: None,
        deb_maintainer: None,
        arch_install: None,
        arch_hook: None,
        residual_lifecycle: None,
    }
}

fn rollback_typed_rpm_bundle() -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "rollback-rpm-fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "typed-runtime-test".to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(b"rollback-evidence")),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![rollback_typed_rpm_entry()],
    }
}

fn set_trove_rpm_provenance(conn: &rusqlite::Connection, trove_id: i64) {
    conn.execute(
        "UPDATE troves SET source_distro = 'fedora', version_scheme = 'rpm' WHERE id = ?1",
        [trove_id],
    )
    .unwrap();
}

#[tokio::test]
async fn rollback_executes_typed_rpm_remove_lifecycle_and_reverses_install() {
    let (temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    create_active_generation_link(temp_dir.path());
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut changeset = Changeset::new("Install rollback RPM fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let trove_id = insert_test_trove(&conn, changeset_id, "rollback-rpm-fixture", "1.0-1", &[]);
    set_trove_rpm_provenance(&conn, trove_id);
    let bundle = rollback_typed_rpm_bundle();
    let mut installed =
        InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), &bundle).unwrap();
    installed.insert_or_replace(&conn).unwrap();
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str)
        .await
        .expect("rollback must execute the typed RPM removal lifecycle");

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_none(),
        "rollback must remove the installed trove"
    );
    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_none(),
        "rollback must remove the lifecycle bundle with its trove"
    );
    let reversed_by: Option<i64> = conn
        .query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(reversed_by.is_some());
}

#[tokio::test]
async fn rollback_snapshot_path_executes_typed_rpm_remove_lifecycle() {
    let (temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    create_active_generation_link(temp_dir.path());
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let mut old_bundle = rollback_typed_rpm_bundle();
    old_bundle.source_version = "0.9-1".to_string();
    let old_snapshot = TroveSnapshot {
        name: "rollback-rpm-fixture".to_string(),
        version: "0.9-1".to_string(),
        architecture: Some("x86_64".to_string()),
        description: None,
        install_source: InstallSource::File.as_str().to_string(),
        source_distro: Some("fedora".to_string()),
        version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        native_lifecycle: Some(NativeLifecycleSnapshot {
            bundle_toml: toml::to_string_pretty(&old_bundle).unwrap(),
            lifecycle_state: "installed".to_string(),
            pending_triggers: Vec::new(),
            awaited_packages: Vec::new(),
        }),
        ccs_remove_hook: None,
        installed_from_repository_id: None,
        files: Vec::new(),
    };

    let mut changeset = Changeset::new("Upgrade rollback RPM fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            metadata_with_removed_troves(vec![old_snapshot]).unwrap(),
            changeset_id
        ],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let trove_id = insert_test_trove(&conn, changeset_id, "rollback-rpm-fixture", "1.0-1", &[]);
    set_trove_rpm_provenance(&conn, trove_id);
    let bundle = rollback_typed_rpm_bundle();
    let mut installed =
        InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), &bundle).unwrap();
    installed.insert_or_replace(&conn).unwrap();
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str)
        .await
        .expect("snapshot rollback must execute the typed RPM removal lifecycle");

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_none(),
        "snapshot rollback must remove the reverted trove"
    );
    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_none(),
        "snapshot rollback must remove the reverted lifecycle bundle"
    );
    let restored = Trove::find_by_name(&conn, "rollback-rpm-fixture").unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].version, "0.9-1");
    assert_eq!(restored[0].source_distro.as_deref(), Some("fedora"));
    assert_eq!(
        restored[0].version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
    let restored_bundle = InstalledNativeLifecycleBundle::find_by_trove(
        &conn,
        restored[0].id.expect("restored trove identity"),
    )
    .unwrap()
    .expect("rollback must restore the previous native lifecycle bundle");
    assert_eq!(restored_bundle.lifecycle_state.as_str(), "installed");
    assert_eq!(restored_bundle.bundle().unwrap().source_version, "0.9-1");
    let reversed_by: Option<i64> = conn
        .query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(reversed_by.is_some());
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
        source_distro: None,
        version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
        native_lifecycle: None,
        ccs_remove_hook: None,
        installed_from_repository_id: None,
        files: vec![regular_snapshot(
            "/usr/share/conary-test/hello.txt",
            v1_hash,
            "hello from v1\n".len() as u64,
            0o644,
        )],
    };

    let mut update_changeset =
        Changeset::new("CCS upgrade conary-test-fixture 1.0.0 -> 2.0.0".to_string());
    let update_changeset_id = update_changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            metadata_with_removed_troves(vec![old_snapshot]).unwrap(),
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

    let err = cmd_rollback(update_changeset_id, &db_path_str)
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
        source_distro: None,
        version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
        native_lifecycle: None,
        ccs_remove_hook: None,
        installed_from_repository_id: None,
        files: vec![
            regular_snapshot("/usr/bin/tool", file_hash, "restored\n".len() as u64, 0o755),
            symlink_snapshot("/usr/bin/tool-link", "tool"),
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

#[test]
fn rollback_snapshot_restores_exact_ccs_remove_hook() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let mut changeset = Changeset::new("Restore CCS hook fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let snapshot = TroveSnapshot {
        name: "ccs-hook-fixture".to_string(),
        version: "1".to_string(),
        architecture: None,
        description: None,
        install_source: InstallSource::File.as_str().to_string(),
        source_distro: None,
        version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
        native_lifecycle: None,
        ccs_remove_hook: Some(crate::commands::CcsRemoveHookSnapshot {
            script: "echo removing\n".to_string(),
            reversible: Some(true),
        }),
        installed_from_repository_id: None,
        files: Vec::new(),
    };

    let tx = conn.transaction().unwrap();
    restore_snapshot(&tx, changeset_id, &snapshot).unwrap();
    tx.commit().unwrap();

    let trove = Trove::find_by_name(&conn, "ccs-hook-fixture")
        .unwrap()
        .pop()
        .unwrap();
    let hook =
        conary_core::db::models::InstalledCcsRemoveHook::find_by_trove(&conn, trove.id.unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(hook.script, "echo removing\n");
    assert_eq!(hook.reversible, Some(true));
}
