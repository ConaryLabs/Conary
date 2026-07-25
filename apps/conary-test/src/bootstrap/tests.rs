// conary-test/src/bootstrap/tests.rs

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
remi_distro = "fedora-44"
repo_name = "remi-fedora-44"
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

fn write_qemu_manifest(path: &Path) {
    std::fs::write(
        path,
        r#"
[suite]
name = "QEMU Smoke"
phase = 3

[[test]]
id = "TQEMU"
name = "qemu_smoke"
description = "Verify QEMU readiness gating"
timeout = 10

[[test.step]]
[test.step.qemu_boot]
image = "unused"
local_image_path = "/tmp/missing.qcow2"
commands = ["true"]
"#,
    )
    .unwrap();
}

struct ReadyBootstrapFixture {
    _root: tempfile::TempDir,
    report: InspectResult,
}

fn ready_bootstrap_report() -> InspectResult {
    ready_bootstrap_fixture().report
}

fn ready_bootstrap_fixture() -> ReadyBootstrapFixture {
    let root = tempdir().unwrap();
    let manifests = root.path().join("manifests");
    std::fs::create_dir_all(&manifests).unwrap();
    write_valid_manifest(&manifests.join("phase1-core.toml"));
    let config = root.path().join("config.toml");
    write_valid_config(&config);
    let report = inspect_with_paths_and_probe(root.path(), &manifests, &config, ready_probe());

    ReadyBootstrapFixture {
        _root: root,
        report,
    }
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
    let options = BootstrapSmokeOptions {
        dry_run: true,
        ..Default::default()
    };
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

#[test]
fn smoke_refuses_unknown_selected_suite_even_when_default_is_ready() {
    let inspect = ready_bootstrap_report();
    let options = BootstrapSmokeOptions {
        suite: "missing-suite".to_string(),
        ..Default::default()
    };
    let report = smoke_with_runner(&inspect, &options, |_command| {
        panic!("unknown selected suite must not execute")
    });

    assert_eq!(report.envelope.status, OperationStatus::Unavailable);
    assert_eq!(report.data["executed"], false);
    assert_eq!(
        report.data["selected_smoke_candidate"]["manifest_available"],
        false
    );
    assert!(
        report
            .envelope
            .warnings
            .iter()
            .any(|warning| warning.contains("selected bootstrap smoke suite is not available"))
    );
}

#[test]
fn smoke_refuses_selected_suite_phase_mismatch() {
    let inspect = ready_bootstrap_report();
    let options = BootstrapSmokeOptions {
        phase: 2,
        ..Default::default()
    };
    let report = smoke_with_runner(&inspect, &options, |_command| {
        panic!("phase-mismatched selected suite must not execute")
    });

    assert_eq!(report.envelope.status, OperationStatus::Unavailable);
    assert_eq!(report.data["executed"], false);
    assert_eq!(
        report.data["selected_smoke_candidate"]["phase_matches"],
        false
    );
}

#[test]
fn smoke_refuses_qemu_selected_suite_without_qemu_readiness() {
    let root = tempdir().unwrap();
    let manifests = root.path().join("manifests");
    std::fs::create_dir_all(&manifests).unwrap();
    write_valid_manifest(&manifests.join("phase1-core.toml"));
    write_qemu_manifest(&manifests.join("qemu-smoke.toml"));
    let config = root.path().join("config.toml");
    write_valid_config(&config);
    let report = inspect_with_paths_and_probe(root.path(), &manifests, &config, ready_probe());
    let options = BootstrapSmokeOptions {
        suite: "qemu-smoke".to_string(),
        phase: 3,
        ..Default::default()
    };
    let report = smoke_with_runner(&report, &options, |_command| {
        panic!("QEMU-selected suite must not execute without QEMU readiness")
    });

    assert_eq!(report.envelope.status, OperationStatus::Unavailable);
    assert_eq!(report.data["executed"], false);
    assert_eq!(
        report.data["selected_smoke_candidate"]["requires_qemu"],
        true
    );
    assert!(
        report
            .envelope
            .warnings
            .iter()
            .any(|warning| warning.contains("QEMU/KVM is required"))
    );
}

#[test]
fn smoke_success_records_command_evidence() {
    let fixture = ready_bootstrap_fixture();
    let report = smoke_with_runner(
        &fixture.report,
        &BootstrapSmokeOptions::default(),
        |_command| SmokeCommandOutput {
            exit_code: 0,
            stdout: r#"{"suite":"phase1-core","status":"passed"}"#.to_string(),
            stderr: String::new(),
        },
    );

    assert_eq!(report.envelope.status, OperationStatus::Ok);
    assert_eq!(report.envelope.evidence[0].kind, EvidenceKind::Command);
    assert_eq!(report.data["executed"], true);
    assert_eq!(report.data["exit_code"], 0);
}

#[test]
fn smoke_failure_records_failed_status_and_stderr() {
    let fixture = ready_bootstrap_fixture();
    let report = smoke_with_runner(
        &fixture.report,
        &BootstrapSmokeOptions::default(),
        |_command| SmokeCommandOutput {
            exit_code: 2,
            stdout: String::new(),
            stderr: "container runtime unavailable".to_string(),
        },
    );

    assert_eq!(report.envelope.status, OperationStatus::Failed);
    assert_eq!(report.data["stderr"], "container runtime unavailable");
}
