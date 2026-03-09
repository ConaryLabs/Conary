// conary-test/src/server/mcp.rs
//! MCP (Model Context Protocol) server for the conary-test infrastructure.
//!
//! Exposes test orchestration operations as MCP tools so that LLM agents can
//! list test suites, start runs, inspect results, and query distro config
//! through a standardised protocol.
//!
//! The MCP endpoint is mounted at `/mcp` on the HTTP server.

use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::config::load_manifest;
use crate::engine::suite::TestSuite;
use crate::report::json::to_json_report;
use crate::server::state::AppState;

/// Serialize a value to pretty JSON, mapping failures to [`McpError`].
fn to_json_text<T: serde::Serialize>(value: &T) -> Result<String, McpError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))
}

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
        let manifest_dir = std::path::Path::new(&self.state.manifest_dir);

        let entries = std::fs::read_dir(manifest_dir).map_err(|e| {
            McpError::internal_error(format!("Cannot read manifest dir: {e}"), None)
        })?;

        let mut suites = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml")
                && let Ok(manifest) = load_manifest(&path)
            {
                suites.push(serde_json::json!({
                    "name": manifest.suite.name,
                    "phase": manifest.suite.phase,
                    "test_count": manifest.test.len(),
                }));
            }
        }

        suites.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

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
        if !self.state.config.distros.contains_key(&params.distro) {
            return Err(McpError::invalid_params(
                format!("Unknown distro: {}", params.distro),
                None,
            ));
        }

        let run_id = AppState::next_run_id();
        let suite = TestSuite::new(&params.suite, params.phase);

        let mut runs = self.state.runs.write().await;
        runs.insert(run_id, suite);

        let result = serde_json::json!({
            "run_id": run_id,
            "status": "pending",
            "suite": params.suite,
            "distro": params.distro,
            "phase": params.phase,
        });
        let text = to_json_text(&result)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get the status and results of a specific test run.
    #[tool(description = "Get status and full results for a test run by run ID.")]
    async fn get_run(
        &self,
        Parameters(params): Parameters<GetRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let runs = self.state.runs.read().await;
        match runs.get(&params.run_id) {
            Some(suite) => {
                let json_str = to_json_report(suite).map_err(|e| {
                    McpError::internal_error(format!("Report error: {e}"), None)
                })?;
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
        let runs = self.state.runs.read().await;

        let mut summaries: Vec<serde_json::Value> = runs
            .iter()
            .map(|(&id, suite)| {
                serde_json::json!({
                    "run_id": id,
                    "suite": suite.name,
                    "phase": suite.phase,
                    "status": serde_json::to_value(suite.status).unwrap_or_default(),
                    "total": suite.total(),
                    "passed": suite.passed(),
                    "failed": suite.failed(),
                    "skipped": suite.skipped(),
                })
            })
            .collect();

        summaries.sort_by(|a, b| {
            b["run_id"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&a["run_id"].as_u64().unwrap_or(0))
        });
        summaries.truncate(limit);

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
        let mut distros: Vec<serde_json::Value> = self
            .state
            .config
            .distros
            .iter()
            .map(|(name, cfg)| {
                serde_json::json!({
                    "name": name,
                    "remi_distro": cfg.remi_distro,
                    "repo_name": cfg.repo_name,
                })
            })
            .collect();

        distros.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        let text = to_json_text(&distros)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler implementation
// ---------------------------------------------------------------------------

impl ServerHandler for TestMcpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "conary-test-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Conary test infrastructure MCP server -- list test suites, \
                 start and inspect test runs, query individual test results, \
                 and list configured distros.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::distro::{DistroConfig, GlobalConfig, PathsConfig, RemiConfig, SetupConfig};
    use std::collections::HashMap;

    fn test_state() -> AppState {
        let mut distros = HashMap::new();
        distros.insert(
            "fedora43".to_string(),
            DistroConfig {
                remi_distro: "fedora43".to_string(),
                repo_name: "conary-fedora43".to_string(),
                containerfile: None,
                test_package_1: None,
                test_binary_1: None,
                test_package_2: None,
                test_binary_2: None,
                test_package_3: None,
                test_binary_3: None,
            },
        );

        let config = GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://localhost".to_string(),
            },
            paths: PathsConfig {
                db: "/tmp/test.db".to_string(),
                conary_bin: "/usr/bin/conary".to_string(),
                results_dir: "/tmp/results".to_string(),
                fixture_dir: None,
            },
            setup: SetupConfig::default(),
            distros,
            fixtures: None,
        };

        AppState::new(config, "/tmp/manifests".to_string())
    }

    #[test]
    fn test_mcp_server_info() {
        let state = test_state();
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
