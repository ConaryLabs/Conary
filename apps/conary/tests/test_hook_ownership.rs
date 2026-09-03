// apps/conary/tests/test_hook_ownership.rs
//! Test-hook ownership and startup fencing regression tests.

use std::path::Path;
use std::process::Command;

#[test]
fn conary_test_environment_names_have_one_source_owner() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let owner = source_root.join("test_hooks.rs");
    let mut violations = Vec::new();

    for entry in walkdir::WalkDir::new(&source_root) {
        let entry = entry.expect("source tree entry should be readable");
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("rs")
            || path == owner
        {
            continue;
        }
        let source = std::fs::read_to_string(path).expect("Rust source should be UTF-8");
        for (index, line) in source.lines().enumerate() {
            if line.contains("CONARY_TEST_") {
                violations.push(format!("{}:{}: {line}", path.display(), index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Conary test-hook names must live only in src/test_hooks.rs:\n{}",
        violations.join("\n")
    );
}

#[cfg(not(feature = "test-hooks"))]
#[test]
fn ordinary_binary_rejects_test_hook_environment_before_clap_exit() {
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_BOOT_ID", "probe")
        .arg("--version")
        .output()
        .expect("conary should execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: test-hook environment variables are disabled"));
    assert!(stderr.contains("CONARY_TEST_BOOT_ID"));
}

#[cfg(feature = "test-hooks")]
#[test]
fn test_hook_binary_accepts_test_hook_environment() {
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .arg("--version")
        .output()
        .expect("conary should execute");

    assert!(output.status.success());
}
