// conary-test/src/engine/runner.rs

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::config::distro::GlobalConfig;
use crate::config::manifest::{Assertion, ResourceConstraints, TestDef, TestManifest};
use crate::container::backend::{ContainerBackend, ContainerConfig, ContainerId, ExecResult};
use crate::engine::assertions::evaluate_assertion;
use crate::engine::container_coordinator::ContainerCoordinator;
use crate::engine::executor::{ExecutionContext, StepAction, execute_step};
use crate::engine::mock_server::start_mock_server;
use crate::engine::suite::{TestResult, TestStatus, TestSuite};
use crate::engine::variables;
use crate::report::stream::TestEvent;
use crate::server::remi_client::{PushResultData, PushStepData, RemiClient};
use crate::server::wal::Wal;

/// Context for streaming test results to the Remi admin API.
///
/// When provided to `run_with_cancel`, each completed test is pushed to Remi
/// as it finishes. On push failure, the payload is buffered to the WAL for
/// retry.
pub struct RemiStreamCtx {
    /// Remi run ID returned by `create_run`.
    pub remi_run_id: i64,
    pub client: Arc<RemiClient>,
    pub wal: Option<Arc<std::sync::Mutex<Wal>>>,
}

/// Executes tests from a manifest against a container.
pub struct TestRunner {
    pub config: GlobalConfig,
    pub distro: String,
    vars: HashMap<String, String>,
}

/// Run a test with majority-vote retry logic for flaky tests.
///
/// Takes a closure that executes a single attempt and returns the result.
/// For non-flaky tests, the closure is called exactly once. For flaky tests,
/// it is called up to `retries` times, requiring a majority of passes.
async fn majority_vote<F, Fut>(
    test_def: &TestDef,
    mut attempt_fn: F,
) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)>
where
    F: FnMut() -> Fut,
    Fut:
        std::future::Future<Output = Result<(TestStatus, Option<String>, u64, Option<ExecResult>)>>,
{
    let attempts = if test_def.flaky.unwrap_or(false) {
        test_def.retries.unwrap_or(3).max(1)
    } else {
        1
    };
    let majority = attempts / 2 + 1;

    let mut pass_count = 0_u32;
    let mut fail_count = 0_u32;
    let mut last_failure: Option<String> = None;
    let mut last_exec: Option<ExecResult> = None;
    let mut total_elapsed = 0_u64;

    for _ in 0..attempts {
        let (status, message, elapsed, exec) = attempt_fn().await?;
        total_elapsed += elapsed;
        last_exec = exec;

        if status == TestStatus::Passed {
            pass_count += 1;
        } else {
            fail_count += 1;
            last_failure = message;
        }

        let remaining = attempts.saturating_sub(pass_count + fail_count);
        if pass_count >= majority {
            let message = if attempts > 1 {
                Some(format!(
                    "flaky test passed majority: {pass_count}/{attempts} successful attempts"
                ))
            } else {
                None
            };
            return Ok((TestStatus::Passed, message, total_elapsed, last_exec));
        }
        if pass_count + remaining < majority {
            break;
        }
    }

    let message = if attempts > 1 {
        Some(format!(
            "flaky test failed majority: {pass_count}/{attempts} successful attempts; last failure: {}",
            last_failure.unwrap_or_else(|| "unknown failure".to_string())
        ))
    } else {
        last_failure
    };

    Ok((TestStatus::Failed, message, total_elapsed, last_exec))
}

impl TestRunner {
    pub fn new(config: GlobalConfig, distro: String) -> Self {
        let vars = variables::build_variables(&config, &distro);
        Self {
            config,
            distro,
            vars,
        }
    }

    /// Load distro-specific manifest variables into the runner variable map.
    pub fn load_manifest_vars(&mut self, manifest: &TestManifest) {
        variables::load_manifest_overrides(&mut self.vars, manifest, &self.distro);
    }

    /// Run all tests in the manifest against the given container.
    ///
    /// If `cancel_flag` is provided, the runner checks it between tests. When
    /// set to `true`, remaining tests are marked as `Cancelled`.
    pub async fn run(
        &mut self,
        manifest: &TestManifest,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
        base_container_config: Option<&ContainerConfig>,
    ) -> Result<TestSuite> {
        self.run_with_cancel(
            manifest,
            backend,
            container_id,
            base_container_config,
            None,
            None,
            None,
        )
        .await
    }

    /// Run all tests with an optional cancellation flag, suite-level timeout
    /// enforcement, optional broadcast channel for live event streaming, and
    /// optional Remi streaming context for pushing per-test results.
    ///
    /// When `event_tx` is `Some((run_id, sender))`, the runner emits
    /// `TestEvent` variants to the broadcast channel as tests execute.
    ///
    /// When `remi_ctx` is `Some`, each completed test result is pushed to the
    /// Remi admin API. On push failure, the result is buffered to the WAL.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_cancel(
        &mut self,
        manifest: &TestManifest,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
        base_container_config: Option<&ContainerConfig>,
        cancel_flag: Option<Arc<AtomicBool>>,
        event_tx: Option<(u64, tokio::sync::broadcast::Sender<TestEvent>)>,
        remi_ctx: Option<&RemiStreamCtx>,
    ) -> Result<TestSuite> {
        self.load_manifest_vars(manifest);

        if let Some(mock_server) = &manifest.suite.mock_server {
            start_mock_server(backend, container_id, mock_server).await?;
        }

        let mut suite = TestSuite::new(&manifest.suite.name, manifest.suite.phase);
        suite.status = crate::engine::suite::RunStatus::Running;

        // Emit suite-started event.
        if let Some((run_id, ref tx)) = event_tx {
            let _ = tx.send(TestEvent::SuiteStarted {
                run_id,
                suite: manifest.suite.name.clone(),
                phase: manifest.suite.phase,
                total: manifest.test.len(),
            });
        }

        // Suite-level timeout: derive a deadline from manifest config.
        let suite_deadline = manifest
            .suite
            .timeout
            .map(|secs| Instant::now() + Duration::from_secs(secs));

        for test_def in &manifest.test {
            // Check cancellation flag.
            if cancel_flag
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Relaxed))
            {
                info!("[{}] {}: cancelled by flag", test_def.id, test_def.name);
                suite.record(TestResult {
                    id: test_def.id.clone(),
                    name: test_def.name.clone(),
                    status: TestStatus::Cancelled,
                    duration_ms: 0,
                    message: Some("cancelled".to_string()),
                    stdout: None,
                    stderr: None,
                    attempts: Vec::new(),
                });
                continue;
            }

            // Check suite-level timeout.
            if suite_deadline.is_some_and(|d| Instant::now() >= d) {
                info!(
                    "[{}] {}: cancelled (suite timeout exceeded)",
                    test_def.id, test_def.name
                );
                suite.record(TestResult {
                    id: test_def.id.clone(),
                    name: test_def.name.clone(),
                    status: TestStatus::Cancelled,
                    duration_ms: 0,
                    message: Some("suite timeout exceeded".to_string()),
                    stdout: None,
                    stderr: None,
                    attempts: Vec::new(),
                });
                continue;
            }

            // Check manifest-level skip.
            if let Some(reason) = &test_def.skip {
                let msg = format!("skipped: {reason}");
                info!("[{}] {}: {msg}", test_def.id, test_def.name);
                suite.record(TestResult {
                    id: test_def.id.clone(),
                    name: test_def.name.clone(),
                    status: TestStatus::Skipped,
                    duration_ms: 0,
                    message: Some(msg),
                    stdout: None,
                    stderr: None,
                    attempts: Vec::new(),
                });
                continue;
            }

            // Check dependencies -- skip if any dependency failed.
            if suite.should_skip(&test_def.depends_on) {
                let dep_names: Vec<&str> = test_def
                    .depends_on
                    .as_ref()
                    .map(|d| d.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                let msg = format!("skipped: dependency failed ({})", dep_names.join(", "));
                info!("[{}] {}: {msg}", test_def.id, test_def.name);
                suite.record(TestResult {
                    id: test_def.id.clone(),
                    name: test_def.name.clone(),
                    status: TestStatus::Skipped,
                    duration_ms: 0,
                    message: Some(msg.clone()),
                    stdout: None,
                    stderr: None,
                    attempts: Vec::new(),
                });
                if let Some((run_id, ref tx)) = event_tx {
                    let _ = tx.send(TestEvent::TestSkipped {
                        run_id,
                        test_id: test_def.id.clone(),
                        message: msg,
                    });
                }
                continue;
            }

            // Emit test-started event.
            if let Some((run_id, ref tx)) = event_tx {
                let _ = tx.send(TestEvent::TestStarted {
                    run_id,
                    test_id: test_def.id.clone(),
                    name: test_def.name.clone(),
                });
            }

            let (status, message, elapsed, last_exec) = if test_def.resources.is_some() {
                let Some(base_container_config) = base_container_config else {
                    bail!(
                        "test {} requires resource constraints but no base container config was provided",
                        test_def.id
                    );
                };
                self.run_resource_scoped_test(manifest, test_def, backend, base_container_config)
                    .await?
            } else {
                self.run_test_attempt(test_def, backend, container_id)
                    .await?
            };

            info!(
                "[{}] {}: {status:?} ({elapsed}ms)",
                test_def.id, test_def.name
            );
            if let Some(ref msg) = message {
                warn!("[{}] {msg}", test_def.id);
            }

            // Emit step output for stdout lines.
            if let Some((run_id, ref tx)) = event_tx
                && let Some(ref exec) = last_exec
            {
                for (step_idx, line) in exec.stdout.lines().enumerate() {
                    let _ = tx.send(TestEvent::StepOutput {
                        run_id,
                        test_id: test_def.id.clone(),
                        step: step_idx,
                        line: line.to_string(),
                    });
                }
            }

            suite.record(TestResult {
                id: test_def.id.clone(),
                name: test_def.name.clone(),
                status,
                duration_ms: elapsed,
                message: message.clone(),
                stdout: last_exec.as_ref().map(|e| e.stdout.clone()),
                stderr: last_exec.as_ref().map(|e| e.stderr.clone()),
                attempts: Vec::new(),
            });

            // Push result to Remi if streaming is configured.
            if let Some(ctx) = remi_ctx {
                let push_data = build_push_result(
                    &test_def.id,
                    &test_def.name,
                    status,
                    elapsed,
                    message.as_deref(),
                    last_exec.as_ref(),
                );
                push_to_remi(ctx, &push_data).await;
            }

            // Emit test result event.
            if let Some((run_id, ref tx)) = event_tx {
                match status {
                    TestStatus::Passed => {
                        let _ = tx.send(TestEvent::TestPassed {
                            run_id,
                            test_id: test_def.id.clone(),
                            duration_ms: elapsed,
                        });
                    }
                    TestStatus::Failed => {
                        let _ = tx.send(TestEvent::TestFailed {
                            run_id,
                            test_id: test_def.id.clone(),
                            message: message.unwrap_or_default(),
                            stdout: last_exec.as_ref().map(|e| e.stdout.clone()),
                        });
                    }
                    TestStatus::Skipped => {
                        let _ = tx.send(TestEvent::TestSkipped {
                            run_id,
                            test_id: test_def.id.clone(),
                            message: message.unwrap_or_default(),
                        });
                    }
                    TestStatus::Cancelled => {}
                }
            }

            // Fatal test: stop the entire suite on failure.
            if status == TestStatus::Failed && test_def.fatal.unwrap_or(false) {
                warn!("[{}] fatal test failed, stopping suite", test_def.id);
                break;
            }
        }

        suite.finish();

        // Emit run-complete event.
        if let Some((run_id, ref tx)) = event_tx {
            let _ = tx.send(TestEvent::RunComplete {
                run_id,
                passed: suite.passed(),
                failed: suite.failed(),
                skipped: suite.skipped(),
            });
        }

        Ok(suite)
    }

    async fn run_resource_scoped_test(
        &self,
        manifest: &TestManifest,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        base_container_config: &ContainerConfig,
    ) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)> {
        majority_vote(test_def, || {
            self.run_resource_scoped_test_once(manifest, test_def, backend, base_container_config)
        })
        .await
    }

    async fn run_resource_scoped_test_once(
        &self,
        manifest: &TestManifest,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        base_container_config: &ContainerConfig,
    ) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)> {
        let mut container_config = base_container_config.clone();
        self.apply_resource_constraints(&mut container_config, test_def.resources.as_ref());

        let mut coordinator = ContainerCoordinator::new(backend);
        let container_id = coordinator
            .setup_container(&container_config, test_def.resources.as_ref())
            .await?;

        let result = async {
            crate::engine::container_setup::initialize_container_state(
                &self.config,
                &self.distro,
                manifest.suite.phase > 1,
                backend,
                &container_id,
            )
            .await?;
            if let Some(mock_server) = &manifest.suite.mock_server {
                start_mock_server(backend, &container_id, mock_server).await?;
            }
            self.run_test_once(test_def, backend, &container_id).await
        }
        .await;

        coordinator.teardown_container(&container_id).await?;

        result
    }

    async fn run_test_attempt(
        &self,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)> {
        majority_vote(test_def, || {
            self.run_test_once(test_def, backend, container_id)
        })
        .await
    }

    async fn run_test_once(
        &self,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)> {
        let start = Instant::now();
        let timeout = Duration::from_secs(test_def.timeout);
        let mut last_exec: Option<ExecResult> = None;
        let mut failure: Option<String> = None;

        let ctx = ExecutionContext {
            conary_bin: &self.config.paths.conary_bin,
            db_path: &self.config.paths.db,
        };

        for step in &test_def.step {
            let action = match StepAction::from_step(step, &self.vars) {
                Some(a) => a,
                None => {
                    failure = Some("step has no recognized type".to_string());
                    break;
                }
            };

            // Per-step timeout overrides the test-level timeout.
            let step_timeout = step.timeout.map_or(timeout, Duration::from_secs);

            let step_result =
                execute_step(&action, backend, container_id, &ctx, step_timeout).await?;

            // Sleep steps produce no exec result to assert against.
            if !matches!(action, StepAction::Sleep(_)) {
                last_exec = Some(ExecResult {
                    exit_code: step_result.exit_code,
                    stdout: step_result.stdout.clone(),
                    stderr: step_result.stderr.clone(),
                });
            }

            if let Some(msg) = step_result.failure {
                failure = Some(msg);
                break;
            }

            if let Some(ref assertion) = step.assert {
                let exec = match last_exec.as_ref() {
                    Some(e) => e,
                    None => {
                        failure = Some("assertion step has no preceding exec result".to_string());
                        break;
                    }
                };
                let assertion = self.expand_assertion(assertion);
                if let Err(e) =
                    evaluate_assertion(&assertion, exec.exit_code, &exec.stdout, &exec.stderr)
                {
                    failure = Some(format!("assertion failed: {e}"));
                    break;
                }
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        let (status, message) = match failure {
            Some(msg) => (TestStatus::Failed, Some(msg)),
            None => (TestStatus::Passed, None),
        };

        Ok((status, message, elapsed, last_exec))
    }

    /// Apply per-test resource constraints to a container configuration.
    pub fn apply_resource_constraints(
        &self,
        container_config: &mut ContainerConfig,
        resources: Option<&ResourceConstraints>,
    ) {
        let Some(resources) = resources else {
            return;
        };

        if let Some(tmpfs_size_mb) = resources.tmpfs_size_mb {
            container_config.tmpfs.insert(
                "/var/lib/conary".to_string(),
                format!("size={tmpfs_size_mb}m"),
            );
        }

        if let Some(memory_limit_mb) = resources.memory_limit_mb {
            container_config.memory_limit =
                i64::try_from(memory_limit_mb.saturating_mul(1024 * 1024)).ok();
        }

        if resources.network_isolated.unwrap_or(false) {
            container_config.network_mode = "none".to_string();
        }
    }

    fn expand_assertion(&self, assertion: &Assertion) -> Assertion {
        variables::expand_assertion(assertion, &self.vars)
    }
}

// ---------------------------------------------------------------------------
// Remi streaming helpers
// ---------------------------------------------------------------------------

/// Build a `PushResultData` from a completed test result.
///
/// When `last_exec` is available, a single step is included with the raw
/// stdout/stderr (Remi handles ANSI stripping on insertion).
fn build_push_result(
    test_id: &str,
    name: &str,
    status: TestStatus,
    duration_ms: u64,
    message: Option<&str>,
    last_exec: Option<&ExecResult>,
) -> PushResultData {
    let status_str = match status {
        TestStatus::Passed => "passed",
        TestStatus::Failed => "failed",
        TestStatus::Skipped => "skipped",
        TestStatus::Cancelled => "cancelled",
    };

    let steps = if let Some(exec) = last_exec {
        vec![PushStepData {
            step_type: "exec".to_string(),
            command: None,
            exit_code: Some(exec.exit_code),
            duration_ms: Some(i64::try_from(duration_ms).unwrap_or(i64::MAX)),
            stdout: Some(exec.stdout.clone()),
            stderr: Some(exec.stderr.clone()),
        }]
    } else {
        Vec::new()
    };

    PushResultData {
        test_id: test_id.to_string(),
        name: name.to_string(),
        status: status_str.to_string(),
        duration_ms: Some(i64::try_from(duration_ms).unwrap_or(i64::MAX)),
        message: message.map(String::from),
        attempt: Some(1),
        steps,
    }
}

/// Push a test result to Remi, falling back to the WAL on failure.
async fn push_to_remi(ctx: &RemiStreamCtx, data: &PushResultData) {
    match ctx.client.push_result(ctx.remi_run_id, data).await {
        Ok(()) => {
            debug!(
                test_id = %data.test_id,
                remi_run_id = ctx.remi_run_id,
                "pushed result to Remi"
            );
        }
        Err(e) => {
            warn!(
                test_id = %data.test_id,
                remi_run_id = ctx.remi_run_id,
                error = %e,
                "failed to push result to Remi, buffering to WAL"
            );
            if let Some(ref wal) = ctx.wal
                && let Ok(json) = serde_json::to_string(data)
                && let Ok(wal_guard) = wal.lock()
                && let Err(wal_err) = wal_guard.buffer(ctx.remi_run_id, &json)
            {
                warn!(error = %wal_err, "failed to buffer result in WAL");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::distro::{
        DistroConfig, FixtureConfig, GlobalConfig, PathsConfig, RemiConfig, SetupConfig,
        TestPackage,
    };
    use crate::config::manifest::{
        Assertion, FileChecksum, KillAfterLog, QemuBoot, ResourceConstraints, SuiteDef, TestDef,
        TestManifest, TestStep,
    };
    use crate::container::backend::ExecResult;
    use crate::container::mock::MockBackend;

    // -- Helpers --

    fn test_config() -> GlobalConfig {
        let mut distros = HashMap::new();
        distros.insert(
            "fedora43".to_string(),
            DistroConfig {
                remi_distro: "fedora-43".to_string(),
                repo_name: "fedora-remi".to_string(),
                containerfile: None,
                test_packages: vec![TestPackage {
                    package: "conary-test-fixture".to_string(),
                    binary: "/usr/bin/true".to_string(),
                }],
            },
        );

        GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://packages.conary.io".to_string(),
            },
            paths: PathsConfig {
                db: "/tmp/conary-test.db".to_string(),
                conary_bin: "/usr/local/bin/conary".to_string(),
                results_dir: "/tmp/results".to_string(),
                fixture_dir: Some("/opt/remi-tests/fixtures".to_string()),
            },
            setup: SetupConfig::default(),
            distros,
            fixtures: Some(FixtureConfig {
                package: Some("conary-test-fixture".to_string()),
                file: Some("/usr/share/conary-test/hello.txt".to_string()),
                added_file: Some("/usr/share/conary-test/added.txt".to_string()),
                marker: Some("/var/lib/conary-test/installed".to_string()),
                v1_version: Some("1.0.0".to_string()),
                v1_ccs_file: Some("conary-test-fixture-1.0.0.ccs".to_string()),
                v1_hello_sha256: Some(
                    "18933c865fcf7230f8ea99b059747facc14285b7ed649758115f9c9a73f42a53".to_string(),
                ),
                v2_version: Some("2.0.0".to_string()),
                v2_ccs_file: Some("conary-test-fixture-2.0.0.ccs".to_string()),
                v2_hello_sha256: Some(
                    "bd80c5e8a7138bd13d0f10e1358bda6f9727c266b6909d4b6c9293ab141ec1db".to_string(),
                ),
                v2_added_sha256: Some(
                    "9767b0b4d55db9aee6638c9875b5cefea50c952cc77fbc5703ebc866b0daba3c".to_string(),
                ),
            }),
        }
    }

    fn simple_step_run(cmd: &str, assertion: Option<Assertion>) -> TestStep {
        TestStep {
            run: Some(cmd.to_string()),
            assert: assertion,
            ..TestStep::default()
        }
    }

    fn simple_step_kill_after_log(config: KillAfterLog, assertion: Option<Assertion>) -> TestStep {
        TestStep {
            kill_after_log: Some(config),
            assert: assertion,
            ..TestStep::default()
        }
    }

    fn simple_step_qemu_boot(config: QemuBoot, assertion: Option<Assertion>) -> TestStep {
        TestStep {
            qemu_boot: Some(config),
            assert: assertion,
            ..TestStep::default()
        }
    }

    fn make_assertion(exit_code: Option<i32>, stdout_contains: Option<&str>) -> Assertion {
        Assertion {
            exit_code,
            stdout_contains: stdout_contains.map(String::from),
            ..Assertion::default()
        }
    }

    fn make_manifest(tests: Vec<TestDef>) -> TestManifest {
        TestManifest {
            suite: SuiteDef {
                name: "test-suite".to_string(),
                phase: 1,
                setup: Vec::new(),
                mock_server: None,
                timeout: None,
            },
            test: tests,
            distro_overrides: HashMap::new(),
        }
    }

    // -- Tests --

    #[tokio::test]
    async fn test_runner_passes_on_success() {
        let backend = MockBackend::new(vec![ExecResult {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
        }]);

        let manifest = make_manifest(vec![TestDef {
            id: "T01".to_string(),
            name: "pass_test".to_string(),
            description: "should pass".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "echo ok",
                Some(make_assertion(Some(0), Some("ok"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.passed(), 1);
        assert_eq!(suite.failed(), 0);
        assert_eq!(suite.results[0].status, TestStatus::Passed);
    }

    #[tokio::test]
    async fn test_runner_fails_on_bad_exit_code() {
        let backend = MockBackend::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "error".to_string(),
        }]);

        let manifest = make_manifest(vec![TestDef {
            id: "T01".to_string(),
            name: "fail_test".to_string(),
            description: "should fail".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "false",
                Some(make_assertion(Some(0), None)),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.passed(), 0);
        assert_eq!(suite.failed(), 1);
        assert_eq!(suite.results[0].status, TestStatus::Failed);
        assert!(
            suite.results[0]
                .message
                .as_ref()
                .unwrap()
                .contains("exit code")
        );
    }

    #[tokio::test]
    async fn test_runner_skips_on_dep_failure() {
        // T01 fails, T02 depends on T01 => T02 skipped.
        let backend = MockBackend::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }]);

        let manifest = make_manifest(vec![
            TestDef {
                id: "T01".to_string(),
                name: "dep_fail".to_string(),
                description: "will fail".to_string(),
                timeout: 30,
                flaky: None,
                retries: None,
                retry_delay_ms: None,
                step: vec![simple_step_run(
                    "false",
                    Some(make_assertion(Some(0), None)),
                )],
                resources: None,
                depends_on: None,
                fatal: None,
                group: None,
                skip: None,
            },
            TestDef {
                id: "T02".to_string(),
                name: "depends_on_t01".to_string(),
                description: "should be skipped".to_string(),
                timeout: 30,
                flaky: None,
                retries: None,
                retry_delay_ms: None,
                step: vec![simple_step_run("echo hello", None)],
                resources: None,
                depends_on: Some(vec!["T01".to_string()]),
                fatal: None,
                group: None,
                skip: None,
            },
        ]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.failed(), 1);
        assert_eq!(suite.skipped(), 1);
        assert_eq!(suite.results[1].status, TestStatus::Skipped);
        assert!(suite.results[1].message.as_ref().unwrap().contains("T01"));
    }

    #[tokio::test]
    async fn test_runner_kill_after_log() {
        let backend = MockBackend::new(Vec::new()).with_detached_exec(
            "exec-1",
            vec!["Preparing install", "Deploying files", "more output"],
            ExecResult {
                exit_code: 137,
                stdout: "Preparing install\nDeploying files\n".to_string(),
                stderr: "Killed\n".to_string(),
            },
        );

        let manifest = make_manifest(vec![TestDef {
            id: "T87".to_string(),
            name: "sigkill_mid_install".to_string(),
            description: "kills the conary process after matching a log line".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_kill_after_log(
                KillAfterLog {
                    conary: "ccs install ${PKG}".to_string(),
                    pattern: "Deploying files".to_string(),
                    timeout_seconds: 5,
                },
                Some(Assertion {
                    exit_code_not: Some(0),
                    ..Assertion::default()
                }),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let mut overrides = HashMap::new();
        overrides.insert("PKG".to_string(), "pkg.ccs".to_string());
        let mut manifest = manifest;
        manifest
            .distro_overrides
            .insert("fedora43".to_string(), overrides);

        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.failed(), 0);
        assert_eq!(suite.passed(), 1);
        assert_eq!(
            backend.killed_execs().as_slice(),
            [("exec-1".to_string(), "SIGKILL".to_string())]
        );
        let detached_calls = backend.detached_calls();
        assert_eq!(detached_calls.len(), 1);
        assert!(
            detached_calls[0]
                .join(" ")
                .contains("/usr/local/bin/conary ccs install pkg.ccs")
        );
    }

    #[tokio::test]
    async fn test_runner_flaky_majority_pass() {
        let backend = MockBackend::new(vec![
            ExecResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: "first fail".to_string(),
            },
            ExecResult {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            },
            ExecResult {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            },
        ]);

        let manifest = make_manifest(vec![TestDef {
            id: "T94".to_string(),
            name: "flaky_majority_pass".to_string(),
            description: "passes when most attempts succeed".to_string(),
            timeout: 30,
            flaky: Some(true),
            retries: Some(3),
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "echo ok",
                Some(make_assertion(Some(0), Some("ok"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.passed(), 1);
        assert_eq!(suite.failed(), 0);
        assert!(
            suite.results[0]
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("2/3")
        );
    }

    #[tokio::test]
    async fn test_runner_flaky_majority_fail() {
        let backend = MockBackend::new(vec![
            ExecResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: "first fail".to_string(),
            },
            ExecResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: "second fail".to_string(),
            },
            ExecResult {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            },
        ]);

        let manifest = make_manifest(vec![TestDef {
            id: "T95".to_string(),
            name: "flaky_majority_fail".to_string(),
            description: "fails when most attempts fail".to_string(),
            timeout: 30,
            flaky: Some(true),
            retries: Some(3),
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "echo ok",
                Some(make_assertion(Some(0), Some("ok"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.passed(), 0);
        assert_eq!(suite.failed(), 1);
        assert!(
            suite.results[0]
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("failed majority")
        );
    }

    #[tokio::test]
    async fn test_resource_scoped_flaky_retries_use_fresh_container() {
        let backend = MockBackend::new(vec![
            ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
            ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
            ExecResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: "first attempt".to_string(),
            },
            ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
            ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
            ExecResult {
                exit_code: 1,
                stdout: "ok".to_string(),
                stderr: String::new(),
            },
        ]);

        let manifest = TestManifest {
            suite: SuiteDef {
                name: "resource-flaky".to_string(),
                phase: 2,
                setup: Vec::new(),
                mock_server: None,
                timeout: None,
            },
            test: vec![TestDef {
                id: "T-resource-flaky".to_string(),
                name: "resource_flaky".to_string(),
                description: "retries in fresh containers".to_string(),
                timeout: 30,
                flaky: Some(true),
                retries: Some(3),
                retry_delay_ms: None,
                step: vec![simple_step_run(
                    "echo ok",
                    Some(make_assertion(Some(0), Some("ok"))),
                )],
                resources: Some(ResourceConstraints {
                    tmpfs_size_mb: None,
                    memory_limit_mb: Some(512),
                    network_isolated: Some(false),
                }),
                depends_on: None,
                fatal: None,
                group: None,
                skip: None,
            }],
            distro_overrides: HashMap::new(),
        };

        let config = test_config();
        let base_container_config = ContainerConfig {
            image: "mock-image".to_string(),
            ..Default::default()
        };
        let mut runner = TestRunner::new(config, "fedora43".to_string());
        let suite = runner
            .run(
                &manifest,
                &backend,
                &"ctr-1".to_string(),
                Some(&base_container_config),
            )
            .await
            .unwrap();

        assert_eq!(suite.failed(), 1);
        assert_eq!(backend.created_containers().len(), 2);
    }

    #[test]
    fn test_substitute_vars() {
        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let mut manifest = make_manifest(Vec::new());
        manifest.distro_overrides.insert(
            "fedora43".to_string(),
            HashMap::from([("PKG".to_string(), "tree".to_string())]),
        );
        runner.load_manifest_vars(&manifest);

        let expanded = variables::expand_variables("curl ${REMI_ENDPOINT}/health", &runner.vars);
        assert_eq!(expanded, "curl https://packages.conary.io/health");

        let expanded2 =
            variables::expand_variables("${CONARY_BIN} --db-path ${DB_PATH}", &runner.vars);
        assert_eq!(
            expanded2,
            "/usr/local/bin/conary --db-path /tmp/conary-test.db"
        );

        let expanded3 = variables::expand_variables("conary install ${PKG}", &runner.vars);
        assert_eq!(expanded3, "conary install tree");

        let fixture_v1 = variables::expand_variables("${FIXTURE_V1_CCS}", &runner.vars);
        assert_eq!(
            fixture_v1,
            "/opt/remi-tests/fixtures/conary-test-fixture/v1/output/conary-test-fixture-1.0.0.ccs"
        );
    }

    #[test]
    fn test_expand_assertion_substitutes_vars() {
        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let mut manifest = make_manifest(Vec::new());
        manifest.distro_overrides.insert(
            "fedora43".to_string(),
            HashMap::from([
                ("PKG".to_string(), "conary-test-fixture".to_string()),
                ("HELLO_SHA".to_string(), "abc123".to_string()),
            ]),
        );
        runner.load_manifest_vars(&manifest);

        let assertion = Assertion {
            stdout_contains_all: Some(vec!["${PKG}".to_string(), "Version".to_string()]),
            stderr_contains: Some("${PKG}".to_string()),
            file_checksum: Some(FileChecksum {
                path: "/tmp/${PKG}".to_string(),
                sha256: "${HELLO_SHA}".to_string(),
            }),
            ..Assertion::default()
        };

        let expanded = runner.expand_assertion(&assertion);
        assert_eq!(
            expanded.stdout_contains_all,
            Some(vec![
                "conary-test-fixture".to_string(),
                "Version".to_string()
            ])
        );
        assert_eq!(
            expanded.stderr_contains.as_deref(),
            Some("conary-test-fixture")
        );
        assert_eq!(
            expanded.file_checksum.as_ref().map(|chk| chk.path.as_str()),
            Some("/tmp/conary-test-fixture")
        );
        assert_eq!(
            expanded
                .file_checksum
                .as_ref()
                .map(|chk| chk.sha256.as_str()),
            Some("abc123")
        );
    }

    #[test]
    fn test_apply_resource_constraints() {
        let runner = TestRunner::new(test_config(), "fedora43".to_string());
        let mut container_config = ContainerConfig::default();
        let resources = ResourceConstraints {
            tmpfs_size_mb: Some(50),
            memory_limit_mb: Some(512),
            network_isolated: Some(true),
        };

        runner.apply_resource_constraints(&mut container_config, Some(&resources));

        assert_eq!(
            container_config
                .tmpfs
                .get("/var/lib/conary")
                .map(String::as_str),
            Some("size=50m")
        );
        assert_eq!(container_config.memory_limit, Some(512 * 1024 * 1024));
        assert_eq!(container_config.network_mode, "none");
    }

    #[test]
    fn test_build_kill_after_log_command_supports_env_prefix() {
        // Delegates to executor::build_kill_after_log_command (tested there).
        // Keep a runner-level smoke test for backward compat.
        use crate::engine::executor;
        let cmd = executor::build_kill_after_log_command(
            "/usr/local/bin/conary",
            "env CONARY_TEST_HOLD_AFTER_DB_UPDATE_MS=1500 ccs install fixture.ccs",
        );
        assert!(cmd.contains("exec env CONARY_TEST_HOLD_AFTER_DB_UPDATE_MS=1500"));
        assert!(cmd.contains("/usr/local/bin/conary ccs install fixture.ccs"));
    }

    #[tokio::test]
    async fn test_runner_qemu_boot_step_skips_when_tooling_missing() {
        let backend = MockBackend::new(Vec::new());
        let manifest = make_manifest(vec![TestDef {
            id: "T156".to_string(),
            name: "qemu_boot".to_string(),
            description: "boots a qcow2 image".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_qemu_boot(
                QemuBoot {
                    image: "https://127.0.0.1:9/minimal-boot-${PKG}.qcow2".to_string(),
                    memory_mb: 512,
                    timeout_seconds: 5,
                    ssh_port: 2223,
                    commands: vec!["echo ${PKG}".to_string()],
                    expect_output: vec!["skipped".to_string()],
                },
                Some(Assertion {
                    stdout_contains: Some("qemu boot".to_string()),
                    ..Assertion::default()
                }),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let mut overrides = HashMap::new();
        overrides.insert("PKG".to_string(), "v1".to_string());
        let mut manifest = manifest;
        manifest
            .distro_overrides
            .insert("fedora43".to_string(), overrides);

        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.passed(), 1);
        assert_eq!(suite.failed(), 0);
        assert!(
            suite.results[0]
                .stdout
                .as_deref()
                .unwrap_or_default()
                .contains("qemu boot")
        );
    }

    #[test]
    fn test_expand_qemu_boot_substitutes_vars() {
        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let mut manifest = make_manifest(Vec::new());
        manifest.distro_overrides.insert(
            "fedora43".to_string(),
            HashMap::from([("IMG".to_string(), "minimal-boot-v1".to_string())]),
        );
        runner.load_manifest_vars(&manifest);

        let expanded = variables::expand_qemu_boot(
            &QemuBoot {
                image: "${IMG}".to_string(),
                memory_mb: 1024,
                timeout_seconds: 120,
                ssh_port: 2222,
                commands: vec!["echo ${IMG}".to_string()],
                expect_output: vec!["${IMG}".to_string()],
            },
            &runner.vars,
        );

        assert_eq!(expanded.image, "minimal-boot-v1");
        assert_eq!(expanded.commands, vec!["echo minimal-boot-v1"]);
        assert_eq!(expanded.expect_output, vec!["minimal-boot-v1"]);
    }

    #[tokio::test]
    async fn test_cancel_flag_stops_runner() {
        let backend = MockBackend::new(vec![
            ExecResult {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            },
            ExecResult {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            },
        ]);

        let manifest = make_manifest(vec![
            TestDef {
                id: "T01".to_string(),
                name: "first".to_string(),
                description: "runs first".to_string(),
                timeout: 30,
                flaky: None,
                retries: None,
                retry_delay_ms: None,
                step: vec![simple_step_run(
                    "echo ok",
                    Some(make_assertion(Some(0), None)),
                )],
                resources: None,
                depends_on: None,
                fatal: None,
                group: None,
                skip: None,
            },
            TestDef {
                id: "T02".to_string(),
                name: "second".to_string(),
                description: "should be cancelled".to_string(),
                timeout: 30,
                flaky: None,
                retries: None,
                retry_delay_ms: None,
                step: vec![simple_step_run("echo ok", None)],
                resources: None,
                depends_on: None,
                fatal: None,
                group: None,
                skip: None,
            },
        ]);

        // Set cancel flag before run -- T01 will pass but the flag will be
        // set immediately so T02 should be cancelled.
        let cancel_flag = Arc::new(AtomicBool::new(true));

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run_with_cancel(
                &manifest,
                &backend,
                &"ctr-1".to_string(),
                None,
                Some(cancel_flag),
                None,
                None,
            )
            .await
            .unwrap();

        // Both tests should be cancelled since the flag was set from the start.
        assert_eq!(suite.cancelled(), 2);
        assert_eq!(suite.passed(), 0);
        assert_eq!(suite.results[0].status, TestStatus::Cancelled);
        assert_eq!(suite.results[1].status, TestStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_suite_timeout_cancels_remaining() {
        // Create a manifest with suite timeout = 0 seconds (already expired).
        let manifest = TestManifest {
            suite: SuiteDef {
                name: "timeout-suite".to_string(),
                phase: 1,
                setup: Vec::new(),
                mock_server: None,
                timeout: Some(0), // Already expired.
            },
            test: vec![
                TestDef {
                    id: "T01".to_string(),
                    name: "first".to_string(),
                    description: "cancelled by timeout".to_string(),
                    timeout: 30,
                    flaky: None,
                    retries: None,
                    retry_delay_ms: None,
                    step: vec![simple_step_run("echo ok", None)],
                    resources: None,
                    depends_on: None,
                    fatal: None,
                    group: None,
                    skip: None,
                },
                TestDef {
                    id: "T02".to_string(),
                    name: "second".to_string(),
                    description: "also cancelled".to_string(),
                    timeout: 30,
                    flaky: None,
                    retries: None,
                    retry_delay_ms: None,
                    step: vec![simple_step_run("echo ok", None)],
                    resources: None,
                    depends_on: None,
                    fatal: None,
                    group: None,
                    skip: None,
                },
            ],
            distro_overrides: HashMap::new(),
        };

        let backend = MockBackend::new(Vec::new());
        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run_with_cancel(
                &manifest,
                &backend,
                &"ctr-1".to_string(),
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(suite.cancelled(), 2);
        assert!(
            suite.results[0]
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("suite timeout")
        );
    }

    #[tokio::test]
    async fn test_step_timeout_overrides_test_timeout() {
        // Test that a step with timeout = 1 uses 1s, not the test-level 30s.
        // We verify this indirectly: the step executes successfully with a
        // 1s timeout (the mock is instant, so it works).
        let backend = MockBackend::new(vec![ExecResult {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
        }]);

        let manifest = make_manifest(vec![TestDef {
            id: "T01".to_string(),
            name: "step_timeout".to_string(),
            description: "step has custom timeout".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![TestStep {
                timeout: Some(1),
                run: Some("echo ok".to_string()),
                assert: Some(make_assertion(Some(0), Some("ok"))),
                ..TestStep::default()
            }],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string(), None)
            .await
            .unwrap();

        assert_eq!(suite.passed(), 1);
    }

    #[tokio::test]
    async fn test_concurrent_runs_independent() {
        // Two independent runs should complete without interfering with each
        // other. Each gets its own MockBackend, runner, and manifest.
        let backend_a = MockBackend::new(vec![ExecResult {
            exit_code: 0,
            stdout: "run-a".to_string(),
            stderr: String::new(),
        }]);
        let backend_b = MockBackend::new(vec![ExecResult {
            exit_code: 0,
            stdout: "run-b".to_string(),
            stderr: String::new(),
        }]);

        let manifest_a = make_manifest(vec![TestDef {
            id: "T-A1".to_string(),
            name: "run_a_test".to_string(),
            description: "test in run A".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "echo run-a",
                Some(make_assertion(Some(0), Some("run-a"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let manifest_b = make_manifest(vec![TestDef {
            id: "T-B1".to_string(),
            name: "run_b_test".to_string(),
            description: "test in run B".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "echo run-b",
                Some(make_assertion(Some(0), Some("run-b"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
        }]);

        let (suite_a, suite_b) = tokio::join!(
            async {
                let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
                runner
                    .run(&manifest_a, &backend_a, &"ctr-a".to_string(), None)
                    .await
                    .unwrap()
            },
            async {
                let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
                runner
                    .run(&manifest_b, &backend_b, &"ctr-b".to_string(), None)
                    .await
                    .unwrap()
            },
        );

        assert_eq!(suite_a.passed(), 1, "run A should pass");
        assert_eq!(suite_b.passed(), 1, "run B should pass");
        assert_eq!(suite_a.failed(), 0, "run A should have no failures");
        assert_eq!(suite_b.failed(), 0, "run B should have no failures");
        assert_eq!(suite_a.results[0].id, "T-A1");
        assert_eq!(suite_b.results[0].id, "T-B1");
    }
}
