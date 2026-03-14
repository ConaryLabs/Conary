// conary-test/src/server/mcp.rs
//! MCP (Model Context Protocol) server for the conary-test infrastructure.
//!
//! Exposes test orchestration operations as MCP tools so that LLM agents can
//! list test suites, start runs, inspect results, and query distro config
//! through a standardised protocol.
//!
//! The MCP endpoint is mounted at `/mcp` on the HTTP server.

use conary_core::mcp::to_json_text;
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::report::json::to_json_report;
use crate::server::service;
use crate::server::state::AppState;

/// MCP server instance that wraps conary-test operations as tools.
///
/// Each MCP session gets its own `TestMcpServer` clone, but they all share
/// the same `AppState` (which contains `Arc<RwLock<...>>` for runs).
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
            .await
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

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
        // MCP get_run returns the full JSON report as text (not a Value),
        // so we use to_json_report here for the string representation.
        let runs = self.state.runs.read().await;
        match runs.get(&params.run_id) {
            Some(suite) => {
                let json_str = to_json_report(suite)
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
        let summaries = service::list_runs(&self.state, limit).await;

        let text = to_json_text(&summaries)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get the result of a single test within a run.
    #[tool(description = "Get a single test result by run ID and test ID (e.g. 'T01').")]
    async fn get_test(
        &self,
        Parameters(params): Parameters<GetTestParams>,
    ) -> Result<CallToolResult, McpError> {
        let runs = self.state.runs.read().await;
        let suite = runs.get(&params.run_id).ok_or_else(|| {
            McpError::invalid_params(format!("Run {} not found", params.run_id), None)
        })?;

        let test = suite
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
        assert_eq!(tools.len(), 6, "Expected 6 MCP tools");
    }
}
