// apps/conary/tests/packaging_m3a.rs

use std::path::Path;
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

fn build_missing_attestation_ccs(
    temp: &tempfile::TempDir,
    key: &SigningKeyPair,
) -> std::path::PathBuf {
    let source = temp.path().join("source");
    let package_path = temp.path().join("dist/missing-attestation.ccs");
    std::fs::create_dir_all(source.join("usr/share/m3a")).unwrap();
    std::fs::create_dir_all(package_path.parent().unwrap()).unwrap();
    std::fs::write(source.join("usr/share/m3a/payload"), "hello\n").unwrap();
    let manifest = CcsManifest::parse(
        r#"
[package]
name = "m3a-missing-attestation"
version = "1.0"
description = "missing attestation fixture"
license = "MIT"

[provenance]
origin_class = "native-built"
hardening_level = "hermetic"
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
fn cook_validate_only_json_writes_redacted_operation_record() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let recipe = project.join("recipe.toml");
    let dist = temp.path().join("dist");
    let source_cache = temp.path().join("sources");
    let records = temp.path().join("records");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("payload.txt"), "hello\n").unwrap();
    std::fs::write(
        &recipe,
        r#"
[package]
name = "m3a-json"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/m3a-json && cp payload.txt %(destdir)s/usr/share/m3a-json/payload.txt"
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg(&recipe)
        .arg("--validate-only")
        .arg("--json")
        .arg("--output")
        .arg(&dist)
        .arg("--source-cache")
        .arg(&source_cache)
        .env("CONARY_PACKAGING_OPERATIONS_DIR", &records)
        .output()
        .expect("run conary cook --json");

    assert_success(&output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "conary cook");
    assert_eq!(value["status"], "succeeded");
    let operation_id = value["operation_id"].as_str().expect("operation id");
    let record_path = records.join(format!("{operation_id}.json"));
    assert!(
        record_path.is_file(),
        "missing record {}",
        record_path.display()
    );
    let record_text = std::fs::read_to_string(record_path).unwrap();
    let record_value: serde_json::Value = serde_json::from_str(&record_text).unwrap();
    assert_eq!(record_value, value);
    assert!(!record_text.contains("sk-"));
}

#[test]
fn publish_artifact_form_json_reports_gate_failure_without_bypass() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let keys = temp.path().join("keys");
    let state_file = temp.path().join("publish-state.toml");
    let records = temp.path().join("records");
    let key = write_publish_key_pair(&keys);
    let artifact = build_missing_attestation_ccs(&temp, &key);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("publish")
        .arg(&artifact)
        .arg(&repo)
        .arg("--key-dir")
        .arg(&keys)
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .env("CONARY_PACKAGING_OPERATIONS_DIR", &records)
        .output()
        .expect("run conary publish --json");

    assert_failure(&output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "publish-gate-failed");
    assert_eq!(
        value["diagnostics"][0]["evidence"][0]["metadata"]["publish_lint_report"]["failures"][0]["code"],
        "missing-attestation"
    );
    let operation_id = value["operation_id"].as_str().expect("operation id");
    let record_path = records.join(format!("{operation_id}.json"));
    assert!(
        record_path.is_file(),
        "missing record {}",
        record_path.display()
    );
    let record_text = std::fs::read_to_string(record_path).unwrap();
    let record_value: serde_json::Value = serde_json::from_str(&record_text).unwrap();
    assert_eq!(record_value["operation_id"], value["operation_id"]);
    assert_eq!(record_value["status"], value["status"]);
    assert_eq!(
        record_value["diagnostics"][0]["evidence"][0]["metadata"]["publish_lint_report"]["failures"]
            [0]["code"],
        "missing-attestation"
    );
    assert!(!repo.exists(), "publish gate failure must not create repo");
}
