// apps/conary/src/commands/install/native_events/debian_runtime/admin_projection/tests.rs

use super::*;
use conary_core::ccs::native_lifecycle::{
    DebMaintainerMetadata, DebTriggerMetadata, NATIVE_LIFECYCLE_SCHEMA_REVISION,
    NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleEntry, NativeLifecycleEntryKind,
    ScriptletFidelity, TransactionOrder, VersionScheme,
};
use std::process::Command;

fn bundle_with_interest() -> NativeLifecycleBundle {
    let body = "interest-noawait refresh-cache\n";
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Deb,
        source_family: "debian".to_string(),
        source_profile: Some("ubuntu-26.04".to_string()),
        source_release: Some("13".to_string()),
        source_arch: Some("amd64".to_string()),
        source_package: "demo".to_string(),
        source_version: "1.2-3".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Deb,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "test".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![NativeLifecycleEntry {
            id: "deb:triggers".to_string(),
            native_slot: None,
            kind: NativeLifecycleEntryKind::ControlArtifact,
            phase: conary_core::ccs::native_lifecycle::LifecyclePath::Trigger,
            lifecycle_paths: vec!["trigger".to_string()],
            interpreter: "package-manager-control-artifact".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: conary_core::hash::sha256(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "control".to_string(),
                ..TransactionOrder::default()
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            evidence_digest: None,
            source_evidence_refs: Vec::new(),
            rpm_trigger: None,
            rpm_runtime: None,
            rpm_sysusers: None,
            deb_maintainer: Some(DebMaintainerMetadata {
                control_member: DebControlMember::Triggers,
                invocations: Vec::new(),
                trigger_declarations: vec![DebTriggerMetadata {
                    directive: DebTriggerDirective::Interest,
                    trigger_name: "refresh-cache".to_string(),
                    await_mode: DebTriggerAwaitMode::NoAwait,
                    raw_line: "interest-noawait refresh-cache".to_string(),
                }],
                noninteractive: false,
            }),
            arch_install: None,
            arch_hook: None,
            residual_lifecycle: None,
        }],
    }
}

fn projected_package() -> PackageCandidate {
    PackageCandidate {
        identity: NativePackageIdentity::new("demo", "1.2-3", Some("amd64")),
        lifecycle_state: DebPackageState::Installed,
        pending_triggers: Vec::new(),
        awaited_packages: Vec::new(),
        paths: BTreeSet::from(["/etc/demo.conf".to_string(), "/usr/bin/demo".to_string()]),
        description: Some("Demo package".to_string()),
        bundle: bundle_with_interest(),
        role: Some(NativeBundleRole::Installed),
        installed_trove: true,
        persisted_state: true,
        want: "install",
    }
}

fn projected_config() -> DebianConfigSnapshot {
    DebianConfigSnapshot {
        package_name: "demo".to_string(),
        package_version: "1.2-3".to_string(),
        package_arch: Some("amd64".to_string()),
        path: "/etc/demo.conf".to_string(),
        status_digest: DebianConfigStatusDigest::Md5(
            "d41d8cd98f00b204e9800998ecf8427e".to_string(),
        ),
        remove_on_upgrade: false,
    }
}

fn projected_remove_on_upgrade_config() -> DebianConfigSnapshot {
    DebianConfigSnapshot {
        package_name: "demo".to_string(),
        package_version: "1.2-3".to_string(),
        package_arch: Some("amd64".to_string()),
        path: "/etc/removed.conf".to_string(),
        status_digest: DebianConfigStatusDigest::NewConffile,
        remove_on_upgrade: true,
    }
}

fn materialize_fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let admin = prepare_admin_directory(root.path()).unwrap();
    write_admin_files(
        &admin,
        &[projected_package()],
        &[],
        &[projected_config(), projected_remove_on_upgrade_config()],
        None,
    )
    .unwrap();
    root
}

#[test]
fn selected_root_projection_contains_exact_dpkg_layout() {
    let root = materialize_fixture();
    let admin = root.path().join("var/lib/conary/dpkg-compat");
    let status = fs::read_to_string(admin.join("status")).unwrap();
    assert!(status.contains("Package: demo\nStatus: install ok installed\n"));
    assert!(status.contains("Architecture: amd64\nVersion: 1.2-3\n"));
    assert!(status.contains("Conffiles:\n /etc/demo.conf d41d8cd98f00b204e9800998ecf8427e\n"));
    assert!(status.contains(" /etc/removed.conf newconffile remove-on-upgrade\n"));
    assert_eq!(
        fs::read_to_string(admin.join("info/demo.list")).unwrap(),
        "/etc/demo.conf\n/etc/removed.conf\n/usr/bin/demo\n"
    );
    assert_eq!(
        fs::read_to_string(admin.join("info/demo.conffiles")).unwrap(),
        "/etc/demo.conf\nremove-on-upgrade /etc/removed.conf\n"
    );
    assert_eq!(
        fs::read_to_string(admin.join("triggers/refresh-cache")).unwrap(),
        "demo/noawait\n"
    );
    assert!(admin.join("triggers/Unincorp").is_file());
    assert!(admin.join("triggers/Lock").is_file());
    assert!(!root.path().join("var/lib/dpkg").exists());
}

#[test]
fn declaration_only_remove_on_upgrade_uses_typed_newconffile_status_authority() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new(
        "demo".to_string(),
        "1.2.3".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    trove.architecture = Some("amd64".to_string());
    let trove_id = trove.insert(&conn).unwrap();
    let mut config = conary_core::db::models::ConfigFile::new_declaration(
        "/etc/removed.conf".to_string(),
        trove_id,
    );
    config.source = conary_core::db::models::ConfigSource::Deb;
    config.remove_on_upgrade = true;
    config.insert(&conn).unwrap();

    let configs = config_snapshots(&conn).unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(
        configs[0].status_digest,
        DebianConfigStatusDigest::NewConffile
    );
    assert!(configs[0].remove_on_upgrade);
}

#[test]
fn projection_refuses_live_root_and_unowned_reserved_directory() {
    assert!(reject_live_root(Path::new("/")).is_err());
    let root = tempfile::tempdir().unwrap();
    let reserved = root.path().join("var/lib/conary/dpkg-compat");
    fs::create_dir_all(&reserved).unwrap();
    fs::write(reserved.join("foreign"), b"state").unwrap();
    assert!(prepare_admin_directory(root.path()).is_err());
}

#[test]
fn real_dpkg_query_and_trigger_accept_projection_when_installed() {
    if Command::new("dpkg-query")
        .arg("--version")
        .output()
        .is_err()
        || Command::new("dpkg-trigger")
            .arg("--version")
            .output()
            .is_err()
    {
        return;
    }
    let root = materialize_fixture();
    let admin = root.path().join("var/lib/conary/dpkg-compat");
    let query = Command::new("dpkg-query")
        .arg(format!("--admindir={}", admin.display()))
        .args(["--show", "--showformat=${db:Status-Status}\\n", "demo"])
        .output()
        .unwrap();
    assert!(
        query.status.success(),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    assert_eq!(query.stdout, b"installed\n");

    let trigger = Command::new("dpkg-trigger")
        .arg(format!("--admindir={}", admin.display()))
        .args(["--by-package=demo", "refresh-cache"])
        .output()
        .unwrap();
    assert!(
        trigger.status.success(),
        "{}",
        String::from_utf8_lossy(&trigger.stderr)
    );
    assert_eq!(
        fs::read_to_string(admin.join("triggers/Unincorp")).unwrap(),
        "refresh-cache demo\n"
    );
}
