// tests/logging_verbosity.rs

use std::process::{Command, Output};

fn run_with_env(args: &[&str], rust_log: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_conary"));
    command.args(args);
    match rust_log {
        Some(value) => {
            command.env("RUST_LOG", value);
        }
        None => {
            command.env_remove("RUST_LOG");
        }
    }
    command.output().expect("failed to run conary")
}

fn temp_db_path() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("conary.db").to_string_lossy().to_string();
    (dir, db)
}

#[test]
fn system_init_is_quiet_by_default() {
    let (_tmp, db) = temp_db_path();
    let out = run_with_env(&["system", "init", "--db-path", &db], None);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("INFO"),
        "default stderr should carry no INFO logs, got:\n{stderr}"
    );
}

#[test]
fn verbose_restores_info_logs() {
    let (_tmp, db) = temp_db_path();
    let out = run_with_env(&["--verbose", "system", "init", "--db-path", &db], None);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("INFO"),
        "--verbose should surface INFO logs, got:\n{stderr}"
    );
}

#[test]
fn rust_log_overrides_quiet() {
    let (_tmp, db) = temp_db_path();
    let out = run_with_env(
        &["--quiet", "system", "init", "--db-path", &db],
        Some("info"),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("INFO"),
        "RUST_LOG=info should override --quiet, got:\n{stderr}"
    );
}
