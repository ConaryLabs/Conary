// tests/cli_output_snapshots.rs

mod common;

use conary_core::db::models::{Trove, TroveType};
use std::process::Command;

fn stdout_of(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .output()
        .expect("failed to run conary");

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn list_empty() {
    let (_tmp, db, _conn) = common::create_test_db();
    assert_eq!(
        stdout_of(&["list", "--db-path", &db]),
        "No packages found.\n"
    );
}

#[test]
fn search_no_match() {
    let (_tmp, db, _conn) = common::create_test_db();
    assert_eq!(
        stdout_of(&["search", "nonesuch", "--db-path", &db]),
        "No packages found matching 'nonesuch'\n"
    );
}

#[test]
fn list_one_package() {
    let (_tmp, db, conn) = common::create_test_db();
    let mut trove = Trove::new(
        "nginx".to_string(),
        "1.27.2".to_string(),
        TroveType::Package,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.insert(&conn).unwrap();

    assert_eq!(
        stdout_of(&["list", "--db-path", &db]),
        "Installed packages:\n  nginx 1.27.2 (Package) [x86_64]\n\nTotal: 1 package(s)\n"
    );
}
