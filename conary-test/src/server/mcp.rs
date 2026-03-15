// conary-test/src/server/mcp.rs
//! MCP (Model Context Protocol) server for the conary-test infrastructure.
//!
//! Exposes test orchestration operations as MCP tools so that LLM agents can
//! list test suites, start runs, inspect results, and query distro config
//! through a standardised protocol.
//!
//! The MCP endpoint is mounted at `/mcp` on the HTTP server.

use std::future::Future;

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
        assert_eq!(tools.len(), 13, "Expected 13 MCP tools");
    }
}
