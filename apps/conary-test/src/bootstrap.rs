// conary-test/src/bootstrap.rs
//! Local developer bootstrap inspection for conary-test.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use conary_agent_contract::{
    EvidenceItem, EvidenceKind, InspectResult, OperationEnvelope, OperationStatus, RiskLevel,
    local_bootstrap_status,
};

pub fn inspect_default() -> InspectResult {
    let root = crate::paths::project_dir()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let (manifests, manifest_source) = env_path_or_default(
        "CONARY_TEST_MANIFESTS",
        root.join("apps/conary/tests/integration/remi/manifests"),
    );
    let (config, config_source) = env_path_or_default(
        "CONARY_TEST_CONFIG",
        root.join("apps/conary/tests/integration/remi/config.toml"),
    );

    inspect_with_resolved_paths(
        BootstrapPaths {
            root,
            manifest_dir: manifests,
            manifest_source,
            config_path: config,
            config_source,
        },
        &BootstrapProbe::detect(),
    )
}

pub fn inspect_with_paths(root: &Path, manifest_dir: &Path) -> InspectResult {
    inspect_with_paths_and_probe(
        root,
        manifest_dir,
        &root.join("apps/conary/tests/integration/remi/config.toml"),
        BootstrapProbe::detect(),
    )
}

pub fn inspect_with_paths_and_probe(
    root: &Path,
    manifest_dir: &Path,
    config_path: &Path,
    probe: BootstrapProbe,
) -> InspectResult {
    inspect_with_resolved_paths(
        BootstrapPaths {
            root: root.to_path_buf(),
            manifest_dir: manifest_dir.to_path_buf(),
            manifest_source: "argument".to_string(),
            config_path: config_path.to_path_buf(),
            config_source: "argument".to_string(),
        },
        &probe,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapSmokeOptions {
    pub suite: String,
    pub distro: String,
    pub phase: u32,
    pub dry_run: bool,
    pub force: bool,
}

impl Default for BootstrapSmokeOptions {
    fn default() -> Self {
        Self {
            suite: "phase1-core".to_string(),
            distro: "fedora44".to_string(),
            phase: 1,
            dry_run: false,
            force: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapSmokeCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

pub fn build_smoke_command(exe: &Path, options: &BootstrapSmokeOptions) -> BootstrapSmokeCommand {
    BootstrapSmokeCommand {
        program: exe.to_path_buf(),
        args: vec![
            "run".to_string(),
            "--suite".to_string(),
            options.suite.clone(),
            "--distro".to_string(),
            options.distro.clone(),
            "--phase".to_string(),
            options.phase.to_string(),
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeCommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn smoke_with_runner(
    inspect: &InspectResult,
    options: &BootstrapSmokeOptions,
    mut runner: impl FnMut(&BootstrapSmokeCommand) -> SmokeCommandOutput,
) -> conary_agent_contract::VerifyResult {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("conary-test"));
    let command = build_smoke_command(&exe, options);
    let mut envelope = OperationEnvelope::new(
        "conary-test.bootstrap.smoke",
        OperationStatus::Planned,
        RiskLevel::Medium,
        "Local Conary developer bootstrap smoke proof loop",
    );
    envelope.subject = Some(local_bootstrap_status());

    let command_json = serde_json::json!({
        "program": command.program.display().to_string(),
        "args": command.args.clone(),
    });

    let ready = inspect
        .data
        .pointer("/default_smoke_candidate/ready")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    if options.dry_run {
        return conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
            "dry_run": true,
            "executed": false,
            "command": command_json,
        }));
    }

    if !ready && !options.force {
        envelope.status = OperationStatus::Unavailable;
        envelope
            .warnings
            .push("bootstrap check is not ready; rerun bootstrap check or use --force".to_string());
        return conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
            "dry_run": false,
            "executed": false,
            "command": command_json,
        }));
    }

    let output = runner(&command);
    envelope.status = if output.exit_code == 0 {
        OperationStatus::Ok
    } else {
        OperationStatus::Failed
    };
    envelope.evidence.push(EvidenceItem {
        kind: EvidenceKind::Command,
        summary: format!("bootstrap smoke exited {}", output.exit_code),
        uri: None,
        path: None,
        id: Some("bootstrap-smoke".to_string()),
        command: Some(
            std::iter::once(command.program.display().to_string())
                .chain(command.args.iter().cloned())
                .collect(),
        ),
        exit_code: Some(output.exit_code),
        metadata: Default::default(),
    });

    conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
        "dry_run": false,
        "executed": true,
        "command": command_json,
        "exit_code": output.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapProbe {
    pub cargo_available: bool,
    pub podman_command_available: bool,
    pub podman_api_accessible: bool,
    pub docker_command_available: bool,
    pub docker_api_accessible: bool,
    pub qemu_system_x86_64_available: bool,
    pub dev_kvm_available: bool,
    pub sqlite_available: bool,
}

impl BootstrapProbe {
    fn detect() -> Self {
        let podman_command_available = command_success("podman", ["--version"]);
        let docker_command_available = command_success("docker", ["--version"]);

        Self {
            cargo_available: command_success("cargo", ["--version"]),
            podman_command_available,
            podman_api_accessible: podman_command_available
                && command_success("podman", ["info", "--format", "json"]),
            docker_command_available,
            docker_api_accessible: docker_command_available
                && command_success("docker", ["info", "--format", "{{json .}}"]),
            qemu_system_x86_64_available: command_success("qemu-system-x86_64", ["--version"]),
            dev_kvm_available: Path::new("/dev/kvm").exists(),
            sqlite_available: sqlite_available(),
        }
    }
}

struct BootstrapPaths {
    root: PathBuf,
    manifest_dir: PathBuf,
    manifest_source: String,
    config_path: PathBuf,
    config_source: String,
}

#[derive(Debug, Default)]
struct ManifestInventory {
    dir_exists: bool,
    toml_files: usize,
    parsed: usize,
    failed: usize,
    suites: Vec<serde_json::Value>,
    errors: Vec<String>,
}

fn inspect_with_resolved_paths(paths: BootstrapPaths, probe: &BootstrapProbe) -> InspectResult {
    let mut envelope = OperationEnvelope::new(
        "conary-test.bootstrap.inspect",
        OperationStatus::Ok,
        RiskLevel::ReadOnly,
        "Local Conary developer bootstrap prerequisites inspected",
    );
    envelope.subject = Some(local_bootstrap_status());

    let manifest_inventory = inspect_manifest_dir(&paths.manifest_dir);
    let config_exists = paths.config_path.is_file();
    let config_parse_ok =
        config_exists && crate::config::load_global_config(&paths.config_path).is_ok();
    let container_runtime_command_ok =
        probe.podman_command_available || probe.docker_command_available;
    let container_runtime_api_ok = probe.podman_api_accessible || probe.docker_api_accessible;
    let smoke_manifest_available = manifest_inventory
        .suites
        .iter()
        .any(|suite| suite.get("id").and_then(serde_json::Value::as_str) == Some("phase1-core"));
    let ready_for_container_smoke = probe.cargo_available
        && container_runtime_api_ok
        && config_parse_ok
        && probe.sqlite_available
        && smoke_manifest_available;

    push_check(
        &mut envelope,
        "cargo",
        format!("cargo available: {}", probe.cargo_available),
        ["cargo", "--version"],
    );
    push_check(
        &mut envelope,
        "sqlite",
        format!(
            "rusqlite in-memory open available: {}",
            probe.sqlite_available
        ),
        ["conary-test", "bootstrap", "check"],
    );

    if !probe.cargo_available {
        envelope.status = OperationStatus::Unavailable;
        envelope
            .warnings
            .push("cargo is required for local Conary development".to_string());
    }

    if !manifest_inventory.dir_exists {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "manifest directory is missing: {}",
            paths.manifest_dir.display()
        ));
    } else if manifest_inventory.parsed == 0 {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "no parseable test manifests found in {}",
            paths.manifest_dir.display()
        ));
    } else if manifest_inventory.failed > 0 {
        envelope.status = OperationStatus::Partial;
        envelope.warnings.push(format!(
            "{} test manifest(s) failed to parse in {}",
            manifest_inventory.failed,
            paths.manifest_dir.display()
        ));
    }

    if !config_exists {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "conary-test config is missing: {}",
            paths.config_path.display()
        ));
    } else if !config_parse_ok {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "conary-test config failed to parse: {}",
            paths.config_path.display()
        ));
    }

    if !probe.sqlite_available {
        envelope.status = OperationStatus::Unavailable;
        envelope
            .warnings
            .push("SQLite is required for conary-test WAL and local state checks".to_string());
    }

    if !container_runtime_command_ok {
        if envelope.status == OperationStatus::Ok {
            envelope.status = OperationStatus::Partial;
        }
        envelope.warnings.push(
            "Podman or Docker is required before container smoke validation can run".to_string(),
        );
    } else if !container_runtime_api_ok {
        if envelope.status == OperationStatus::Ok {
            envelope.status = OperationStatus::Partial;
        }
        envelope.warnings.push(
            "Podman or Docker is installed, but API access failed; container smoke validation is not ready"
                .to_string(),
        );
    }

    if !probe.qemu_system_x86_64_available || !probe.dev_kvm_available {
        envelope
            .warnings
            .push("QEMU/KVM is unavailable; non-QEMU bootstrap checks can still run".to_string());
    }

    let data = serde_json::json!({
        "project_root": paths.root.display().to_string(),
        "config": {
            "path": paths.config_path.display().to_string(),
            "source": paths.config_source,
            "exists": config_exists,
            "parse_ok": config_parse_ok,
        },
        "manifests": {
            "dir": paths.manifest_dir.display().to_string(),
            "source": paths.manifest_source,
            "dir_exists": manifest_inventory.dir_exists,
            "toml_files": manifest_inventory.toml_files,
            "parsed": manifest_inventory.parsed,
            "failed": manifest_inventory.failed,
            "suites": manifest_inventory.suites,
            "errors": manifest_inventory.errors,
        },
        "required": {
            "cargo": probe.cargo_available,
            "container_runtime_api": container_runtime_api_ok,
            "config": config_parse_ok,
            "manifest_dir": manifest_inventory.dir_exists,
            "manifest_parse": manifest_inventory.parsed > 0,
            "sqlite": probe.sqlite_available,
        },
        "container_runtime": {
            "command_available": container_runtime_command_ok,
            "api_accessible": container_runtime_api_ok,
            "podman": {
                "command_available": probe.podman_command_available,
                "api_accessible": probe.podman_api_accessible,
            },
            "docker": {
                "command_available": probe.docker_command_available,
                "api_accessible": probe.docker_api_accessible,
            },
        },
        "optional_toolchain": {
            "qemu_system_x86_64": probe.qemu_system_x86_64_available,
            "dev_kvm": probe.dev_kvm_available,
        },
        "default_smoke_candidate": {
            "suite": "phase1-core",
            "distro": "fedora44",
            "requires_container_runtime": true,
            "requires_qemu": false,
            "manifest_available": smoke_manifest_available,
            "ready": ready_for_container_smoke,
        },
    });

    InspectResult::new(envelope).with_data(data)
}

fn env_path_or_default(var: &str, default: PathBuf) -> (PathBuf, String) {
    match std::env::var_os(var) {
        Some(value) => (PathBuf::from(value), var.to_string()),
        None => (default, "default".to_string()),
    }
}

fn inspect_manifest_dir(manifest_dir: &Path) -> ManifestInventory {
    let mut inventory = ManifestInventory {
        dir_exists: manifest_dir.is_dir(),
        ..Default::default()
    };

    let Ok(entries) = std::fs::read_dir(manifest_dir) else {
        return inventory;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }

        inventory.toml_files += 1;
        let id = path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_string();

        match crate::config::load_manifest(&path) {
            Ok(manifest) => {
                inventory.parsed += 1;
                inventory.suites.push(serde_json::json!({
                    "id": id,
                    "name": manifest.suite.name,
                    "phase": manifest.suite.phase,
                    "test_count": manifest.test.len(),
                }));
            }
            Err(error) => {
                inventory.failed += 1;
                let file = path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("<unknown>");
                inventory.errors.push(format!("{file}: {error}"));
            }
        }
    }

    inventory
}

fn push_check(
    envelope: &mut OperationEnvelope,
    id: &str,
    summary: String,
    command: impl IntoIterator<Item = &'static str>,
) {
    envelope.evidence.push(EvidenceItem {
        kind: EvidenceKind::Check,
        summary,
        uri: None,
        path: None,
        id: Some(id.to_string()),
        command: Some(command.into_iter().map(ToString::to_string).collect()),
        exit_code: None,
        metadata: Default::default(),
    });
}

fn command_success(command: &str, args: impl IntoIterator<Item = &'static str>) -> bool {
    Command::new(command)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn sqlite_available() -> bool {
    rusqlite::Connection::open_in_memory()
        .and_then(|connection| connection.execute_batch("SELECT 1"))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ready_probe() -> BootstrapProbe {
        BootstrapProbe {
            cargo_available: true,
            podman_command_available: true,
            podman_api_accessible: true,
            docker_command_available: false,
            docker_api_accessible: false,
            qemu_system_x86_64_available: false,
            dev_kvm_available: false,
            sqlite_available: true,
        }
    }

    fn write_valid_config(path: &Path) {
        std::fs::write(
            path,
            r#"
[remi]
endpoint = "https://remi.conary.io"

[paths]
db = "/tmp/conary.db"
conary_bin = "/usr/bin/conary"
results_dir = "/tmp/results"

[distros.fedora44]
remi_distro = "fedora"
repo_name = "fedora-remi"
"#,
        )
        .unwrap();
    }

    fn write_valid_manifest(path: &Path) {
        std::fs::write(
            path,
            r#"
[suite]
name = "Phase 1 Core"
phase = 1

[[test]]
id = "T01"
name = "health_check"
description = "Verify local smoke plumbing"
timeout = 10

[[test.step]]
run = "true"

[test.step.assert]
exit_code = 0
"#,
        )
        .unwrap();
    }

    fn ready_bootstrap_report() -> InspectResult {
        let root = tempdir().unwrap();
        let manifests = root.path().join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();
        write_valid_manifest(&manifests.join("phase1-core.toml"));
        let config = root.path().join("config.toml");
        write_valid_config(&config);
        inspect_with_paths_and_probe(root.path(), &manifests, &config, ready_probe())
    }

    #[test]
    fn inspect_reports_missing_manifest_dir_without_success() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing-manifests");
        let config = root.path().join("config.toml");
        write_valid_config(&config);
        let report = inspect_with_paths_and_probe(root.path(), &missing, &config, ready_probe());

        assert_ne!(
            report.envelope.status,
            conary_agent_contract::OperationStatus::Ok
        );
        assert!(
            report
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("manifest directory"))
        );
    }

    #[test]
    fn inspect_uses_local_bootstrap_subject_uri() {
        let root = tempdir().unwrap();
        let manifests = root.path().join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();
        write_valid_manifest(&manifests.join("phase1-core.toml"));
        let config = root.path().join("config.toml");
        write_valid_config(&config);

        let report = inspect_with_paths_and_probe(root.path(), &manifests, &config, ready_probe());
        assert_eq!(
            report.envelope.subject.unwrap().uri,
            "conary-local://bootstrap/status"
        );
    }

    #[test]
    fn inspect_distinguishes_runtime_command_from_api_access() {
        let root = tempdir().unwrap();
        let manifests = root.path().join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();
        write_valid_manifest(&manifests.join("phase1-core.toml"));
        let config = root.path().join("config.toml");
        write_valid_config(&config);
        let mut probe = ready_probe();
        probe.podman_api_accessible = false;

        let report = inspect_with_paths_and_probe(root.path(), &manifests, &config, probe);
        let data = &report.data;
        assert_eq!(report.envelope.status, OperationStatus::Partial);
        assert_eq!(data["container_runtime"]["command_available"], true);
        assert_eq!(data["required"]["container_runtime_api"], false);
        assert_eq!(data["default_smoke_candidate"]["ready"], false);
        assert!(
            report
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("API access failed"))
        );
    }

    #[test]
    fn inspect_reports_manifest_parse_inventory() {
        let root = tempdir().unwrap();
        let manifests = root.path().join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();
        std::fs::write(manifests.join("broken.toml"), "not = [valid").unwrap();
        let config = root.path().join("config.toml");
        write_valid_config(&config);

        let report = inspect_with_paths_and_probe(root.path(), &manifests, &config, ready_probe());
        let data = &report.data;
        assert_eq!(report.envelope.status, OperationStatus::Unavailable);
        assert_eq!(data["manifests"]["toml_files"], 1);
        assert_eq!(data["manifests"]["parsed"], 0);
        assert_eq!(data["manifests"]["failed"], 1);
        assert!(
            report
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("no parseable test manifests"))
        );
    }

    #[test]
    fn smoke_options_default_to_phase1_core_fedora44() {
        let options = BootstrapSmokeOptions::default();
        assert_eq!(options.suite, "phase1-core");
        assert_eq!(options.distro, "fedora44");
        assert_eq!(options.phase, 1);
        assert!(!options.force);
        assert!(!options.dry_run);
    }

    #[test]
    fn smoke_command_invokes_existing_run_path() {
        let exe = Path::new("/tmp/conary-test");
        let command = build_smoke_command(exe, &BootstrapSmokeOptions::default());
        assert_eq!(command.program, exe);
        assert_eq!(
            command.args,
            vec![
                "run",
                "--suite",
                "phase1-core",
                "--distro",
                "fedora44",
                "--phase",
                "1",
            ]
        );
    }

    #[test]
    fn smoke_dry_run_returns_planned_command_without_execution() {
        let mut options = BootstrapSmokeOptions::default();
        options.dry_run = true;
        let inspect = ready_bootstrap_report();
        let report = smoke_with_runner(&inspect, &options, |_command| {
            panic!("dry-run must not execute the smoke command")
        });

        assert_eq!(report.envelope.status, OperationStatus::Planned);
        assert_eq!(report.envelope.risk, RiskLevel::Medium);
        assert_eq!(report.data["dry_run"], true);
        assert_eq!(report.data["command"]["args"][0], "run");
    }

    #[test]
    fn smoke_refuses_when_bootstrap_check_is_not_ready() {
        let mut inspect = ready_bootstrap_report();
        inspect.data["default_smoke_candidate"]["ready"] = serde_json::json!(false);
        let report = smoke_with_runner(&inspect, &BootstrapSmokeOptions::default(), |_command| {
            panic!("not-ready smoke must not execute")
        });

        assert_eq!(report.envelope.status, OperationStatus::Unavailable);
        assert_eq!(report.data["executed"], false);
        assert!(
            report
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("bootstrap check is not ready"))
        );
    }
}
