// conary/src/commands/state/tests.rs

use super::execute_restore_plan_with_root;
use conary_core::ccs::legacy_scriptlets::{
    DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
    LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy, PublicationStatus,
    ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder,
    VersionScheme,
};
use conary_core::db::models::{
    Changeset, ChangesetStatus, InstalledLegacyScriptletBundle, PackageResolution, PrimaryStrategy,
    Repository, RepositoryPackage, ResolutionStrategy, Trove, TroveType,
};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

fn build_test_ccs_package(dir: &Path, name: &str, version: &str) -> PathBuf {
    build_test_ccs_package_with_bundle(dir, name, version, None)
}

fn build_test_ccs_package_with_bundle(
    dir: &Path,
    name: &str,
    version: &str,
    legacy_scriptlets: Option<LegacyScriptletBundle>,
) -> PathBuf {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let binary_content = format!("#!/bin/sh\necho {name} {version}\n").into_bytes();
    let binary_hash = hash::sha256(&binary_content);
    let files = vec![FileEntry {
        path: format!("/usr/bin/{name}"),
        hash: binary_hash.clone(),
        size: binary_content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];
    let package_path = dir.join(format!("{name}-{version}.ccs"));
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.legacy_scriptlets = legacy_scriptlets;
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: format!("{name}-runtime"),
                size: binary_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(binary_hash, binary_content)]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();
    package_path
}

fn legacy_pre_install_entry() -> LegacyScriptletEntry {
    let body = "getent group conary-test >/dev/null || true\n";
    LegacyScriptletEntry {
        id: "rpm:%pre".to_string(),
        native_slot: "%pre".to_string(),
        phase: LifecyclePath::PreInstall,
        lifecycle_paths: vec!["install:pre".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "before-payload".to_string(),
            before: vec!["payload".to_string()],
            after: Vec::new(),
            extra: BTreeMap::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision: ScriptletDecision::Legacy,
        reason_code: "legacy-replay-required".to_string(),
        human_reason: Some("fixture legacy pre-install".to_string()),
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

fn legacy_post_remove_entry() -> LegacyScriptletEntry {
    let body = "systemctl daemon-reload\n";
    LegacyScriptletEntry {
        id: "rpm:%postun".to_string(),
        native_slot: "%postun".to_string(),
        phase: LifecyclePath::PostRemove,
        lifecycle_paths: vec!["remove:last".to_string()],
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
        human_reason: Some("fixture legacy post-remove".to_string()),
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

fn legacy_pre_install_bundle(package: &str, version: &str) -> LegacyScriptletBundle {
    legacy_bundle(package, version, legacy_pre_install_entry())
}

fn legacy_post_remove_bundle(package: &str, version: &str) -> LegacyScriptletBundle {
    legacy_bundle(package, version, legacy_post_remove_entry())
}

fn legacy_bundle(
    package: &str,
    version: &str,
    entry: LegacyScriptletEntry,
) -> LegacyScriptletBundle {
    LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 2,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package.to_string(),
        source_version: version.to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "goal6-test".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{package}-{version}-evidence").as_bytes(),
        )),
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
        entries: vec![entry],
        extra: BTreeMap::new(),
    }
}

fn insert_legacy_restore_fixture(
    conn: &mut rusqlite::Connection,
    package: &str,
    version: &str,
) -> i64 {
    conary_core::db::transaction(conn, |tx| {
        let mut cs = Changeset::new(format!("Install {package}-{version}"));
        let cs_id = cs.insert(tx)?;
        let mut trove = Trove::new(package.to_string(), version.to_string(), TroveType::Package);
        trove.architecture = Some("x86_64".to_string());
        trove.installed_by_changeset_id = Some(cs_id);
        let trove_id = trove.insert(tx)?;
        let bundle = legacy_post_remove_bundle(package, version);
        let mut installed = InstalledLegacyScriptletBundle::new(
            trove_id,
            Some(cs_id),
            "rpm/fedora/44/x86_64".to_string(),
            "allow-legacy-replay".to_string(),
            true,
            &bundle,
        )
        .map_err(|error| conary_core::Error::InitError(error.to_string()))?;
        installed
            .insert_or_replace(tx)
            .map_err(|error| conary_core::Error::InitError(error.to_string()))?;
        cs.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(trove_id)
    })
    .unwrap()
}

fn table_count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn serve_test_file(file_path: PathBuf) -> (String, std::thread::JoinHandle<()>) {
    let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
    let bytes = std::fs::read(&file_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(&bytes).unwrap();
    });
    (format!("http://{addr}/{filename}"), handle)
}

#[tokio::test]
async fn state_restore_dry_run_preserves_installed_legacy_bundle_rows() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();
    let trove_id = insert_legacy_restore_fixture(&mut conn, "vim", "9.1.0");
    conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets = table_count(&conn, "changesets");
    let before_troves = table_count(&conn, "troves");
    let before_bundles = table_count(&conn, "installed_legacy_scriptlet_bundles");
    drop(conn);

    execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        true,
    )
    .await
    .unwrap();

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(table_count(&conn, "changesets"), before_changesets);
    assert_eq!(table_count(&conn, "troves"), before_troves);
    assert_eq!(
        table_count(&conn, "installed_legacy_scriptlet_bundles"),
        before_bundles
    );
    assert!(
        InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn state_restore_refuses_installed_legacy_remove_replay_before_mutation() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();
    let trove_id = insert_legacy_restore_fixture(&mut conn, "vim", "9.1.0");
    conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets = table_count(&conn, "changesets");
    let before_troves = table_count(&conn, "troves");
    let before_bundles = table_count(&conn, "installed_legacy_scriptlet_bundles");
    drop(conn);

    let err = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await
    .expect_err("restore should fail closed before removing legacy bundle trove")
    .to_string();

    assert!(
        err.contains("LegacyReplayFeatureDisabled"),
        "unexpected restore error: {err}"
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(table_count(&conn, "changesets"), before_changesets);
    assert_eq!(table_count(&conn, "troves"), before_troves);
    assert_eq!(
        table_count(&conn, "installed_legacy_scriptlet_bundles"),
        before_bundles
    );
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_some(),
        "restore refusal must happen before deleting the installed trove"
    );
    assert!(
        InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_some(),
        "restore refusal must preserve the installed legacy bundle row"
    );
}

#[tokio::test]
async fn state_restore_refuses_legacy_install_bundle_before_mutation() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let package_path = build_test_ccs_package_with_bundle(
        package_dir.path(),
        "vim",
        "9.1.0",
        Some(legacy_pre_install_bundle("vim", "9.1.0")),
    );
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let mut repo = Repository::new("arch-test".to_string(), package_url.clone());
    let repo_id = repo.insert(&conn).unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        package_checksum.clone(),
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url.clone(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    resolution.version = Some("9.1.0".to_string());
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.insert(&conn).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = Trove::new("vim".to_string(), "9.1.0".to_string(), TroveType::Package);
        vim.architecture = Some("x86_64".to_string());
        vim.installed_by_changeset_id = Some(cs_id);
        vim.insert(tx)?;
        cs.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();
    let baseline = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("baseline", None, None)
        .unwrap();

    conn.execute("DELETE FROM troves WHERE name = 'vim'", [])
        .unwrap();
    conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets = table_count(&conn, "changesets");
    let before_troves = table_count(&conn, "troves");
    drop(conn);

    let err = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await
    .expect_err("restore should fail closed before installing legacy replay bundle")
    .to_string();

    assert!(
        err.contains("LegacyReplayFeatureDisabled"),
        "unexpected restore error: {err}"
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(table_count(&conn, "changesets"), before_changesets);
    assert_eq!(table_count(&conn, "troves"), before_troves);
    assert!(
        conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
            .unwrap()
            .is_none(),
        "restore refusal must happen before reinstalling the target trove"
    );
}

#[tokio::test]
async fn test_state_restore_remove_only_executes_and_creates_one_changeset_and_snapshot() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = conary_core::db::models::Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        vim.architecture = Some("x86_64".to_string());
        vim.installed_by_changeset_id = Some(cs_id);
        vim.insert(tx)?;
        cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();

    let _drifted = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0))
        .unwrap();
    let before_states: i64 = conn
        .query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0))
        .unwrap();
    drop(conn);

    let result = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await;

    assert!(
        result.is_ok(),
        "remove-only restore should succeed: {:?}",
        result
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert!(
        conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        before_changesets + 1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        before_states + 1
    );
}

#[tokio::test]
async fn test_state_restore_missing_repo_version_rolls_back_without_snapshot() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = conary_core::db::models::Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        vim.architecture = Some("x86_64".to_string());
        vim.installed_by_changeset_id = Some(cs_id);
        vim.insert(tx)?;
        cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();

    let drifted = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();
    assert!(drifted.state_number > baseline.state_number);

    conn.execute(
            "UPDATE state_members SET trove_version = '9.9.9' WHERE state_id = ?1 AND trove_name = 'nginx'",
            [baseline.id.unwrap()],
        )
        .unwrap();

    let before_changesets: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0))
        .unwrap();
    let before_states: i64 = conn
        .query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0))
        .unwrap();
    drop(conn);

    let result = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await;

    let err = result.expect_err("missing repo version should fail");
    let message = format!("{err:#}");
    assert!(
        message.contains("9.9.9"),
        "missing-version restore should surface the unresolved target version, got: {message}"
    );
    assert!(
        !message.contains("not yet implemented"),
        "missing-version restore should fail in preflight, not via the placeholder bail: {message}"
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        before_changesets
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        before_states
    );
}

#[tokio::test]
async fn test_state_restore_changeset_rolls_back_via_revert_metadata_wrapper() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = conary_core::db::models::Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        vim.architecture = Some("x86_64".to_string());
        vim.installed_by_changeset_id = Some(cs_id);
        vim.insert(tx)?;
        cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();
    conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();
    drop(conn);

    execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await
    .unwrap();

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert!(
        conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
            .unwrap()
            .is_none()
    );
    let restore_changeset_id: i64 = conn
        .query_row(
            "SELECT id FROM changesets WHERE description = ?1 ORDER BY id DESC LIMIT 1",
            [format!(
                "Restore state {} -> {}",
                baseline.state_number + 1,
                baseline.state_number
            )],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);

    crate::commands::cmd_rollback(
        restore_changeset_id,
        &db_path,
        root.path().to_str().unwrap(),
    )
    .await
    .unwrap();

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert!(
        conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
            .unwrap()
            .is_some()
    );
    let restore_changeset =
        conary_core::db::models::Changeset::find_by_id(&conn, restore_changeset_id)
            .unwrap()
            .unwrap();
    assert_eq!(
        restore_changeset.status,
        conary_core::db::models::ChangesetStatus::RolledBack
    );
}

#[tokio::test]
async fn test_state_restore_install_plan_executes_under_wrapping_changeset() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let package_path = build_test_ccs_package(package_dir.path(), "vim", "9.1.0");
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let mut repo = Repository::new("arch-test".to_string(), package_url.clone());
    let repo_id = repo.insert(&conn).unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        package_checksum.clone(),
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url.clone(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    resolution.version = Some("9.1.0".to_string());
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.insert(&conn).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = Trove::new("vim".to_string(), "9.1.0".to_string(), TroveType::Package);
        vim.architecture = Some("x86_64".to_string());
        vim.installed_by_changeset_id = Some(cs_id);
        vim.insert(tx)?;
        cs.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();
    let baseline = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("baseline", None, None)
        .unwrap();

    conn.execute("DELETE FROM troves WHERE name = 'vim'", [])
        .unwrap();
    let _drifted = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0))
        .unwrap();
    let before_states: i64 = conn
        .query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0))
        .unwrap();
    drop(conn);

    let result = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await;

    assert!(
        result.is_ok(),
        "install restore should succeed under one wrapping changeset: {result:?}"
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    let vim = conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
        .unwrap()
        .expect("vim should be restored");
    assert_eq!(vim.version, "9.1.0");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        before_changesets + 1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        before_states + 1
    );
}
