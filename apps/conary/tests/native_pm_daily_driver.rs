// tests/native_pm_daily_driver.rs

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
