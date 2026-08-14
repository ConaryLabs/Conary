// tests/native_pm_daily_driver.rs

mod common;

use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation,
    NativeLifecycleBundle, NativeLifecycleEntry, NativeLifecycleEntryKind, RpmCriticality,
    RpmProgram, RpmRuntimeMetadata, ScriptletFidelity, SourceFormat, TransactionOrder,
    VersionScheme as LifecycleVersionScheme,
};
use conary_core::db;
use conary_core::db::models::{
    ExistingDirectoryMaterialization, FileEntry, InstallReason, InstallSource,
    InstalledNativeLifecycleBundle, Trove, TroveType,
};
use conary_core::packages::InstalledPackageIdentity;
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use std::fs;
use std::process::{Command, Output};

fn run_conary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn regular_file_entry(path: String, content: &[u8], trove_id: i64) -> FileEntry {
    FileEntry::new(
        path,
        resolved_test_node(PayloadNode::regular(0o644)),
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(content),
            size: content.len() as u64,
        }),
        trove_id,
    )
}

fn directory_file_entry(path: String, trove_id: i64) -> FileEntry {
    let mut node = PayloadNode::regular(0o755);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | 0o755;
    FileEntry::new(path, resolved_test_node(node), None, trove_id)
}

fn resolved_test_node(mut node: PayloadNode) -> ResolvedPayloadNode {
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn seed_orphan(
    conn: &rusqlite::Connection,
    root: &std::path::Path,
    name: &str,
    source: InstallSource,
) {
    let version = if source.is_adopted() {
        "1.0.0-1"
    } else {
        "1.0.0"
    };
    let version_scheme = if source.is_adopted() {
        conary_core::repository::versioning::VersionScheme::Rpm
    } else {
        conary_core::repository::versioning::VersionScheme::Conary
    };
    let mut trove = Trove::new_with_source(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        source.clone(),
        version_scheme,
    );
    if source.is_adopted() {
        trove.architecture = Some("x86_64".to_string());
        trove.native_package_identity = Some(
            InstalledPackageIdentity::rpm(
                format!("{name}-1.0.0-1.x86_64"),
                name,
                None,
                "1.0.0",
                "1",
                "x86_64",
            )
            .unwrap(),
        );
    }
    trove.install_reason = InstallReason::Dependency;
    trove.selection_reason = Some("Required by removed-parent".to_string());
    let trove_id = trove.insert(conn).unwrap();
    seed_orphan_payload(conn, root, name, trove_id);
}

fn seed_orphan_payload(
    conn: &rusqlite::Connection,
    root: &std::path::Path,
    name: &str,
    trove_id: i64,
) {
    let payload = root.join(format!("usr/share/{name}/payload.txt"));
    fs::create_dir_all(payload.parent().unwrap()).unwrap();
    fs::write(&payload, name).unwrap();

    let package_dir = format!("/usr/share/{name}");
    for path in ["/usr", "/usr/share", package_dir.as_str()] {
        directory_file_entry(path.to_string(), trove_id)
            .insert_or_replace(
                conn,
                ExistingDirectoryMaterialization::PreserveExistingDirectory,
            )
            .unwrap();
    }
    let stored_hash = conary_core::filesystem::CasStore::new(root.join("objects"))
        .unwrap()
        .store(name.as_bytes())
        .unwrap();
    assert_eq!(stored_hash, conary_core::hash::sha256(name.as_bytes()));
    regular_file_entry(
        format!("/usr/share/{name}/payload.txt"),
        name.as_bytes(),
        trove_id,
    )
    .insert(conn)
    .unwrap();
}

fn seed_broken_orphan(conn: &rusqlite::Connection, root: &std::path::Path, name: &str) {
    let version = "1.0.0-1.fc44";
    let mut trove = Trove::new_with_source(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.install_reason = InstallReason::Dependency;
    trove.selection_reason = Some("Required by removed-parent".to_string());
    let trove_id = trove.insert(conn).unwrap();
    seed_orphan_payload(conn, root, name, trove_id);

    let bundle = missing_interpreter_remove_bundle(name, version);
    let mut installed = InstalledNativeLifecycleBundle::new(trove_id, None, &bundle).unwrap();
    installed.insert_or_replace(conn).unwrap();
}

fn missing_interpreter_remove_bundle(package: &str, version: &str) -> NativeLifecycleBundle {
    let body = "exit 0\n".to_string();
    let entry = NativeLifecycleEntry {
        id: "rpm:%preun".to_string(),
        native_slot: Some(conary_core::packages::native_abi::RpmScriptletSlot::PreUn),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PreRemove,
        lifecycle_paths: vec![LifecyclePath::PreRemove.as_str().to_string()],
        interpreter: "/definitely/missing/conary-hook".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body,
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "pre-remove".to_string(),
            ..TransactionOrder::default()
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::External,
            body_transforms: Vec::new(),
            criticality: RpmCriticality::SlotDefault,
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
    };
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package.to_string(),
        source_version: version.to_string(),
        source_checksum: None,
        version_scheme: LifecycleVersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "autoremove-preflight-fixture".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![entry],
    }
}

#[test]
fn autoremove_dry_run_lists_conary_owned_orphans_and_skips_adopted() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(
        &conn,
        root.path(),
        "owned-orphan",
        InstallSource::Repository,
    );
    seed_orphan(
        &conn,
        root.path(),
        "adopted-orphan",
        InstallSource::AdoptedTrack,
    );
    drop(conn);

    let output = run_conary(&[
        "autoremove",
        "--dry-run",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("owned-orphan 1.0.0"), "{stdout}");
    assert!(
        stdout.contains("Skipping adopted orphaned package(s)"),
        "{stdout}"
    );
    assert!(stdout.contains("adopted-orphan"), "{stdout}");
}

#[test]
fn autoremove_apply_removes_owned_orphan_without_deleting_adopted_orphan() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(
        &conn,
        root.path(),
        "owned-orphan",
        InstallSource::Repository,
    );
    seed_orphan(
        &conn,
        root.path(),
        "adopted-orphan",
        InstallSource::AdoptedTrack,
    );
    drop(conn);

    let output = run_conary(&[
        "autoremove",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "always",
        "--yes",
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(
        root.path()
            .join("usr/share/owned-orphan/payload.txt")
            .exists()
    );
    assert!(
        root.path()
            .join("usr/share/adopted-orphan/payload.txt")
            .exists()
    );
    let conn = db::open(&db_path).unwrap();
    assert!(
        Trove::find_by_name(&conn, "owned-orphan")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        Trove::find_by_name(&conn, "adopted-orphan").unwrap().len(),
        1
    );
}

#[test]
fn autoremove_preflight_failure_leaves_all_orphans_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_broken_orphan(&conn, root.path(), "broken-orphan");
    seed_orphan(
        &conn,
        root.path(),
        "owned-orphan",
        InstallSource::Repository,
    );
    drop(conn);

    let output = run_conary(&[
        "autoremove",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "always",
        "--yes",
    ]);
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        root.path()
            .join("usr/share/owned-orphan/payload.txt")
            .exists()
    );

    let text = output_text(&output);
    assert!(
        text.contains("autoremove lifecycle execution preflight failed"),
        "{text}"
    );
    assert!(text.contains("at stage PackagePreRemove"), "{text}");
    let conn = db::open(&db_path).unwrap();
    assert!(
        Trove::find_one_by_name(&conn, "broken-orphan")
            .unwrap()
            .is_some()
    );
    assert!(
        Trove::find_one_by_name(&conn, "owned-orphan")
            .unwrap()
            .is_some()
    );
}

#[test]
fn list_info_files_and_path_show_installed_package_identity() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let info = run_conary(&["list", "nginx", "--info", "--db-path", &db_path]);
    assert!(info.status.success(), "{}", output_text(&info));
    let info_stdout = String::from_utf8_lossy(&info.stdout);
    assert!(info_stdout.contains("Name        : nginx"), "{info_stdout}");
    assert!(
        info_stdout.contains("Authority   : conary-owned"),
        "{info_stdout}"
    );
    assert!(info_stdout.contains("Pinned      : no"), "{info_stdout}");

    let files = run_conary(&["list", "nginx", "--files", "--db-path", &db_path]);
    assert!(files.status.success(), "{}", output_text(&files));
    let files_stdout = String::from_utf8_lossy(&files.stdout);
    assert!(files_stdout.contains("/usr/sbin/nginx"), "{files_stdout}");
    assert!(
        files_stdout.contains("/etc/nginx/nginx.conf"),
        "{files_stdout}"
    );

    let path = run_conary(&["list", "--path", "/usr/sbin/nginx", "--db-path", &db_path]);
    assert!(path.status.success(), "{}", output_text(&path));
    let path_stdout = String::from_utf8_lossy(&path.stdout);
    assert!(
        path_stdout.contains("nginx 1.24.0 provides"),
        "{path_stdout}"
    );
}

#[test]
fn pin_blocks_remove_and_unpin_allows_remove() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(
        &conn,
        root.path(),
        "pin-remove-demo",
        InstallSource::Repository,
    );
    conn.execute(
        "UPDATE troves SET install_reason = 'explicit', selection_reason = 'Explicitly installed' WHERE name = 'pin-remove-demo'",
        [],
    )
    .unwrap();
    drop(conn);

    let pin = run_conary(&[
        "pin",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
    ]);
    assert!(pin.status.success(), "{}", output_text(&pin));

    let blocked = run_conary(&[
        "remove",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "always",
        "--yes",
    ]);
    assert!(!blocked.status.success(), "{}", output_text(&blocked));
    assert!(output_text(&blocked).contains("is pinned"));

    let unpin = run_conary(&[
        "unpin",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
    ]);
    assert!(unpin.status.success(), "{}", output_text(&unpin));

    let removed = run_conary(&[
        "remove",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "always",
        "--yes",
    ]);
    assert!(removed.status.success(), "{}", output_text(&removed));
}

// Provider and breakage parity are covered by apps/conary/tests/query.rs:
// - whatprovides_reports_installed_and_repository_providers
// - whatbreaks_reports_same_dependency_blocker_as_remove
