// apps/conary/tests/query_scripts.rs

use clap::Parser;
use conary::cli::{Cli, Commands, QueryCommands};
use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
use conary_core::ccs::legacy_scriptlets::{
    DecisionCounts, EffectConfidence, EffectReplacement, EffectSource, ForeignReplayPolicy,
    LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
    NativeInvocation, PublicationPolicy, PublicationStatus, RpmTriggerMetadata,
    RpmTriggerTargetConstraint, ScriptletDecision, ScriptletEffect, ScriptletFidelity,
    SourceFormat, TargetCompatibility, TransactionOrder, VersionScheme,
};
use conary_core::ccs::manifest::CcsManifest;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

fn parse_query_scripts(args: &[&str]) -> QueryCommands {
    let cli = Cli::try_parse_from(args).expect("parse CLI");
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
    bundle: Option<LegacyScriptletBundle>,
) -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source_dir = temp.path().join("src");
    std::fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
    std::fs::write(source_dir.join("usr/bin/fixture"), b"fixture\n").unwrap();

    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.legacy_scriptlets = bundle;

    let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
    let package_path = temp.path().join(format!("{name}.ccs"));
    write_ccs_package(&result, &package_path).unwrap();
    (temp, package_path)
}

fn bundle_fixture() -> LegacyScriptletBundle {
    let replaced_body = "ldconfig\n";
    let legacy_body = "systemctl daemon-reload\n";
    LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_distro: Some("fedora".to_string()),
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
        conversion_policy: "safe-or-legacy".to_string(),
        adapter_registry_digest: Some(
            "sha256:4444444444444444444444444444444444444444444444444444444444444444".to_string(),
        ),
        target_policy_digest: None,
        evidence_digest: Some(
            "sha256:5555555555555555555555555555555555555555555555555555555555555555".to_string(),
        ),
        target_compatibility: TargetCompatibility::SourceNative,
        allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::PublicIfNoBlocked,
        publication_status: PublicationStatus::PrivateReview,
        scriptlet_fidelity: ScriptletFidelity::Mixed,
        decision_counts: DecisionCounts {
            replaced: 1,
            legacy: 1,
            blocked: 0,
            review: 0,
            extra: BTreeMap::new(),
        },
        unsupported_class_counts: BTreeMap::new(),
        entries: vec![
            entry_fixture(
                "rpm:%preun",
                ScriptletDecision::Replaced,
                replaced_body,
                true,
            ),
            entry_fixture("rpm:%post", ScriptletDecision::Legacy, legacy_body, false),
        ],
        extra: BTreeMap::new(),
    }
}

fn zero_entry_bundle_fixture() -> LegacyScriptletBundle {
    let mut bundle = bundle_fixture();
    bundle.entries.clear();
    bundle.decision_counts = DecisionCounts::default();
    bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;
    bundle
}

fn entry_fixture(
    id: &str,
    decision: ScriptletDecision,
    body: &str,
    with_reserved_metadata: bool,
) -> LegacyScriptletEntry {
    LegacyScriptletEntry {
        id: id.to_string(),
        native_slot: id.split(':').nth(1).unwrap_or("%post").to_string(),
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
            extra: BTreeMap::new(),
        },
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: vec![],
            after: vec!["payload".to_string()],
            extra: BTreeMap::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: vec!["ldconfig".to_string()],
        decision,
        reason_code: "protected-replay-required".to_string(),
        human_reason: Some("fixture reason".to_string()),
        evidence_digest: Some(
            "sha256:6666666666666666666666666666666666666666666666666666666666666666".to_string(),
        ),
        source_evidence_refs: vec!["capture:rpm:%post".to_string()],
        effects: vec![ScriptletEffect {
            kind: "ldconfig".to_string(),
            source: EffectSource::StaticSignal,
            confidence: EffectConfidence::Declared,
            replacement: EffectReplacement::Complete,
            adapter_id: Some("ldconfig/v1".to_string()),
            adapter_digest: Some(
                "sha256:7777777777777777777777777777777777777777777777777777777777777777"
                    .to_string(),
            ),
            command: Some("ldconfig".to_string()),
            args: vec!["-X".to_string()],
            path: Some("/usr/lib64".to_string()),
            reason_code: Some("ldconfig-cache-refresh".to_string()),
            extra: BTreeMap::new(),
        }],
        unknown_commands: vec!["systemctl".to_string()],
        blocked_classes: vec![],
        rpm_trigger: with_reserved_metadata.then(|| RpmTriggerMetadata {
            kind: "file-trigger".to_string(),
            condition: Some("in".to_string()),
            target_constraints: vec![RpmTriggerTargetConstraint {
                package: "systemd".to_string(),
                operator: Some(">=".to_string()),
                version: Some("255".to_string()),
                extra: BTreeMap::new(),
            }],
            priority: Some(100),
            file_globs: vec!["/usr/lib/systemd/system/*.service".to_string()],
            stdin_contract: Some("paths".to_string()),
            transaction_order: Some("post-transaction".to_string()),
            extra: BTreeMap::new(),
        }),
        deb_maintainer: None,
        arch_install: None,
        residual_replay: None,
        extra: BTreeMap::new(),
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
fn query_scripts_ccs_bundle_prints_summary() {
    let (_temp, package_path) = build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&["query", "scripts", package_path.to_str().unwrap()]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Legacy scriptlet bundle: conary.legacy-scriptlets.v1"));
    assert!(stdout.contains("Entries: 1 replaced, 1 legacy, 0 blocked, 0 review"));
    assert!(stdout.contains("rpm:%post"));
    assert!(!stdout.contains("systemctl daemon-reload"));
}

#[test]
fn query_scripts_ccs_bundle_verbose_prints_effects() {
    let (_temp, package_path) = build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--verbose",
    ]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Effects:"));
    assert!(stdout.contains("ldconfig"));
    assert!(stdout.contains("body_sha256="));
    assert!(!stdout.contains("systemctl daemon-reload"));
}

#[test]
fn query_scripts_ccs_bundle_entry_filter_prints_single_entry() {
    let (_temp, package_path) = build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--entry",
        "rpm:%post",
    ]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rpm:%post"));
    assert!(!stdout.contains("rpm:%preun"));
}

#[test]
fn query_scripts_ccs_bundle_missing_entry_exits_with_error() {
    let (_temp, package_path) = build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&[
        "query",
        "scripts",
        package_path.to_str().unwrap(),
        "--entry",
        "rpm:%missing",
    ]);

    assert_failure(&output);
    assert!(
        output_text(&output).contains("legacy scriptlet bundle entry 'rpm:%missing' not found")
    );
}

#[test]
fn query_scripts_ccs_bundle_json_is_stable() {
    let (_temp, package_path) = build_ccs_fixture("nginx", "1.28.0", Some(bundle_fixture()));
    let output = run_conary(&["query", "scripts", package_path.to_str().unwrap(), "--json"]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(json["package"]["name"], "nginx");
    assert_eq!(json["bundle_present"], true);
    assert_eq!(json["bundle"]["schema"], "conary.legacy-scriptlets.v1");
    assert_eq!(json["entries"][0]["id"], "rpm:%preun");
    assert!(json["entries"][0]["body"].is_null());
    assert!(stdout.contains("body_sha256"));
    assert!(!stdout.contains("systemctl daemon-reload"));
}

#[test]
fn query_scripts_ccs_without_bundle_exits_successfully() {
    let (_temp, package_path) = build_ccs_fixture("plain", "1.0.0", None);
    let output = run_conary(&["query", "scripts", package_path.to_str().unwrap()]);

    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("No legacy scriptlet bundle found"));
}

#[test]
fn query_scripts_ccs_without_bundle_json_reports_absent_bundle() {
    let (_temp, package_path) = build_ccs_fixture("plain", "1.0.0", None);
    let output = run_conary(&["query", "scripts", package_path.to_str().unwrap(), "--json"]);

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
    let (_temp, package_path) =
        build_ccs_fixture("native-free", "1.0.0", Some(zero_entry_bundle_fixture()));
    let output = run_conary(&["query", "scripts", package_path.to_str().unwrap(), "--json"]);

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
    let temp = tempfile::NamedTempFile::with_suffix(".rpm").unwrap();
    std::fs::write(temp.path(), b"not really rpm").unwrap();
    let output = run_conary(&["query", "scripts", temp.path().to_str().unwrap(), "--json"]);

    assert_failure(&output);
    assert!(output_text(&output).contains("only available for CCS legacy scriptlet bundles"));
}

#[test]
fn query_scripts_native_entry_filter_reports_ccs_bundle_only() {
    let temp = tempfile::NamedTempFile::with_suffix(".rpm").unwrap();
    std::fs::write(temp.path(), b"not really rpm").unwrap();
    let output = run_conary(&[
        "query",
        "scripts",
        temp.path().to_str().unwrap(),
        "--entry",
        "rpm:%post",
    ]);

    assert_failure(&output);
    assert!(output_text(&output).contains("only available for CCS legacy scriptlet bundles"));
}
