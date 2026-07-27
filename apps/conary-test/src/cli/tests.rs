// conary-test/src/cli/tests.rs

use super::*;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn set_test_port_env(value: Option<&str>) {
    // SAFETY: every caller holds `env_lock`, serializing mutation of this test
    // variable for the duration of the read/restore sequence.
    match value {
        Some(value) => unsafe { std::env::set_var("CONARY_TEST_PORT", value) },
        None => unsafe { std::env::remove_var("CONARY_TEST_PORT") },
    }
}

#[test]
fn default_manifest_dir_exists_under_workspace_root() {
    let root = PathBuf::from(project_dir().expect("project dir"));
    let manifests = manifest_dir().expect("manifest dir");
    assert!(
        manifests.is_dir(),
        "expected default manifest dir to exist at {}",
        manifests.display()
    );
    assert!(
        manifests.starts_with(&root),
        "expected manifest dir {} to live under {}",
        manifests.display(),
        root.display()
    );
}

#[test]
fn load_config_succeeds_from_workspace_root() {
    let _guard = cwd_lock().lock().expect("cwd lock");
    let original = std::env::current_dir().expect("current dir");
    let root = PathBuf::from(project_dir().expect("project dir"));
    assert!(
        std::env::var_os("CONARY_TEST_CONFIG").is_none(),
        "this test expects CONARY_TEST_CONFIG to be unset"
    );
    std::env::set_current_dir(&root).expect("set workspace root");
    let result = load_config();
    std::env::set_current_dir(original).expect("restore current dir");

    assert!(
        result.is_ok(),
        "expected load_config() to work from {}, got {result:?}",
        root.display()
    );
}

#[test]
fn images_build_accepts_an_explicit_native_package() {
    let cli = Cli::try_parse_from([
        "conary-test",
        "images",
        "build",
        "--distro",
        "fedora44",
        "--native-package",
        "/tmp/conary-release.rpm",
    ])
    .unwrap();

    match cli.command {
        Commands::Images {
            command:
                ImageCommands::Build {
                    distro,
                    native_package,
                },
        } => {
            assert_eq!(distro, "fedora44");
            assert_eq!(
                native_package,
                Some(PathBuf::from("/tmp/conary-release.rpm"))
            );
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn deploy_status_port_defaults_to_9090() {
    let _guard = env_lock().lock().expect("env lock");
    set_test_port_env(None);

    let cli = Cli::try_parse_from(["conary-test", "deploy", "status"]).unwrap();
    match cli.command {
        Commands::Deploy {
            command: DeployCommands::Status { port },
        } => assert_eq!(port, 9090),
        _ => panic!("unexpected command"),
    }
}

#[test]
fn deploy_status_port_uses_env_when_flag_is_absent() {
    let _guard = env_lock().lock().expect("env lock");
    set_test_port_env(Some("9191"));

    let cli = Cli::try_parse_from(["conary-test", "deploy", "status"]).unwrap();
    set_test_port_env(None);

    match cli.command {
        Commands::Deploy {
            command: DeployCommands::Status { port },
        } => assert_eq!(port, 9191),
        _ => panic!("unexpected command"),
    }
}

#[test]
fn explicit_port_flag_overrides_env_for_health() {
    let _guard = env_lock().lock().expect("env lock");
    set_test_port_env(Some("9191"));

    let cli = Cli::try_parse_from(["conary-test", "health", "--port", "8181"]).unwrap();
    set_test_port_env(None);

    match cli.command {
        Commands::Health { port } => assert_eq!(port, 8181),
        _ => panic!("unexpected command"),
    }
}

#[test]
fn cli_accepts_bootstrap_smoke_dry_run() {
    Cli::try_parse_from(["conary-test", "--json", "bootstrap", "smoke", "--dry-run"])
        .expect("bootstrap smoke dry-run should parse");
}

#[test]
fn cli_accepts_bootstrap_smoke_overrides() {
    Cli::try_parse_from([
        "conary-test",
        "bootstrap",
        "smoke",
        "--suite",
        "phase1-core",
        "--distro",
        "fedora44",
        "--phase",
        "1",
        "--force",
    ])
    .expect("bootstrap smoke overrides should parse");
}

#[test]
fn bootstrap_smoke_exit_code_reflects_contract_status() {
    use conary_agent_contract::OperationStatus;

    assert_eq!(bootstrap_smoke_exit_code(OperationStatus::Planned), 0);
    assert_eq!(bootstrap_smoke_exit_code(OperationStatus::Ok), 0);
    assert_eq!(bootstrap_smoke_exit_code(OperationStatus::Unavailable), 1);
    assert_eq!(bootstrap_smoke_exit_code(OperationStatus::Failed), 1);
    assert_eq!(bootstrap_smoke_exit_code(OperationStatus::Partial), 1);
}

#[test]
fn deploy_rollout_parses_unit_with_ref() {
    let cli = Cli::try_parse_from([
        "conary-test",
        "deploy",
        "rollout",
        "--unit",
        "conary_test",
        "--ref",
        "main",
    ])
    .unwrap();

    match cli.command {
        Commands::Deploy {
            command:
                DeployCommands::Rollout {
                    unit,
                    group,
                    git_ref,
                    path,
                },
        } => {
            assert_eq!(unit.as_deref(), Some("conary_test"));
            assert_eq!(group, None);
            assert_eq!(git_ref.as_deref(), Some("main"));
            assert_eq!(path, None);
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn deploy_rollout_parses_group_with_path() {
    let cli = Cli::try_parse_from([
        "conary-test",
        "deploy",
        "rollout",
        "--group",
        "control_plane",
        "--path",
        "~/Conary",
    ])
    .unwrap();

    match cli.command {
        Commands::Deploy {
            command:
                DeployCommands::Rollout {
                    unit,
                    group,
                    git_ref,
                    path,
                },
        } => {
            assert_eq!(unit, None);
            assert_eq!(group.as_deref(), Some("control_plane"));
            assert_eq!(git_ref, None);
            assert_eq!(path.as_deref(), Some(std::path::Path::new("~/Conary")));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn deploy_rollout_rejects_unit_and_group_together() {
    let error = Cli::try_parse_from([
        "conary-test",
        "deploy",
        "rollout",
        "--unit",
        "conary_test",
        "--group",
        "control_plane",
        "--ref",
        "main",
    ])
    .err()
    .expect("mixed target rejected");

    let rendered = error.to_string();
    assert!(rendered.contains("--unit"));
    assert!(rendered.contains("--group"));
}

#[test]
fn deploy_rollout_rejects_ref_and_path_together() {
    let error = Cli::try_parse_from([
        "conary-test",
        "deploy",
        "rollout",
        "--unit",
        "conary_test",
        "--ref",
        "main",
        "--path",
        "~/Conary",
    ])
    .err()
    .expect("mixed source rejected");

    let rendered = error.to_string();
    assert!(rendered.contains("--ref"));
    assert!(rendered.contains("--path"));
}

#[test]
fn deploy_rollout_requires_target_and_source() {
    let target_error = Cli::try_parse_from(["conary-test", "deploy", "rollout", "--ref", "main"])
        .err()
        .expect("missing target rejected");
    assert!(
        target_error.to_string().contains("--unit") || target_error.to_string().contains("--group")
    );

    let source_error =
        Cli::try_parse_from(["conary-test", "deploy", "rollout", "--unit", "conary_test"])
            .err()
            .expect("missing source rejected");
    assert!(
        source_error.to_string().contains("--ref") || source_error.to_string().contains("--path")
    );
}
