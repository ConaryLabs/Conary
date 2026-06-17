// apps/conary/tests/packaging_m3d.rs

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::builder::write_signed_ccs_package;
use conary_core::ccs::{CcsBuilder, CcsManifest, SigningKeyPair};

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn assert_failure(output: &Output) {
    assert!(!output.status.success(), "{}", output_text(output));
}

fn write_record_source(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("payload.txt"), "hello record\n").unwrap();
    std::fs::write(
        root.join("install.sh"),
        r#"#!/bin/sh
set -eu
mkdir -p "$DESTDIR/usr/share/record-demo"
cp payload.txt "$DESTDIR/usr/share/record-demo/payload.txt"
"#,
    )
    .unwrap();
}

fn record_base_command(source: &Path, recorded: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_conary"));
    command
        .arg("cook")
        .arg("--record")
        .arg(source)
        .args(["--record-backend", "inotify"])
        .arg("--record-output")
        .arg(recorded);
    command
}

fn add_install_command(mut command: Command) -> Command {
    command.arg("--").arg("/bin/sh").arg("install.sh");
    command
}

fn build_recorded_draft_ccs(temp: &tempfile::TempDir, key: &SigningKeyPair) -> PathBuf {
    let source = temp.path().join("recorded-draft-source");
    let package_path = temp.path().join("dist/recorded-draft.ccs");
    std::fs::create_dir_all(source.join("usr/share/m3d")).unwrap();
    std::fs::create_dir_all(package_path.parent().unwrap()).unwrap();
    std::fs::write(source.join("usr/share/m3d/payload"), "hello\n").unwrap();
    let manifest = CcsManifest::parse(
        r#"
[package]
name = "m3d-recorded-draft"
version = "1.0"
description = "recorded draft fixture"
license = "MIT"

[provenance]
origin_class = "recorded-draft"
hardening_level = "host"
"#,
    )
    .unwrap();
    let result = CcsBuilder::new(manifest, &source).build().unwrap();
    write_signed_ccs_package(&result, &package_path, key).unwrap();
    package_path
}

fn write_publish_key_pair(key_dir: &Path) -> SigningKeyPair {
    std::fs::create_dir_all(key_dir).unwrap();
    let key = SigningKeyPair::generate().with_key_id("publish");
    key.save_to_files(
        &key_dir.join("publish.private"),
        &key_dir.join("publish.public"),
    )
    .unwrap();
    key
}

#[test]
fn cook_record_is_hidden_and_requires_command() {
    let help = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["cook", "--help"])
        .output()
        .expect("cook help");
    assert_success(&help);
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(!help_text.contains("--record"));

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_record_source(&source);
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg("--record")
        .arg(&source)
        .output()
        .expect("cook record without command");
    assert_failure(&output);
    assert!(output_text(&output).contains("requires a command"));
}

#[test]
fn cook_record_inotify_generates_source_recipe_and_redacted_report() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let recorded = temp.path().join("recorded/demo");
    write_record_source(&source);

    let mut command = record_base_command(&source, &recorded);
    command.arg("--record-unsafe-host");
    let output = add_install_command(command).output().expect("cook record");

    assert_success(&output);
    assert!(
        output_text(&output).contains("WARNING: executing record command directly on the host")
    );
    assert!(recorded.join("source/payload.txt").is_file());
    let recipe = std::fs::read_to_string(recorded.join("recipe.toml")).unwrap();
    assert!(recipe.contains("path = \"source\""));
    assert!(recipe.contains("install.sh"));
    assert!(!recipe.contains(temp.path().to_str().unwrap()));

    let report = std::fs::read_to_string(recorded.join("trace-report.json")).unwrap();
    assert!(report.contains("\"backend\""));
    assert!(report.contains("incomplete-read-evidence"));
    assert!(report.contains("unsafe-host"));
    assert!(report.contains("usr/share/record-demo/payload.txt"));
    assert!(!report.contains(temp.path().to_str().unwrap()));
}

#[test]
fn cook_record_json_emits_packaging_output() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let recorded = temp.path().join("recorded/demo");
    write_record_source(&source);

    let mut command = record_base_command(&source, &recorded);
    command.arg("--record-unsafe-host").arg("--json");
    let output = add_install_command(command)
        .output()
        .expect("cook record json");

    assert_success(&output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "conary cook --record");
    assert_eq!(value["status"], "succeeded");
    assert_eq!(value["artifacts"][0]["kind"], "recipe");
}

#[test]
fn cook_record_sandbox_mode_fails_closed_or_records_successfully() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let recorded = temp.path().join("recorded/demo");
    write_record_source(&source);

    let output = add_install_command(record_base_command(&source, &recorded))
        .output()
        .expect("cook record sandboxed");

    if output.status.success() {
        assert!(recorded.join("recipe.toml").is_file());
        assert!(recorded.join("trace-report.json").is_file());
    } else {
        assert!(output_text(&output).contains("record mode failed"));
        assert!(recorded.join("trace-report.json").is_file());
    }
}

#[test]
fn cook_record_allow_network_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_record_source(&source);
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg("--record")
        .arg("--record-allow-network")
        .arg(&source)
        .arg("--")
        .arg("/bin/true")
        .output()
        .expect("cook record allow network");
    assert_failure(&output);
    assert!(output_text(&output).contains("reserved"));
}

#[test]
fn cook_record_validate_reports_success_or_validation_failure() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let recorded = temp.path().join("recorded/demo");
    write_record_source(&source);

    let mut command = record_base_command(&source, &recorded);
    command.arg("--record-unsafe-host").arg("--record-validate");
    let output = add_install_command(command)
        .output()
        .expect("cook record validate");

    if output.status.success() {
        assert!(recorded.join("dist").is_dir());
    } else {
        assert!(output_text(&output).contains("record mode failed"));
        let report = std::fs::read_to_string(recorded.join("trace-report.json")).unwrap();
        assert!(report.contains("validation-failed"));
    }
}

#[test]
fn publish_recorded_draft_artifact_form_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let keys = temp.path().join("keys");
    let key = write_publish_key_pair(&keys);
    let artifact = build_recorded_draft_ccs(&temp, &key);
    let repo = temp.path().join("repo");
    let state_file = temp.path().join("publish-state.toml");

    let publish = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("publish")
        .arg(&artifact)
        .arg(&repo)
        .arg("--key-dir")
        .arg(&keys)
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .output()
        .expect("publish recorded draft artifact");

    assert_failure(&publish);
    let value: serde_json::Value = serde_json::from_slice(&publish.stdout).expect("valid json");
    let failure_code = value["diagnostics"][0]["evidence"][0]["metadata"]["publish_lint_report"]
        ["failures"][0]["code"]
        .as_str()
        .expect("publish gate failure code");
    assert!(
        matches!(
            failure_code,
            "missing-attestation" | "recorded-draft-artifact" | "non-hermetic-hardening-level"
        ),
        "{}",
        output_text(&publish)
    );
    assert!(!repo.exists(), "publish gate failure must not create repo");
}
