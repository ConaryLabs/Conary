// apps/conary-test/src/config/tests/native_corpus/performance.rs

use super::super::conary_fixture_path;
use std::process::Command;

#[test]
fn command_performance_recorder_keeps_exact_identity_and_failure_metrics() {
    let temp = tempfile::tempdir().unwrap();
    let output_path = temp.path().join("sample.json");
    let script = conary_fixture_path("native/record-command-performance.py");
    let source_commit = "a".repeat(40);
    let fixture_sha256 = "b".repeat(64);

    let output = Command::new("python3")
        .arg(&script)
        .args([
            "--output",
            output_path.to_str().unwrap(),
            "--implementation",
            "conary",
            "--operation",
            "install",
            "--cache-state",
            "cold",
            "--source-commit",
            &source_commit,
            "--fixture-sha256",
            &fixture_sha256,
            "--sample",
            "1",
            "--",
            "/usr/bin/python3",
            "-c",
            "import sys; sys.exit(7)",
        ])
        .output()
        .expect("run command performance recorder");
    assert_eq!(output.status.code(), Some(7));

    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output_path).unwrap()).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["identity"]["source_commit"], source_commit);
    assert_eq!(value["identity"]["fixture_sha256"], fixture_sha256);
    assert_eq!(value["identity"]["implementation"], "conary");
    assert_eq!(value["identity"]["operation"], "install");
    assert_eq!(value["identity"]["cache_state"], "cold");
    assert_eq!(value["identity"]["sample"], 1);
    assert_eq!(value["command"]["argv"][0], "/usr/bin/python3");
    assert_eq!(value["outcome"]["exit_code"], 7);
    assert!(value["outcome"]["signal"].is_null());
    for field in [
        "wall_ns",
        "user_cpu_ns",
        "system_cpu_ns",
        "max_rss_kib",
        "minor_page_faults",
        "major_page_faults",
        "block_input_operations",
        "block_output_operations",
        "voluntary_context_switches",
        "involuntary_context_switches",
    ] {
        assert!(
            value["process"][field].as_u64().is_some(),
            "missing non-negative process metric {field}: {value}"
        );
    }

    let original = std::fs::read(&output_path).unwrap();
    let duplicate = Command::new("python3")
        .arg(&script)
        .args([
            "--output",
            output_path.to_str().unwrap(),
            "--implementation",
            "apt",
            "--operation",
            "install",
            "--cache-state",
            "warm",
            "--source-commit",
            &source_commit,
            "--fixture-sha256",
            &fixture_sha256,
            "--sample",
            "2",
            "--",
            "/usr/bin/true",
        ])
        .output()
        .expect("refuse duplicate performance record");
    assert_eq!(duplicate.status.code(), Some(73));
    assert_eq!(std::fs::read(&output_path).unwrap(), original);
}
