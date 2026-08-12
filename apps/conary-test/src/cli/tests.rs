// conary-test/src/cli/tests.rs

use super::*;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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
fn images_base_ref_accepts_a_derivative_lane() {
    let cli = Cli::try_parse_from([
        "conary-test",
        "images",
        "base-ref",
        "--distro",
        "pop-os-24.04",
    ])
    .unwrap();

    match cli.command {
        Commands::Images {
            command: ImageCommands::BaseRef { distro },
        } => assert_eq!(distro, "pop-os-24.04"),
        _ => panic!("unexpected command"),
    }
}

#[test]
fn deploy_status_and_health_have_no_server_port() {
    let cli = Cli::try_parse_from(["conary-test", "deploy", "status"]).unwrap();
    match cli.command {
        Commands::Deploy {
            command: DeployCommands::Status,
        } => {}
        _ => panic!("unexpected command"),
    }

    let cli = Cli::try_parse_from(["conary-test", "health"]).unwrap();
    match cli.command {
        Commands::Health => {}
        _ => panic!("unexpected command"),
    }

    assert!(Cli::try_parse_from(["conary-test", "health", "--port", "8181"]).is_err());
    assert!(Cli::try_parse_from(["conary-test", "deploy", "status", "--port", "8181"]).is_err());
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
