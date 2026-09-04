// conary-core/src/db/models/installed_native_lifecycle_bundle/tests.rs

use crate::ccs::native_lifecycle::{
    ArchInstallMetadata, DebMaintainerMetadata, LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1,
    NativeInvocation, NativeLifecycleBundle, NativeLifecycleEntry, NativeLifecycleEntryKind,
    RpmTriggerMetadata, ScriptletFidelity, ScriptletSandboxRequirements, SourceFormat,
    TransactionOrder, VersionScheme,
};
use crate::ccs::native_transaction::{DebPackageState, NativePackageIdentity};
use crate::db::models::{
    Changeset, ChangesetStatus, InstalledNativeLifecycleBundle, Trove, TroveType,
};
use crate::db::testing::create_test_db;
use rusqlite::params;

fn fixture_changeset(conn: &rusqlite::Connection) -> i64 {
    let mut changeset = Changeset::new("Install native lifecycle fixture".to_string());
    let id = changeset.insert(conn).expect("insert changeset");
    changeset
        .update_status(conn, ChangesetStatus::Applied)
        .expect("mark changeset applied");
    id
}

fn fixture_trove(conn: &rusqlite::Connection) -> i64 {
    let mut trove = Trove::new(
        "native-lifecycle-fixture".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.insert(conn).expect("insert trove")
}

fn sha256_prefixed(body: &str) -> String {
    crate::hash::sha256_prefixed(body.as_bytes())
}

fn fixture_entry(id: &str, body: &str) -> NativeLifecycleEntry {
    NativeLifecycleEntry {
        id: id.to_string(),
        native_slot: crate::packages::native_abi::RpmScriptletSlot::from_tag(
            id.strip_prefix("rpm:").expect("fixture id"),
        ),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PostInstall,
        lifecycle_paths: vec!["install:last".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: sha256_prefixed(body),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: Vec::new(),
            after: vec!["payload".to_string()],
        },
        timeout_ms: 30_000,
        sandbox: Some(ScriptletSandboxRequirements {
            network: false,
            namespaces: vec!["mount".to_string()],
            seccomp_profile: Some("native-lifecycle/default".to_string()),
        }),
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None::<RpmTriggerMetadata>,
        rpm_runtime: Some(crate::ccs::native_lifecycle::RpmRuntimeMetadata {
            program: crate::ccs::native_lifecycle::RpmProgram::External,
            body_transforms: Vec::new(),
            criticality: crate::ccs::native_lifecycle::RpmCriticality::WarningOnly,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: Default::default(),
            header_context: Default::default(),
            package_rpm_version: None,
        }),
        rpm_sysusers: None,
        deb_maintainer: None::<DebMaintainerMetadata>,
        arch_install: None::<ArchInstallMetadata>,
        arch_hook: None,
        residual_lifecycle: None,
    }
}

fn fixture_bundle() -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: crate::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "native-lifecycle-fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "goal6-test".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"fixture-evidence")),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![fixture_entry("rpm:%post", "systemctl daemon-reload\n")],
    }
}

fn insert_fixture(
    conn: &rusqlite::Connection,
) -> (
    i64,
    i64,
    NativeLifecycleBundle,
    InstalledNativeLifecycleBundle,
) {
    let changeset_id = fixture_changeset(conn);
    let trove_id = fixture_trove(conn);
    let bundle = fixture_bundle();
    let mut installed = InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), &bundle)
        .expect("build installed bundle");
    installed
        .insert_or_replace(conn)
        .expect("insert installed bundle");
    (trove_id, changeset_id, bundle, installed)
}

#[test]
fn insert_and_find_by_trove_round_trips_scalars_and_bundle() {
    let (_tmp, conn) = create_test_db();
    let (trove_id, changeset_id, bundle, installed) = insert_fixture(&conn);

    assert!(installed.id.is_some());
    let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
        .expect("find installed bundle")
        .expect("installed bundle row");

    assert_eq!(found.trove_id, trove_id);
    assert_eq!(found.installed_changeset_id, Some(changeset_id));
    assert_eq!(found.source_format, "rpm");
    assert_eq!(found.source_family, "fedora");
    assert_eq!(found.source_package, "native-lifecycle-fixture");
    assert_eq!(found.scriptlet_fidelity, "native-lifecycle");
    assert_eq!(found.evidence_digest, bundle.evidence_digest);
    assert!(found.installed_at.is_some());
    assert_eq!(found.bundle().expect("decode bundle"), bundle);
}

#[test]
fn insert_or_replace_updates_existing_trove_row() {
    let (_tmp, conn) = create_test_db();
    let (trove_id, _changeset_id, mut bundle, _installed) = insert_fixture(&conn);
    bundle.evidence_digest = Some(crate::hash::sha256_prefixed(b"replacement-evidence"));
    bundle.entries[0].body = "echo replacement\n".to_string();
    bundle.entries[0].body_sha256 = sha256_prefixed(&bundle.entries[0].body);

    let mut installed =
        InstalledNativeLifecycleBundle::new(trove_id, None, &bundle).expect("build replacement");
    installed
        .insert_or_replace(&conn)
        .expect("replace installed bundle");

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM installed_native_lifecycle_bundles WHERE trove_id = ?1",
            [trove_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
        .unwrap()
        .unwrap();
    assert_eq!(found.installed_changeset_id, None);
    assert_eq!(found.bundle().unwrap(), bundle);
}

#[test]
fn bundle_rejects_evidence_digest_mismatch() {
    let (_tmp, conn) = create_test_db();
    let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

    conn.execute(
        "UPDATE installed_native_lifecycle_bundles SET evidence_digest = ?1 WHERE trove_id = ?2",
        params![
            "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            trove_id
        ],
    )
    .unwrap();

    let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
        .unwrap()
        .unwrap();
    let error = found.bundle().expect_err("mismatched evidence must fail");

    assert!(error.to_string().contains("evidence_digest mismatch"));
}

#[test]
fn malformed_bundle_toml_is_loaded_but_bundle_returns_error() {
    let (_tmp, conn) = create_test_db();
    let changeset_id = fixture_changeset(&conn);
    let trove_id = fixture_trove(&conn);

    conn.execute(
        "INSERT INTO installed_native_lifecycle_bundles (
                trove_id, source_format, source_family, source_package, source_version,
                scriptlet_fidelity, evidence_digest, bundle_toml,
                installed_changeset_id
             ) VALUES (?1, 'rpm', 'fedora', 'native-lifecycle-fixture', '1.0-1',
                'native-lifecycle', NULL, ?2, ?3)",
        params![trove_id, "not = [valid toml", changeset_id],
    )
    .unwrap();

    let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
        .unwrap()
        .unwrap();
    let error = found.bundle().expect_err("malformed TOML must fail");

    assert!(error.to_string().contains("native lifecycle bundle TOML"));
}

#[test]
fn debian_runtime_state_round_trips_by_exact_trove_identity() {
    let (_tmp, conn) = create_test_db();
    let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

    let updated = InstalledNativeLifecycleBundle::update_runtime_state_by_trove(
        &conn,
        trove_id,
        DebPackageState::TriggersAwaited,
        &["ldconfig".to_string(), "update-mime".to_string()],
        &[NativePackageIdentity {
            package_name: "consumer-a".to_string(),
            package_version: "1".to_string(),
            package_arch: Some("x86_64".to_string()),
        }],
    )
    .expect("update runtime state");
    assert_eq!(updated, 1);

    let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
        .expect("read runtime state")
        .expect("installed bundle");
    assert_eq!(found.lifecycle_state, DebPackageState::TriggersAwaited);
    assert_eq!(found.pending_triggers, ["ldconfig", "update-mime"]);
    assert_eq!(
        found.awaited_packages,
        [NativePackageIdentity {
            package_name: "consumer-a".to_string(),
            package_version: "1".to_string(),
            package_arch: Some("x86_64".to_string()),
        }]
    );
}

#[test]
fn malformed_runtime_state_json_is_rejected_during_row_materialization() {
    let (_tmp, conn) = create_test_db();
    let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);
    conn.execute(
        "UPDATE installed_native_lifecycle_bundles
             SET pending_triggers_json = '{\"not\":\"a-list\"}'
             WHERE trove_id = ?1",
        [trove_id],
    )
    .unwrap();

    InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
        .expect_err("malformed trigger state must fail closed");
}

#[test]
fn deleting_trove_cascades_installed_bundle_row() {
    let (_tmp, conn) = create_test_db();
    let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

    Trove::delete(&conn, trove_id).expect("delete trove");

    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn delete_by_trove_removes_installed_bundle_row() {
    let (_tmp, conn) = create_test_db();
    let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

    let deleted = InstalledNativeLifecycleBundle::delete_by_trove(&conn, trove_id)
        .expect("delete installed bundle");

    assert_eq!(deleted, 1);
    assert!(
        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .is_none()
    );
}
