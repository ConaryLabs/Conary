// apps/conary/tests/live_host_mutation_readiness.rs

mod common;

use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

#[test]
fn system_restore_all_dry_run_reports_missing_cas_without_live_mounting() {
    let (_temp_dir, db_path) = common::setup_command_test_db();
    let root_dir = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "system",
        "restore",
        "all",
        "--dry-run",
        "--db-path",
        &db_path,
        "--root",
        root_dir.path().to_str().unwrap(),
    ]);

    assert!(
        output.status.success(),
        "restore dry-run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
