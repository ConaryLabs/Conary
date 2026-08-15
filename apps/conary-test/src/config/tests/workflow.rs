// conary-test/src/config/tests/workflow.rs

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Command, Stdio},
};

const MATRIX_JOB_ID: &str = "native-cross-source-lifecycle";
const MATRIX_GATE_ID: &str = "native-cross-source-lifecycle-gate";
const STABLE_CHECK_CONTEXT: &str = "native-cross-source-lifecycle";
const DAILY_DRIVER_JOB_ID: &str = "native-daily-driver-corpus";
const DAILY_DRIVER_GATE_ID: &str = "native-daily-driver-corpus-gate";
const RELEASE_ARTIFACT_MATRIX_JOB_ID: &str = "native-package-lifecycle";
const RELEASE_ARTIFACT_GATE_ID: &str = "release-artifact-proof";

#[derive(Debug, Deserialize)]
struct Workflow {
    jobs: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
struct MatrixJob {
    name: String,
    #[serde(rename = "timeout-minutes")]
    timeout_minutes: u64,
    #[serde(rename = "continue-on-error", default)]
    continue_on_error: bool,
    #[serde(rename = "if", default)]
    condition: Option<String>,
    strategy: MatrixStrategy,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct MatrixStrategy {
    #[serde(rename = "fail-fast")]
    fail_fast: bool,
    matrix: DistroMatrix,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DistroMatrix {
    distro: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GateJob {
    name: String,
    #[serde(rename = "if")]
    condition: String,
    #[serde(rename = "continue-on-error", default)]
    continue_on_error: bool,
    needs: String,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceTestJob {
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct ReleaseArtifactMatrixJob {
    name: String,
    #[serde(rename = "timeout-minutes")]
    timeout_minutes: u64,
    strategy: ReleaseArtifactStrategy,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct ReleaseArtifactStrategy {
    #[serde(rename = "fail-fast")]
    fail_fast: bool,
    matrix: ReleaseArtifactMatrix,
}

#[derive(Debug, Deserialize)]
struct ReleaseArtifactMatrix {
    include: Vec<ReleaseArtifactLane>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ReleaseArtifactLane {
    distro: String,
    native_format: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowStep {
    name: Option<String>,
    uses: Option<String>,
    run: Option<String>,
    #[serde(rename = "continue-on-error", default)]
    continue_on_error: bool,
    #[serde(rename = "if", default)]
    condition: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

fn workflow_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/pr-gate.yml")
}

fn load_workflow() -> Workflow {
    load_workflow_from(workflow_path())
}

fn load_workflow_from(path: PathBuf) -> Workflow {
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&source)
        .unwrap_or_else(|error| panic!("parse {} as typed YAML: {error}", path.display()))
}

fn release_artifact_workflow_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.github/workflows/release-artifact-proof.yml")
}

fn parse_job<T>(workflow: &Workflow, id: &str) -> T
where
    T: for<'de> Deserialize<'de>,
{
    let value = workflow
        .jobs
        .get(id)
        .unwrap_or_else(|| panic!("workflow is missing required job `{id}`"))
        .clone();
    serde_yaml::from_value(value).unwrap_or_else(|error| panic!("parse job `{id}`: {error}"))
}

fn named_step<'a>(steps: &'a [WorkflowStep], name: &str) -> &'a WorkflowStep {
    let matches = steps
        .iter()
        .filter(|step| step.name.as_deref() == Some(name))
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "job must contain exactly one step named `{name}`"
    );
    matches[0]
}

#[test]
fn native_cross_source_pr_gate_executes_every_supported_target_lane() {
    let workflow = load_workflow();
    let job: MatrixJob = parse_job(&workflow, MATRIX_JOB_ID);

    assert_eq!(
        job.name,
        "native-cross-source-lifecycle (${{ matrix.distro }})"
    );
    assert_eq!(job.timeout_minutes, 90);
    assert!(!job.continue_on_error);
    assert_eq!(job.condition, None);
    assert!(!job.strategy.fail_fast);
    assert_eq!(
        job.strategy.matrix.distro,
        [
            "fedora44",
            "ubuntu-26.04",
            "arch",
            "artix",
            "cachyos",
            "opensuse-tumbleweed",
            "linux-mint-22.3",
            "pop-os-24.04",
        ]
    );

    let runtime = named_step(&job.steps, "Require the hosted container runtime");
    assert_eq!(runtime.run.as_deref(), Some("docker info"));
    assert!(!runtime.continue_on_error);
    assert_eq!(runtime.condition, None);

    let build = named_step(&job.steps, "Build the lifecycle harness");
    assert_eq!(
        build.run.as_deref(),
        Some("cargo build -p conary -p conary-test")
    );
    assert!(!build.continue_on_error);
    assert_eq!(build.condition, None);

    let image = named_step(&job.steps, "Build the ${{ matrix.distro }} test image");
    assert_eq!(
        image.run.as_deref(),
        Some("cargo run -p conary-test -- images build --distro \"${{ matrix.distro }}\"")
    );
    assert!(!image.continue_on_error);
    assert_eq!(image.condition, None);

    let run = named_step(
        &job.steps,
        "Capture the native oracle and run Cartesian lifecycle parity",
    );
    assert_eq!(
        run.run.as_deref(),
        Some(
            "cargo run -p conary-test -- run --distro \"${{ matrix.distro }}\" --phase 4 --suite native-cross-source-lifecycle"
        )
    );
    assert_eq!(
        run.env.get("CONARY_TEST_REUSE_IMAGE").map(String::as_str),
        Some("1")
    );
    assert!(!run.continue_on_error);
    assert_eq!(run.condition, None);

    let derivative = named_step(
        &job.steps,
        "Prove authentic Debian derivative state and native APT behavior",
    );
    assert_eq!(
        derivative.condition.as_deref(),
        Some("${{ matrix.distro == 'linux-mint-22.3' || matrix.distro == 'pop-os-24.04' }}")
    );
    assert!(
        derivative
            .run
            .as_deref()
            .is_some_and(|command| command.contains("--suite debian-derivative-acceptance"))
    );
    let evidence = named_step(&job.steps, "Verify Debian derivative evidence");
    assert_eq!(evidence.condition, derivative.condition);
    assert!(
        evidence
            .run
            .as_deref()
            .is_some_and(|command| command.contains(".target_release.stages"))
    );
    for required in [
        ".target_release.checkout_commit",
        ".target_release.checkout_dirty == false",
        "base_image",
        "keyring_package",
        "identity_package",
        "digest_source",
        "fingerprints",
    ] {
        assert!(
            evidence.run.as_deref().unwrap().contains(required),
            "derivative evidence gate must require {required}"
        );
    }

    let rolling = named_step(
        &job.steps,
        "Prove authentic rolling derivative state and native repository behavior",
    );
    assert_eq!(
        rolling.condition.as_deref(),
        Some("${{ matrix.distro == 'cachyos' || matrix.distro == 'opensuse-tumbleweed' }}")
    );
    assert!(
        rolling
            .run
            .as_deref()
            .is_some_and(|command| command.contains("--suite rolling-derivative-acceptance"))
    );
    let rolling_evidence = named_step(&job.steps, "Verify rolling derivative evidence");
    assert_eq!(rolling_evidence.condition, rolling.condition);
    for required in [
        ".target_release.packages",
        "native_repository_declaration",
        "running_target_bytes",
        "fingerprints",
    ] {
        assert!(
            rolling_evidence.run.as_deref().unwrap().contains(required),
            "rolling evidence gate must require {required}"
        );
    }
}

#[test]
fn native_cross_source_gate_is_a_stable_all_lane_required_context() {
    let workflow = load_workflow();
    let job: GateJob = parse_job(&workflow, MATRIX_GATE_ID);

    assert_eq!(job.name, STABLE_CHECK_CONTEXT);
    assert_eq!(job.condition, "${{ always() }}");
    assert!(!job.continue_on_error);
    assert_eq!(job.needs, MATRIX_JOB_ID);

    let step = named_step(&job.steps, "Require every distro lifecycle job");
    assert_eq!(
        step.env.get("MATRIX_RESULT").map(String::as_str),
        Some("${{ needs.native-cross-source-lifecycle.result }}")
    );
    assert!(!step.continue_on_error);
    assert_eq!(step.condition, None);
    let script = step
        .run
        .as_deref()
        .expect("matrix gate must execute a script");

    for (result, expected_success) in [
        ("success", true),
        ("failure", false),
        ("cancelled", false),
        ("skipped", false),
    ] {
        let status = Command::new("bash")
            .args(["-euo", "pipefail", "-c", script])
            .env("MATRIX_RESULT", result)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap_or_else(|error| panic!("execute matrix gate for `{result}`: {error}"));
        assert_eq!(
            status.success(),
            expected_success,
            "matrix gate produced the wrong result for `{result}`"
        );
    }
}

#[test]
fn native_daily_driver_gate_executes_the_three_attributable_lanes() {
    let workflow = load_workflow();
    let job: MatrixJob = parse_job(&workflow, DAILY_DRIVER_JOB_ID);

    assert_eq!(
        job.name,
        "native-daily-driver-corpus (${{ matrix.distro }})"
    );
    assert_eq!(job.timeout_minutes, 90);
    assert!(!job.continue_on_error);
    assert_eq!(job.condition, None);
    assert!(!job.strategy.fail_fast);
    assert_eq!(
        job.strategy.matrix.distro,
        ["fedora44", "ubuntu-26.04", "arch"]
    );

    let runtime = named_step(&job.steps, "Require the hosted container runtime");
    assert_eq!(runtime.run.as_deref(), Some("docker info"));

    let run = named_step(&job.steps, "Run the attributable daily-driver corpus");
    assert_eq!(
        run.run.as_deref(),
        Some(
            "cargo run -p conary-test -- run --distro \"${{ matrix.distro }}\" --phase 4 --suite phase4-native-pm-parity"
        )
    );
    assert_eq!(
        run.env.get("CONARY_TEST_REUSE_IMAGE").map(String::as_str),
        Some("1")
    );
    assert!(!run.continue_on_error);

    let evidence = named_step(&job.steps, "Verify daily-driver corpus evidence");
    let evidence_run = evidence
        .run
        .as_deref()
        .expect("daily-driver evidence command");
    assert!(evidence_run.contains("check-conary-test-result-gate.sh"));
    assert!(evidence_run.contains("check-conary-corpus-result-gate.sh"));
    assert!(!evidence.continue_on_error);

    let gate: GateJob = parse_job(&workflow, DAILY_DRIVER_GATE_ID);
    assert_eq!(gate.name, "native-daily-driver-corpus");
    assert_eq!(gate.condition, "${{ always() }}");
    assert_eq!(gate.needs, DAILY_DRIVER_JOB_ID);
    assert!(!gate.continue_on_error);
    let gate_step = named_step(&gate.steps, "Require every daily-driver corpus job");
    assert_eq!(
        gate_step.env.get("MATRIX_RESULT").map(String::as_str),
        Some("${{ needs.native-daily-driver-corpus.result }}")
    );
}

#[test]
fn workspace_gate_provisions_the_exact_namespace_test_boundary() {
    let workflow = load_workflow();
    let job: WorkspaceTestJob = parse_job(&workflow, "workspace-tests");

    let namespace = named_step(&job.steps, "Enable exact ownership-test namespaces");
    assert_eq!(
        namespace.uses.as_deref(),
        Some("./.github/actions/setup-exact-ownership-tests")
    );
    assert_eq!(namespace.run, None);
    assert!(!namespace.continue_on_error);
    assert_eq!(namespace.condition, None);

    let tests = named_step(&job.steps, "Run workspace tests");
    let namespace_index = job
        .steps
        .iter()
        .position(|step| step.name.as_deref() == Some("Enable exact ownership-test namespaces"))
        .expect("namespace setup step should exist");
    let tests_index = job
        .steps
        .iter()
        .position(|step| step.name.as_deref() == Some("Run workspace tests"))
        .expect("workspace test step should exist");
    assert!(
        namespace_index < tests_index,
        "namespace setup must run before workspace tests"
    );
    assert_eq!(
        tests.run.as_deref(),
        Some("cargo test --workspace --exclude conary-test --verbose")
    );
    assert!(!tests.continue_on_error);
    assert_eq!(tests.condition, None);
}

#[test]
fn release_artifact_workflow_installs_every_published_native_package() {
    let workflow = load_workflow_from(release_artifact_workflow_path());
    let job: ReleaseArtifactMatrixJob = parse_job(&workflow, RELEASE_ARTIFACT_MATRIX_JOB_ID);

    assert_eq!(job.name, "native-package-lifecycle (${{ matrix.distro }})");
    assert_eq!(job.timeout_minutes, 90);
    assert!(!job.strategy.fail_fast);
    assert_eq!(
        job.strategy.matrix.include,
        [
            ReleaseArtifactLane {
                distro: "fedora44".to_string(),
                native_format: "rpm".to_string(),
            },
            ReleaseArtifactLane {
                distro: "ubuntu-26.04".to_string(),
                native_format: "deb".to_string(),
            },
            ReleaseArtifactLane {
                distro: "arch".to_string(),
                native_format: "arch".to_string(),
            },
        ]
    );

    let resolve = named_step(&job.steps, "Resolve the published native package");
    let resolve_script = resolve.run.as_deref().expect("release resolver script");
    for required in [
        "release-matrix.sh resolve-tag",
        "git cat-file -t",
        "sha256sum -c SHA256SUMS --ignore-missing",
        "published_digest",
        "actual_digest",
    ] {
        assert!(
            resolve_script.contains(required),
            "release resolver must contain {required}"
        );
    }

    let image = named_step(
        &job.steps,
        "Install the published native package in the test image",
    );
    assert!(
        image
            .run
            .as_deref()
            .is_some_and(|run| run.contains("--native-package"))
    );

    let lifecycle = named_step(
        &job.steps,
        "Run Cartesian lifecycle parity with the published binary",
    );
    assert!(
        lifecycle
            .run
            .as_deref()
            .is_some_and(|run| run.contains("--suite native-cross-source-lifecycle"))
    );
    assert_eq!(
        lifecycle
            .env
            .get("CONARY_TEST_REUSE_IMAGE")
            .map(String::as_str),
        Some("1")
    );

    let gate: GateJob = parse_job(&workflow, RELEASE_ARTIFACT_GATE_ID);
    assert_eq!(gate.name, RELEASE_ARTIFACT_GATE_ID);
    assert_eq!(gate.condition, "${{ always() }}");
    assert_eq!(gate.needs, RELEASE_ARTIFACT_MATRIX_JOB_ID);
    assert!(!gate.continue_on_error);
}
