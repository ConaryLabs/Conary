// conary-test/src/engine/runner.rs

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::time::Instant;
use tracing::{info, warn};

use crate::config::distro::GlobalConfig;
use crate::config::manifest::{
    Assertion, FileChecksum, KillAfterLog, QemuBoot, ResourceConstraints, StepType, TestDef,
    TestManifest,
};
use crate::container::backend::{ContainerBackend, ContainerConfig, ContainerId, ExecResult};
use crate::engine::assertions::evaluate_assertion;
use crate::engine::mock_server::start_mock_server;
use crate::engine::qemu::run_qemu_boot;
use crate::engine::suite::{TestResult, TestStatus, TestSuite};

/// Executes tests from a manifest against a container.
pub struct TestRunner {
    pub config: GlobalConfig,
    pub distro: String,
    vars: HashMap<String, String>,
}

impl TestRunner {
    fn build_kill_after_log_command(&self, expanded: &str) -> String {
        if let Some(rest) = expanded.strip_prefix("env ") {
            let mut env_vars = Vec::new();
            let mut conary_args = Vec::new();
            let mut parsing_env = true;

            for token in rest.split_whitespace() {
                if parsing_env && token.contains('=') {
                    env_vars.push(token);
                } else {
                    parsing_env = false;
                    conary_args.push(token);
                }
            }

            if !env_vars.is_empty() && !conary_args.is_empty() {
                return format!(
                    "printf '__CONARY_TEST_PID__=%s\\n' \"$$\"; exec env {} {} {}",
                    env_vars.join(" "),
                    self.config.paths.conary_bin,
                    conary_args.join(" ")
                );
            }
        }

        format!(
            "printf '__CONARY_TEST_PID__=%s\\n' \"$$\"; exec {} {}",
            self.config.paths.conary_bin, expanded
        )
    }

    pub fn new(config: GlobalConfig, distro: String) -> Self {
        let mut vars = HashMap::new();
        vars.insert("REMI_ENDPOINT".to_string(), config.remi.endpoint.clone());
        vars.insert("DB_PATH".to_string(), config.paths.db.clone());
        vars.insert("CONARY_BIN".to_string(), config.paths.conary_bin.clone());
        if let Some(fixture_dir) = &config.paths.fixture_dir {
            vars.insert("FIXTURE_DIR".to_string(), fixture_dir.clone());
        }

        if let Some(fixtures) = &config.fixtures {
            if let Some(value) = &fixtures.package {
                vars.insert("FIXTURE_PKG_NAME".to_string(), value.clone());
            }
            if let Some(value) = &fixtures.file {
                vars.insert("FIXTURE_FILE".to_string(), value.clone());
            }
            if let Some(value) = &fixtures.added_file {
                vars.insert("FIXTURE_ADDED_FILE".to_string(), value.clone());
            }
            if let Some(value) = &fixtures.marker {
                vars.insert("FIXTURE_MARKER".to_string(), value.clone());
            }
            if let Some(fixture_dir) = &config.paths.fixture_dir {
                if let Some(value) = &fixtures.v1_ccs_file {
                    vars.insert(
                        "FIXTURE_V1_CCS".to_string(),
                        format!("{fixture_dir}/conary-test-fixture/v1/output/{value}"),
                    );
                }
                if let Some(value) = &fixtures.v2_ccs_file {
                    vars.insert(
                        "FIXTURE_V2_CCS".to_string(),
                        format!("{fixture_dir}/conary-test-fixture/v2/output/{value}"),
                    );
                }
            }
            if let Some(value) = &fixtures.v1_hello_sha256 {
                vars.insert("FIXTURE_V1_HELLO_SHA256".to_string(), value.clone());
            }
            if let Some(value) = &fixtures.v2_hello_sha256 {
                vars.insert("FIXTURE_V2_HELLO_SHA256".to_string(), value.clone());
            }
            if let Some(value) = &fixtures.v2_added_sha256 {
                vars.insert("FIXTURE_V2_ADDED_SHA256".to_string(), value.clone());
            }
        }

        // Add distro-specific variables if present.
        if let Some(dc) = config.distros.get(&distro) {
            vars.insert("REMI_DISTRO".to_string(), dc.remi_distro.clone());
            vars.insert("REPO_NAME".to_string(), dc.repo_name.clone());
            for (i, tp) in dc.test_packages.iter().enumerate() {
                let n = i + 1;
                vars.insert(format!("TEST_PACKAGE_{n}"), tp.package.clone());
                vars.insert(format!("TEST_BINARY_{n}"), tp.binary.clone());
            }
        }

        Self {
            config,
            distro,
            vars,
        }
    }

    /// Load distro-specific manifest variables into the runner variable map.
    pub fn load_manifest_vars(&mut self, manifest: &TestManifest) {
        if let Some(overrides) = manifest.distro_overrides.get(&self.distro) {
            self.vars.extend(overrides.clone());
        }
    }

    /// Run all tests in the manifest against the given container.
    pub async fn run(
        &mut self,
        manifest: &TestManifest,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
        base_container_config: Option<&ContainerConfig>,
    ) -> Result<TestSuite> {
        self.load_manifest_vars(manifest);

        if let Some(mock_server) = &manifest.suite.mock_server {
            start_mock_server(backend, container_id, mock_server).await?;
        }

        let mut suite = TestSuite::new(&manifest.suite.name, manifest.suite.phase);
        suite.status = crate::engine::suite::RunStatus::Running;

        for test_def in &manifest.test {
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
                    message: Some(msg),
                    stdout: None,
                    stderr: None,
                });
                continue;
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

            suite.record(TestResult {
                id: test_def.id.clone(),
                name: test_def.name.clone(),
                status,
                duration_ms: elapsed,
                message,
                stdout: last_exec.as_ref().map(|e| e.stdout.clone()),
                stderr: last_exec.as_ref().map(|e| e.stderr.clone()),
            });

            // Fatal test: stop the entire suite on failure.
            if status == TestStatus::Failed && test_def.fatal.unwrap_or(false) {
                warn!("[{}] fatal test failed, stopping suite", test_def.id);
                break;
            }
        }

        suite.finish();
        Ok(suite)
    }

    async fn run_resource_scoped_test(
        &self,
        manifest: &TestManifest,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        base_container_config: &ContainerConfig,
    ) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)> {
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
            let (status, message, elapsed, exec) = self
                .run_resource_scoped_test_once(manifest, test_def, backend, base_container_config)
                .await?;
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

    async fn run_resource_scoped_test_once(
        &self,
        manifest: &TestManifest,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        base_container_config: &ContainerConfig,
    ) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)> {
        let mut container_config = base_container_config.clone();
        self.apply_resource_constraints(&mut container_config, test_def.resources.as_ref());

        let container_id = backend.create(container_config).await?;
        backend.start(&container_id).await?;

        let result = async {
            self.initialize_container_state(backend, &container_id, manifest.suite.phase)
                .await?;
            if let Some(mock_server) = &manifest.suite.mock_server {
                start_mock_server(backend, &container_id, mock_server).await?;
            }
            self.run_test_once(test_def, backend, &container_id).await
        }
        .await;

        if let Err(err) = backend.stop(&container_id).await {
            warn!(test = %test_def.id, error = %err, "failed to stop resource-scoped container");
        }
        if let Err(err) = backend.remove(&container_id).await {
            warn!(test = %test_def.id, error = %err, "failed to remove resource-scoped container");
        }

        result
    }

    async fn initialize_container_state(
        &self,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
        phase: u32,
    ) -> Result<()> {
        let db_parent = std::path::Path::new(&self.config.paths.db)
            .parent()
            .context("db path has no parent directory")?
            .display()
            .to_string();
        let init_cmd = format!(
            "mkdir -p {db_parent} && {} system init --db-path {}",
            self.config.paths.conary_bin, self.config.paths.db
        );
        let init_result = backend
            .exec(
                container_id,
                &["sh", "-c", &init_cmd],
                Duration::from_secs(120),
            )
            .await?;
        if init_result.exit_code != 0 {
            bail!(
                "failed to initialize conary database: {}{}",
                init_result.stdout,
                init_result.stderr
            );
        }

        for repo in &self.config.setup.remove_default_repos {
            let remove_cmd = format!(
                "{} repo remove {} --db-path {} >/dev/null 2>&1 || true",
                self.config.paths.conary_bin, repo, self.config.paths.db
            );
            backend
                .exec(
                    container_id,
                    &["sh", "-c", &remove_cmd],
                    Duration::from_secs(30),
                )
                .await?;
        }

        if phase > 1 {
            let distro_config = self
                .config
                .distros
                .get(&self.distro)
                .with_context(|| format!("unknown distro: {}", self.distro))?;
            let add_repo_cmd = format!(
                "{} repo add {} {} --default-strategy remi --remi-endpoint {} --remi-distro {} --no-gpg-check --db-path {} >/dev/null 2>&1 || true",
                self.config.paths.conary_bin,
                distro_config.repo_name,
                self.config.remi.endpoint,
                self.config.remi.endpoint,
                distro_config.remi_distro,
                self.config.paths.db
            );
            backend
                .exec(
                    container_id,
                    &["sh", "-c", &add_repo_cmd],
                    Duration::from_secs(60),
                )
                .await?;
        }

        Ok(())
    }

    async fn run_test_attempt(
        &self,
        test_def: &TestDef,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<(TestStatus, Option<String>, u64, Option<ExecResult>)> {
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
            let (status, message, elapsed, exec) =
                self.run_test_once(test_def, backend, container_id).await?;
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

        for step in &test_def.step {
            let step_type = match step.step_type() {
                Some(st) => st,
                None => {
                    failure = Some("step has no recognized type".to_string());
                    break;
                }
            };

            match step_type {
                StepType::Sleep(secs) => {
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
                StepType::Run(cmd) => {
                    let expanded = self.substitute_vars(&cmd);
                    let result = backend
                        .exec(container_id, &["sh", "-c", &expanded], timeout)
                        .await?;
                    last_exec = Some(result);
                }
                StepType::Conary(args) => {
                    let expanded = self.substitute_vars(&args);
                    let full_cmd = format!(
                        "{} {} --db-path {}",
                        self.config.paths.conary_bin, expanded, self.config.paths.db
                    );
                    let result = backend
                        .exec(container_id, &["sh", "-c", &full_cmd], timeout)
                        .await?;
                    last_exec = Some(result);
                }
                StepType::FileExists(path) => {
                    let expanded = self.substitute_vars(&path);
                    let result = backend
                        .exec(container_id, &["test", "-e", &expanded], timeout)
                        .await?;
                    if result.exit_code != 0 {
                        failure = Some(format!("file does not exist: {expanded}"));
                        last_exec = Some(result);
                        break;
                    }
                    last_exec = Some(result);
                }
                StepType::FileNotExists(path) => {
                    let expanded = self.substitute_vars(&path);
                    let result = backend
                        .exec(container_id, &["test", "!", "-e", &expanded], timeout)
                        .await?;
                    if result.exit_code != 0 {
                        failure = Some(format!("file unexpectedly exists: {expanded}"));
                        last_exec = Some(result);
                        break;
                    }
                    last_exec = Some(result);
                }
                StepType::FileExecutable(path) => {
                    let expanded = self.substitute_vars(&path);
                    let result = backend
                        .exec(container_id, &["test", "-x", &expanded], timeout)
                        .await?;
                    if result.exit_code != 0 {
                        failure = Some(format!("file is not executable: {expanded}"));
                        last_exec = Some(result);
                        break;
                    }
                    last_exec = Some(result);
                }
                StepType::DirExists(path) => {
                    let expanded = self.substitute_vars(&path);
                    let result = backend
                        .exec(container_id, &["test", "-d", &expanded], timeout)
                        .await?;
                    if result.exit_code != 0 {
                        failure = Some(format!("directory does not exist: {expanded}"));
                        last_exec = Some(result);
                        break;
                    }
                    last_exec = Some(result);
                }
                StepType::FileChecksum(chk) => {
                    let expanded_path = self.substitute_vars(&chk.path);
                    let expected_hash = self.substitute_vars(&chk.sha256);
                    let cmd = format!("sha256sum {expanded_path}");
                    let result = backend
                        .exec(container_id, &["sh", "-c", &cmd], timeout)
                        .await?;
                    if result.exit_code != 0 {
                        failure = Some(format!(
                            "sha256sum failed on {expanded_path}: {}",
                            result.stderr.trim()
                        ));
                        last_exec = Some(result);
                        break;
                    }
                    let actual_hash = result
                        .stdout
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if actual_hash != expected_hash {
                        failure = Some(format!(
                            "checksum mismatch for {expanded_path}: expected {}, got {actual_hash}",
                            expected_hash
                        ));
                        last_exec = Some(result);
                        break;
                    }
                    last_exec = Some(result);
                }
                StepType::KillAfterLog(config) => {
                    let result = self
                        .run_kill_after_log(backend, container_id, &config)
                        .await?;
                    last_exec = Some(result);
                }
                StepType::QemuBoot(config) => {
                    let result = run_qemu_boot(&self.expand_qemu_boot(&config)).await?;
                    last_exec = Some(result);
                }
            }

            if let Some(ref assertion) = step.assert {
                let exec = last_exec.as_ref().expect("assertion without exec result");
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

    async fn run_kill_after_log(
        &self,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
        config: &KillAfterLog,
    ) -> Result<ExecResult> {
        let expanded = self.substitute_vars(&config.conary);
        let full_cmd = self.build_kill_after_log_command(&expanded);
        let exec_id = backend
            .exec_detached(container_id, &["sh", "-lc", &full_cmd])
            .await?;
        let mut logs = backend.exec_logs(&exec_id).await?;
        let timeout = Duration::from_secs(config.timeout_seconds);

        let matched = tokio::time::timeout(timeout, async {
            while let Some(line) = logs.recv().await {
                if line.contains(&config.pattern) {
                    return Ok::<bool, anyhow::Error>(true);
                }
            }
            Ok(false)
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for log pattern {:?} after {}s",
                config.pattern,
                config.timeout_seconds
            )
        })??;

        if !matched {
            let result = backend.exec_result(&exec_id).await?;
            bail!(
                "log stream ended before pattern {:?} appeared; stdout: {}; stderr: {}",
                config.pattern,
                result.stdout.trim(),
                result.stderr.trim()
            );
        }

        backend.kill_exec(&exec_id, "SIGKILL").await?;
        backend.exec_result(&exec_id).await
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

    /// Replace `${VAR}` patterns in a string with values from the variable map.
    fn substitute_vars(&self, input: &str) -> String {
        if !input.contains("${") {
            return input.to_string();
        }
        let mut result = input.to_string();
        for (key, value) in &self.vars {
            let pattern = format!("${{{key}}}");
            result = result.replace(&pattern, value);
        }
        result
    }

    fn expand_qemu_boot(&self, config: &QemuBoot) -> QemuBoot {
        QemuBoot {
            image: self.substitute_vars(&config.image),
            memory_mb: config.memory_mb,
            timeout_seconds: config.timeout_seconds,
            ssh_port: config.ssh_port,
            commands: config
                .commands
                .iter()
                .map(|cmd| self.substitute_vars(cmd))
                .collect(),
            expect_output: config
                .expect_output
                .iter()
                .map(|s| self.substitute_vars(s))
                .collect(),
        }
    }

    fn expand_assertion(&self, assertion: &Assertion) -> Assertion {
        Assertion {
            exit_code: assertion.exit_code,
            exit_code_not: assertion.exit_code_not,
            stdout_contains: assertion
                .stdout_contains
                .as_ref()
                .map(|value| self.substitute_vars(value)),
            stdout_not_contains: assertion
                .stdout_not_contains
                .as_ref()
                .map(|value| self.substitute_vars(value)),
            stdout_contains_all: assertion.stdout_contains_all.as_ref().map(|values| {
                values
                    .iter()
                    .map(|value| self.substitute_vars(value))
                    .collect()
            }),
            stdout_contains_any: assertion.stdout_contains_any.as_ref().map(|values| {
                values
                    .iter()
                    .map(|value| self.substitute_vars(value))
                    .collect()
            }),
            stdout_contains_if_success: assertion
                .stdout_contains_if_success
                .as_ref()
                .map(|value| self.substitute_vars(value)),
            stdout_contains_any_if_success: assertion.stdout_contains_any_if_success.as_ref().map(
                |values| {
                    values
                        .iter()
                        .map(|value| self.substitute_vars(value))
                        .collect()
                },
            ),
            stderr_contains: assertion
                .stderr_contains
                .as_ref()
                .map(|value| self.substitute_vars(value)),
            file_exists: assertion
                .file_exists
                .as_ref()
                .map(|value| self.substitute_vars(value)),
            file_not_exists: assertion
                .file_not_exists
                .as_ref()
                .map(|value| self.substitute_vars(value)),
            file_checksum: assertion
                .file_checksum
                .as_ref()
                .map(|checksum| FileChecksum {
                    path: self.substitute_vars(&checksum.path),
                    sha256: self.substitute_vars(&checksum.sha256),
                }),
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
        Assertion, KillAfterLog, QemuBoot, ResourceConstraints, SuiteDef, TestDef, TestManifest,
        TestStep,
    };
    use crate::container::backend::{ContainerConfig, ExecResult};
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    // -- Mock backend --

    struct MockBackend {
        exec_calls: Mutex<Vec<Vec<String>>>,
        exec_results: Mutex<Vec<ExecResult>>,
        created_containers: Mutex<Vec<ContainerConfig>>,
        detached_calls: Mutex<Vec<Vec<String>>>,
        log_sequences: Mutex<HashMap<String, Vec<String>>>,
        detached_results: Mutex<HashMap<String, ExecResult>>,
        killed_execs: Mutex<Vec<(String, String)>>,
    }

    impl MockBackend {
        fn new(results: Vec<ExecResult>) -> Self {
            Self {
                exec_calls: Mutex::new(Vec::new()),
                exec_results: Mutex::new(results),
                created_containers: Mutex::new(Vec::new()),
                detached_calls: Mutex::new(Vec::new()),
                log_sequences: Mutex::new(HashMap::new()),
                detached_results: Mutex::new(HashMap::new()),
                killed_execs: Mutex::new(Vec::new()),
            }
        }

        fn with_detached_exec(self, exec_id: &str, logs: Vec<&str>, result: ExecResult) -> Self {
            self.log_sequences.lock().unwrap().insert(
                exec_id.to_string(),
                logs.into_iter().map(String::from).collect(),
            );
            self.detached_results
                .lock()
                .unwrap()
                .insert(exec_id.to_string(), result);
            self
        }

        #[allow(dead_code)]
        fn calls(&self) -> Vec<Vec<String>> {
            self.exec_calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ContainerBackend for MockBackend {
        async fn build_image(
            &self,
            _dockerfile: &Path,
            _tag: &str,
            _build_args: HashMap<String, String>,
        ) -> Result<String> {
            Ok("mock-image".to_string())
        }

        async fn create(&self, _config: ContainerConfig) -> Result<ContainerId> {
            let mut created = self.created_containers.lock().unwrap();
            created.push(_config);
            Ok(format!("mock-container-{}", created.len()))
        }

        async fn start(&self, _id: &ContainerId) -> Result<()> {
            Ok(())
        }

        async fn exec(
            &self,
            _id: &ContainerId,
            cmd: &[&str],
            _timeout: Duration,
        ) -> Result<ExecResult> {
            self.exec_calls
                .lock()
                .unwrap()
                .push(cmd.iter().map(|s| (*s).to_string()).collect());
            let mut results = self.exec_results.lock().unwrap();
            if results.is_empty() {
                Ok(ExecResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                Ok(results.remove(0))
            }
        }

        async fn exec_detached(&self, _id: &ContainerId, cmd: &[&str]) -> Result<String> {
            self.detached_calls
                .lock()
                .unwrap()
                .push(cmd.iter().map(|s| (*s).to_string()).collect());
            Ok("exec-1".to_string())
        }

        async fn exec_logs(&self, exec_id: &str) -> Result<mpsc::Receiver<String>> {
            let mut rx_logs = self
                .log_sequences
                .lock()
                .unwrap()
                .remove(exec_id)
                .unwrap_or_default();
            let (tx, rx) = mpsc::channel(16);
            tokio::spawn(async move {
                for line in rx_logs.drain(..) {
                    if tx.send(line).await.is_err() {
                        break;
                    }
                }
            });
            Ok(rx)
        }

        async fn exec_result(&self, exec_id: &str) -> Result<ExecResult> {
            self.detached_results
                .lock()
                .unwrap()
                .remove(exec_id)
                .ok_or_else(|| anyhow::anyhow!("missing detached result for {exec_id}"))
        }

        async fn kill(&self, _id: &ContainerId, _signal: &str) -> Result<()> {
            Ok(())
        }

        async fn kill_exec(&self, exec_id: &str, signal: &str) -> Result<()> {
            self.killed_execs
                .lock()
                .unwrap()
                .push((exec_id.to_string(), signal.to_string()));
            Ok(())
        }

        async fn stop(&self, _id: &ContainerId) -> Result<()> {
            Ok(())
        }

        async fn remove(&self, _id: &ContainerId) -> Result<()> {
            Ok(())
        }

        async fn copy_from(&self, _id: &ContainerId, _path: &str) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }

        async fn copy_to(&self, _id: &ContainerId, _path: &str, _data: &[u8]) -> Result<()> {
            Ok(())
        }

        async fn logs(&self, _id: &ContainerId) -> Result<String> {
            Ok(String::new())
        }
    }

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
            step: vec![simple_step_run(
                "echo ok",
                Some(make_assertion(Some(0), Some("ok"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
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
            step: vec![simple_step_run(
                "false",
                Some(make_assertion(Some(0), None)),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
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
                step: vec![simple_step_run(
                    "false",
                    Some(make_assertion(Some(0), None)),
                )],
                resources: None,
                depends_on: None,
                fatal: None,
                group: None,
            },
            TestDef {
                id: "T02".to_string(),
                name: "depends_on_t01".to_string(),
                description: "should be skipped".to_string(),
                timeout: 30,
                flaky: None,
                retries: None,
                step: vec![simple_step_run("echo hello", None)],
                resources: None,
                depends_on: Some(vec!["T01".to_string()]),
                fatal: None,
                group: None,
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
            backend.killed_execs.lock().unwrap().as_slice(),
            [("exec-1".to_string(), "SIGKILL".to_string())]
        );
        let detached_calls = backend.detached_calls.lock().unwrap().clone();
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
            step: vec![simple_step_run(
                "echo ok",
                Some(make_assertion(Some(0), Some("ok"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
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
            step: vec![simple_step_run(
                "echo ok",
                Some(make_assertion(Some(0), Some("ok"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
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
            },
            test: vec![TestDef {
                id: "T-resource-flaky".to_string(),
                name: "resource_flaky".to_string(),
                description: "retries in fresh containers".to_string(),
                timeout: 30,
                flaky: Some(true),
                retries: Some(3),
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
        assert_eq!(backend.created_containers.lock().unwrap().len(), 2);
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

        let expanded = runner.substitute_vars("curl ${REMI_ENDPOINT}/health");
        assert_eq!(expanded, "curl https://packages.conary.io/health");

        let expanded2 = runner.substitute_vars("${CONARY_BIN} --db-path ${DB_PATH}");
        assert_eq!(
            expanded2,
            "/usr/local/bin/conary --db-path /tmp/conary-test.db"
        );

        let expanded3 = runner.substitute_vars("conary install ${PKG}");
        assert_eq!(expanded3, "conary install tree");

        let fixture_v1 = runner.substitute_vars("${FIXTURE_V1_CCS}");
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
        let runner = TestRunner::new(test_config(), "fedora43".to_string());
        let cmd = runner.build_kill_after_log_command(
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

        let expanded = runner.expand_qemu_boot(&QemuBoot {
            image: "${IMG}".to_string(),
            memory_mb: 1024,
            timeout_seconds: 120,
            ssh_port: 2222,
            commands: vec!["echo ${IMG}".to_string()],
            expect_output: vec!["${IMG}".to_string()],
        });

        assert_eq!(expanded.image, "minimal-boot-v1");
        assert_eq!(expanded.commands, vec!["echo minimal-boot-v1"]);
        assert_eq!(expanded.expect_output, vec!["minimal-boot-v1"]);
    }
}
