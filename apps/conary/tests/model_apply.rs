// apps/conary/tests/model_apply.rs

mod common;

use conary_core::db::models::Trove;
use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .args(args)
        .output()
        .expect("failed to run conary")
}

#[test]
fn model_apply_executes_remove_actions() {
    let (_dir, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let model_dir = tempfile::tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        "[model]\nversion = 1\ninstall = [\"openssl\"]\nexclude = [\"nginx\"]\n",
    )
    .unwrap();

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "model",
        "apply",
        "--model",
        model_path.to_str().unwrap(),
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--offline",
    ]);

    assert!(output.status.success(), "{:?}", output);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert!(Trove::find_one_by_name(&conn, "nginx").unwrap().is_none());
    assert!(Trove::find_one_by_name(&conn, "openssl").unwrap().is_some());
}

#[test]
fn model_apply_returns_err_when_every_operation_fails() {
    let (_dir, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let model_dir = tempfile::tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        "[model]\nversion = 1\ninstall = [\"does-not-exist\"]\nexclude = [\"openssl\"]\n",
    )
    .unwrap();

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "model",
        "apply",
        "--model",
        model_path.to_str().unwrap(),
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--offline",
    ]);

    assert!(!output.status.success(), "{:?}", output);
}
