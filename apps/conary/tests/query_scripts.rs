// apps/conary/tests/query_scripts.rs

mod common;

use clap::Parser;
use conary::cli::{Cli, Commands, QueryCommands};
use conary_core::ccs::builder::{CcsBuilder, write_signed_current_ccs_package};
use conary_core::ccs::manifest::CcsManifest;
use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation,
    NativeLifecycleBundle, NativeLifecycleEntry, NativeLifecycleEntryKind, RpmTriggerAction,
    RpmTriggerKind, RpmTriggerMetadata, RpmTriggerTargetConstraint, ScriptletFidelity,
    SourceFormat, TransactionOrder, VersionScheme,
};
use conary_core::db;
use conary_core::db::models::{
    Changeset, ChangesetStatus, InstalledCcsRemoveHook, InstalledNativeLifecycleBundle, Trove,
    TroveType,
};
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

fn parse_query_scripts(args: &[&str]) -> QueryCommands {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    let cli = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || Cli::try_parse_from(args))
        .expect("parser thread should spawn")
        .join()
        .expect("parser thread should not panic")
        .expect("parse CLI");
    match cli.command.expect("command") {
        Commands::Query(command @ QueryCommands::Scripts { .. }) => command,
        _ => panic!("expected query scripts command"),
    }
}

fn run_conary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
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

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, got {}\n{}",
        output.status,
        output_text(output)
    );
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "expected failure, got success\n{}",
        output_text(output)
    );
}

fn build_ccs_fixture(
    name: &str,
    version: &str,
    bundle: Option<NativeLifecycleBundle>,
) -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source_dir = temp.path().join("src");
    std::fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
    std::fs::write(source_dir.join("usr/bin/fixture"), b"fixture\n").unwrap();

    let mut manifest = CcsManifest::new_minimal(name, version);
    if let Some(bundle) = &bundle {
        manifest.package.version_scheme = match &bundle.source_format {
            SourceFormat::Rpm => conary_core::repository::versioning::VersionScheme::Rpm,
            SourceFormat::Deb => conary_core::repository::versioning::VersionScheme::Debian,
            SourceFormat::Arch => conary_core::repository::versioning::VersionScheme::Arch,
        };
    }
    manifest.native_lifecycle = bundle;

    let result = CcsBuilder::new(manifest, &source_dir)
        .unwrap()
        .build()
        .unwrap();
    let package_path = temp.path().join(format!("{name}.ccs"));
    let policy_path = temp.path().join("ccs-trust-policy.toml");
    let signer = conary_core::ccs::SigningKeyPair::generate().with_key_id("query-integration");
    write_signed_current_ccs_package(&result, &package_path, &signer, false).unwrap();
    std::fs::write(
        &policy_path,
        format!("trusted_keys = [\"{}\"]\n", signer.public_key_base64()),
    )
    .unwrap();
    let policy = conary_core::ccs::TrustPolicy::from_file(&policy_path).unwrap();
    conary_core::ccs::verify::verify_package(&package_path, &policy)
        .expect("query fixture must carry trusted current authority");
    (temp, package_path, policy_path)
}

fn build_rpm_fixture() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source_dir = temp.path().join("src");
    std::fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
    std::fs::write(
        source_dir.join("usr/bin/native-query-fixture"),
        b"fixture\n",
    )
    .unwrap();

    let mut manifest = CcsManifest::new_minimal("native-query-fixture", "1.0.0");
    manifest.package.license = Some("MIT".to_string());
    manifest.package.platform = Some(conary_core::ccs::manifest::Platform {
        arch: Some("x86_64".to_string()),
        ..Default::default()
    });
    let result = CcsBuilder::new(manifest, &source_dir)
        .unwrap()
        .build()
        .unwrap();
    let package_path = temp.path().join("native-query-fixture.rpm");
    conary_core::ccs::native_export::rpm::generate(&result, &package_path)
        .expect("generate native RPM query fixture");
    (temp, package_path)
}

fn bundle_fixture() -> NativeLifecycleBundle {
    let pre_remove_body = "ldconfig\n";
    let post_install_body = "systemctl daemon-reload\n";
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "nginx".to_string(),
        source_version: "1.28.0-1.fc44".to_string(),
        source_checksum: Some(
            "sha256:3333333333333333333333333333333333333333333333333333333333333333".to_string(),
        ),
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "typed-native-lifecycle".to_string(),
        evidence_digest: Some(
            "sha256:5555555555555555555555555555555555555555555555555555555555555555".to_string(),
        ),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![
            entry_fixture("rpm:%preun", pre_remove_body, true),
            entry_fixture("rpm:%post", post_install_body, false),
        ],
    }
}

fn zero_entry_bundle_fixture() -> NativeLifecycleBundle {
    let mut bundle = bundle_fixture();
    bundle.entries.clear();
    bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;
    bundle
}

fn entry_fixture(id: &str, body: &str, with_reserved_metadata: bool) -> NativeLifecycleEntry {
    NativeLifecycleEntry {
        id: id.to_string(),
        native_slot: id.split(':').nth(1).unwrap_or("%post").to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: if id.ends_with("%preun") {
            LifecyclePath::PreRemove
        } else {
            LifecyclePath::PostInstall
        },
        lifecycle_paths: vec!["install:first".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: vec!["-e".to_string()],
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation {
            args: vec!["1".to_string()],
            environment: vec!["RPM_INSTALL_PREFIX=/".to_string()],
            stdin: Some("none".to_string()),
            chroot: Some("install-root".to_string()),
        },
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: vec![],
            after: vec!["payload".to_string()],
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: vec!["ldconfig".to_string()],
        evidence_digest: Some(
            "sha256:6666666666666666666666666666666666666666666666666666666666666666".to_string(),
        ),
        source_evidence_refs: vec!["capture:rpm:%post".to_string()],
        rpm_trigger: with_reserved_metadata.then(|| RpmTriggerMetadata {
            kind: RpmTriggerKind::File,
            target_constraints: vec![RpmTriggerTargetConstraint {
                package: "systemd".to_string(),
                action: RpmTriggerAction::Install,
                operator: Some(">=".to_string()),
                version: Some("255".to_string()),
                raw_flags: 0,
            }],
            priority: Some(100),
            path_prefixes: vec!["/usr/lib/systemd/system".to_string()],
        }),
        rpm_runtime: Some(conary_core::ccs::native_lifecycle::RpmRuntimeMetadata {
            program: conary_core::ccs::native_lifecycle::RpmProgram::External,
            body_transforms: Vec::new(),
            critical: false,
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

#[test]
fn query_scripts_accepts_verbose_flag() {
    let command = parse_query_scripts(&["conary", "query", "scripts", "nginx.ccs", "--verbose"]);

    match command {
        QueryCommands::Scripts {
            package_path,
            verbose,
            entry,
            json,
            ..
        } => {
            assert_eq!(package_path, "nginx.ccs");
            assert!(verbose);
            assert_eq!(entry, None);
            assert!(!json);
        }
        _ => panic!("expected query scripts command"),
    }
}

#[test]
fn query_scripts_accepts_entry_filter() {
    let command = parse_query_scripts(&[
        "conary",
        "query",
        "scripts",
        "nginx.ccs",
        "--entry",
        "rpm:%post",
    ]);

    match command {
        QueryCommands::Scripts {
            package_path,
            verbose,
            entry,
            json,
            ..
        } => {
            assert_eq!(package_path, "nginx.ccs");
            assert!(!verbose);
            assert_eq!(entry.as_deref(), Some("rpm:%post"));
            assert!(!json);
        }
        _ => panic!("expected query scripts command"),
    }
}

#[test]
fn query_scripts_accepts_json_flag() {
    let command = parse_query_scripts(&["conary", "query", "scripts", "nginx.ccs", "--json"]);

    match command {
        QueryCommands::Scripts {
            package_path,
            verbose,
            entry,
            json,
            ..
        } => {
            assert_eq!(package_path, "nginx.ccs");
            assert!(!verbose);
            assert_eq!(entry, None);
            assert!(json);
        }
        _ => panic!("expected query scripts command"),
    }
}

#[test]
fn query_scripts_accepts_installed_package_selectors() {
    let command = parse_query_scripts(&[
        "conary",
        "query",
        "scripts",
        "nginx",
        "--db-path",
        "/tmp/conary-test.db",
        "--version",
        "1.28.0",
        "--arch",
        "x86_64",
    ]);

    match command {
        QueryCommands::Scripts { package_path, .. } => {
            assert_eq!(package_path, "nginx");
        }
        _ => panic!("expected query scripts command"),
    }
}

fn install_scriptlet_query_fixture() -> (TempDir, String) {
    let (temp, db_path, mut conn) = common::create_test_db();

    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install nginx scriptlet query fixture".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.28.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        InstalledCcsRemoveHook::new(
            trove_id,
            "echo CCS remove hook body must stay hidden\n".to_string(),
            Some(true),
        )
        .insert_or_replace(tx)?;

        let bundle = bundle_fixture();
        let mut installed_bundle =
            InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), &bundle)
                .expect("build installed bundle row");
        installed_bundle
            .insert_or_replace(tx)
            .expect("insert installed bundle row");

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();

    (temp, db_path)
}

#[test]
fn query_scripts_installed_package_distinguishes_ccs_hook_and_native_entries() {
    let (_temp, db_path) = install_scriptlet_query_fixture();
    let output = run_conary(&[
        "query",
        "scripts",
        "nginx",
        "--db-path",
        &db_path,
        "--version",
        "1.28.0",
        "--arch",
        "x86_64",
    ]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Installed package: nginx 1.28.0 [x86_64]"));
    assert!(stdout.contains("Installed CCS remove hook (installed_ccs_remove_hooks): present"));
    assert!(
        stdout.contains("source=installed_ccs_remove_hooks interpreter=/bin/sh reversible=true")
    );
    assert!(stdout.contains("Installed native lifecycle entries: 2"));
    assert!(stdout.contains("rpm:%post"));
    assert!(stdout.contains("lifecycle=post-install"));
    assert!(!stdout.contains("echo CCS remove hook body must stay hidden"));
    assert!(!stdout.contains("systemctl daemon-reload"));
}

#[test]
fn query_scripts_ccs_bundle_prints_summary() {
    let (_temp, package_path, policy_path) =
        build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Native lifecycle bundle: conary.native-lifecycles.v1"));
    assert!(stdout.contains("Native entries: 2"));
    assert!(stdout.contains("rpm:%post"));
    assert!(!stdout.contains("systemctl daemon-reload"));
}

#[test]
fn query_scripts_ccs_bundle_verbose_prints_exact_metadata() {
    let (_temp, package_path, policy_path) =
        build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--verbose",
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Interpreter: /bin/sh"));
    assert!(stdout.contains("Lifecycle paths: install:first"));
    assert!(stdout.contains("body_sha256="));
    assert!(!stdout.contains("systemctl daemon-reload"));
}

#[test]
fn query_scripts_ccs_bundle_entry_filter_prints_single_entry() {
    let (_temp, package_path, policy_path) =
        build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--entry",
        "rpm:%post",
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rpm:%post"));
    assert!(!stdout.contains("rpm:%preun"));
}

#[test]
fn query_scripts_ccs_bundle_missing_entry_exits_with_error() {
    let (_temp, package_path, policy_path) =
        build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--entry",
        "rpm:%missing",
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_failure(&output);
    assert!(
        output_text(&output).contains("native lifecycle bundle entry 'rpm:%missing' not found")
    );
}

#[test]
fn query_scripts_ccs_bundle_json_is_stable() {
    let (_temp, package_path, policy_path) =
        build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--json",
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(json["package"]["name"], "nginx");
    assert_eq!(json["bundle_present"], true);
    assert_eq!(json["bundle"]["schema"], "conary.native-lifecycles.v1");
    assert_eq!(json["entries"][0]["id"], "rpm:%preun");
    assert!(json["entries"][0]["body"].is_null());
    assert!(stdout.contains("body_sha256"));
    assert!(!stdout.contains("systemctl daemon-reload"));
}

#[test]
fn query_scripts_ccs_without_bundle_exits_successfully() {
    let (_temp, package_path, policy_path) = build_ccs_fixture("plain", "1.0.0", None);
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("No native lifecycle bundle found"));
}

#[test]
fn query_scripts_ccs_without_bundle_json_reports_absent_bundle() {
    let (_temp, package_path, policy_path) = build_ccs_fixture("plain", "1.0.0", None);
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--json",
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid absent-bundle json");
    assert_eq!(json["bundle_present"], false);
    assert!(json["bundle"].is_null());
    assert!(
        json["entries"]
            .as_array()
            .expect("entries array")
            .is_empty()
    );
}

#[test]
fn query_scripts_ccs_zero_entry_bundle_json_reports_empty_entries() {
    let (_temp, package_path, policy_path) =
        build_ccs_fixture("native-free", "1.0.0", Some(zero_entry_bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--json",
        "--policy",
        policy_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid zero-entry json");
    assert_eq!(json["bundle_present"], true);
    assert!(
        json["entries"]
            .as_array()
            .expect("entries array")
            .is_empty()
    );
}

#[test]
fn query_scripts_native_json_reports_ccs_bundle_only() {
    let (_temp, package_path) = build_rpm_fixture();
    let output = run_conary(&["query", "scripts", package_path.to_str().unwrap(), "--json"]);

    assert_failure(&output);
    assert!(output_text(&output).contains("only available for CCS native lifecycle bundles"));
}

#[test]
fn query_scripts_native_entry_filter_reports_ccs_bundle_only() {
    let (_temp, package_path) = build_rpm_fixture();
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--entry",
        "rpm:%post",
    ]);

    assert_failure(&output);
    assert!(output_text(&output).contains("only available for CCS native lifecycle bundles"));
}
