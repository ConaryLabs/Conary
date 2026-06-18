// apps/conary/tests/packaging_m4d.rs

mod common;

use std::process::Command;

#[test]
fn packaging_m4d_distro_list_exposes_only_supported_profiles() {
    let (_db_temp, db_path) = common::setup_command_test_db();
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["distro", "list", "--db-path", &db_path])
        .output()
        .expect("run conary distro list");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("fedora-44"));
    assert!(stdout.contains("ubuntu-26.04"));
    assert!(stdout.contains("arch"));
    assert!(!stdout.contains("debian"));
    assert!(!stdout.contains("linux-mint"));
}

#[test]
fn packaging_m4d_distro_set_rejects_unsupported_target() {
    let (_db_temp, db_path) = common::setup_command_test_db();
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["distro", "set", "debian-13", "--db-path", &db_path])
        .output()
        .expect("run conary distro set");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unsupported distro"));
}

#[test]
fn packaging_m4d_supported_profiles_stay_narrow() {
    let (_db_temp, db_path) = common::setup_command_test_db();
    let list = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["distro", "list", "--db-path", &db_path])
        .output()
        .expect("run conary distro list");
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    for supported in ["fedora-44", "ubuntu-26.04", "arch"] {
        assert!(stdout.contains(supported), "{supported}");
    }
    for unsupported in ["debian", "linux-mint", "ubuntu-noble", "fedora-45"] {
        assert!(!stdout.contains(unsupported), "{unsupported}");
    }
}
