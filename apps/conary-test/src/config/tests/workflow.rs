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
struct WorkflowStep {
    name: Option<String>,
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
    let path = workflow_path();
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&source)
        .unwrap_or_else(|error| panic!("parse {} as typed YAML: {error}", path.display()))
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
        ["fedora44", "ubuntu-26.04", "arch"]
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
fn workspace_gate_provisions_the_exact_namespace_test_boundary() {
    let workflow = load_workflow();
    let job: WorkspaceTestJob = parse_job(&workflow, "workspace-tests");

    let namespace = named_step(&job.steps, "Enable exact ownership-test namespaces");
    assert_eq!(
        namespace.run.as_deref(),
        Some(
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0\nunshare --user --map-root-user --mount --propagation private /bin/true\n"
        )
    );
    assert!(!namespace.continue_on_error);
    assert_eq!(namespace.condition, None);

    let tests = named_step(&job.steps, "Run workspace tests");
    assert_eq!(
        tests.run.as_deref(),
        Some("cargo test --workspace --exclude conary-test --verbose")
    );
    assert!(!tests.continue_on_error);
    assert_eq!(tests.condition, None);
}
