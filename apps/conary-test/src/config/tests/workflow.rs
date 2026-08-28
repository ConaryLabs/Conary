// conary-test/src/config/tests/workflow.rs

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Command, Stdio},
};

const MATRIX_JOB_ID: &str = "native-cross-source-lifecycle";
const MATRIX_ARTIFACT_JOB_ID: &str = "native-matrix-artifacts";
const MATRIX_GATE_ID: &str = "native-cross-source-lifecycle-gate";
const STABLE_CHECK_CONTEXT: &str = "native-cross-source-lifecycle";
const DAILY_DRIVER_JOB_ID: &str = "native-daily-driver-corpus";
const DAILY_DRIVER_GATE_ID: &str = "native-daily-driver-corpus-gate";
const NATIVE_PARITY_JOB_ID: &str = "native-pm-parity";
const NATIVE_PARITY_GATE_ID: &str = "native-pm-parity-gate";
const RELEASE_ARTIFACT_MATRIX_JOB_ID: &str = "native-package-lifecycle";
const RELEASE_ARTIFACT_GATE_ID: &str = "release-artifact-proof";
const COMPILER_CACHE_SETUP_ACTION: &str = "./.github/actions/setup-rust-workspace";
const COMPILER_CACHE_SUMMARY_ACTION: &str = "./.github/actions/summarize-rust-cache";
const NATIVE_COMPILER_CACHE_ACTION: &str = "./.github/actions/setup-native-matrix-compiler-cache";

#[derive(Debug, Deserialize)]
struct Workflow {
    jobs: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
struct CompositeAction {
    runs: CompositeActionRuns,
}

#[derive(Debug, Deserialize)]
struct CompositeActionRuns {
    using: String,
    steps: Vec<WorkflowStep>,
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
    needs: String,
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
    #[serde(default)]
    needs: Option<String>,
    #[serde(rename = "if", default)]
    condition: Option<String>,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceTestShardJob {
    name: String,
    needs: String,
    #[serde(rename = "if")]
    condition: String,
    strategy: WorkspaceTestShardStrategy,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceTestShardStrategy {
    #[serde(rename = "fail-fast")]
    fail_fast: bool,
    matrix: WorkspaceTestShardMatrix,
}

#[derive(Debug, Deserialize)]
struct WorkspaceTestShardMatrix {
    include: Vec<WorkspaceTestShardLane>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct WorkspaceTestShardLane {
    shard: String,
}

#[derive(Debug, Deserialize)]
struct CompilerCachePrimerJob {
    name: String,
    #[serde(rename = "timeout-minutes")]
    timeout_minutes: u64,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct MatrixArtifactJob {
    name: String,
    #[serde(rename = "timeout-minutes")]
    timeout_minutes: u64,
    env: BTreeMap<String, serde_yaml::Value>,
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
    #[serde(default)]
    with: BTreeMap<String, serde_yaml::Value>,
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

fn load_native_compiler_cache_action() -> CompositeAction {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.github/actions/setup-native-matrix-compiler-cache/action.yml");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&source)
        .unwrap_or_else(|error| panic!("parse {} as typed YAML: {error}", path.display()))
}

fn release_artifact_workflow_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.github/workflows/release-artifact-proof.yml")
}

fn merge_validation_workflow_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/merge-validation.yml")
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

fn action_step<'a>(steps: &'a [WorkflowStep], action: &str) -> &'a WorkflowStep {
    let matches = steps
        .iter()
        .filter(|step| step.uses.as_deref() == Some(action))
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "job must contain exactly one `{action}` step"
    );
    matches[0]
}

fn assert_protected_compiler_cache_reader(job: &WorkspaceTestJob, phase: &str) {
    assert_eq!(job.needs.as_deref(), Some("gnu-compiler-cache"));
    assert_eq!(job.condition.as_deref(), Some("${{ always() }}"));

    let require = named_step(&job.steps, "Require exact compiler-cache seed");
    assert_eq!(
        require.env.get("PRIMER_RESULT").map(String::as_str),
        Some("${{ needs.gnu-compiler-cache.result }}")
    );
    assert_eq!(
        require.run.as_deref(),
        Some("test \"$PRIMER_RESULT\" = success")
    );

    let setup = action_step(&job.steps, COMPILER_CACHE_SETUP_ACTION);
    assert_eq!(
        setup
            .with
            .get("compiler-cache")
            .and_then(serde_yaml::Value::as_str),
        Some("reader"),
        "protected GNU consumer must use the exact read-only cache"
    );

    let summary = action_step(&job.steps, COMPILER_CACHE_SUMMARY_ACTION);
    assert_eq!(summary.condition.as_deref(), Some("${{ always() }}"));
    assert_eq!(
        summary
            .with
            .get("phase")
            .and_then(serde_yaml::Value::as_str),
        Some(phase),
        "compiler-cache evidence must use the exact protected phase"
    );
}

fn assert_compiler_cache_primer(workflow: &Workflow, phase: &str) {
    let job: CompilerCachePrimerJob = parse_job(workflow, "gnu-compiler-cache");
    assert_eq!(job.name, "gnu-compiler-cache");
    assert_eq!(job.timeout_minutes, 30);

    let setup = action_step(&job.steps, COMPILER_CACHE_SETUP_ACTION);
    assert_eq!(
        setup
            .with
            .get("compiler-cache")
            .and_then(serde_yaml::Value::as_str),
        Some("writer")
    );
    let prime = named_step(&job.steps, "Prime exact workspace test compilation");
    assert_eq!(
        prime.run.as_deref(),
        Some("cargo test --workspace --no-run --verbose")
    );
    let summary = action_step(&job.steps, COMPILER_CACHE_SUMMARY_ACTION);
    assert_eq!(summary.condition.as_deref(), Some("${{ always() }}"));
    assert_eq!(
        summary
            .with
            .get("phase")
            .and_then(serde_yaml::Value::as_str),
        Some(phase)
    );
}

fn assert_exact_head_artifact_consumer(job: &MatrixJob) {
    assert_eq!(job.needs, MATRIX_ARTIFACT_JOB_ID);
    let restore = named_step(&job.steps, "Restore exact-head native matrix executables");
    assert_eq!(
        restore.uses.as_deref(),
        Some("./.github/actions/restore-native-matrix-artifact")
    );
    for (key, value) in [
        ("commit-sha", "${{ github.sha }}"),
        ("run-id", "${{ github.run_id }}"),
        ("event-name", "${{ github.event_name }}"),
    ] {
        assert_eq!(
            restore.with.get(key).and_then(serde_yaml::Value::as_str),
            Some(value),
            "artifact consumer must bind {key}"
        );
    }
    assert!(
        job.steps
            .iter()
            .all(|step| step.uses.as_deref() != Some("./.github/actions/setup-rust-workspace"))
    );
    assert!(
        job.steps
            .iter()
            .all(|step| step.uses.as_deref() != Some("./.github/actions/build-static-conary"))
    );
    assert!(job.steps.iter().all(|step| {
        !step.run.as_deref().is_some_and(|run| {
            run.contains("cargo build")
                || run.contains("cargo run -p conary-test")
                || run.contains("cargo test -p conary-test")
        })
    }));
}

#[test]
fn native_matrix_artifact_is_built_once_with_exact_source_and_cache_policy() {
    let workflow = load_workflow();
    let job: MatrixArtifactJob = parse_job(&workflow, MATRIX_ARTIFACT_JOB_ID);

    assert_eq!(job.name, MATRIX_ARTIFACT_JOB_ID);
    assert_eq!(job.timeout_minutes, 30);
    for (key, expected) in [
        ("CARGO_INCREMENTAL", "0"),
        ("CARGO_PROFILE_DEV_DEBUG", "0"),
        ("CARGO_PROFILE_TEST_DEBUG", "0"),
    ] {
        assert_eq!(
            job.env.get(key).and_then(|value| match value {
                serde_yaml::Value::String(value) => Some(value.as_str()),
                serde_yaml::Value::Number(value) => Some(if value.as_u64() == Some(0) {
                    "0"
                } else {
                    "unexpected"
                }),
                _ => None,
            }),
            Some(expected),
            "producer must pin {key}"
        );
    }
    assert!(!job.env.contains_key("SCCACHE_GHA_ENABLED"));
    assert!(
        !job.env.contains_key("SCCACHE_DIR"),
        "runner.temp is step-scoped and must be exported by the cache-policy step"
    );

    let cache_setup = action_step(&job.steps, NATIVE_COMPILER_CACHE_ACTION);
    assert_eq!(
        cache_setup.condition.as_deref(),
        Some("${{ steps.native-artifact-restore.outputs.cache-hit != 'true' }}")
    );

    let cache_action = load_native_compiler_cache_action();
    assert_eq!(cache_action.runs.using, "composite");
    let cache_policy = named_step(
        &cache_action.runs.steps,
        "Bind exact native compiler-cache policy",
    );
    let cache_policy_run = cache_policy.run.as_deref().expect("native cache policy");
    for binding in [
        "rustc=%s",
        "cargo=%s",
        "lock=%s",
        "target=x86_64-unknown-linux-musl",
        "cc=%s",
        "native_abi=%s",
        "builder=%s",
        "header_probe=%s",
        "build_action=%s",
        "cache_action=%s",
        "features=default",
        "test_harness=true",
        "rustflags=%s",
        "encoded_rustflags=%s",
        "incremental=%s",
        "dev_debug=%s",
        "test_debug=%s",
    ] {
        assert!(
            cache_policy_run.contains(binding),
            "native cache identity must bind {binding}"
        );
    }
    assert!(cache_policy_run.contains("native-matrix-musl-local-v1-${identity}"));
    assert!(cache_policy_run.contains("${restore_prefix}${GITHUB_SHA}"));
    assert!(cache_policy_run.contains("SCCACHE_CACHE_BACKEND=local-disk-bulk-v1"));
    assert!(cache_policy_run.contains("SCCACHE_CACHE_SIZE=4G"));
    assert!(cache_policy_run.contains("SCCACHE_LOCAL_RW_MODE=READ_WRITE"));
    assert!(cache_policy_run.contains("SCCACHE_VERSION=0.16.0"));
    assert!(cache_policy_run.contains("SCCACHE_DIR=$RUNNER_TEMP/native-matrix-sccache"));

    let artifact_restore = named_step(
        &job.steps,
        "Restore exact prior-attempt native matrix artifact",
    );
    assert_eq!(
        artifact_restore.uses.as_deref(),
        Some("actions/cache/restore@668228422ae6a00e4ad889ee87cd7109ec5666a7")
    );
    assert_eq!(
        artifact_restore
            .with
            .get("path")
            .and_then(serde_yaml::Value::as_str),
        Some("${{ runner.temp }}/native-matrix-artifact")
    );
    assert_eq!(
        artifact_restore
            .with
            .get("key")
            .and_then(serde_yaml::Value::as_str),
        Some("native-matrix-artifact-v1-${{ github.run_id }}-${{ github.sha }}")
    );
    let reused_verify = named_step(&job.steps, "Verify reusable exact-run matrix artifact");
    assert_eq!(
        reused_verify.condition.as_deref(),
        Some("${{ steps.native-artifact-restore.outputs.cache-hit == 'true' }}")
    );
    assert!(
        reused_verify
            .run
            .as_deref()
            .is_some_and(|run| run.contains("native-matrix-artifact.sh verify"))
    );

    let restore = named_step(
        &cache_action.runs.steps,
        "Restore native compiler-cache seed",
    );
    assert_eq!(
        restore.uses.as_deref(),
        Some("actions/cache/restore@668228422ae6a00e4ad889ee87cd7109ec5666a7")
    );
    assert_eq!(
        restore.with.get("path").and_then(serde_yaml::Value::as_str),
        Some("${{ runner.temp }}/native-matrix-sccache")
    );
    assert_eq!(
        restore.with.get("key").and_then(serde_yaml::Value::as_str),
        Some("${{ steps.policy.outputs.exact_key }}")
    );
    assert_eq!(
        restore
            .with
            .get("restore-keys")
            .and_then(serde_yaml::Value::as_str),
        Some("${{ steps.policy.outputs.restore_prefix }}")
    );

    let save = named_step(&job.steps, "Save exact native compiler cache");
    assert_eq!(
        save.uses.as_deref(),
        Some("actions/cache/save@668228422ae6a00e4ad889ee87cd7109ec5666a7")
    );
    assert_eq!(
        save.condition.as_deref(),
        Some(
            "${{ steps.native-artifact-restore.outputs.cache-hit != 'true' && steps.native-cache.outputs.cache_hit != 'true' }}"
        )
    );
    assert_eq!(
        save.with.get("key").and_then(serde_yaml::Value::as_str),
        Some("${{ steps.native-cache.outputs.exact_key }}")
    );

    let build = named_step(&job.steps, "Build all static matrix executables once");
    assert_eq!(
        build.uses.as_deref(),
        Some("./.github/actions/build-static-conary")
    );
    assert_eq!(
        build
            .with
            .get("with-test-harness")
            .and_then(serde_yaml::Value::as_str),
        Some("true")
    );
    assert_eq!(
        build.condition.as_deref(),
        Some("${{ steps.native-artifact-restore.outputs.cache-hit != 'true' }}")
    );

    let package = named_step(&job.steps, "Package immutable exact-head matrix evidence");
    let package_run = package.run.as_deref().expect("artifact package command");
    assert!(package_run.contains("native-matrix-artifact.sh package"));
    assert!(package_run.contains("$GITHUB_SHA"));
    assert!(package_run.contains("$GITHUB_RUN_ID"));
    assert_eq!(
        package.condition.as_deref(),
        Some("${{ steps.native-artifact-restore.outputs.cache-hit != 'true' }}")
    );

    let verify = named_step(&job.steps, "Verify fresh exact-head matrix artifact");
    assert!(
        verify
            .run
            .as_deref()
            .is_some_and(|run| run.contains("native-matrix-artifact.sh verify"))
    );
    assert_eq!(
        verify.condition.as_deref(),
        Some("${{ steps.native-artifact-restore.outputs.cache-hit != 'true' }}")
    );

    let artifact_save = named_step(&job.steps, "Save verified exact-run matrix artifact");
    assert_eq!(
        artifact_save.uses.as_deref(),
        Some("actions/cache/save@668228422ae6a00e4ad889ee87cd7109ec5666a7")
    );
    assert_eq!(
        artifact_save.condition.as_deref(),
        Some("${{ steps.native-artifact-restore.outputs.cache-hit != 'true' }}")
    );
    assert_eq!(
        artifact_save
            .with
            .get("key")
            .and_then(serde_yaml::Value::as_str),
        Some("native-matrix-artifact-v1-${{ github.run_id }}-${{ github.sha }}")
    );

    let upload = named_step(&job.steps, "Upload immutable exact-head matrix artifact");
    assert_eq!(
        upload.uses.as_deref(),
        Some("actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f")
    );
    assert_eq!(
        upload.with.get("name").and_then(serde_yaml::Value::as_str),
        Some("native-matrix-${{ github.sha }}")
    );
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
    assert_exact_head_artifact_consumer(&job);
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

    let image = named_step(&job.steps, "Build the ${{ matrix.distro }} test image");
    assert!(image.run.as_deref().is_some_and(|run| {
        run.contains("target/x86_64-unknown-linux-musl/debug/conary-test")
            && run.contains("images build --distro \"${{ matrix.distro }}\"")
            && run.contains("image_build_ms=")
    }));
    assert!(!image.continue_on_error);
    assert_eq!(image.condition, None);

    let run = named_step(
        &job.steps,
        "Capture the native oracle and run Cartesian lifecycle parity",
    );
    assert!(run.run.as_deref().is_some_and(|command| {
        command.contains("target/x86_64-unknown-linux-musl/debug/conary-test")
            && command.contains("--suite native-cross-source-lifecycle")
            && command.contains("lifecycle_execution_ms=")
    }));
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
    assert_exact_head_artifact_consumer(&job);
    assert!(!job.strategy.fail_fast);
    assert_eq!(
        job.strategy.matrix.distro,
        ["fedora44", "ubuntu-26.04", "arch"]
    );

    let runtime = named_step(&job.steps, "Require the hosted container runtime");
    assert_eq!(runtime.run.as_deref(), Some("docker info"));

    let run = named_step(&job.steps, "Run the attributable daily-driver corpus");
    assert!(run.run.as_deref().is_some_and(|command| {
        command.contains("target/x86_64-unknown-linux-musl/debug/conary-test")
            && command.contains("--suite phase4-native-daily-driver-corpus")
            && command.contains("daily_driver_execution_ms=")
    }));
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
fn native_pm_parity_gate_runs_the_full_deterministic_suite_and_timeout_contract() {
    let workflow = load_workflow();
    let job: MatrixJob = parse_job(&workflow, NATIVE_PARITY_JOB_ID);

    assert_eq!(job.name, "native-pm-parity (${{ matrix.distro }})");
    assert_eq!(job.timeout_minutes, 90);
    assert!(!job.continue_on_error);
    assert_eq!(job.condition, None);
    assert_exact_head_artifact_consumer(&job);
    assert!(!job.strategy.fail_fast);
    assert_eq!(
        job.strategy.matrix.distro,
        ["fedora44", "ubuntu-26.04", "arch"]
    );

    let timeout = named_step(
        &job.steps,
        "Prove timed-out exec process groups are terminated and reaped",
    );
    assert_eq!(
        timeout
            .env
            .get("CONARY_TEST_TIMEOUT_IMAGE")
            .map(String::as_str),
        Some("conary-test-${{ matrix.distro }}:latest")
    );
    let timeout_run = timeout.run.as_deref().expect("timeout regression command");
    assert!(timeout_run.contains("conary-test-library-tests"));
    assert!(timeout_run.contains("timed_out_container_exec_terminates_and_reaps_process_group"));
    assert!(timeout_run.contains("--ignored --exact"));

    let parity = named_step(&job.steps, "Run the full deterministic native parity suite");
    assert!(parity.run.as_deref().is_some_and(|command| {
        command.contains("target/x86_64-unknown-linux-musl/debug/conary-test")
            && command.contains("--suite phase4-native-pm-parity")
            && command.contains("native_parity_execution_ms=")
    }));
    assert_eq!(
        parity
            .env
            .get("CONARY_TEST_REUSE_IMAGE")
            .map(String::as_str),
        Some("1")
    );

    let evidence = named_step(&job.steps, "Verify full native parity evidence");
    assert!(
        evidence
            .run
            .as_deref()
            .is_some_and(|run| run.contains("check-conary-test-result-gate.sh"))
    );

    let gate: GateJob = parse_job(&workflow, NATIVE_PARITY_GATE_ID);
    assert_eq!(gate.name, "native-pm-parity");
    assert_eq!(gate.condition, "${{ always() }}");
    assert_eq!(gate.needs, NATIVE_PARITY_JOB_ID);
    assert!(!gate.continue_on_error);
    let gate_step = named_step(&gate.steps, "Require every full native parity job");
    assert_eq!(
        gate_step.env.get("MATRIX_RESULT").map(String::as_str),
        Some("${{ needs.native-pm-parity.result }}")
    );
}

#[test]
fn workspace_gate_provisions_the_exact_namespace_test_boundary() {
    let workflow = load_workflow();
    let job: WorkspaceTestShardJob = parse_job(&workflow, "workspace-test-shards");

    assert_eq!(job.name, "workspace-test (${{ matrix.shard }})");
    assert_eq!(job.needs, "gnu-compiler-cache");
    assert_eq!(job.condition, "${{ always() }}");
    assert!(!job.strategy.fail_fast);
    assert_eq!(
        job.strategy.matrix.include,
        [
            WorkspaceTestShardLane {
                shard: "conary".to_string(),
            },
            WorkspaceTestShardLane {
                shard: "conary-core-repository".to_string(),
            },
            WorkspaceTestShardLane {
                shard: "conary-core-remaining".to_string(),
            },
            WorkspaceTestShardLane {
                shard: "conary-core-targets".to_string(),
            },
            WorkspaceTestShardLane {
                shard: "remaining".to_string(),
            },
        ]
    );

    let namespace = named_step(&job.steps, "Enable exact ownership-test namespaces");
    assert_eq!(
        namespace.uses.as_deref(),
        Some("./.github/actions/setup-exact-ownership-tests")
    );
    assert_eq!(namespace.run, None);
    assert!(!namespace.continue_on_error);
    assert_eq!(namespace.condition, None);

    let tests = named_step(&job.steps, "Run workspace test shard");
    let namespace_index = job
        .steps
        .iter()
        .position(|step| step.name.as_deref() == Some("Enable exact ownership-test namespaces"))
        .expect("namespace setup step should exist");
    let tests_index = job
        .steps
        .iter()
        .position(|step| step.name.as_deref() == Some("Run workspace test shard"))
        .expect("workspace test step should exist");
    assert!(
        namespace_index < tests_index,
        "namespace setup must run before workspace tests"
    );
    assert_eq!(
        tests.env.get("TEST_SHARD").map(String::as_str),
        Some("${{ matrix.shard }}")
    );
    let command = tests.run.as_deref().expect("workspace shard dispatch");
    for predicate in [
        "conary) cargo test -p conary --verbose",
        "conary-core-repository) cargo test -p conary-core --lib repository:: --verbose",
        "cargo test -p conary-core --lib --verbose -- --skip repository::",
        "conary-core-targets) cargo test -p conary-core --bins --test '*' --verbose",
        "cargo test --workspace --exclude conary-test",
        "--exclude conary --exclude conary-core --verbose",
        "unknown workspace test shard",
    ] {
        assert!(
            command.contains(predicate),
            "workspace shards need {predicate}"
        );
    }
    assert!(!tests.continue_on_error);
    assert_eq!(tests.condition, None);

    let aggregate: GateJob = parse_job(&workflow, "workspace-tests");
    assert_eq!(aggregate.name, "workspace-tests");
    assert_eq!(aggregate.condition, "${{ always() }}");
    assert_eq!(aggregate.needs, "workspace-test-shards");
    let require = named_step(&aggregate.steps, "Require every workspace test shard");
    assert_eq!(
        require.env.get("SHARDS_RESULT").map(String::as_str),
        Some("${{ needs.workspace-test-shards.result }}")
    );
    assert_eq!(
        require.run.as_deref(),
        Some("test \"$SHARDS_RESULT\" = success")
    );
}

#[test]
fn compatible_protected_jobs_share_compiler_outputs_with_typed_evidence() {
    let pr = load_workflow();
    assert_compiler_cache_primer(&pr, "pr-gnu-cache-primer");
    for (job_id, phase) in [
        ("clippy", "pr-clippy"),
        ("generation-db-reflink", "pr-generation-db-reflink"),
        ("conary-test-crate", "pr-conary-test"),
        ("doctests", "pr-doctests"),
    ] {
        let job: WorkspaceTestJob = parse_job(&pr, job_id);
        assert_protected_compiler_cache_reader(&job, phase);
    }
    let pr_shards: WorkspaceTestJob = parse_job(&pr, "workspace-test-shards");
    assert_protected_compiler_cache_reader(&pr_shards, "pr-workspace-${{ matrix.shard }}");

    let main = load_workflow_from(merge_validation_workflow_path());
    assert_compiler_cache_primer(&main, "main-gnu-cache-primer");
    for (job_id, phase) in [
        ("clippy", "main-clippy"),
        ("generation-db-reflink", "main-generation-db-reflink"),
        ("conary-test-crate", "main-conary-test"),
        ("doctests", "main-doctests"),
        ("local-smoke", "main-local-smoke"),
    ] {
        let job: WorkspaceTestJob = parse_job(&main, job_id);
        assert_protected_compiler_cache_reader(&job, phase);
    }
    let main_shards: WorkspaceTestJob = parse_job(&main, "workspace-test-shards");
    assert_protected_compiler_cache_reader(&main_shards, "main-workspace-${{ matrix.shard }}");
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
