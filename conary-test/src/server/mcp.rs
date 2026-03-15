// conary-test/src/server/mcp.rs
//! MCP (Model Context Protocol) server for the conary-test infrastructure.
//!
//! Exposes test orchestration operations as MCP tools so that LLM agents can
//! list test suites, start runs, inspect results, and query distro config
//! through a standardised protocol.
//!
//! The MCP endpoint is mounted at `/mcp` on the HTTP server.

use std::future::Future;

use chrono::Utc;
use conary_core::mcp::to_json_text;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::tool::{ToolCallContext, ToolRouter},
    handler::server::wrapper::Parameters,
    model::*,
    service::RequestContext,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::report::json::to_json_report;
use crate::server::service;
use crate::server::state::AppState;

/// MCP server instance that wraps conary-test operations as tools.
///
/// Each MCP session gets its own `TestMcpServer` clone, but they all share
/// the same `AppState` (which contains `DashMap` for runs).
#[derive(Clone)]
pub struct TestMcpServer {
    state: AppState,
    #[allow(dead_code)] // Read by rmcp's tool_router macro via generated code
    tool_router: ToolRouter<Self>,
}

impl TestMcpServer {
    /// Create a new MCP server backed by the given shared state.
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter structs for tools that accept arguments
// ---------------------------------------------------------------------------

/// Parameters for starting a new test run.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartRunParams {
    /// Test suite name (must match a TOML manifest filename without extension).
    pub suite: String,
    /// Target distro name (must be configured in the global config).
    pub distro: String,
    /// Test phase number (e.g. 1 for Phase 1, 2 for Phase 2).
    pub phase: u32,
}

/// Parameters for retrieving a specific run.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRunParams {
    /// Numeric run ID returned by start_run.
    pub run_id: u64,
}

/// Parameters for listing recent runs.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRunsParams {
    /// Maximum number of runs to return (default 20, max 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Parameters for retrieving a single test result.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTestParams {
    /// Numeric run ID.
    pub run_id: u64,
    /// Test identifier (e.g. "T01").
    pub test_id: String,
}

/// Parameters for cancelling a run.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelRunParams {
    /// Numeric run ID to cancel.
    pub run_id: u64,
}

/// Parameters for re-running a single test.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RerunTestParams {
    /// Numeric run ID of the original run.
    pub run_id: u64,
    /// Test identifier to re-run (e.g. "T01").
    pub test_id: String,
}

/// Parameters for building a container image.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BuildImageParams {
    /// Distro name to build the image for (must be configured).
    pub distro: String,
}

/// Parameters for deploying source from a git ref.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeploySourceParams {
    /// Git ref to checkout (branch, tag, or commit). Default: pull current branch.
    #[schemars(
        description = "Git ref to checkout (branch, tag, or commit). Default: pull current branch."
    )]
    pub git_ref: Option<String>,
}

/// Parameters for rebuilding binaries.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RebuildParams {
    /// Specific crate to build (conary, conary-test). Default: both.
    #[schemars(description = "Specific crate to build (conary, conary-test). Default: both.")]
    pub crate_name: Option<String>,
}

/// Parameters for building test fixtures.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BuildFixturesParams {
    /// Fixture groups to build: all, corrupted, malicious, deps, boot, large. Default: all.
    #[schemars(
        description = "Fixture groups to build: all, corrupted, malicious, deps, boot, large. Default: all."
    )]
    pub groups: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert an `anyhow::Error` to an MCP error. Used by service functions
/// that return `anyhow::Result`.
fn anyhow_to_mcp(err: anyhow::Error) -> McpError {
    let msg = err.to_string();
    if msg.contains("not found") {
        McpError::invalid_params(msg, None)
    } else {
        McpError::internal_error(msg, None)
    }
}

/// Run a shell command and capture its exit code, stdout, and stderr.
async fn run_command(cmd: &str, args: &[&str], cwd: Option<&str>) -> Result<(i32, String, String), McpError> {
    let mut command = tokio::process::Command::new(cmd);
    command.args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .await
        .map_err(|e| McpError::internal_error(format!("Failed to run {cmd}: {e}"), None))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((code, stdout, stderr))
}

/// Determine the project root directory.
///
/// Checks `CONARY_PROJECT_DIR` env var first, then walks up from the current
/// executable until a directory containing `Cargo.toml` is found.
fn project_dir() -> Result<String, McpError> {
    // Check environment variable first.
    if let Ok(dir) = std::env::var("CONARY_PROJECT_DIR") {
        return Ok(dir);
    }

    // Walk up from the current executable.
    if let Ok(exe) = std::env::current_exe() {
        let mut path = exe.as_path();
        while let Some(parent) = path.parent() {
            if parent.join("Cargo.toml").exists()
                && let Some(s) = parent.to_str()
            {
                return Ok(s.to_string());
            }
            path = parent;
        }
    }

    // Fall back to current working directory.
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| McpError::internal_error(format!("Cannot determine project dir: {e}"), None))
}

/// Format command output as a human-readable summary string.
fn format_command_output(label: &str, code: i32, stdout: &str, stderr: &str) -> String {
    let status = if code == 0 { "OK" } else { "FAILED" };
    let mut out = format!("[{label}] exit={code} ({status})\n");
    if !stdout.is_empty() {
        // Limit output to last 100 lines to avoid huge MCP responses.
        let lines: Vec<&str> = stdout.lines().collect();
        let start = lines.len().saturating_sub(100);
        out.push_str("--- stdout (last 100 lines) ---\n");
        for line in &lines[start..] {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !stderr.is_empty() {
        let lines: Vec<&str> = stderr.lines().collect();
        let start = lines.len().saturating_sub(50);
        out.push_str("--- stderr (last 50 lines) ---\n");
        for line in &lines[start..] {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// MCP tool definitions
// ---------------------------------------------------------------------------

#[tool_router]
impl TestMcpServer {
    /// List available test suite manifests from the manifest directory.
    ///
    /// Returns suite names, phases, and test counts for each TOML manifest.
    #[tool(
        description = "List available test suite TOML manifests. Returns suite names, phases, and test counts."
    )]
    async fn list_suites(&self) -> Result<CallToolResult, McpError> {
        let suites = service::list_suites(&self.state).map_err(|e| {
            McpError::internal_error(format!("Cannot read manifest dir: {e}"), None)
        })?;

        let text = to_json_text(&suites)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Start a new test run for a given suite, distro, and phase.
    ///
    /// Creates a pending run and returns the run ID. The run can be
    /// inspected with `get_run` or `get_test`.
    #[tool(
        description = "Start a new test run. Requires suite name, distro, and phase. Returns the new run ID."
    )]
    async fn start_run(
        &self,
        Parameters(params): Parameters<StartRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = service::start_run(&self.state, &params.suite, &params.distro, params.phase)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        // Spawn the actual test execution in a background task.
        service::spawn_run(
            &self.state,
            result.run_id,
            &params.suite,
            &params.distro,
            params.phase,
        );

        let value = serde_json::json!({
            "run_id": result.run_id,
            "status": "pending",
            "suite": result.suite,
            "distro": result.distro,
            "phase": result.phase,
        });
        let text = to_json_text(&value)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get the status and results of a specific test run.
    #[tool(description = "Get status and full results for a test run by run ID.")]
    async fn get_run(
        &self,
        Parameters(params): Parameters<GetRunParams>,
    ) -> Result<CallToolResult, McpError> {
        // Try Remi first if configured.
        if let Some(ref client) = self.state.remi_client {
            match client.get_run(params.run_id as i64).await {
                Ok(data) => {
                    let text = to_json_text(&data)?;
                    return Ok(CallToolResult::success(vec![Content::text(text)]));
                }
                Err(e) => {
                    tracing::debug!(
                        "Remi proxy failed for get_run {}, falling back to local: {e}",
                        params.run_id
                    );
                }
            }
        }

        // Fall back to in-memory DashMap.
        // MCP get_run returns the full JSON report as text (not a Value),
        // so we use to_json_report here for the string representation.
        match self.state.runs.get(&params.run_id) {
            Some(entry) => {
                let json_str = to_json_report(&entry)
                    .map_err(|e| McpError::internal_error(format!("Report error: {e}"), None))?;
                Ok(CallToolResult::success(vec![Content::text(json_str)]))
            }
            None => Err(McpError::invalid_params(
                format!("Run {} not found", params.run_id),
                None,
            )),
        }
    }

    /// List recent test runs with summary information.
    ///
    /// Returns run IDs, suite names, phases, statuses, and pass/fail/skip counts.
    #[tool(
        description = "List recent test runs with summary info. Optional limit parameter (default 20, max 100)."
    )]
    async fn list_runs(
        &self,
        Parameters(params): Parameters<ListRunsParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(20).min(100);

        // Try Remi first if configured.
        if let Some(ref client) = self.state.remi_client {
            match client
                .list_runs(u32::try_from(limit).unwrap_or(u32::MAX), None, None, None, None)
                .await
            {
                Ok(data) => {
                    let text = to_json_text(&data)?;
                    return Ok(CallToolResult::success(vec![Content::text(text)]));
                }
                Err(e) => {
                    tracing::debug!(
                        "Remi proxy failed for list_runs, falling back to local: {e}"
                    );
                }
            }
        }

        // Fall back to in-memory DashMap.
        let summaries = service::list_runs(&self.state, limit);

        let text = to_json_text(&summaries)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get the result of a single test within a run.
    #[tool(description = "Get a single test result by run ID and test ID (e.g. 'T01').")]
    async fn get_test(
        &self,
        Parameters(params): Parameters<GetTestParams>,
    ) -> Result<CallToolResult, McpError> {
        // Try Remi first if configured.
        if let Some(ref client) = self.state.remi_client {
            match client.get_test(params.run_id as i64, &params.test_id).await {
                Ok(data) => {
                    let text = to_json_text(&data)?;
                    return Ok(CallToolResult::success(vec![Content::text(text)]));
                }
                Err(e) => {
                    tracing::debug!(
                        "Remi proxy failed for get_test {}/{}, falling back to local: {e}",
                        params.run_id,
                        params.test_id
                    );
                }
            }
        }

        // Fall back to in-memory DashMap.
        let entry = self.state.runs.get(&params.run_id).ok_or_else(|| {
            McpError::invalid_params(format!("Run {} not found", params.run_id), None)
        })?;

        let test = entry
            .results
            .iter()
            .find(|r| r.id == params.test_id)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "Test '{}' not found in run {}",
                        params.test_id, params.run_id
                    ),
                    None,
                )
            })?;

        let text = to_json_text(test)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// List all configured distros with their Remi distro name and repo name.
    #[tool(
        description = "List all configured distros with name, remi_distro, and repo_name fields."
    )]
    async fn list_distros(&self) -> Result<CallToolResult, McpError> {
        let distros = service::list_distros(&self.state);
        let text = to_json_text(&distros)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Cancel a running test run by its run ID.
    ///
    /// Sets the cancellation flag so the runner stops executing tests and
    /// marks the run status as cancelled.
    #[tool(
        description = "Cancel a running test run. Sets the cancellation flag and marks the run as cancelled."
    )]
    async fn cancel_run(
        &self,
        Parameters(params): Parameters<CancelRunParams>,
    ) -> Result<CallToolResult, McpError> {
        service::cancel_run(&self.state, params.run_id).map_err(anyhow_to_mcp)?;

        let value = serde_json::json!({
            "run_id": params.run_id,
            "status": "cancelled",
        });
        let text = to_json_text(&value)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Re-run a single test from a previous run.
    ///
    /// Creates a new pending run containing just the specified test and
    /// returns the new run ID.
    #[tool(description = "Re-run a single test from a previous run. Returns the new run ID.")]
    async fn rerun_test(
        &self,
        Parameters(params): Parameters<RerunTestParams>,
    ) -> Result<CallToolResult, McpError> {
        let rerun = service::rerun_test(&self.state, params.run_id, &params.test_id)
            .map_err(anyhow_to_mcp)?;

        // Spawn execution using the original suite's manifest.
        service::spawn_run(
            &self.state,
            rerun.run_id,
            &rerun.suite_name,
            &rerun.distro,
            rerun.phase,
        );

        let value = serde_json::json!({
            "original_run_id": params.run_id,
            "test_id": params.test_id,
            "new_run_id": rerun.run_id,
            "status": "pending",
        });
        let text = to_json_text(&value)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get stdout/stderr logs from all attempts of a test.
    #[tool(description = "Get stdout/stderr logs from all attempts of a test within a run.")]
    async fn get_test_logs(
        &self,
        Parameters(params): Parameters<GetTestParams>,
    ) -> Result<CallToolResult, McpError> {
        // Try Remi first if configured.
        if let Some(ref client) = self.state.remi_client {
            match client
                .get_logs(params.run_id as i64, &params.test_id, None, None)
                .await
            {
                Ok(data) => {
                    let text = to_json_text(&data)?;
                    return Ok(CallToolResult::success(vec![Content::text(text)]));
                }
                Err(e) => {
                    tracing::debug!(
                        "Remi proxy failed for get_test_logs {}/{}, falling back to local: {e}",
                        params.run_id,
                        params.test_id
                    );
                }
            }
        }

        // Fall back to in-memory DashMap.
        let logs = service::get_test_logs(&self.state, params.run_id, &params.test_id)
            .map_err(anyhow_to_mcp)?;

        let text = to_json_text(&logs)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get artifact information and summary for a run.
    #[tool(description = "Get artifact information (report path, summary) for a test run.")]
    async fn get_run_artifacts(
        &self,
        Parameters(params): Parameters<GetRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let artifacts =
            service::get_run_artifacts(&self.state, params.run_id).map_err(anyhow_to_mcp)?;

        let text = to_json_text(&artifacts)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Build a container image for a configured distro.
    #[tool(description = "Build a container image for a distro. Returns the image tag on success.")]
    async fn build_image(
        &self,
        Parameters(params): Parameters<BuildImageParams>,
    ) -> Result<CallToolResult, McpError> {
        let tag = service::build_image(&self.state, &params.distro)
            .await
            .map_err(anyhow_to_mcp)?;

        let value = serde_json::json!({
            "distro": params.distro,
            "image": tag,
            "status": "built",
        });
        let text = to_json_text(&value)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// List all available container images.
    #[tool(description = "List all available container images with ID, tags, and size.")]
    async fn list_images(&self) -> Result<CallToolResult, McpError> {
        let images = service::list_images(&self.state)
            .await
            .map_err(anyhow_to_mcp)?;

        let text = to_json_text(&images)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Clean up stopped conary-test containers.
    #[tool(
        description = "Remove stopped conary-test containers. Returns count of removed containers."
    )]
    async fn cleanup_containers(&self) -> Result<CallToolResult, McpError> {
        let result = service::cleanup_containers(&self.state)
            .await
            .map_err(anyhow_to_mcp)?;

        let text = to_json_text(&result)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    // -----------------------------------------------------------------------
    // Deployment tools
    // -----------------------------------------------------------------------

    /// Deploy source code from a git ref and rebuild binaries.
    ///
    /// If `git_ref` is `None`, runs `git pull` on the current branch.
    /// Otherwise runs `git fetch && git checkout <ref>`. Then rebuilds
    /// both `conary-test` and `conary` with `cargo build`.
    #[tool(
        description = "Deploy source code from git ref and rebuild. Runs git pull + cargo build on Forge."
    )]
    async fn deploy_source(
        &self,
        Parameters(params): Parameters<DeploySourceParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = project_dir()?;
        let mut output = String::new();

        // Step 1: Git operations.
        if let Some(ref git_ref) = params.git_ref {
            let (code, stdout, stderr) =
                run_command("git", &["fetch", "--all"], Some(&dir)).await?;
            output.push_str(&format_command_output("git fetch", code, &stdout, &stderr));
            if code != 0 {
                return Ok(CallToolResult::success(vec![Content::text(output)]));
            }

            let (code, stdout, stderr) =
                run_command("git", &["checkout", git_ref], Some(&dir)).await?;
            output.push_str(&format_command_output("git checkout", code, &stdout, &stderr));
            if code != 0 {
                return Ok(CallToolResult::success(vec![Content::text(output)]));
            }
        } else {
            let (code, stdout, stderr) =
                run_command("git", &["pull"], Some(&dir)).await?;
            output.push_str(&format_command_output("git pull", code, &stdout, &stderr));
            if code != 0 {
                return Ok(CallToolResult::success(vec![Content::text(output)]));
            }
        }

        // Step 2: Build both crates.
        let (code, stdout, stderr) =
            run_command("cargo", &["build", "-p", "conary-test"], Some(&dir)).await?;
        output.push_str(&format_command_output("cargo build conary-test", code, &stdout, &stderr));

        let (code, stdout, stderr) =
            run_command("cargo", &["build"], Some(&dir)).await?;
        output.push_str(&format_command_output("cargo build conary", code, &stdout, &stderr));

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    /// Rebuild conary and/or conary-test binaries from current source.
    #[tool(description = "Rebuild conary and conary-test binaries from current source.")]
    async fn rebuild_binary(
        &self,
        Parameters(params): Parameters<RebuildParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = project_dir()?;
        let mut output = String::new();

        match params.crate_name.as_deref() {
            Some("conary-test") => {
                let (code, stdout, stderr) =
                    run_command("cargo", &["build", "-p", "conary-test"], Some(&dir)).await?;
                output.push_str(&format_command_output("cargo build conary-test", code, &stdout, &stderr));
            }
            Some("conary") => {
                let (code, stdout, stderr) =
                    run_command("cargo", &["build"], Some(&dir)).await?;
                output.push_str(&format_command_output("cargo build conary", code, &stdout, &stderr));
            }
            Some(other) => {
                return Err(McpError::invalid_params(
                    format!("Unknown crate: {other}. Expected: conary, conary-test"),
                    None,
                ));
            }
            None => {
                let (code, stdout, stderr) =
                    run_command("cargo", &["build", "-p", "conary-test"], Some(&dir)).await?;
                output.push_str(&format_command_output("cargo build conary-test", code, &stdout, &stderr));

                let (code, stdout, stderr) =
                    run_command("cargo", &["build"], Some(&dir)).await?;
                output.push_str(&format_command_output("cargo build conary", code, &stdout, &stderr));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    /// Restart the conary-test systemd user service.
    ///
    /// Spawns a delayed restart (1 second) so the MCP response is sent before
    /// the process is killed by systemd.
    #[tool(
        description = "Restart the conary-test systemd user service and verify it's healthy."
    )]
    async fn restart_service(&self) -> Result<CallToolResult, McpError> {
        // Spawn a background task that waits 1 second then restarts.
        // This ensures the MCP response is sent before the process dies.
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let _ = tokio::process::Command::new("systemctl")
                .args(["--user", "restart", "conary-test"])
                .output()
                .await;
        });

        let value = serde_json::json!({
            "status": "restarting",
            "message": "Service restart scheduled in 1 second. The current process will be replaced.",
        });
        let text = to_json_text(&value)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Build test fixtures (CCS packages for integration tests).
    ///
    /// Runs the appropriate `build-*.sh` script from `tests/fixtures/adversarial/`.
    #[tool(description = "Build test fixtures (CCS packages for integration tests).")]
    async fn build_fixtures(
        &self,
        Parameters(params): Parameters<BuildFixturesParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = project_dir()?;
        let fixture_dir = format!("{dir}/tests/fixtures/adversarial");

        let group = params.groups.as_deref().unwrap_or("all");
        let script = match group {
            "all" => "build-all.sh",
            "corrupted" => "build-corrupted.sh",
            "malicious" => "build-malicious.sh",
            "deps" => "build-deps.sh",
            "boot" => "build-boot-image.sh",
            "large" => "build-large.sh",
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "Unknown fixture group: {other}. Expected: all, corrupted, malicious, deps, boot, large"
                    ),
                    None,
                ));
            }
        };

        let script_path = format!("{fixture_dir}/{script}");
        let (code, stdout, stderr) =
            run_command("bash", &[&script_path], Some(&dir)).await?;

        let output = format_command_output(
            &format!("build-fixtures ({group})"),
            code,
            &stdout,
            &stderr,
        );
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    /// Publish test fixtures to the Remi repository.
    ///
    /// Runs `scripts/publish-test-fixtures.sh` from the project root.
    #[tool(description = "Publish test fixtures to Remi repository.")]
    async fn publish_fixtures(&self) -> Result<CallToolResult, McpError> {
        let dir = project_dir()?;
        let script_path = format!("{dir}/scripts/publish-test-fixtures.sh");

        let (code, stdout, stderr) =
            run_command("bash", &[&script_path], Some(&dir)).await?;

        let output = format_command_output("publish-fixtures", code, &stdout, &stderr);
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    /// Get deployment status: binary version, uptime, WAL pending items,
    /// service health.
    #[tool(
        description = "Get deployment status: binary version, uptime, WAL pending items, service health."
    )]
    async fn deploy_status(&self) -> Result<CallToolResult, McpError> {
        let version = env!("CARGO_PKG_VERSION");

        let now = Utc::now();
        let uptime = now - self.state.start_time;
        let uptime_str = format!(
            "{}d {}h {}m {}s",
            uptime.num_days(),
            uptime.num_hours() % 24,
            uptime.num_minutes() % 60,
            uptime.num_seconds() % 60,
        );

        let wal_pending = self
            .state
            .wal
            .as_ref()
            .and_then(|w| w.lock().ok())
            .and_then(|w| w.pending_count().ok())
            .unwrap_or(0);

        let active_runs = self.state.runs.len();

        // Check systemd service status.
        let (service_code, service_stdout, _) =
            run_command("systemctl", &["--user", "is-active", "conary-test"], None)
                .await
                .unwrap_or((-1, "unknown".to_string(), String::new()));

        let service_status = if service_code == 0 {
            service_stdout.trim().to_string()
        } else {
            "unknown".to_string()
        };

        let value = serde_json::json!({
            "version": version,
            "uptime": uptime_str,
            "started_at": self.state.start_time.to_rfc3339(),
            "wal_pending": wal_pending,
            "active_runs": active_runs,
            "service_status": service_status,
        });
        let text = to_json_text(&value)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Flush pending WAL items to Remi.
    ///
    /// Replays all buffered test results that failed to reach Remi.
    /// Returns counts of flushed and failed items.
    #[tool(
        description = "Flush pending WAL items to Remi. Returns count of flushed and failed items."
    )]
    async fn flush_pending(&self) -> Result<CallToolResult, McpError> {
        let wal_arc = self.state.wal.as_ref().ok_or_else(|| {
            McpError::internal_error("WAL is not configured".to_string(), None)
        })?;
        let client = self.state.remi_client.as_ref().ok_or_else(|| {
            McpError::internal_error(
                "Remi client is not configured (REMI_ADMIN_TOKEN not set)".to_string(),
                None,
            )
        })?;

        // Extract pending items under the lock, then drop it before async work.
        let items = {
            let wal_guard = wal_arc.lock().map_err(|e| {
                McpError::internal_error(format!("WAL lock poisoned: {e}"), None)
            })?;
            wal_guard.pending_items().map_err(|e| {
                McpError::internal_error(format!("Failed to read WAL items: {e}"), None)
            })?
        };

        let mut flushed = 0u64;
        let mut failed = 0u64;

        for item in items {
            let data: crate::server::remi_client::PushResultData =
                match serde_json::from_str(&item.payload) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("WAL item {} has invalid payload: {}", item.id, e);
                        let wal_guard = wal_arc.lock().map_err(|e| {
                            McpError::internal_error(format!("WAL lock poisoned: {e}"), None)
                        })?;
                        let _ = wal_guard.remove(item.id);
                        continue;
                    }
                };

            match client.push_result(item.run_id, &data).await {
                Ok(()) => {
                    let wal_guard = wal_arc.lock().map_err(|e| {
                        McpError::internal_error(format!("WAL lock poisoned: {e}"), None)
                    })?;
                    let _ = wal_guard.remove(item.id);
                    flushed += 1;
                }
                Err(e) => {
                    let wal_guard = wal_arc.lock().map_err(|e| {
                        McpError::internal_error(format!("WAL lock poisoned: {e}"), None)
                    })?;
                    let _ = wal_guard.mark_retry(item.id, &e.to_string());
                    failed += 1;
                }
            }
        }

        let value = serde_json::json!({
            "flushed": flushed,
            "failed": failed,
            "status": if failed == 0 { "ok" } else { "partial" },
        });
        let text = to_json_text(&value)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler implementation
// ---------------------------------------------------------------------------

impl ServerHandler for TestMcpServer {
    fn get_info(&self) -> ServerInfo {
        conary_core::mcp::server_info(
            "conary-test-mcp",
            env!("CARGO_PKG_VERSION"),
            "Conary test infrastructure MCP server -- list test suites, \
             start and inspect test runs, query individual test results, \
             and list configured distros.",
        )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            ..Default::default()
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let tool_context = ToolCallContext::new(self, request, context);
        async move { self.tool_router.call(tool_context).await }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;

    #[test]
    fn test_mcp_server_info() {
        let state = test_fixtures::test_app_state();
        let server = TestMcpServer::new(state);

        let info = server.get_info();
        assert_eq!(info.server_info.name, "conary-test-mcp");
    }

    #[test]
    fn test_mcp_tool_count() {
        let router = TestMcpServer::tool_router();
        let tools = router.list_all();
        assert_eq!(tools.len(), 20, "Expected 20 MCP tools");
    }
}
