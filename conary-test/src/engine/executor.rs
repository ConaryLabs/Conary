// conary-test/src/engine/executor.rs

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use tracing::info;

use crate::config::manifest::{KillAfterLog, QemuBoot, StepType, TestStep};
use crate::container::backend::{ContainerBackend, ContainerId, ExecResult};
use crate::engine::qemu::run_qemu_boot;
use crate::engine::variables;

/// Concrete action to execute within a container. Each variant maps to a
/// single manifest step type with variables already expanded.
#[derive(Debug, Clone)]
pub enum StepAction {
    Run(String),
    Conary(String),
    FileExists(PathBuf),
    FileNotExists(PathBuf),
    FileExecutable(PathBuf),
    DirExists(PathBuf),
    FileChecksum { path: PathBuf, sha256: String },
    Sleep(u64),
    KillAfterLog(KillAfterLog),
    QemuBoot(QemuBoot),
}

/// Outcome of executing a single step action.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
    /// If the step itself determined a failure (e.g. file-not-found check),
    /// this carries the message. `None` means the step succeeded structurally
    /// and assertion checking should proceed normally.
    pub failure: Option<String>,
}

impl StepResult {
    /// Build from a raw `ExecResult` and elapsed time.
    fn from_exec(exec: &ExecResult, duration: Duration) -> Self {
        Self {
            exit_code: exec.exit_code,
            stdout: exec.stdout.clone(),
            stderr: exec.stderr.clone(),
            duration,
            failure: None,
        }
    }

    fn failed(exec: &ExecResult, duration: Duration, message: String) -> Self {
        Self {
            exit_code: exec.exit_code,
            stdout: exec.stdout.clone(),
            stderr: exec.stderr.clone(),
            duration,
            failure: Some(message),
        }
    }
}

impl StepAction {
    /// Convert a manifest `TestStep` into an expanded `StepAction`.
    ///
    /// Returns `None` if the step has no recognized type field set.
    pub fn from_step(step: &TestStep, vars: &HashMap<String, String>) -> Option<Self> {
        let step_type = step.step_type()?;
        Some(Self::from_step_type(&step_type, vars))
    }

    /// Convert an already-parsed `StepType` into an expanded `StepAction`.
    fn from_step_type(step_type: &StepType, vars: &HashMap<String, String>) -> Self {
        match step_type {
            StepType::Run(cmd) => Self::Run(variables::expand_variables(cmd, vars)),
            StepType::Conary(args) => Self::Conary(variables::expand_variables(args, vars)),
            StepType::FileExists(path) => {
                Self::FileExists(PathBuf::from(variables::expand_variables(path, vars)))
            }
            StepType::FileNotExists(path) => {
                Self::FileNotExists(PathBuf::from(variables::expand_variables(path, vars)))
            }
            StepType::FileExecutable(path) => {
                Self::FileExecutable(PathBuf::from(variables::expand_variables(path, vars)))
            }
            StepType::DirExists(path) => {
                Self::DirExists(PathBuf::from(variables::expand_variables(path, vars)))
            }
            StepType::FileChecksum(chk) => Self::FileChecksum {
                path: PathBuf::from(variables::expand_variables(&chk.path, vars)),
                sha256: variables::expand_variables(&chk.sha256, vars),
            },
            StepType::Sleep(secs) => Self::Sleep(*secs),
            StepType::KillAfterLog(config) => {
                let mut expanded = config.clone();
                expanded.conary = variables::expand_variables(&config.conary, vars);
                Self::KillAfterLog(expanded)
            }
            StepType::QemuBoot(config) => Self::QemuBoot(variables::expand_qemu_boot(config, vars)),
        }
    }
}

/// Configuration needed to execute conary commands (paths, binary location).
pub struct ExecutionContext<'a> {
    pub conary_bin: &'a str,
    pub db_path: &'a str,
}

/// Execute a single step action against a container backend.
///
/// Returns a `StepResult` with execution output and optional structural failure.
/// Assertion evaluation is NOT performed here -- the caller (runner) handles that.
pub async fn execute_step(
    action: &StepAction,
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    ctx: &ExecutionContext<'_>,
    timeout: Duration,
) -> Result<StepResult> {
    let start = tokio::time::Instant::now();

    match action {
        StepAction::Sleep(secs) => {
            info!("sleeping for {secs}s");
            tokio::time::sleep(Duration::from_secs(*secs)).await;
            let duration = start.elapsed();
            Ok(StepResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                duration,
                failure: None,
            })
        }
        StepAction::Run(cmd) => {
            let result = backend
                .exec(container_id, &["sh", "-c", cmd], timeout)
                .await?;
            Ok(StepResult::from_exec(&result, start.elapsed()))
        }
        StepAction::Conary(args) => {
            let full_cmd = format!("{} {} --db-path {}", ctx.conary_bin, args, ctx.db_path);
            let result = backend
                .exec(container_id, &["sh", "-c", &full_cmd], timeout)
                .await?;
            Ok(StepResult::from_exec(&result, start.elapsed()))
        }
        StepAction::FileExists(path) => {
            let path_str = path.display().to_string();
            let result = backend
                .exec(container_id, &["test", "-e", &path_str], timeout)
                .await?;
            let duration = start.elapsed();
            if result.exit_code != 0 {
                Ok(StepResult::failed(
                    &result,
                    duration,
                    format!("file does not exist: {path_str}"),
                ))
            } else {
                Ok(StepResult::from_exec(&result, duration))
            }
        }
        StepAction::FileNotExists(path) => {
            let path_str = path.display().to_string();
            let result = backend
                .exec(container_id, &["test", "!", "-e", &path_str], timeout)
                .await?;
            let duration = start.elapsed();
            if result.exit_code != 0 {
                Ok(StepResult::failed(
                    &result,
                    duration,
                    format!("file unexpectedly exists: {path_str}"),
                ))
            } else {
                Ok(StepResult::from_exec(&result, duration))
            }
        }
        StepAction::FileExecutable(path) => {
            let path_str = path.display().to_string();
            let result = backend
                .exec(container_id, &["test", "-x", &path_str], timeout)
                .await?;
            let duration = start.elapsed();
            if result.exit_code != 0 {
                Ok(StepResult::failed(
                    &result,
                    duration,
                    format!("file is not executable: {path_str}"),
                ))
            } else {
                Ok(StepResult::from_exec(&result, duration))
            }
        }
        StepAction::DirExists(path) => {
            let path_str = path.display().to_string();
            let result = backend
                .exec(container_id, &["test", "-d", &path_str], timeout)
                .await?;
            let duration = start.elapsed();
            if result.exit_code != 0 {
                Ok(StepResult::failed(
                    &result,
                    duration,
                    format!("directory does not exist: {path_str}"),
                ))
            } else {
                Ok(StepResult::from_exec(&result, duration))
            }
        }
        StepAction::FileChecksum { path, sha256 } => {
            let path_str = path.display().to_string();
            let cmd = format!("sha256sum {path_str}");
            let result = backend
                .exec(container_id, &["sh", "-c", &cmd], timeout)
                .await?;
            let duration = start.elapsed();
            if result.exit_code != 0 {
                return Ok(StepResult::failed(
                    &result,
                    duration,
                    format!("sha256sum failed on {path_str}: {}", result.stderr.trim()),
                ));
            }
            let actual_hash = result
                .stdout
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            if actual_hash != *sha256 {
                Ok(StepResult::failed(
                    &result,
                    duration,
                    format!(
                        "checksum mismatch for {path_str}: expected {sha256}, got {actual_hash}",
                    ),
                ))
            } else {
                Ok(StepResult::from_exec(&result, duration))
            }
        }
        StepAction::KillAfterLog(config) => {
            let result = run_kill_after_log(backend, container_id, config, ctx.conary_bin).await?;
            Ok(StepResult::from_exec(&result, start.elapsed()))
        }
        StepAction::QemuBoot(config) => {
            let result = run_qemu_boot(config).await?;
            Ok(StepResult::from_exec(&result, start.elapsed()))
        }
    }
}

/// Build the shell command for kill-after-log with PID tracking.
pub(crate) fn build_kill_after_log_command(conary_bin: &str, expanded: &str) -> String {
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
                conary_bin,
                conary_args.join(" ")
            );
        }
    }

    format!(
        "printf '__CONARY_TEST_PID__=%s\\n' \"$$\"; exec {} {}",
        conary_bin, expanded
    )
}

/// Run a conary command, wait for a log pattern, then kill the process.
async fn run_kill_after_log(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    config: &KillAfterLog,
    conary_bin: &str,
) -> Result<ExecResult> {
    let full_cmd = build_kill_after_log_command(conary_bin, &config.conary);
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
        // The process completed before we saw the pattern in the log stream.
        // This can happen when the operation is fast (few files) and Podman's
        // output buffering delivers everything in a single chunk after the
        // process exits. Treat this as the process having run past the kill
        // point — subsequent test steps will validate the resulting state.
        let result = backend.exec_result(&exec_id).await?;
        tracing::info!(
            pattern = config.pattern,
            exit_code = result.exit_code,
            "process exited before kill_after_log could match pattern, treating as completed"
        );
        return Ok(result);
    }

    backend.kill_exec(&exec_id, "SIGKILL").await?;
    backend.exec_result(&exec_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::manifest::{FileChecksum, TestStep};

    #[test]
    fn from_step_run_produces_action() {
        let step = TestStep {
            run: Some("echo ${GREETING}".to_string()),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("GREETING".to_string(), "hello".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::Run(cmd) => assert_eq!(cmd, "echo hello"),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn from_step_file_exists_produces_action() {
        let step = TestStep {
            file_exists: Some("/usr/bin/${TOOL}".to_string()),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("TOOL".to_string(), "conary".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::FileExists(path) => {
                assert_eq!(path, PathBuf::from("/usr/bin/conary"));
            }
            other => panic!("expected FileExists, got {other:?}"),
        }
    }

    #[test]
    fn from_step_none_for_empty() {
        let step = TestStep::default();
        assert!(StepAction::from_step(&step, &HashMap::new()).is_none());
    }

    #[test]
    fn from_step_conary_expands_vars() {
        let step = TestStep {
            conary: Some("install ${PKG}".to_string()),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("PKG".to_string(), "tree".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::Conary(args) => assert_eq!(args, "install tree"),
            other => panic!("expected Conary, got {other:?}"),
        }
    }

    #[test]
    fn from_step_file_checksum_expands_both_fields() {
        let step = TestStep {
            file_checksum: Some(FileChecksum {
                path: "/tmp/${FILE}".to_string(),
                sha256: "${HASH}".to_string(),
            }),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("FILE".to_string(), "hello.txt".to_string());
        vars.insert("HASH".to_string(), "abc123".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::FileChecksum { path, sha256 } => {
                assert_eq!(path, PathBuf::from("/tmp/hello.txt"));
                assert_eq!(sha256, "abc123");
            }
            other => panic!("expected FileChecksum, got {other:?}"),
        }
    }

    #[test]
    fn from_step_sleep_passes_through() {
        let step = TestStep {
            sleep: Some(5),
            ..TestStep::default()
        };
        let action = StepAction::from_step(&step, &HashMap::new()).unwrap();
        match action {
            StepAction::Sleep(s) => assert_eq!(s, 5),
            other => panic!("expected Sleep, got {other:?}"),
        }
    }

    #[test]
    fn from_step_kill_after_log_expands_conary_field() {
        let step = TestStep {
            kill_after_log: Some(KillAfterLog {
                conary: "ccs install ${PKG}".to_string(),
                pattern: "Deploying".to_string(),
                timeout_seconds: 10,
            }),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("PKG".to_string(), "pkg.ccs".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::KillAfterLog(config) => {
                assert_eq!(config.conary, "ccs install pkg.ccs");
                assert_eq!(config.pattern, "Deploying");
            }
            other => panic!("expected KillAfterLog, got {other:?}"),
        }
    }

    #[test]
    fn exhaustive_match_compiles() {
        // This test exists to verify that all StepAction variants are handled.
        // If a new variant is added, the match below will fail to compile.
        let action = StepAction::Sleep(0);
        match &action {
            StepAction::Run(_) => {}
            StepAction::Conary(_) => {}
            StepAction::FileExists(_) => {}
            StepAction::FileNotExists(_) => {}
            StepAction::FileExecutable(_) => {}
            StepAction::DirExists(_) => {}
            StepAction::FileChecksum { .. } => {}
            StepAction::Sleep(_) => {}
            StepAction::KillAfterLog(_) => {}
            StepAction::QemuBoot(_) => {}
        }
    }

    #[test]
    fn build_kill_command_plain() {
        let cmd = build_kill_after_log_command("/usr/bin/conary", "ccs install foo.ccs");
        assert!(cmd.contains("exec /usr/bin/conary ccs install foo.ccs"));
        assert!(cmd.contains("__CONARY_TEST_PID__"));
    }

    #[test]
    fn build_kill_command_with_env() {
        let cmd =
            build_kill_after_log_command("/usr/bin/conary", "env HOLD_MS=1500 ccs install foo.ccs");
        assert!(cmd.contains("exec env HOLD_MS=1500 /usr/bin/conary ccs install foo.ccs"));
    }

    #[test]
    fn step_result_from_exec_no_failure() {
        let exec = ExecResult {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
        };
        let result = StepResult::from_exec(&exec, Duration::from_millis(42));
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "ok");
        assert!(result.failure.is_none());
    }

    #[test]
    fn step_result_failed_carries_message() {
        let exec = ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "err".to_string(),
        };
        let result = StepResult::failed(&exec, Duration::from_millis(10), "boom".to_string());
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.failure, Some("boom".to_string()));
    }

    // ---- from_step tests for FileNotExists, FileExecutable, DirExists ----

    #[test]
    fn from_step_file_not_exists_produces_action() {
        let step = TestStep {
            file_not_exists: Some("/tmp/${NAME}".to_string()),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("NAME".to_string(), "gone.txt".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::FileNotExists(path) => {
                assert_eq!(path, PathBuf::from("/tmp/gone.txt"));
            }
            other => panic!("expected FileNotExists, got {other:?}"),
        }
    }

    #[test]
    fn from_step_file_executable_produces_action() {
        let step = TestStep {
            file_executable: Some("/usr/bin/${BIN}".to_string()),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("BIN".to_string(), "conary".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::FileExecutable(path) => {
                assert_eq!(path, PathBuf::from("/usr/bin/conary"));
            }
            other => panic!("expected FileExecutable, got {other:?}"),
        }
    }

    #[test]
    fn from_step_dir_exists_produces_action() {
        let step = TestStep {
            dir_exists: Some("/var/${DIR}".to_string()),
            ..TestStep::default()
        };
        let mut vars = HashMap::new();
        vars.insert("DIR".to_string(), "lib".to_string());

        let action = StepAction::from_step(&step, &vars).unwrap();
        match action {
            StepAction::DirExists(path) => {
                assert_eq!(path, PathBuf::from("/var/lib"));
            }
            other => panic!("expected DirExists, got {other:?}"),
        }
    }

    // ---- execute_step unit tests with inline mock ----

    use crate::container::backend::{
        ContainerBackend, ContainerConfig, ContainerInspection, ImageInfo,
    };
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    /// Mock backend for execute_step tests. Returns pre-configured
    /// `ExecResult` values in FIFO order and records exec calls.
    struct ExecMock {
        exec_results: Mutex<Vec<ExecResult>>,
        exec_calls: Mutex<Vec<Vec<String>>>,
    }

    impl ExecMock {
        fn new(results: Vec<ExecResult>) -> Self {
            Self {
                exec_results: Mutex::new(results),
                exec_calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.exec_calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ContainerBackend for ExecMock {
        async fn build_image(
            &self,
            _dockerfile: &Path,
            _tag: &str,
            _build_args: HashMap<String, String>,
        ) -> Result<String> {
            Ok("mock".to_string())
        }
        async fn create(&self, _config: ContainerConfig) -> Result<ContainerId> {
            Ok("mock-ctr".to_string())
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
        async fn exec_detached(&self, _id: &ContainerId, _cmd: &[&str]) -> Result<String> {
            Ok("exec-1".to_string())
        }
        async fn exec_logs(&self, _exec_id: &str) -> Result<mpsc::Receiver<String>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
        async fn exec_result(&self, _exec_id: &str) -> Result<ExecResult> {
            Ok(ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
        async fn kill(&self, _id: &ContainerId, _signal: &str) -> Result<()> {
            Ok(())
        }
        async fn kill_exec(&self, _exec_id: &str, _signal: &str) -> Result<()> {
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
        async fn inspect_container(&self, _id: &ContainerId) -> Result<ContainerInspection> {
            Ok(ContainerInspection::default())
        }
        async fn list_images(&self) -> Result<Vec<ImageInfo>> {
            Ok(Vec::new())
        }
    }

    fn test_ctx() -> ExecutionContext<'static> {
        ExecutionContext {
            conary_bin: "/usr/bin/conary",
            db_path: "/var/lib/conary/db",
        }
    }

    #[tokio::test]
    async fn execute_step_run() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: "hello world\n".to_string(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::Run("echo hello world".to_string());
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello world\n");
        assert!(result.failure.is_none());
        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec!["sh", "-c", "echo hello world"]);
    }

    #[tokio::test]
    async fn execute_step_conary() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: "installed tree\n".to_string(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::Conary("install tree".to_string());
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "installed tree\n");
        assert!(result.failure.is_none());
        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0][2].contains("/usr/bin/conary install tree --db-path"));
    }

    #[tokio::test]
    async fn execute_step_file_exists_success() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileExists(PathBuf::from("/usr/bin/conary"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none());
        let calls = mock.calls();
        assert_eq!(calls[0], vec!["test", "-e", "/usr/bin/conary"]);
    }

    #[tokio::test]
    async fn execute_step_file_exists_failure() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileExists(PathBuf::from("/missing/file"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 1);
        assert!(result.failure.is_some());
        assert!(
            result
                .failure
                .as_ref()
                .unwrap()
                .contains("file does not exist")
        );
    }

    #[tokio::test]
    async fn execute_step_file_not_exists_success() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileNotExists(PathBuf::from("/tmp/gone.txt"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none());
        let calls = mock.calls();
        assert_eq!(calls[0], vec!["test", "!", "-e", "/tmp/gone.txt"]);
    }

    #[tokio::test]
    async fn execute_step_file_not_exists_failure() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileNotExists(PathBuf::from("/tmp/exists.txt"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .failure
                .as_ref()
                .unwrap()
                .contains("file unexpectedly exists")
        );
    }

    #[tokio::test]
    async fn execute_step_file_executable_success() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileExecutable(PathBuf::from("/usr/bin/conary"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none());
        let calls = mock.calls();
        assert_eq!(calls[0], vec!["test", "-x", "/usr/bin/conary"]);
    }

    #[tokio::test]
    async fn execute_step_file_executable_failure() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileExecutable(PathBuf::from("/tmp/script.sh"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .failure
                .as_ref()
                .unwrap()
                .contains("file is not executable")
        );
    }

    #[tokio::test]
    async fn execute_step_dir_exists_success() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::DirExists(PathBuf::from("/var/lib"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none());
        let calls = mock.calls();
        assert_eq!(calls[0], vec!["test", "-d", "/var/lib"]);
    }

    #[tokio::test]
    async fn execute_step_dir_exists_failure() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::DirExists(PathBuf::from("/nonexistent"));
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .failure
                .as_ref()
                .unwrap()
                .contains("directory does not exist")
        );
    }

    #[tokio::test]
    async fn execute_step_file_checksum_match() {
        let hash = "abc123def456";
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: format!("{hash}  /tmp/file.txt\n"),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileChecksum {
            path: PathBuf::from("/tmp/file.txt"),
            sha256: hash.to_string(),
        };
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none());
    }

    #[tokio::test]
    async fn execute_step_file_checksum_mismatch() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 0,
            stdout: "wronghash  /tmp/file.txt\n".to_string(),
            stderr: String::new(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileChecksum {
            path: PathBuf::from("/tmp/file.txt"),
            sha256: "expectedhash".to_string(),
        };
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert!(result.failure.is_some());
        assert!(result.failure.as_ref().unwrap().contains("checksum mismatch"));
    }

    #[tokio::test]
    async fn execute_step_file_checksum_sha256sum_fails() {
        let mock = ExecMock::new(vec![ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "No such file\n".to_string(),
        }]);
        let ctx = test_ctx();
        let action = StepAction::FileChecksum {
            path: PathBuf::from("/tmp/missing.txt"),
            sha256: "abc".to_string(),
        };
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert!(result.failure.is_some());
        assert!(result.failure.as_ref().unwrap().contains("sha256sum failed"));
    }

    #[tokio::test]
    async fn execute_step_sleep() {
        let mock = ExecMock::new(vec![]);
        let ctx = test_ctx();
        let action = StepAction::Sleep(0);
        let result = execute_step(
            &action,
            &mock,
            &"ctr-1".to_string(),
            &ctx,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none());
        assert!(result.stdout.is_empty());
        // No exec calls should have been made.
        assert!(mock.calls().is_empty());
    }
}
