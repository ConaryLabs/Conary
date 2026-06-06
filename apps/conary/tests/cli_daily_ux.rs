// apps/conary/tests/cli_daily_ux.rs

mod common;

use conary_core::db::models::{InstallSource, Repository, RepositoryPackage, Trove, TroveType};
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

fn seed_adopted_package(
    conn: &rusqlite::Connection,
    name: &str,
    version: &str,
    source: InstallSource,
) -> i64 {
    let mut trove = Trove::new_with_source(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        source,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.version_scheme = Some("rpm".to_string());
    trove.insert(conn).unwrap()
}

fn seed_update_candidate(conn: &rusqlite::Connection, name: &str, version: &str) -> i64 {
    let mut repo = Repository::new(
        "daily-ux-repo".to_string(),
        "https://example.test/daily-ux".to_string(),
    );
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(conn).unwrap();

    let mut candidate = RepositoryPackage::new(
        repo_id,
        name.to_string(),
        version.to_string(),
        format!("sha256:{name}-{version}"),
        123,
        format!("https://example.test/daily-ux/{name}-{version}.ccs"),
    );
    candidate.architecture = Some("x86_64".to_string());
    candidate.distro = Some("fedora-44".to_string());
    candidate.version_scheme = Some("rpm".to_string());
    candidate.insert(conn).unwrap();
    repo_id
}

#[test]
fn root_help_includes_daily_workflow_examples() {
    let output = run_conary(&["--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Daily workflow examples"), "{stdout}");
    assert!(stdout.contains("conary install nginx --yes"), "{stdout}");
    assert!(stdout.contains("conary system adopt --refresh"), "{stdout}");
    assert!(
        stdout.contains("conary system completions bash"),
        "{stdout}"
    );
    assert!(stdout.contains("conaryd"), "{stdout}");
}

#[test]
fn phase2_pruning_repo_add_help_lists_only_supported_remi_distro_examples() {
    let output = run_conary(&["repo", "add", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fedora-44, ubuntu-26.04, arch"), "{stdout}");
    assert!(!stdout.to_lowercase().contains("debian"), "{stdout}");
}

#[test]
fn phase2_pruning_ccs_init_next_steps_use_current_build_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_conary(&[
        "ccs",
        "init",
        dir.path().to_str().unwrap(),
        "--name",
        "phase2-pruning",
        "--version",
        "1.0.0",
    ]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let retired_build_command = ["conary ccs", "build"].join("-");
    assert!(stdout.contains("conary ccs build"), "{stdout}");
    assert!(!stdout.contains(&retired_build_command), "{stdout}");
    assert!(dir.path().join("ccs.toml").exists());
}

#[test]
fn shell_completion_rendering_covers_bash_and_zsh() {
    let bash = run_conary(&["system", "completions", "bash"]);
    assert!(bash.status.success(), "{}", output_text(&bash));
    let bash_stdout = String::from_utf8_lossy(&bash.stdout);
    assert!(bash_stdout.contains("_conary"), "{bash_stdout}");
    assert!(bash_stdout.contains("system"), "{bash_stdout}");

    let zsh = run_conary(&["system", "completions", "zsh"]);
    assert!(zsh.status.success(), "{}", output_text(&zsh));
    let zsh_stdout = String::from_utf8_lossy(&zsh.stdout);
    assert!(zsh_stdout.contains("#compdef conary"), "{zsh_stdout}");
    assert!(zsh_stdout.contains("completions"), "{zsh_stdout}");
}

#[test]
fn live_mutation_refusal_routes_to_preview_ack_and_daemon_jobs() {
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

    assert!(!output.status.success(), "{}", output_text(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"), "{stderr}");
    assert!(!stderr.contains("live-host acknowledgement"), "{stderr}");
    assert!(!stderr.contains("may change packages"), "{stderr}");
}

#[test]
fn adopted_install_refusal_routes_to_refresh_and_takeover() {
    let (_tmp, db_path, conn) = common::create_test_db();
    seed_adopted_package(&conn, "curl", "8.8.0-1.fc44", InstallSource::AdoptedFull);
    drop(conn);
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "install",
        "curl",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert!(!output.status.success(), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(text.contains("conary system adopt --refresh"), "{text}");
    assert!(
        text.contains("conary install curl --dep-mode takeover"),
        "{text}"
    );
    assert!(text.contains("conary system takeover"), "{text}");
}

#[test]
fn adopted_remove_refusal_routes_to_unadopt_or_purge() {
    let (_tmp, db_path, conn) = common::create_test_db();
    seed_adopted_package(&conn, "curl", "8.8.0-1.fc44", InstallSource::AdoptedTrack);
    drop(conn);
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "remove",
        "curl",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert!(!output.status.success(), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(text.contains("native package manager authority"), "{text}");
    assert!(text.contains("conary system unadopt curl"), "{text}");
    assert!(text.contains("--purge-files"), "{text}");
}

#[test]
fn adopted_update_routes_to_native_pm_and_refresh() {
    let (_tmp, db_path, conn) = common::create_test_db();
    let repo_id = seed_update_candidate(&conn, "curl", "8.9.0-1.fc44");
    let trove_id = seed_adopted_package(&conn, "curl", "8.8.0-1.fc44", InstallSource::AdoptedFull);
    conn.execute(
        "UPDATE troves SET installed_from_repository_id = ?1, source_distro = 'fedora-44' WHERE id = ?2",
        rusqlite::params![repo_id, trove_id],
    )
    .unwrap();
    drop(conn);
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "update",
        "curl",
        "--dry-run",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
    ]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("native package-manager authority"),
        "{stdout}"
    );
    assert!(stdout.contains("dnf update curl"), "{stdout}");
    assert!(stdout.contains("conary system adopt --refresh"), "{stdout}");
}
