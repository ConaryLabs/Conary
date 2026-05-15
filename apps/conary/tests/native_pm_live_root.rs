// apps/conary/tests/native_pm_live_root.rs

use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

#[test]
fn no_generation_remove_deletes_file_and_history_records_apply() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let payload = root.path().join("usr/bin/fixture");
    std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
    std::fs::write(&payload, "fixture").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new_with_source(
        "fixture".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
    );
    let trove_id = trove.insert(&conn).unwrap();
    conary_core::db::models::FileEntry::new(
        "/usr/bin/fixture".to_string(),
        "0".repeat(64),
        7,
        0o100755,
        trove_id,
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "fixture",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!payload.exists());

    let history = run_conary(&["system", "history", "--db-path", db_path.to_str().unwrap()]);
    assert!(
        history.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&history.stdout),
        String::from_utf8_lossy(&history.stderr)
    );
    let stdout = String::from_utf8_lossy(&history.stdout);
    assert!(stdout.contains("Remove fixture-1.0.0"), "{stdout}");
    assert!(stdout.contains("Applied"), "{stdout}");
}
