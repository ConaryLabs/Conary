// conary-test/src/engine/runner.rs

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use tokio::time::Instant;
use tracing::{info, warn};

use crate::config::distro::GlobalConfig;
use crate::config::manifest::{ResourceConstraints, StepType, TestManifest};
use crate::container::backend::{ContainerBackend, ContainerConfig, ContainerId, ExecResult};
use crate::engine::assertions::evaluate_assertion;
use crate::engine::suite::{TestResult, TestStatus, TestSuite};

/// Executes tests from a manifest against a container.
pub struct TestRunner {
    pub config: GlobalConfig,
    pub distro: String,
    vars: HashMap<String, String>,
}

impl TestRunner {
    pub fn new(config: GlobalConfig, distro: String) -> Self {
        let mut vars = HashMap::new();
        vars.insert("REMI_ENDPOINT".to_string(), config.remi.endpoint.clone());
        vars.insert("DB_PATH".to_string(), config.paths.db.clone());
        vars.insert("CONARY_BIN".to_string(), config.paths.conary_bin.clone());

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
    ) -> Result<TestSuite> {
        self.load_manifest_vars(manifest);

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
                        // sha256sum output format: "<hash>  <path>"
                        let actual_hash = result
                            .stdout
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if actual_hash != chk.sha256 {
                            failure = Some(format!(
                                "checksum mismatch for {expanded_path}: expected {}, got {actual_hash}",
                                chk.sha256
                            ));
                            last_exec = Some(result);
                            break;
                        }
                        last_exec = Some(result);
                    }
                }

                // Evaluate assertion if present and we have an exec result.
                if let Some(ref assertion) = step.assert {
                    let exec = last_exec.as_ref().expect("assertion without exec result");
                    if let Err(e) =
                        evaluate_assertion(assertion, exec.exit_code, &exec.stdout, &exec.stderr)
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
            container_config
                .tmpfs
                .insert("/conary".to_string(), format!("size={tmpfs_size_mb}m"));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::distro::{GlobalConfig, PathsConfig, RemiConfig, SetupConfig};
    use crate::config::manifest::{
        Assertion, ResourceConstraints, SuiteDef, TestDef, TestManifest, TestStep,
    };
    use crate::container::backend::{ContainerConfig, ExecResult};
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::Mutex;

    // -- Mock backend --

    struct MockBackend {
        exec_calls: Mutex<Vec<Vec<String>>>,
        exec_results: Mutex<Vec<ExecResult>>,
    }

    impl MockBackend {
        fn new(results: Vec<ExecResult>) -> Self {
            Self {
                exec_calls: Mutex::new(Vec::new()),
                exec_results: Mutex::new(results),
            }
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
            Ok("mock-container".to_string())
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
        GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://packages.conary.io".to_string(),
            },
            paths: PathsConfig {
                db: "/tmp/conary-test.db".to_string(),
                conary_bin: "/usr/local/bin/conary".to_string(),
                results_dir: "/tmp/results".to_string(),
                fixture_dir: None,
            },
            setup: SetupConfig::default(),
            distros: HashMap::new(),
            fixtures: None,
        }
    }

    fn simple_step_run(cmd: &str, assertion: Option<Assertion>) -> TestStep {
        TestStep {
            run: Some(cmd.to_string()),
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
            .run(&manifest, &backend, &"ctr-1".to_string())
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
            .run(&manifest, &backend, &"ctr-1".to_string())
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
                step: vec![simple_step_run("echo hello", None)],
                resources: None,
                depends_on: Some(vec!["T01".to_string()]),
                fatal: None,
                group: None,
            },
        ]);

        let mut runner = TestRunner::new(test_config(), "fedora43".to_string());
        let suite = runner
            .run(&manifest, &backend, &"ctr-1".to_string())
            .await
            .unwrap();

        assert_eq!(suite.failed(), 1);
        assert_eq!(suite.skipped(), 1);
        assert_eq!(suite.results[1].status, TestStatus::Skipped);
        assert!(suite.results[1].message.as_ref().unwrap().contains("T01"));
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
            container_config.tmpfs.get("/conary").map(String::as_str),
            Some("size=50m")
        );
        assert_eq!(container_config.memory_limit, Some(512 * 1024 * 1024));
        assert_eq!(container_config.network_mode, "none");
    }
}
