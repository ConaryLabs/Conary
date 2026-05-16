// tests/native_pm_daily_driver.rs

mod common;

use conary_core::db;
use conary_core::db::models::{FileEntry, InstallReason, InstallSource, Trove, TroveType};
use std::fs;
use std::process::{Command, Output};

fn run_conary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn seed_orphan(
    conn: &rusqlite::Connection,
    root: &std::path::Path,
    name: &str,
    source: InstallSource,
) {
    let payload = root.join(format!("usr/share/{name}/payload.txt"));
    fs::create_dir_all(payload.parent().unwrap()).unwrap();
    fs::write(&payload, name).unwrap();

    let mut trove = Trove::new_with_source(
        name.to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        source,
    );
    trove.install_reason = InstallReason::Dependency;
    trove.selection_reason = Some("Required by removed-parent".to_string());
    let trove_id = trove.insert(conn).unwrap();
    FileEntry::new(
        format!("/usr/share/{name}/payload.txt"),
        "0".repeat(64),
        name.len() as i64,
        0o100644,
        trove_id,
    )
    .insert(conn)
    .unwrap();
}

fn seed_broken_orphan(conn: &rusqlite::Connection, name: &str) {
    let mut trove = Trove::new_with_source(
        name.to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
    );
    trove.install_reason = InstallReason::Dependency;
    trove.selection_reason = Some("Required by removed-parent".to_string());
    let trove_id = trove.insert(conn).unwrap();
    FileEntry::new(
        "../escape".to_string(),
        "0".repeat(64),
        6,
        0o100644,
        trove_id,
    )
    .insert(conn)
    .unwrap();
}

#[test]
fn autoremove_dry_run_lists_conary_owned_orphans_and_skips_adopted() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(
        &conn,
        root.path(),
        "owned-orphan",
        InstallSource::Repository,
    );
    seed_orphan(
        &conn,
        root.path(),
        "adopted-orphan",
        InstallSource::AdoptedTrack,
    );
    drop(conn);

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "autoremove",
        "--dry-run",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("owned-orphan 1.0.0"), "{stdout}");
    assert!(
        stdout.contains("Skipping adopted orphaned package(s)"),
        "{stdout}"
    );
    assert!(stdout.contains("adopted-orphan"), "{stdout}");
}

#[test]
fn autoremove_apply_removes_owned_orphan_without_deleting_adopted_orphan() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(
        &conn,
        root.path(),
        "owned-orphan",
        InstallSource::Repository,
    );
    seed_orphan(
        &conn,
        root.path(),
        "adopted-orphan",
        InstallSource::AdoptedTrack,
    );
    drop(conn);

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "autoremove",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(
        !root
            .path()
            .join("usr/share/owned-orphan/payload.txt")
            .exists()
    );
    assert!(
        root.path()
            .join("usr/share/adopted-orphan/payload.txt")
            .exists()
    );

    let conn = db::open(&db_path).unwrap();
    assert!(
        Trove::find_by_name(&conn, "owned-orphan")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        Trove::find_by_name(&conn, "adopted-orphan").unwrap().len(),
        1
    );
}

#[test]
fn autoremove_reports_each_failed_orphan_once_across_replans() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_broken_orphan(&conn, "broken-orphan");
    seed_orphan(
        &conn,
        root.path(),
        "owned-orphan",
        InstallSource::Repository,
    );
    drop(conn);

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "autoremove",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        !root
            .path()
            .join("usr/share/owned-orphan/payload.txt")
            .exists()
    );

    let text = output_text(&output);
    assert!(text.contains("Failed: 1 package(s)"), "{text}");
    assert!(!text.contains("Failed: 2 package(s)"), "{text}");
}

#[test]
fn list_info_files_and_path_show_installed_package_identity() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let info = run_conary(&["list", "nginx", "--info", "--db-path", &db_path]);
    assert!(info.status.success(), "{}", output_text(&info));
    let info_stdout = String::from_utf8_lossy(&info.stdout);
    assert!(info_stdout.contains("Name        : nginx"), "{info_stdout}");
    assert!(
        info_stdout.contains("Authority   : conary-owned"),
        "{info_stdout}"
    );
    assert!(info_stdout.contains("Pinned      : no"), "{info_stdout}");

    let files = run_conary(&["list", "nginx", "--files", "--db-path", &db_path]);
    assert!(files.status.success(), "{}", output_text(&files));
    let files_stdout = String::from_utf8_lossy(&files.stdout);
    assert!(files_stdout.contains("/usr/sbin/nginx"), "{files_stdout}");
    assert!(
        files_stdout.contains("/etc/nginx/nginx.conf"),
        "{files_stdout}"
    );

    let path = run_conary(&["list", "--path", "/usr/sbin/nginx", "--db-path", &db_path]);
    assert!(path.status.success(), "{}", output_text(&path));
    let path_stdout = String::from_utf8_lossy(&path.stdout);
    assert!(
        path_stdout.contains("nginx 1.24.0 provides"),
        "{path_stdout}"
    );
}

#[test]
fn pin_blocks_remove_and_unpin_allows_remove() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(
        &conn,
        root.path(),
        "pin-remove-demo",
        InstallSource::Repository,
    );
    conn.execute(
        "UPDATE troves SET install_reason = 'explicit', selection_reason = 'Explicitly installed' WHERE name = 'pin-remove-demo'",
        [],
    )
    .unwrap();
    drop(conn);

    let pin = run_conary(&[
        "pin",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
    ]);
    assert!(pin.status.success(), "{}", output_text(&pin));

    let blocked = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(!blocked.status.success(), "{}", output_text(&blocked));
    assert!(output_text(&blocked).contains("is pinned"));

    let unpin = run_conary(&[
        "unpin",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
    ]);
    assert!(unpin.status.success(), "{}", output_text(&unpin));

    let removed = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(removed.status.success(), "{}", output_text(&removed));
}

// Provider and breakage parity are covered by apps/conary/tests/query.rs:
// - whatprovides_reports_installed_and_repository_providers
// - whatbreaks_reports_same_dependency_blocker_as_remove
