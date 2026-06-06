// apps/conary/tests/live_host_mutation_safety.rs

mod common;

use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn seed_adopted_trove_without_source_identity(db_path: &str, name: &str) {
    use conary_core::db;
    use conary_core::db::models::{Changeset, ChangesetStatus, InstallSource, Trove, TroveType};

    let mut conn = db::open(db_path).unwrap();
    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new(format!("Seed adopted {name}"));
        let changeset_id = changeset.insert(tx)?;
        let mut trove = Trove::new_with_source(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.source_distro = None;
        trove.version_scheme = None;
        trove.insert(tx)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();
}

fn source_identity_for(db_path: &str, name: &str) -> (Option<String>, Option<String>) {
    let conn = conary_core::db::open(db_path).unwrap();
    conn.query_row(
        "SELECT source_distro, version_scheme FROM troves WHERE name = ?1",
        [name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

#[test]
fn install_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "install",
        "nginx",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may change packages"));
}

#[test]
fn install_refuses_without_apply_intent_and_mentions_yes() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "install",
        "nginx",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary install"));
    assert!(stderr.contains("--dry-run"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn install_with_yes_reaches_underlying_package_resolution() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "install",
        "nginx",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may mutate"));
}

#[test]
fn deprecated_global_flag_still_reaches_underlying_package_resolution() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "install",
        "nginx",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("unrecognized option"));
    assert!(!stderr.contains("may mutate"));
}

#[test]
fn collection_install_refusal_uses_collection_label() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "install",
        "@web-stack",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary install @collection"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn state_revert_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "state", "revert", "1", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system state revert"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn model_apply_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let model_dir = tempfile::tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        "[model]\nversion = 1\ninstall = [\"openssl\"]\nexclude = [\"nginx\"]\n",
    )
    .unwrap();

    let output = run_conary(&[
        "model",
        "apply",
        "--model",
        model_path.to_str().unwrap(),
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary model apply"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn automation_apply_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "automation",
        "apply",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary automation apply"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn automation_apply_dry_run_bypasses_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "automation",
        "apply",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--dry-run",
        "--yes",
    ]);

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_restore_dry_run_bypasses_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "system",
        "restore",
        "all",
        "--dry-run",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
    ]);

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn allow_flag_reaches_underlying_restore_error() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "system",
        "restore",
        "missing-package",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));
    assert!(!stderr.contains("allow-live-system-mutation only if"));
}

#[test]
fn excluded_system_gc_is_not_gated() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let missing_objects = tempfile::tempdir().unwrap().path().join("objects");

    let output = run_conary(&[
        "system",
        "gc",
        "--db-path",
        &db_path,
        "--objects-dir",
        missing_objects.to_str().unwrap(),
        "--dry-run",
    ]);

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_package_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "curl", "--db-path", &db_path]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may update Conary DB"));
}

#[test]
fn system_adopt_package_no_longer_requires_live_mutation_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "curl", "--db-path", &db_path]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may mutate"));
}

#[test]
fn system_adopt_system_help_does_not_reference_live_mutation_flag() {
    let output = run_conary(&["system", "adopt", "--help"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may update Conary DB"));
}

#[test]
fn system_adopt_refresh_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "--refresh", "--db-path", &db_path]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may update Conary DB"));
}

#[test]
fn system_adopt_convert_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();
    seed_adopted_trove_without_source_identity(&db_path, "curl");

    let output = run_conary(&["system", "adopt", "--convert", "--db-path", &db_path]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may update Conary DB"));
}

#[test]
fn system_adopt_sync_hook_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "--sync-hook", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system adopt --sync-hook"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_status_bypasses_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "--status", "--db-path", &db_path]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_package_dry_run_is_rejected_without_ack_prompt() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&[
        "system",
        "adopt",
        "curl",
        "--dry-run",
        "--db-path",
        &db_path,
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("single-package adoption dry-run is not implemented"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_convert_dry_run_does_not_backfill_source_identity() {
    let (_tmp, db_path) = common::setup_command_test_db();
    seed_adopted_trove_without_source_identity(&db_path, "curl");

    let output = run_conary(&[
        "system",
        "adopt",
        "--convert",
        "--dry-run",
        "--db-path",
        &db_path,
    ]);

    assert!(output.status.success());
    assert_eq!(source_identity_for(&db_path, "curl"), (None, None));
}
