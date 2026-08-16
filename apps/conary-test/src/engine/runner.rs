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
use crate::remi_client::{PushResultData, PushStepData, RemiClient};
use crate::report::stream::TestEvent;
use crate::wal::Wal;

/// Context for streaming test results to the Remi admin API.
///
/// When provided to `run_with_cancel`, each completed test is pushed to Remi
/// as it finishes. On push failure, the payload is buffered to the WAL for
/// retry.
pub struct RemiStreamCtx {
    /// Remi run ID returned by `create_run`.
    pub remi_run_id: i64,
    pub client: Arc<RemiClient>,
    pub wal: Option<Arc<tokio::sync::Mutex<Wal>>>,
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

        match status {
            TestStatus::Passed => {
                pass_count += 1;
            }
            TestStatus::Skipped => {
                return Ok((TestStatus::Skipped, message, total_elapsed, last_exec));
            }
            TestStatus::Failed | TestStatus::Cancelled => {
                fail_count += 1;
                last_failure = message;
            }
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

        self.run_setup_steps(manifest, backend, container_id)
            .await?;

        let mut suite = TestSuite::new(&manifest.suite.name, manifest.suite.phase);
        suite.expect_corpus_cases(
            manifest
                .test
                .iter()
                .filter(|test| test.corpus.is_some())
                .count(),
        );
        suite.expect_corpus_coverage(
            manifest
                .suite
                .corpus
                .iter()
                .flat_map(|corpus| corpus.required.iter().copied()),
        );
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
                self.record_unrun_corpus(&mut suite, test_def, "cancelled");
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
                self.record_unrun_corpus(&mut suite, test_def, "suite timeout exceeded");
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
                    message: Some(msg.clone()),
                    stdout: None,
                    stderr: None,
                    attempts: Vec::new(),
                });
                self.record_unrun_corpus(&mut suite, test_def, &msg);
                continue;
            }

            if let Some(reason) = self
                .missing_runtime_requirement(test_def, backend, container_id)
                .await?
            {
                let msg = format!("skipped: {reason}");
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
                self.record_unrun_corpus(&mut suite, test_def, &msg);
                if let Some((run_id, ref tx)) = event_tx {
                    let _ = tx.send(TestEvent::TestSkipped {
                        run_id,
                        test_id: test_def.id.clone(),
                        message: msg,
                    });
                }
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
                self.record_unrun_corpus(&mut suite, test_def, &msg);
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

            if let Some(corpus) = &test_def.corpus {
                let corpus = variables::expand_corpus_case(corpus, &self.vars);
                let corpus_result = crate::engine::corpus::capture_case(
                    &corpus,
                    &test_def.id,
                    &self.distro,
                    status,
                    message.as_deref(),
                    backend,
                    container_id,
                )
                .await;
                suite.record_corpus(corpus_result);
            }

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

    fn record_unrun_corpus(&self, suite: &mut TestSuite, test_def: &TestDef, reason: &str) {
        if let Some(corpus) = &test_def.corpus {
            let corpus = variables::expand_corpus_case(corpus, &self.vars);
            suite.record_corpus(crate::engine::corpus::case_did_not_run(
                &corpus,
                &test_def.id,
                &self.distro,
                reason,
            ));
        }
    }

    async fn run_setup_steps(
        &self,
        manifest: &TestManifest,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<()> {
        let ctx = ExecutionContext {
            conary_bin: &self.config.paths.conary_bin,
            db_path: &self.config.paths.db,
        };

        for step in &manifest.suite.setup {
            let action = StepAction::from_step(step, &self.vars).ok_or_else(|| {
                anyhow::anyhow!("suite setup failed: suite setup step has no recognized type")
            })?;
            let timeout = Duration::from_secs(step.timeout.unwrap_or(300));
            let result = execute_step(&action, backend, container_id, &ctx, timeout)
                .await
                .map_err(|err| anyhow::anyhow!("suite setup failed: {err}"))?;

            if let Some(msg) = &result.failure {
                bail!("suite setup failed: {msg}");
            }

            if let Some(ref assertion) = step.assert {
                let assertion = self.expand_assertion(assertion);
                evaluate_assertion(&assertion, result.exit_code, &result.stdout, &result.stderr)
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "suite setup assertion failed: {err}\nstdout:\n{}\nstderr:\n{}",
                            result.stdout,
                            result.stderr
                        )
                    })?;
            }
        }

        Ok(())
    }

    async fn missing_runtime_requirement(
        &self,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<Option<String>> {
        for requirement in &test_def.requires {
            match requirement.as_str() {
                "composefs_runtime" => {
                    if !self
                        .composefs_runtime_available(backend, container_id)
                        .await?
                    {
                        return Ok(Some(
                            "missing composefs runtime support (overlayfs, EROFS, loop devices, or mount.composefs)"
                                .to_string(),
                        ));
                    }
                }
                other => bail!(
                    "test {} has unknown runtime requirement `{}`",
                    test_def.id,
                    other
                ),
            }
        }

        Ok(None)
    }

    async fn composefs_runtime_available(
        &self,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<bool> {
        let probe = "grep -qw erofs /proc/filesystems && \
             grep -qw overlay /proc/filesystems && \
             test -e /dev/loop-control && \
             command -v mount.composefs >/dev/null 2>&1";
        let result = backend
            .exec(container_id, &["sh", "-c", probe], Duration::from_secs(10))
            .await?;

        Ok(result.exit_code == 0)
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
        let mut skipped: Option<String> = None;

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

            if matches!(action, StepAction::QemuBoot(_))
                && crate::engine::qemu::is_skip_exit_code(step_result.exit_code)
            {
                let message = step_result
                    .stdout
                    .lines()
                    .next()
                    .filter(|line| !line.trim().is_empty())
                    .or_else(|| {
                        step_result
                            .stderr
                            .lines()
                            .next()
                            .filter(|line| !line.trim().is_empty())
                    })
                    .unwrap_or("qemu boot skipped")
                    .to_string();
                skipped = Some(message);
                break;
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
        let (status, message) = match (failure, skipped) {
            (Some(msg), _) => (TestStatus::Failed, Some(msg)),
            (None, Some(msg)) => (TestStatus::Skipped, Some(msg)),
            (None, None) => (TestStatus::Passed, None),
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
            if let Some(ref wal) = ctx.wal {
                match serde_json::to_string(data) {
                    Ok(json) => {
                        let wal_guard = wal.lock().await;
                        if let Err(wal_err) = wal_guard.buffer(ctx.remi_run_id, &json) {
                            warn!(error = %wal_err, "failed to buffer result in WAL");
                        }
                    }
                    Err(json_error) => {
                        warn!(error = %json_error, "failed to serialize result for WAL");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
