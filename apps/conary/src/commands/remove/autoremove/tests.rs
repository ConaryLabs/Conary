// apps/conary/src/commands/remove/autoremove/tests.rs

use super::*;
#[cfg(feature = "test-hooks")]
use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleBundle,
    NativeLifecycleEntry, NativeLifecycleEntryKind, ScriptletFidelity, SourceFormat,
    TransactionOrder, VersionScheme,
};
#[cfg(feature = "test-hooks")]
use conary_core::db::models::InstalledNativeLifecycleBundle;
use conary_core::db::models::{InstallReason, InstallSource, TroveType};
use tempfile::TempDir;

#[test]
fn autoremove_plan_uses_recorded_ownership_and_pin_state() {
    let owned = Trove::new_with_source(
        "owned-orphan".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let adopted = Trove::new_with_source(
        "adopted-orphan".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::AdoptedTrack,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let mut pinned = Trove::new_with_source(
        "pinned-orphan".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    pinned.pinned = true;
    let ordinary_named_bash = Trove::new_with_source(
        "bash".to_string(),
        "5.2.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Conary,
    );

    let plan = plan_autoremove(vec![owned, adopted, pinned, ordinary_named_bash]);

    assert_eq!(
        plan.removable
            .iter()
            .map(|trove| trove.name.as_str())
            .collect::<Vec<_>>(),
        vec!["owned-orphan", "bash"]
    );
    assert_eq!(
        plan.skipped
            .iter()
            .map(|(trove, reason)| (trove.name.as_str(), reason))
            .collect::<Vec<_>>(),
        vec![
            (
                "adopted-orphan",
                &AutoremoveSkipReason::AdoptedNativeAuthority
            ),
            ("pinned-orphan", &AutoremoveSkipReason::Pinned),
        ]
    );
}

#[tokio::test]
#[cfg(feature = "test-hooks")]
async fn autoremove_automatically_replays_native_remove_lifecycle() {
    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    let conn = conary_core::db::open(&db_path).unwrap();
    seed_dependency_trove(
        &conn,
        "aa-plain-orphan",
        "1.0.0",
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let native_trove_id = seed_dependency_trove(
        &conn,
        "zz-native-orphan",
        "1.0.0-1.fc44",
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    seed_installed_native_lifecycle_bundle(&conn, native_trove_id, "zz-native-orphan");
    drop(conn);

    cmd_autoremove(
        db_path.to_string_lossy().as_ref(),
        false,
        SandboxMode::Always,
    )
    .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(table_count(&conn, "troves"), 1);
    assert!(
        Trove::find_one_by_name(&conn, "test-runtime-base")
            .unwrap()
            .is_some()
    );
    assert_eq!(table_count(&conn, "installed_native_lifecycle_bundles"), 0);
    assert_eq!(table_count(&conn, "changesets"), 2);
    let metadata = changeset_metadata_by_description(&conn, "Remove zz-native-orphan-1.0.0-1.fc44");
    assert!(
        metadata["removed_troves"][0]["native_lifecycle"]["bundle_toml"]
            .as_str()
            .unwrap()
            .contains("rpm:%postun")
    );

    let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.clone());
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .expect("autoremove should publish a generation");
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation),
    )
    .unwrap();
    let marker = artifact
        .generation_root
        .entries
        .iter()
        .find(|entry| entry.path == "/autoremove-native-lifecycle-ran")
        .expect("RPM remove lifecycle marker in published root");
    let content = marker.content.as_ref().expect("marker content authority");
    assert_eq!(
        conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
            .unwrap()
            .retrieve(&content.sha256)
            .unwrap(),
        b"zz-native-orphan"
    );
}

#[test]
fn autoremove_preflight_reads_package_identity_under_the_mutation_lock() {
    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    let conn = conary_core::db::open(&db_path).unwrap();
    let trove_id = seed_dependency_trove(
        &conn,
        "autoremove-lock-fixture",
        "1.0.0",
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
    drop(conn);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let locked =
        crate::commands::generation::selected_root::LockedRuntimeRoot::acquire(&db_path_string)
            .unwrap();
    let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
    let waiter_db_path = db_path_string.clone();
    let waiter = std::thread::spawn(move || {
        let conn = conary_core::db::open(&waiter_db_path).unwrap();
        attempt_tx.send(()).unwrap();
        let result = preflight_autoremove_round(
            &conn,
            &[trove],
            &waiter_db_path,
            RemoveLifecycleOptions::new(SandboxMode::Always),
        )
        .map_err(|error| format!("{error:#}"));
        result_tx.send(result).unwrap();
    });

    attempt_rx.recv().unwrap();
    assert!(
        matches!(
            result_rx.recv_timeout(std::time::Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "autoremove preflight reached a verdict while another transaction held the mutation lock"
    );

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        conn.execute(
            "UPDATE troves SET version = '2.0.0' WHERE id = ?1",
            [trove_id],
        )
        .unwrap(),
        1
    );
    drop(conn);
    drop(locked);

    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("autoremove preflight must finish once the mutation lock is released")
        .expect_err("autoremove preflight accepted a package identity changed before it locked");
    waiter.join().unwrap();
    assert!(
        error.contains("identity changed during removal preparation"),
        "{error}"
    );
}

fn seed_dependency_trove(
    conn: &rusqlite::Connection,
    name: &str,
    version: &str,
    version_scheme: conary_core::repository::versioning::VersionScheme,
) -> i64 {
    let mut trove = Trove::new_with_source(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        InstallSource::Repository,
        version_scheme,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.install_reason = InstallReason::Dependency;
    trove.selection_reason = Some("Required by fixture-root".to_string());
    trove.insert(conn).unwrap()
}

#[cfg(feature = "test-hooks")]
fn seed_installed_native_lifecycle_bundle(
    conn: &rusqlite::Connection,
    trove_id: i64,
    package: &str,
) {
    let bundle = native_post_remove_bundle(package);
    let mut installed = InstalledNativeLifecycleBundle::new(trove_id, None, &bundle).unwrap();
    installed.insert_or_replace(conn).unwrap();
}

#[cfg(feature = "test-hooks")]
fn native_post_remove_bundle(package: &str) -> NativeLifecycleBundle {
    let entry = native_post_remove_entry();
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package.to_string(),
        source_version: "1.0.0-1.fc44".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "goal6-autoremove-test".to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{package}-native-remove-evidence").as_bytes(),
        )),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![entry],
    }
}

#[cfg(feature = "test-hooks")]
fn native_post_remove_entry() -> NativeLifecycleEntry {
    let body = r#"
local marker = assert(io.open("/autoremove-native-lifecycle-ran", "w"))
marker:write("zz-native-orphan")
marker:close()
"#;
    NativeLifecycleEntry {
        id: "rpm:%postun".to_string(),
        native_slot: Some(conary_core::packages::native_abi::RpmScriptletSlot::PostUn),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PostRemove,
        lifecycle_paths: vec!["remove:post".to_string()],
        interpreter: "<lua>".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation {
            args: Vec::new(),
            environment: Vec::new(),
            stdin: None,
            chroot: None,
        },
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: Vec::new(),
            after: Vec::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            b"rpm:%postun:print replay-post-remove",
        )),
        source_evidence_refs: vec!["capture:rpm:%postun".to_string()],
        rpm_trigger: None,
        rpm_runtime: Some(conary_core::ccs::native_lifecycle::RpmRuntimeMetadata {
            program: conary_core::ccs::native_lifecycle::RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            criticality: conary_core::ccs::native_lifecycle::RpmCriticality::WarningOnly,
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

#[cfg(feature = "test-hooks")]
fn table_count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[cfg(feature = "test-hooks")]
fn changeset_metadata_by_description(
    conn: &rusqlite::Connection,
    description: &str,
) -> serde_json::Value {
    let raw: Option<String> = conn
        .query_row(
            "SELECT metadata FROM changesets WHERE description = ?1",
            [description],
            |row| row.get(0),
        )
        .expect("changeset metadata");
    serde_json::from_str(&raw.expect("changeset metadata should be present"))
        .expect("changeset metadata is JSON")
}
