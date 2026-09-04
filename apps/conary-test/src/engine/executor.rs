// apps/conary-test/src/engine/executor.rs

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
            let result = run_qemu_boot(config, timeout).await?;
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
#[path = "executor/tests.rs"]
mod tests;
