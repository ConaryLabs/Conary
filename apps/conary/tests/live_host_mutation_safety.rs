// apps/conary/tests/live_host_mutation_safety.rs

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn tree_snapshot(root: &Path, db_name: &str) -> Vec<(PathBuf, String, Vec<u8>)> {
    let mut snapshot = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.path() != root)
        .filter(|entry| {
            !entry
                .path()
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(db_name))
        })
        .map(|entry| {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().is_dir() {
                (relative, "directory".to_string(), Vec::new())
            } else if entry.file_type().is_symlink() {
                (
                    relative,
                    "symlink".to_string(),
                    fs::read_link(path)
                        .unwrap()
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec(),
                )
            } else {
                (relative, "file".to_string(), fs::read(path).unwrap())
            }
        })
        .collect::<Vec<_>>();
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

#[test]
fn install_with_yes_needs_no_retired_live_mutation_flag() {
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
        "always",
        "--yes",
    ]);

    assert!(
        output.status.success(),
        "install with explicit apply intent failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
        "always",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary install"));
    assert!(stderr.contains("--dry-run"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn removed_global_flag_is_rejected_before_package_resolution() {
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
        "always",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--allow-live-system-mutation'"));
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
        "always",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary install @collection"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn ccs_install_refuses_without_apply_intent_and_mentions_yes() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let package = package_dir.path().join("missing.ccs");

    let output = run_conary(&[
        "ccs",
        "install",
        package.to_str().unwrap(),
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "always",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary ccs install"));
    assert!(stderr.contains("--dry-run"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn ccs_install_dry_run_bypasses_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let package = package_dir.path().join("missing.ccs");

    let output = run_conary(&[
        "ccs",
        "install",
        package.to_str().unwrap(),
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "always",
        "--dry-run",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("conary ccs install"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may mutate"));
}

#[test]
fn ccs_install_with_yes_reaches_underlying_package_read() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let package = package_dir.path().join("missing.ccs");

    let output = run_conary(&[
        "ccs",
        "install",
        package.to_str().unwrap(),
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "always",
        "--yes",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("conary ccs install"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may mutate"));
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
fn removed_global_flag_is_rejected_before_restore() {
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
    assert!(stderr.contains("unexpected argument '--allow-live-system-mutation'"));
}

#[test]
fn removed_system_gc_surface_is_rejected() {
    let output = run_conary(&["system", "gc"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand 'gc'"));
}

#[test]
fn retired_schema_rebuild_refuses_without_confirmation_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO schema_version (version) VALUES (66);",
    )
    .unwrap();
    drop(conn);
    let before = fs::read(&db_path).unwrap();

    let output = run_conary(&[
        "system",
        "rebuild-db",
        "--discard-state",
        "--db-path",
        db_path.to_str().unwrap(),
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system rebuild-db"), "{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert_eq!(fs::read(&db_path).unwrap(), before);
    assert!(!temp.path().join("backups").exists());
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

#[cfg(unix)]
#[test]
fn system_adopt_package_dry_run_previews_without_mutation_or_ack_prompt() {
    if !Path::new("/var/lib/dpkg/info/base-files.list").is_file()
        || !Path::new("/usr/bin/dpkg-query").is_file()
        || !Path::new("/usr/bin/apt-mark").is_file()
    {
        return;
    }
    let (tmp, db_path, conn) = common::create_test_db();
    drop(conn);

    let db_name = Path::new(&db_path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let database_before = common::database_snapshot(&db_path);
    let tree_before = tree_snapshot(tmp.path(), &db_name);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "system",
            "adopt",
            "base-files",
            "--package-manager",
            "dpkg",
            "--dry-run",
            "--db-path",
            &db_path,
        ])
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .output()
        .expect("failed to run conary package adoption preview");

    assert!(
        output.status.success(),
        "preview failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Package adoption preview"));
    assert!(stdout.contains("base-files"));
    assert!(stdout.contains("track (metadata only)"));
    assert!(stderr.contains("Preview only:"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("--yes"));

    assert_eq!(common::database_snapshot(&db_path), database_before);
    assert_eq!(tree_snapshot(tmp.path(), &db_name), tree_before);
}
