// conary-test/src/bootstrap.rs
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

    let (selected_smoke_candidate, readiness_warnings) = selected_smoke_candidate(inspect, options);
    let ready = selected_smoke_candidate
        .get("ready")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    if options.dry_run {
        return conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
            "dry_run": true,
            "executed": false,
            "command": command_json,
            "selected_smoke_candidate": selected_smoke_candidate,
        }));
    }

    if !ready && !options.force {
        envelope.status = OperationStatus::Unavailable;
        envelope
            .warnings
            .push("bootstrap check is not ready; rerun bootstrap check or use --force".to_string());
        envelope.warnings.extend(readiness_warnings);
        return conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
            "dry_run": false,
            "executed": false,
            "command": command_json,
            "selected_smoke_candidate": selected_smoke_candidate,
        }));
    } else if !ready {
        envelope
            .warnings
            .push("bootstrap check is not ready, but --force was set".to_string());
        envelope.warnings.extend(readiness_warnings);
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
        redactions: Vec::new(),
    });

    conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
        "dry_run": false,
        "executed": true,
        "command": command_json,
        "selected_smoke_candidate": selected_smoke_candidate,
        "exit_code": output.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

pub fn run_smoke(options: &BootstrapSmokeOptions) -> conary_agent_contract::VerifyResult {
    let inspect = inspect_default();
    smoke_with_runner(&inspect, options, |command| {
        let output = Command::new(&command.program).args(&command.args).output();
        match output {
            Ok(output) => SmokeCommandOutput {
                exit_code: output.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            },
            Err(error) => SmokeCommandOutput {
                exit_code: 127,
                stdout: String::new(),
                stderr: error.to_string(),
            },
        }
    })
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
    let parsed_config = if config_exists {
        crate::config::load_global_config(&paths.config_path).ok()
    } else {
        None
    };
    let config_parse_ok = parsed_config.is_some();
    let mut configured_distros = parsed_config
        .as_ref()
        .map(|config| config.distros.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    configured_distros.sort();
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
            "distros": configured_distros,
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
                let requires_qemu = manifest_requires_qemu(&manifest);
                let qemu_only = manifest.is_qemu_only();
                inventory.suites.push(serde_json::json!({
                    "id": id,
                    "name": manifest.suite.name,
                    "phase": manifest.suite.phase,
                    "test_count": manifest.test.len(),
                    "requires_container_runtime": !qemu_only,
                    "requires_qemu": requires_qemu,
                    "qemu_only": qemu_only,
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

fn manifest_requires_qemu(manifest: &crate::config::TestManifest) -> bool {
    manifest
        .suite
        .setup
        .iter()
        .any(|step| step.qemu_boot.is_some())
        || manifest
            .test
            .iter()
            .any(|test| test.step.iter().any(|step| step.qemu_boot.is_some()))
}

fn selected_smoke_candidate(
    inspect: &InspectResult,
    options: &BootstrapSmokeOptions,
) -> (serde_json::Value, Vec<String>) {
    let suites = inspect
        .data
        .pointer("/manifests/suites")
        .and_then(serde_json::Value::as_array);
    let suite = suites.and_then(|suites| {
        suites.iter().find(|suite| {
            suite.get("id").and_then(serde_json::Value::as_str) == Some(options.suite.as_str())
        })
    });
    let suite_phase = suite
        .and_then(|suite| suite.get("phase"))
        .and_then(serde_json::Value::as_u64)
        .map(|phase| phase as u32);
    let phase_matches = suite_phase == Some(options.phase);
    let manifest_available = suite.is_some();
    let qemu_only = suite
        .and_then(|suite| suite.get("qemu_only"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let requires_qemu = suite
        .and_then(|suite| suite.get("requires_qemu"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let requires_container_runtime = suite
        .and_then(|suite| suite.get("requires_container_runtime"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let configured_distros = inspect
        .data
        .pointer("/config/distros")
        .and_then(serde_json::Value::as_array);
    let distro_configured = configured_distros
        .map(|distros| {
            distros
                .iter()
                .any(|distro| distro.as_str() == Some(options.distro.as_str()))
        })
        .unwrap_or(false);
    let cargo_ready = data_bool(&inspect.data, "/required/cargo");
    let config_ready = data_bool(&inspect.data, "/required/config");
    let manifest_dir_ready = data_bool(&inspect.data, "/required/manifest_dir");
    let manifest_parse_ready = data_bool(&inspect.data, "/required/manifest_parse");
    let sqlite_ready = data_bool(&inspect.data, "/required/sqlite");
    let container_runtime_api_ready = data_bool(&inspect.data, "/container_runtime/api_accessible");
    let qemu_ready = data_bool(&inspect.data, "/optional_toolchain/qemu_system_x86_64")
        && data_bool(&inspect.data, "/optional_toolchain/dev_kvm");
    let mut warnings = Vec::new();
    if !cargo_ready {
        warnings.push("cargo is required for bootstrap smoke".to_string());
    }
    if !config_ready {
        warnings.push("conary-test config is required for bootstrap smoke".to_string());
    }
    if !manifest_dir_ready {
        warnings.push("test manifest directory is required for bootstrap smoke".to_string());
    }
    if !manifest_parse_ready {
        warnings.push("parseable test manifests are required for bootstrap smoke".to_string());
    }
    if !sqlite_ready {
        warnings.push("SQLite is required for bootstrap smoke".to_string());
    }
    if !manifest_available {
        warnings.push(format!(
            "selected bootstrap smoke suite is not available: {}",
            options.suite
        ));
    } else if !phase_matches {
        warnings.push(format!(
            "selected bootstrap smoke suite {} is phase {}, not requested phase {}",
            options.suite,
            suite_phase
                .map(|phase| phase.to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            options.phase
        ));
    }
    if config_ready && !distro_configured {
        warnings.push(format!(
            "selected bootstrap smoke distro is not configured: {}",
            options.distro
        ));
    }
    if requires_container_runtime && !container_runtime_api_ready {
        warnings.push("container runtime API access is required for bootstrap smoke".to_string());
    }
    if requires_qemu && !qemu_ready {
        warnings.push("QEMU/KVM is required for selected bootstrap smoke suite".to_string());
    }
    let ready = warnings.is_empty();
    (
        serde_json::json!({
            "suite": options.suite,
            "distro": options.distro,
            "phase": options.phase,
            "requires_container_runtime": requires_container_runtime,
            "requires_qemu": requires_qemu,
            "qemu_only": qemu_only,
            "manifest_available": manifest_available,
            "suite_phase": suite_phase,
            "phase_matches": phase_matches,
            "distro_configured": distro_configured,
            "ready": ready,
        }),
        warnings,
    )
}

fn data_bool(data: &serde_json::Value, pointer: &str) -> bool {
    data.pointer(pointer)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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
        redactions: Vec::new(),
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
mod tests;
