// apps/conary/src/dispatch/root/tests.rs

use super::run_try_session_preflight_for_test;
use crate::cli::Cli;
use clap::Parser;
use conary_core::db::models::{CreateTrySession, TrySession, TrySessionMode, TrySessionStatus};
use std::ffi::OsString;

fn parse_cli<I, S>(args: I) -> Cli
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || Cli::try_parse_from(args))
        .expect("parser thread should spawn")
        .join()
        .expect("parser thread should not panic")
        .unwrap()
}

struct TryPreflightFixture {
    _temp: tempfile::TempDir,
    db_path: std::path::PathBuf,
    db_path_string: String,
}

impl TryPreflightFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let db_path_string = db_path.to_string_lossy().into_owned();
        Self {
            _temp: temp,
            db_path,
            db_path_string,
        }
    }

    fn open(&self) -> rusqlite::Connection {
        conary_core::db::open(&self.db_path).unwrap()
    }

    fn parse_with_db(&self, args: &[&str]) -> Cli {
        let mut full = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        full.push("--db-path".to_string());
        full.push(self.db_path_string.clone());
        parse_cli(full)
    }

    fn create_session(&self, id: &str, mode: TrySessionMode) -> TrySession {
        TrySession::create_active(
            &self.open(),
            CreateTrySession {
                id,
                package_path: &self
                    ._temp
                    .path()
                    .join(format!("{id}.ccs"))
                    .to_string_lossy(),
                package_name: Some("demo"),
                package_version: Some("1.0.0"),
                previous_generation_id: Some(1),
                mode,
                work_dir: &self._temp.path().join("try").join(id).to_string_lossy(),
            },
        )
        .unwrap()
    }

    fn stored_session(&self, id: &str) -> TrySession {
        TrySession::find_by_id(&self.open(), id)
            .unwrap()
            .expect("stored try session")
    }

    fn set_current_generation(&self, generation: i64) {
        std::fs::create_dir_all(self._temp.path().join(format!("generations/{generation}")))
            .unwrap();
        conary_core::generation::mount::update_current_symlink(self._temp.path(), generation)
            .unwrap();
    }
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read_only_cli(fixture: &TryPreflightFixture) -> Cli {
    fixture.parse_with_db(&["conary", "list"])
}

fn mutating_cli(fixture: &TryPreflightFixture) -> Cli {
    fixture.parse_with_db(&["conary", "pin", "demo"])
}

fn dry_run_cli(fixture: &TryPreflightFixture) -> Cli {
    fixture.parse_with_db(&["conary", "install", "demo", "--dry-run"])
}

fn set_launcher(session: &TrySession, fixture: &TryPreflightFixture, pid: i64, boot_id: &str) {
    session.set_launcher(&fixture.open(), pid, boot_id).unwrap();
}

fn set_try_generation(session: &TrySession, fixture: &TryPreflightFixture, generation: i64) {
    session
        .set_try_generation(&fixture.open(), generation)
        .unwrap();
}

fn assert_message_mentions_try_actions(message: &str) {
    assert!(message.contains("try status"), "{message}");
    assert!(message.contains("try rollback"), "{message}");
    assert!(message.contains("try keep"), "{message}");
}

#[test]
fn try_dispatch_watch_defaults_to_current_dir() {
    match super::try_dispatch_action(super::TryDispatchInput {
        target: None,
        activate: false,
        allow_irreversible: false,
        isolated: false,
        run: &[],
        watch: true,
        recipe: None,
        json: false,
    })
    .unwrap()
    {
        super::TryDispatchAction::Watch(watch) => {
            assert_eq!(watch.target, ".");
            assert_eq!(watch.recipe, None);
            assert!(!watch.json);
            assert!(!watch.isolated);
        }
        other => panic!("unexpected try dispatch action: {other:?}"),
    }
}

#[test]
fn try_dispatch_watch_accepts_isolated() {
    match super::try_dispatch_action(super::TryDispatchInput {
        target: Some(".".to_string()),
        activate: false,
        allow_irreversible: false,
        isolated: true,
        run: &[],
        watch: true,
        recipe: None,
        json: true,
    })
    .unwrap()
    {
        super::TryDispatchAction::Watch(watch) => {
            assert_eq!(watch.target, ".");
            assert!(watch.isolated);
            assert!(watch.json);
        }
        other => panic!("unexpected try dispatch action: {other:?}"),
    }
}

#[test]
fn try_dispatch_rejects_isolated_without_watch() {
    let err = super::try_dispatch_action(super::TryDispatchInput {
        target: Some("pkg.ccs".to_string()),
        activate: false,
        allow_irreversible: false,
        isolated: true,
        run: &[],
        watch: false,
        recipe: None,
        json: false,
    })
    .expect_err("isolated without watch should fail");
    assert!(
        err.to_string().contains("--isolated requires --watch"),
        "{err:#}"
    );
}

#[test]
fn try_dispatch_watch_rejects_artifacts_actions_activation_and_run_commands() {
    for (target, activate, allow_irreversible, run, message) in [
        (
            Some("pkg.ccs".to_string()),
            false,
            false,
            vec![],
            "does not accept prebuilt .ccs artifacts",
        ),
        (
            Some("status".to_string()),
            false,
            false,
            vec![],
            "cannot be combined with try action",
        ),
        (
            Some("rollback".to_string()),
            false,
            false,
            vec![],
            "cannot be combined with try action",
        ),
        (
            Some("keep".to_string()),
            false,
            false,
            vec![],
            "cannot be combined with try action",
        ),
        (
            None,
            true,
            false,
            vec![],
            "cannot be combined with --activate",
        ),
        (
            None,
            false,
            true,
            vec![],
            "cannot be combined with --allow-irreversible",
        ),
        (
            None,
            false,
            false,
            vec!["/bin/true".to_string()],
            "cannot run a command",
        ),
    ] {
        let err = super::try_dispatch_action(super::TryDispatchInput {
            target,
            activate,
            allow_irreversible,
            isolated: false,
            run: &run,
            watch: true,
            recipe: None,
            json: false,
        })
        .expect_err("watch conflict should fail");
        assert!(err.to_string().contains(message), "{err:#}");
    }
}

#[test]
fn live_namespace_read_only_preflight_allows_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    let session = fixture.create_session("try-live-ns", TrySessionMode::Namespace);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");

    run_try_session_preflight_for_test(&read_only_cli(&fixture), true).unwrap();

    assert_eq!(
        fixture.stored_session("try-live-ns").status,
        TrySessionStatus::Active
    );
}

#[test]
fn live_namespace_mutating_preflight_blocks_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    let session = fixture.create_session("try-live-ns", TrySessionMode::Namespace);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");

    let err = run_try_session_preflight_for_test(&mutating_cli(&fixture), true)
        .expect_err("live namespace try session should block mutating commands");

    let message = err.to_string();
    assert!(
        message.contains("another try session is active"),
        "{message}"
    );
    assert_message_mentions_try_actions(&message);
    assert_eq!(
        fixture.stored_session("try-live-ns").status,
        TrySessionStatus::Active
    );
}

#[test]
fn live_namespace_dry_run_preflight_allows_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    let session = fixture.create_session("try-live-ns", TrySessionMode::Namespace);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");

    run_try_session_preflight_for_test(&dry_run_cli(&fixture), true).unwrap();

    assert_eq!(
        fixture.stored_session("try-live-ns").status,
        TrySessionStatus::Active
    );
}

#[test]
fn completed_namespace_preflight_stays_active_and_blocks_mutation() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    fixture.create_session("try-complete-ns", TrySessionMode::Namespace);

    let err = run_try_session_preflight_for_test(&mutating_cli(&fixture), true)
        .expect_err("decision-pending namespace try session should block mutating commands");

    let message = err.to_string();
    assert!(
        message.contains("another try session is active"),
        "{message}"
    );
    assert_message_mentions_try_actions(&message);
    assert_eq!(
        fixture.stored_session("try-complete-ns").status,
        TrySessionStatus::Active
    );
}

#[test]
fn orphaned_namespace_preflight_marks_orphaned_and_blocks_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    let session = fixture.create_session("try-orphan-ns", TrySessionMode::Namespace);
    set_launcher(&session, &fixture, 9_999_999, "boot-a");

    let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), false)
        .expect_err("orphaned namespace try session should block ordinary commands");

    let message = err.to_string();
    assert!(message.contains("orphaned try session"), "{message}");
    assert_message_mentions_try_actions(&message);
    assert_eq!(
        fixture.stored_session("try-orphan-ns").status,
        TrySessionStatus::Orphaned
    );
}

#[test]
fn live_activated_read_only_preflight_allows_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    fixture.set_current_generation(7);
    let session = fixture.create_session("try-live-activated", TrySessionMode::Activated);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
    set_try_generation(&session, &fixture, 7);

    run_try_session_preflight_for_test(&read_only_cli(&fixture), true).unwrap();

    assert_eq!(
        fixture.stored_session("try-live-activated").status,
        TrySessionStatus::Active
    );
}

#[test]
fn live_activated_dry_run_preflight_allows_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    fixture.set_current_generation(7);
    let session = fixture.create_session("try-live-activated", TrySessionMode::Activated);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
    set_try_generation(&session, &fixture, 7);

    run_try_session_preflight_for_test(&dry_run_cli(&fixture), true).unwrap();

    assert_eq!(
        fixture.stored_session("try-live-activated").status,
        TrySessionStatus::Active
    );
}

#[test]
fn live_activated_mutating_preflight_blocks_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    fixture.set_current_generation(7);
    let session = fixture.create_session("try-live-activated", TrySessionMode::Activated);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
    set_try_generation(&session, &fixture, 7);

    let err = run_try_session_preflight_for_test(&mutating_cli(&fixture), true)
        .expect_err("live activated try session should block mutating commands");

    let message = err.to_string();
    assert!(message.contains("activated try session"), "{message}");
    assert!(message.contains("is active"), "{message}");
    assert!(message.contains("try rollback"), "{message}");
    assert!(message.contains("try keep"), "{message}");
    assert_eq!(
        fixture.stored_session("try-live-activated").status,
        TrySessionStatus::Active
    );
}

#[test]
fn orphaned_activated_interactive_preflight_marks_orphaned_and_blocks_command() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    fixture.set_current_generation(8);
    let session = fixture.create_session("try-orphan-activated", TrySessionMode::Activated);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
    set_try_generation(&session, &fixture, 7);

    let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), true)
        .expect_err("orphaned activated interactive preflight should block command");

    let message = err.to_string();
    assert!(
        message.contains("orphaned activated try session"),
        "{message}"
    );
    assert!(message.contains("try rollback"), "{message}");
    assert!(message.contains("try keep"), "{message}");
    assert_eq!(
        fixture.stored_session("try-orphan-activated").status,
        TrySessionStatus::Orphaned
    );
}

#[test]
fn orphaned_activated_env_forced_non_interactive_attempts_rollback() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let _non_interactive_guard = EnvVarGuard::set("CONARY_NON_INTERACTIVE", "1");
    let fixture = TryPreflightFixture::new();
    fixture.set_current_generation(8);
    let session = fixture.create_session("try-orphan-activated", TrySessionMode::Activated);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
    set_try_generation(&session, &fixture, 7);

    let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), true)
        .expect_err("CONARY_NON_INTERACTIVE=1 should force automatic rollback");

    let message = err.to_string();
    assert!(message.contains("automatic rollback"), "{message}");
    assert_eq!(
        fixture.stored_session("try-orphan-activated").status,
        TrySessionStatus::Orphaned
    );
}

#[test]
fn orphaned_activated_non_interactive_preflight_attempts_rollback() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    fixture.set_current_generation(8);
    let session = fixture.create_session("try-orphan-activated", TrySessionMode::Activated);
    set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
    set_try_generation(&session, &fixture, 7);

    let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), false)
        .expect_err("rollback attempt should surface rollback error for invalid test package");

    let message = err.to_string();
    assert!(message.contains("automatic rollback"), "{message}");
    assert_eq!(
        fixture.stored_session("try-orphan-activated").status,
        TrySessionStatus::Orphaned
    );
}

#[test]
fn preflight_uses_default_db_path_for_commands_without_db_args() {
    let cli = parse_cli(["conary", "cook", "."]);

    let command = cli.command.as_ref().expect("parsed command");
    assert_eq!(super::selected_db_path(command), super::DEFAULT_DB_PATH);
}

#[test]
fn commands_without_db_args_do_not_use_try_session_preflight_scope() {
    for args in [
        ["conary", "cook", "."].as_slice(),
        ["conary", "new", "hello-m1b"].as_slice(),
        ["conary", "publish", "./repo", "--recipe", "recipe.toml"].as_slice(),
        ["conary", "publish", "dist/pkg.ccs", "./repo"].as_slice(),
        [
            "conary",
            "bootstrap",
            "verify-convergence",
            "--run-a",
            "/tmp/run-a",
            "--run-b",
            "/tmp/run-b",
        ]
        .as_slice(),
        [
            "conary",
            "bootstrap",
            "diff-seeds",
            "/tmp/seed-a",
            "/tmp/seed-b",
        ]
        .as_slice(),
        ["conary", "system", "completions", "bash"].as_slice(),
        [
            "conary",
            "ccs",
            "init",
            "/tmp/ccs-demo",
            "--name",
            "ccs-demo",
        ]
        .as_slice(),
        ["conary", "ccs", "build", "/tmp/ccs-demo"].as_slice(),
        [
            "conary",
            "ccs",
            "build",
            "/tmp/ccs-demo",
            "--format",
            "v2",
            "--local-dev",
        ]
        .as_slice(),
        ["conary", "ccs", "lint", "/tmp/ccs-demo"].as_slice(),
        ["conary", "ccs", "inspect", "/tmp/pkg.ccs"].as_slice(),
        ["conary", "ccs", "verify", "/tmp/pkg.ccs"].as_slice(),
        ["conary", "ccs", "test", "/tmp/pkg.ccs", "--dry-run"].as_slice(),
        ["conary", "ccs", "sign", "/tmp/pkg.ccs", "--key", "/tmp/key"].as_slice(),
        ["conary", "ccs", "keygen", "--output", "/tmp/key"].as_slice(),
        ["conary", "capability", "validate", "/tmp/ccs.toml"].as_slice(),
        ["conary", "trust", "key-gen", "root", "--output", "/tmp"].as_slice(),
        ["conary", "query", "scripts", "/tmp/pkg.ccs"].as_slice(),
        ["conary", "query", "scripts", "/tmp/pkg.rpm"].as_slice(),
    ] {
        let cli = parse_cli(args);
        let command = cli.command.as_ref().expect("parsed command");
        assert!(!super::command_uses_try_session_preflight_db(command));
    }

    let cli = parse_cli(["conary", "pin", "demo"]);
    let command = cli.command.as_ref().expect("parsed command");
    assert!(super::command_uses_try_session_preflight_db(command));

    let cli = parse_cli(["conary", "query", "scripts", "bash"]);
    let command = cli.command.as_ref().expect("parsed command");
    assert!(super::command_uses_try_session_preflight_db(command));
}

#[tokio::test]
async fn artifact_form_publish_reaches_artifact_reader_without_preflight_db() {
    let cli = parse_cli(["conary", "publish", "dist/pkg.ccs", "./repo"]);

    let err = crate::dispatch::dispatch(cli)
        .await
        .expect_err("artifact-form publish should reach artifact handling");

    assert!(
        err.to_string()
            .contains("Failed to open package: dist/pkg.ccs")
    );
}

#[test]
fn nested_db_command_preflights_selected_db_path() {
    let _env_lock = lock_env();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let fixture = TryPreflightFixture::new();
    let session = fixture.create_session("try-nested-db", TrySessionMode::Namespace);
    set_launcher(&session, &fixture, 9_999_999, "boot-a");

    let cli = fixture.parse_with_db(&["conary", "repo", "list"]);
    let err = run_try_session_preflight_for_test(&cli, true)
        .expect_err("nested repo command should inspect the selected active-session DB");

    let message = err.to_string();
    assert!(message.contains("orphaned try session"), "{message}");
    assert_eq!(
        fixture.stored_session("try-nested-db").status,
        TrySessionStatus::Orphaned
    );
}

#[test]
fn try_action_commands_skip_orphan_preflight() {
    let _env_lock = lock_env();
    let fixture = TryPreflightFixture::new();
    let session = fixture.create_session("try-action", TrySessionMode::Namespace);
    set_launcher(&session, &fixture, 9_999_999, "old-boot");

    for action in ["status", "rollback", "keep"] {
        let cli = fixture.parse_with_db(&["conary", "try", action]);
        run_try_session_preflight_for_test(&cli, false).unwrap();
    }

    assert_eq!(
        fixture.stored_session("try-action").status,
        TrySessionStatus::Active
    );
}

#[test]
fn database_not_found_is_no_active_try_session() {
    let missing_db = tempfile::tempdir()
        .unwrap()
        .path()
        .join("missing")
        .join("conary.db");
    let db_path = missing_db.to_string_lossy();
    let cli = parse_cli(vec![
        "conary".to_string(),
        "list".to_string(),
        "--db-path".to_string(),
        db_path.into_owned(),
    ]);

    run_try_session_preflight_for_test(&cli, true).unwrap();
}

#[test]
fn package_named_try_action_requires_explicit_path_prefix() {
    let fixture = TryPreflightFixture::new();
    for package in ["./status", "./rollback", "./keep"] {
        let cli = fixture.parse_with_db(&["conary", "try", package]);
        run_try_session_preflight_for_test(&cli, true).unwrap();
    }
}

#[test]
fn activated_liveness_rejects_recorded_dead_pid_but_allows_absent_pid() {
    let session = TrySession {
        id: "try-liveness".to_string(),
        package_path: "/tmp/demo.ccs".to_string(),
        package_name: None,
        package_version: None,
        previous_generation_id: Some(1),
        try_generation_id: Some(7),
        launcher_pid: Some(9_999_999),
        launcher_boot_id: Some("boot-a".to_string()),
        status: TrySessionStatus::Active,
        mode: TrySessionMode::Activated,
        work_dir: "/tmp/try-liveness".to_string(),
        last_error: None,
        started_at: None,
        updated_at: None,
        completed_at: None,
    };

    assert!(!super::activated_try_session_is_live(
        &session,
        "boot-a",
        Some(7)
    ));

    let no_pid = TrySession {
        launcher_pid: None,
        ..session
    };
    assert!(super::activated_try_session_is_live(
        &no_pid,
        "boot-a",
        Some(7)
    ));
}
