// apps/conary/src/commands/state/tests.rs

use super::{execute_restore_plan_with_root, find_installed_trove_for_member};
use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleBundle,
    NativeLifecycleEntry, NativeLifecycleEntryKind, RpmCriticality, RpmProgram, RpmRuntimeMetadata,
    ScriptletFidelity, SourceFormat, TransactionOrder, VersionScheme,
};
use conary_core::db::models::{
    Changeset, ChangesetStatus, InstalledNativeLifecycleBundle, RepositoryPackage, StateMember,
    Trove, TroveType,
};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

fn build_test_ccs_package_with_bundle(
    dir: &Path,
    name: &str,
    version: &str,
    native_lifecycle: Option<NativeLifecycleBundle>,
) -> PathBuf {
    build_test_ccs_package_with_manifest(dir, name, version, native_lifecycle, |_| {})
}

fn build_test_ccs_package_with_manifest(
    dir: &Path,
    name: &str,
    version: &str,
    native_lifecycle: Option<NativeLifecycleBundle>,
    configure: impl FnOnce(&mut conary_core::ccs::CcsManifest),
) -> PathBuf {
    use conary_core::ccs::builder::write_signed_current_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry};
    use conary_core::hash;
    use conary_core::payload::PayloadContentAuthority;

    let binary_content = format!("#!/bin/sh\necho {name} {version}\n").into_bytes();
    let binary_hash = hash::sha256(&binary_content);
    let files = vec![FileEntry {
        path: format!("/usr/bin/{name}"),
        node: crate::commands::test_helpers::test_regular_payload_node(0o755),
        content: Some(PayloadContentAuthority {
            sha256: binary_hash.clone(),
            size: binary_content.len() as u64,
        }),
        component: "runtime".to_string(),
        chunks: None,
    }];
    let package_path = dir.join(format!("{name}-{version}.ccs"));
    let mut manifest = CcsManifest::new_minimal(name, version);
    if native_lifecycle.is_some() {
        manifest.package.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    }
    manifest.native_lifecycle = native_lifecycle;
    configure(&mut manifest);
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
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(binary_hash, binary_content)]),
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();
    package_path
}

fn typed_rpm_pre_install_entry() -> NativeLifecycleEntry {
    typed_rpm_entry(
        LifecyclePath::PreInstall,
        "install:pre",
        "before-payload",
        "print('state-restore-pre-install')\n",
    )
}

fn typed_rpm_post_remove_entry() -> NativeLifecycleEntry {
    typed_rpm_entry(
        LifecyclePath::PostRemove,
        "remove:last",
        "after-payload",
        "print('state-restore-post-remove')\n",
    )
}

fn typed_rpm_entry(
    phase: LifecyclePath,
    lifecycle_path: &str,
    position: &str,
    body: &str,
) -> NativeLifecycleEntry {
    let (before, after) = if position == "before-payload" {
        (vec!["payload".to_string()], Vec::new())
    } else {
        (Vec::new(), vec!["payload".to_string()])
    };
    NativeLifecycleEntry {
        id: match phase {
            LifecyclePath::PreInstall => "rpm:%pre".to_string(),
            LifecyclePath::PostRemove => "rpm:%postun".to_string(),
            _ => unreachable!("test fixture only builds install and remove entries"),
        },
        native_slot: match phase {
            LifecyclePath::PreInstall => "%pre".to_string(),
            LifecyclePath::PostRemove => "%postun".to_string(),
            _ => unreachable!("test fixture only builds install and remove entries"),
        },
        kind: NativeLifecycleEntryKind::Executable,
        phase,
        lifecycle_paths: vec![lifecycle_path.to_string()],
        interpreter: "<lua>".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: position.to_string(),
            before,
            after,
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

fn typed_rpm_pre_install_bundle(package: &str, version: &str) -> NativeLifecycleBundle {
    typed_rpm_bundle(package, version, typed_rpm_pre_install_entry())
}

fn typed_rpm_post_remove_bundle(package: &str, version: &str) -> NativeLifecycleBundle {
    typed_rpm_bundle(package, version, typed_rpm_post_remove_entry())
}

fn typed_rpm_bundle(
    package: &str,
    version: &str,
    entry: NativeLifecycleEntry,
) -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package.to_string(),
        source_version: version.to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "typed-runtime-test".to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{package}-{version}-evidence").as_bytes(),
        )),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![entry],
    }
}

fn insert_typed_rpm_restore_fixture(
    conn: &mut rusqlite::Connection,
    package: &str,
    version: &str,
) -> i64 {
    conary_core::db::transaction(conn, |tx| {
        let mut cs = Changeset::new(format!("Install {package}-{version}"));
        let cs_id = cs.insert(tx)?;
        let mut trove = Trove::new(
            package.to_string(),
            version.to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.source_profile = Some("fedora-44".to_string());
        trove.installed_by_changeset_id = Some(cs_id);
        let trove_id = trove.insert(tx)?;
        let bundle = typed_rpm_post_remove_bundle(package, version);
        let mut installed = InstalledNativeLifecycleBundle::new(trove_id, Some(cs_id), &bundle)
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

#[test]
fn restore_state_member_requires_exact_optional_architecture() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let conn = crate::commands::open_db(&db_path).unwrap();

    let mut installed = Trove::new(
        "glibc".to_string(),
        "2.40".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    installed.architecture = Some("x86_64".to_string());
    installed.insert(&conn).unwrap();

    let architecture_independent = StateMember::new(1, "glibc".to_string(), "2.40".to_string());
    let error = find_installed_trove_for_member(&conn, &architecture_independent)
        .expect_err("absent architecture must not wildcard-match a concrete variant");
    assert!(error.to_string().contains("expected installed package"));

    let mut exact = architecture_independent;
    exact.architecture = Some("x86_64".to_string());
    let matched = find_installed_trove_for_member(&conn, &exact).unwrap();
    assert_eq!(matched.architecture.as_deref(), Some("x86_64"));
}

fn active_generation_has_path(db_path: &Path, path: &str) -> bool {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .expect("state restore should publish an active generation");
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation),
    )
    .unwrap();
    artifact
        .generation_root
        .entries
        .iter()
        .chain(&artifact.mutable_state.entries)
        .any(|entry| entry.path == path)
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
async fn state_restore_dry_run_preserves_installed_native_bundle_rows() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();
    let trove_id = insert_typed_rpm_restore_fixture(&mut conn, "vim", "9.1.0");
    conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets = table_count(&conn, "changesets");
    let before_troves = table_count(&conn, "troves");
    let before_bundles = table_count(&conn, "installed_native_lifecycle_bundles");
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
        table_count(&conn, "installed_native_lifecycle_bundles"),
        before_bundles
    );
    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn state_restore_executes_typed_rpm_remove_lifecycle_and_commits() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();
    let trove_id = insert_typed_rpm_restore_fixture(&mut conn, "vim", "9.1.0");
    conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets = table_count(&conn, "changesets");
    let before_troves = table_count(&conn, "troves");
    drop(conn);

    execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await
    .expect("restore should execute the typed RPM post-remove lifecycle");

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(table_count(&conn, "changesets"), before_changesets + 1);
    assert_eq!(table_count(&conn, "troves"), before_troves - 1);
    assert_eq!(table_count(&conn, "installed_native_lifecycle_bundles"), 0);
    assert!(
        Trove::find_by_id(&conn, trove_id).unwrap().is_none(),
        "restore must remove the installed trove after its typed lifecycle succeeds"
    );
    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_none(),
        "restore must remove the lifecycle bundle with its trove"
    );
}

#[tokio::test]
async fn state_restore_executes_typed_rpm_install_lifecycle_and_commits() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let package_path = build_test_ccs_package_with_bundle(
        package_dir.path(),
        "vim",
        "9.1.0",
        Some(typed_rpm_pre_install_bundle("vim", "9.1.0")),
    );
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let repo_id = crate::commands::test_helpers::insert_test_static_ccs_repository(
        &conn,
        "arch-test",
        &package_url,
    );

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        package_checksum.clone(),
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url.clone(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.source_profile = Some("fedora-44".to_string());
    repo_pkg.insert(&conn).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
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

    execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await
    .expect("restore should execute the typed RPM pre-install lifecycle");

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(table_count(&conn, "changesets"), before_changesets + 1);
    assert_eq!(table_count(&conn, "troves"), before_troves + 1);
    assert!(
        conary_core::db::models::Trove::find_one_by_name(&conn, "vim")
            .unwrap()
            .is_some(),
        "restore must reinstall the target trove"
    );
    let restored = Trove::find_one_by_name(&conn, "vim")
        .unwrap()
        .expect("restored trove");
    assert_eq!(restored.installed_from_repository_id, Some(repo_id));
    assert_eq!(restored.source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(
        restored.version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(
            &conn,
            restored.id.expect("restored trove id")
        )
        .unwrap()
        .is_some(),
        "restore must retain the lifecycle bundle for future upgrades and removal"
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
            conary_core::repository::versioning::VersionScheme::Conary,
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
async fn state_restore_rolls_back_every_package_after_mid_graph_database_failure() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let baseline = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("baseline", None, None)
        .unwrap();
    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install atomic restore fixtures".to_string());
        let changeset_id = changeset.insert(tx)?;
        for name in ["alpha-state-fixture", "omega-state-fixture"] {
            let mut trove = Trove::new(
                name.to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                conary_core::repository::versioning::VersionScheme::Conary,
            );
            trove.installed_by_changeset_id = Some(changeset_id);
            trove.insert(tx)?;
        }
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();
    let drifted = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    // State members are ordered by package name. Failing omega only after
    // alpha disappeared proves the graph had already mutated the first
    // package inside the caller-owned transaction.
    conn.execute_batch(
        "CREATE TRIGGER fail_restore_after_first_delete
         BEFORE DELETE ON troves
         WHEN OLD.name = 'omega-state-fixture'
          AND NOT EXISTS (
              SELECT 1 FROM troves WHERE name = 'alpha-state-fixture'
          )
         BEGIN
             SELECT RAISE(ABORT, 'forced restore failure after first package');
         END;",
    )
    .unwrap();
    let before_changesets = table_count(&conn, "changesets");
    let before_states = table_count(&conn, "system_states");
    drop(conn);

    let error = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await
    .expect_err("second package deletion should abort the entire restore");
    assert!(
        format!("{error:#}").contains("forced restore failure after first package"),
        "{error:#}"
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    for name in ["alpha-state-fixture", "omega-state-fixture"] {
        assert!(
            Trove::find_one_by_name(&conn, name).unwrap().is_some(),
            "{name} must remain installed after wrapper rollback"
        );
    }
    assert_eq!(table_count(&conn, "changesets"), before_changesets);
    assert_eq!(table_count(&conn, "system_states"), before_states);
    assert_eq!(
        conary_core::db::models::SystemState::get_active(&conn)
            .unwrap()
            .unwrap()
            .state_number,
        drifted.state_number
    );
    let wrapper_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM changesets WHERE description = ?1",
            [format!(
                "Restore state {} -> {}",
                drifted.state_number, baseline.state_number
            )],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(wrapper_count, 0);
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
            conary_core::repository::versioning::VersionScheme::Conary,
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
            conary_core::repository::versioning::VersionScheme::Conary,
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

    crate::commands::cmd_rollback(restore_changeset_id, &db_path)
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

    let package_path = build_test_ccs_package_with_manifest(
        package_dir.path(),
        "vim",
        "9.1.0",
        None,
        |manifest| {
            manifest.provides.binaries.push("state-vim".to_string());
            let mut capabilities = conary_core::capability::CapabilityDeclaration::default();
            capabilities.network.none = true;
            manifest.capabilities = Some(capabilities);
            manifest
                .hooks
                .directories
                .push(conary_core::ccs::manifest::DirectoryHook {
                    path: "/var/lib/state-vim".to_string(),
                    mode: "0750".to_string(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    cleanup: None,
                    reversible: Some(true),
                });
        },
    );
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let repo_id = crate::commands::test_helpers::insert_test_static_ccs_repository(
        &conn,
        "arch-test",
        &package_url,
    );

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Conary,
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

    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
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
    let vim_id = vim.id.unwrap();
    assert_eq!(
        conary_core::db::models::ProvideEntry::find_by_trove_and_kind(
            &conn,
            vim_id,
            conary_core::repository::dependency_model::RepositoryCapabilityKind::Binary,
        )
        .unwrap()
        .into_iter()
        .map(|provide| provide.capability)
        .collect::<Vec<_>>(),
        vec!["state-vim"]
    );
    assert!(
        conary_core::capability::load_capabilities(&conn, vim_id)
            .unwrap()
            .unwrap()
            .network
            .none
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
    assert!(active_generation_has_path(
        Path::new(&db_path),
        "/var/lib/state-vim"
    ));
}
