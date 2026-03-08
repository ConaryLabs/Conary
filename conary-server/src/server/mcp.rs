// conary-server/src/server/mcp.rs
//! MCP (Model Context Protocol) server for LLM agent integration.
//!
//! Exposes Remi admin operations as MCP tools so that LLM agents (Claude,
//! etc.) can inspect CI status, trigger workflows, manage tokens, and
//! force mirror syncs through a standardised protocol.
//!
//! The MCP endpoint is mounted on the external admin router at `/mcp` and
//! sits behind the same Bearer-token auth middleware as other admin endpoints.

use std::sync::Arc;
use tokio::sync::RwLock;

use rmcp::{
    ErrorData as McpError,
    ServerHandler,
    handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::*,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::ServerState;

/// Validate a path parameter against a safe pattern for URL interpolation.
///
/// Rejects values containing slashes, `..`, null bytes, or characters
/// outside `[a-zA-Z0-9._-]`.
fn validate_path_param(value: &str, param_name: &str) -> Result<(), McpError> {
    if value.is_empty()
        || value.contains('/')
        || value.contains("..")
        || value.contains('\0')
        || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        Err(McpError::invalid_params(
            format!("Invalid {param_name}: must match [a-zA-Z0-9._-]+"),
            None,
        ))
    } else {
        Ok(())
    }
}

/// MCP server instance that wraps Remi admin operations as tools.
///
/// Each MCP session gets its own `RemiMcpServer` clone, but they all share
/// the same `Arc<RwLock<ServerState>>` so mutations (if any) are visible
/// across sessions.
#[derive(Clone)]
pub struct RemiMcpServer {
    state: Arc<RwLock<ServerState>>,
    #[allow(dead_code)] // Read by rmcp's tool_router macro via generated code
    tool_router: ToolRouter<Self>,
}

impl RemiMcpServer {
    /// Create a new MCP server backed by the given shared state.
    pub fn new(state: Arc<RwLock<ServerState>>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

// ---------------------------------------------------------------------------
// Forgejo proxy helpers (MCP-specific — returns McpError instead of Response)
// ---------------------------------------------------------------------------

impl RemiMcpServer {
    /// GET a Forgejo API path, returning the raw JSON text.
    async fn forgejo_get(&self, path: &str) -> Result<String, McpError> {
        let (url, token, client) = {
            let s = self.state.read().await;
            let base = s.forgejo_url.as_ref().ok_or_else(|| {
                McpError::internal_error("Forgejo not configured", None)
            })?;
            let token = s.forgejo_token.clone().unwrap_or_default();
            (
                format!("{}/api/v1{}", base.trim_end_matches('/'), path),
                token,
                s.http_client.clone(),
            )
        };

        let resp = client
            .get(&url)
            .header("Authorization", format!("token {token}"))
            .send()
            .await
            .map_err(|e| McpError::internal_error(format!("Forgejo unreachable: {e}"), None))?;

        if !resp.status().is_success() {
            return Err(McpError::internal_error(
                format!("Forgejo returned {}", resp.status()),
                None,
            ));
        }

        resp.text()
            .await
            .map_err(|e| McpError::internal_error(format!("Response error: {e}"), None))
    }

    /// POST to a Forgejo API path with a JSON body, returning the raw response text.
    async fn forgejo_post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<String, McpError> {
        let (url, token, client) = {
            let s = self.state.read().await;
            let base = s.forgejo_url.as_ref().ok_or_else(|| {
                McpError::internal_error("Forgejo not configured", None)
            })?;
            let token = s.forgejo_token.clone().unwrap_or_default();
            (
                format!("{}/api/v1{}", base.trim_end_matches('/'), path),
                token,
                s.http_client.clone(),
            )
        };

        let resp = client
            .post(&url)
            .header("Authorization", format!("token {token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| McpError::internal_error(format!("Forgejo unreachable: {e}"), None))?;

        if !resp.status().is_success() {
            return Err(McpError::internal_error(
                format!("Forgejo returned {}", resp.status()),
                None,
            ));
        }

        // Some Forgejo POSTs return 204 No Content
        if resp.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(r#"{"status":"ok"}"#.to_string());
        }

        resp.text()
            .await
            .map_err(|e| McpError::internal_error(format!("Response error: {e}"), None))
    }
}

// ---------------------------------------------------------------------------
// Parameter structs for tools that accept arguments
// ---------------------------------------------------------------------------

/// Parameters for tools that accept a workflow filename.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkflowParams {
    /// Workflow filename, e.g. "ci.yaml".
    pub workflow: String,
}

/// Parameters for tools that accept a CI run ID.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunIdParams {
    /// Numeric CI run ID.
    pub run_id: i64,
}

// ---------------------------------------------------------------------------
// MCP tool definitions
// ---------------------------------------------------------------------------

#[tool_router]
impl RemiMcpServer {
    /// List all CI/CD workflows configured in the Forgejo repository.
    ///
    /// Returns workflow names and filenames. Use the filename (e.g.
    /// `ci.yaml`) with the `ci_list_runs` and `ci_dispatch` tools.
    #[tool(description = "List all CI/CD workflows. Returns workflow names and filenames. Use the filename (e.g. 'ci.yaml') with ci_list_runs and ci_dispatch.")]
    async fn ci_list_workflows(&self) -> Result<CallToolResult, McpError> {
        let text = self
            .forgejo_get("/repos/peter/Conary/actions/workflows")
            .await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// List recent CI runs for a specific workflow.
    #[tool(description = "List recent CI runs for a workflow. The 'workflow' param is the filename, e.g. 'ci.yaml'.")]
    async fn ci_list_runs(
        &self,
        Parameters(params): Parameters<WorkflowParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_path_param(&params.workflow, "workflow")?;
        let text = self
            .forgejo_get(&format!(
                "/repos/peter/Conary/actions/workflows/{}/runs",
                params.workflow
            ))
            .await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get details for a specific CI run including job statuses.
    #[tool(description = "Get details for a specific CI run including job statuses.")]
    async fn ci_get_run(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let text = self
            .forgejo_get(&format!(
                "/repos/peter/Conary/actions/runs/{}",
                params.run_id
            ))
            .await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get raw log output for a CI run.  Can be large.
    #[tool(description = "Get raw log output for a CI run. Can be large.")]
    async fn ci_get_logs(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let text = self
            .forgejo_get(&format!(
                "/repos/peter/Conary/actions/runs/{}/logs",
                params.run_id
            ))
            .await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Trigger a new CI workflow run on the main branch.
    ///
    /// **Not idempotent** — every call queues a new run.
    #[tool(description = "Trigger a new CI workflow run on main. NOT idempotent — every call queues a new run.")]
    async fn ci_dispatch(
        &self,
        Parameters(params): Parameters<WorkflowParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_path_param(&params.workflow, "workflow")?;
        let text = self
            .forgejo_post(
                &format!(
                    "/repos/peter/Conary/actions/workflows/{}/dispatches",
                    params.workflow
                ),
                serde_json::json!({"ref": "main"}),
            )
            .await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Force an immediate GitHub mirror sync.
    ///
    /// Without this, the mirror polls every 10 minutes.
    #[tool(description = "Force GitHub mirror sync. Normally the mirror polls every 10 minutes.")]
    async fn ci_mirror_sync(&self) -> Result<CallToolResult, McpError> {
        let text = self
            .forgejo_post(
                "/repos/peter/Conary/mirror-sync",
                serde_json::json!({}),
            )
            .await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// List all admin API tokens with names, scopes, and last-used timestamps.
    ///
    /// Token hashes are redacted — only metadata is returned.
    #[tool(description = "List all admin API tokens with names, scopes, and last-used timestamps. Token hashes are redacted.")]
    async fn list_tokens(&self) -> Result<CallToolResult, McpError> {
        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let result = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            conary_core::db::models::admin_token::list(&conn)
                .map_err(|e| McpError::internal_error(format!("DB query error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler implementation
// ---------------------------------------------------------------------------

impl ServerHandler for RemiMcpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new("remi-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "Remi MCP server -- manage CI workflows, inspect runs, \
             trigger builds, sync mirrors, and list admin tokens.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that the MCP server can be constructed and its info is correct.
    #[tokio::test]
    async fn test_mcp_server_info() {
        let config = crate::server::ServerConfig::default();
        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));
        let server = RemiMcpServer::new(state);

        let info = server.get_info();
        assert_eq!(info.server_info.name, "remi-mcp");
    }

    /// Verify the tool router registers the expected number of tools.
    #[test]
    fn test_mcp_tool_count() {
        // Build the tool router directly to inspect registered tools
        let router = RemiMcpServer::tool_router();
        let tools = router.list_all();
        assert_eq!(tools.len(), 7, "Expected 7 MCP tools");
    }
}
