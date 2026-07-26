// apps/conary/tests/live_host_mutation_safety.rs

mod common;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
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
    let (tmp, db_path, conn) = common::create_test_db();
    drop(conn);

    let live_root = tmp.path().join("live-root");
    let live_file = live_root.join("usr/bin/fixture");
    fs::create_dir_all(live_file.parent().unwrap()).unwrap();
    fs::write(&live_file, b"native fixture payload").unwrap();

    let fake_bin = tmp.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    write_executable(
        &fake_bin.join("dpkg-query"),
        "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then
    printf 'dpkg-query fixture 1.0\\n'
    exit 0
fi
case \"$3\" in
    *Description*)
        printf 'fixture\\036fixture\\0361.2.3\\036amd64\\036Fixture package\\036Test maintainer\\036\\036utils\\036optional\\0361\\037'
        ;;
    *Depends*)
        printf '\\036libc6 (>= 2.0)\\037'
        ;;
    *Provides*)
        printf 'fixture\\036fixture\\036fixture-capability\\037'
        ;;
    *)
        exit 1
        ;;
esac
",
    );
    write_executable(
        &fake_bin.join("dpkg"),
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"-L\" ] && [ \"$2\" = \"fixture\" ]; then
    printf '%s\\n' '{}'
    exit 0
fi
exit 1
",
            live_file.display()
        ),
    );

    let db_name = Path::new(&db_path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let database_before = common::database_snapshot(&db_path);
    let tree_before = tree_snapshot(tmp.path(), &db_name);
    let live_file_before = fs::read(&live_file).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "system",
            "adopt",
            "fixture",
            "--dry-run",
            "--db-path",
            &db_path,
        ])
        .env("PATH", &fake_bin)
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
    assert!(stdout.contains("fixture 1.2.3 [amd64]"));
    assert!(stdout.contains("track (metadata only)"));
    assert!(stdout.contains("1 package, 1 files, 1 dependencies, 2 provides"));
    assert!(stderr.contains("Preview only:"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("--yes"));

    assert_eq!(common::database_snapshot(&db_path), database_before);
    assert_eq!(tree_snapshot(tmp.path(), &db_name), tree_before);
    assert_eq!(fs::read(&live_file).unwrap(), live_file_before);
}
