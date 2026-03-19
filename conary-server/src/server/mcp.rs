// conary-server/src/server/mcp.rs
//! MCP (Model Context Protocol) server for LLM agent integration.
//!
//! Exposes Remi admin operations as MCP tools so that LLM agents (Claude,
//! etc.) can inspect CI status, trigger workflows, manage tokens, and
//! force mirror syncs through a standardised protocol.
//!
//! The MCP endpoint is mounted on the external admin router at `/mcp` and
//! sits behind the same Bearer-token auth middleware as other admin endpoints.
//!
//! DB-touching tools delegate to [`crate::server::admin_service`] so that
//! business logic is shared with the HTTP admin handlers.

use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;

use conary_core::mcp::{to_json_text, validate_path_param};
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

use crate::server::ServerState;
use crate::server::admin_service::{self, AddPeerInput, ServiceError};
use crate::server::forgejo::FORGEJO_REPO_PATH;

/// Map a [`ServiceError`] to the appropriate [`McpError`] variant.
///
/// Each variant maps to a distinct JSON-RPC error code so that callers
/// can distinguish between bad input, missing resources, conflicts, and
/// internal failures.
fn service_err_to_mcp(e: ServiceError) -> McpError {
    match e {
        ServiceError::BadRequest(msg) => McpError::invalid_params(msg, None),
        ServiceError::NotFound(msg) => McpError::resource_not_found(msg, None),
        ServiceError::Conflict(msg) => McpError::invalid_request(msg, None),
        ServiceError::Internal(msg) => McpError::internal_error(msg, None),
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
// Forgejo proxy helpers (map ForgejoError -> McpError)
// ---------------------------------------------------------------------------

/// Map a [`crate::server::forgejo::ForgejoError`] to an MCP internal error.
fn forgejo_err_to_mcp(e: crate::server::forgejo::ForgejoError) -> McpError {
    McpError::internal_error(e.message, None)
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

/// Parameters for creating an admin API token.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateTokenParams {
    /// Human-readable name for the token (1-128 characters).
    pub name: String,
    /// Comma-separated scopes (defaults to "admin" if omitted).
    #[serde(default)]
    pub scopes: Option<String>,
}

/// Parameters for deleting an admin API token.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTokenParams {
    /// ID of the token to delete.
    pub token_id: i64,
}

/// Parameters for getting a specific repository.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RepoNameParams {
    /// Repository name.
    pub name: String,
}

/// Parameters for adding a federation peer.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddPeerParams {
    /// HTTP(S) endpoint URL of the peer.
    pub endpoint: String,
    /// Peer tier: "leaf", "cell_hub", or "region_hub". Defaults to "leaf".
    #[serde(default)]
    pub tier: Option<String>,
}

/// Parameters for operations on a specific peer.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeerIdParams {
    /// SHA-256 hash ID of the peer.
    pub peer_id: String,
}

/// Parameters for querying the audit log.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryAuditParams {
    /// Max entries to return (default 50, max 500).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Filter by action prefix (e.g., "repo" matches "repo.create").
    #[serde(default)]
    pub action: Option<String>,
    /// Only entries after this ISO 8601 timestamp.
    #[serde(default)]
    pub since: Option<String>,
    /// Filter by token name.
    #[serde(default)]
    pub token_name: Option<String>,
}

/// Parameters for purging old audit entries.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PurgeAuditParams {
    /// Delete entries older than this ISO 8601 timestamp.
    pub before: String,
}

/// Parameters for listing test runs.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TestListRunsParams {
    /// Maximum number of runs to return (default 20, max 100).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Cursor for pagination (run ID to start after).
    #[serde(default)]
    pub cursor: Option<i64>,
    /// Filter by suite name.
    #[serde(default)]
    pub suite: Option<String>,
    /// Filter by distro name.
    #[serde(default)]
    pub distro: Option<String>,
    /// Filter by status (pending, running, completed, failed, cancelled).
    #[serde(default)]
    pub status: Option<String>,
}

/// Parameters for getting a specific test run.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TestGetRunParams {
    /// Numeric run ID.
    pub run_id: i64,
}

/// Parameters for getting a specific test result.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TestGetTestParams {
    /// Numeric run ID.
    pub run_id: i64,
    /// Test identifier (e.g. "T01").
    pub test_id: String,
}

/// Parameters for getting test execution logs.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TestGetLogsParams {
    /// Numeric run ID.
    pub run_id: i64,
    /// Test identifier (e.g. "T01").
    pub test_id: String,
    /// Filter by log stream: stdout, stderr, or trace.
    #[serde(default)]
    pub stream: Option<String>,
    /// Filter by step index (0-based).
    #[serde(default)]
    pub step_index: Option<u32>,
}

/// Parameters for the chunk garbage collection tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChunkGcParams {
    /// Show what would be deleted without deleting (default false).
    #[serde(default)]
    pub dry_run: Option<bool>,
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
    #[tool(
        description = "List all CI/CD workflows. Returns workflow names and filenames. Use the filename (e.g. 'ci.yaml') with ci_list_runs and ci_dispatch."
    )]
    async fn ci_list_workflows(&self) -> Result<CallToolResult, McpError> {
        let text = crate::server::forgejo::get(
            &self.state,
            &format!("{FORGEJO_REPO_PATH}/actions/workflows"),
        )
        .await
        .map_err(forgejo_err_to_mcp)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// List recent CI runs for a specific workflow.
    #[tool(
        description = "List recent CI runs for a workflow. The 'workflow' param is the filename, e.g. 'ci.yaml'."
    )]
    async fn ci_list_runs(
        &self,
        Parameters(params): Parameters<WorkflowParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_path_param(&params.workflow, "workflow")?;
        let text = crate::server::forgejo::get(
            &self.state,
            &format!(
                "{FORGEJO_REPO_PATH}/actions/workflows/{}/runs",
                params.workflow
            ),
        )
        .await
        .map_err(forgejo_err_to_mcp)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get details for a specific CI run including job statuses.
    #[tool(description = "Get details for a specific CI run including job statuses.")]
    async fn ci_get_run(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let text = crate::server::forgejo::get(
            &self.state,
            &format!("{FORGEJO_REPO_PATH}/actions/runs/{}", params.run_id),
        )
        .await
        .map_err(forgejo_err_to_mcp)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get raw log output for a CI run.  Can be large.
    #[tool(description = "Get raw log output for a CI run. Can be large.")]
    async fn ci_get_logs(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let text = crate::server::forgejo::get(
            &self.state,
            &format!("{FORGEJO_REPO_PATH}/actions/runs/{}/logs", params.run_id),
        )
        .await
        .map_err(forgejo_err_to_mcp)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Trigger a new CI workflow run on the main branch.
    ///
    /// **Not idempotent** -- every call queues a new run.
    #[tool(
        description = "Trigger a new CI workflow run on main. NOT idempotent -- every call queues a new run."
    )]
    async fn ci_dispatch(
        &self,
        Parameters(params): Parameters<WorkflowParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_path_param(&params.workflow, "workflow")?;
        let body = serde_json::json!({"ref": "main"});
        let text = crate::server::forgejo::post(
            &self.state,
            &format!(
                "{FORGEJO_REPO_PATH}/actions/workflows/{}/dispatches",
                params.workflow
            ),
            Some(&body),
        )
        .await
        .map_err(forgejo_err_to_mcp)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Force an immediate GitHub mirror sync.
    ///
    /// Without this, the mirror polls every 10 minutes.
    #[tool(description = "Force GitHub mirror sync. Normally the mirror polls every 10 minutes.")]
    async fn ci_mirror_sync(&self) -> Result<CallToolResult, McpError> {
        let text = crate::server::forgejo::post(
            &self.state,
            &format!("{FORGEJO_REPO_PATH}/mirror-sync"),
            None,
        )
        .await
        .map_err(forgejo_err_to_mcp)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    // -----------------------------------------------------------------------
    // Token management (delegates to admin_service)
    // -----------------------------------------------------------------------

    /// List all admin API tokens with names, scopes, and last-used timestamps.
    ///
    /// Token hashes are redacted -- only metadata is returned.
    #[tool(
        description = "List all admin API tokens with names, scopes, and last-used timestamps. Token hashes are redacted."
    )]
    async fn list_tokens(&self) -> Result<CallToolResult, McpError> {
        let tokens = admin_service::list_tokens(&self.state)
            .await
            .map_err(service_err_to_mcp)?;

        let text = to_json_text(&tokens)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Create a new admin API token. Returns the plaintext token once.
    ///
    /// The plaintext token is only shown in this response -- store it
    /// securely. Subsequent `list_tokens` calls only return metadata.
    #[tool(
        description = "Create a new admin API token. Returns the plaintext token once -- store it securely."
    )]
    async fn create_token(
        &self,
        Parameters(params): Parameters<CreateTokenParams>,
    ) -> Result<CallToolResult, McpError> {
        let created =
            admin_service::create_token(&self.state, &params.name, params.scopes.as_deref())
                .await
                .map_err(service_err_to_mcp)?;

        let result = serde_json::json!({
            "id": created.id,
            "name": created.name,
            "token": created.raw_token,
            "scopes": created.scopes,
        });
        let text = to_json_text(&result)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Delete an admin API token by ID.
    #[tool(description = "Delete an admin API token by ID. Returns success or error if not found.")]
    async fn delete_token(
        &self,
        Parameters(params): Parameters<DeleteTokenParams>,
    ) -> Result<CallToolResult, McpError> {
        let deleted = admin_service::delete_token(&self.state, params.token_id)
            .await
            .map_err(service_err_to_mcp)?;

        if deleted {
            let result = serde_json::json!({"status": "deleted", "token_id": params.token_id});
            let text = to_json_text(&result)?;
            Ok(CallToolResult::success(vec![Content::text(text)]))
        } else {
            Err(McpError::invalid_params(
                format!("Token with ID {} not found", params.token_id),
                None,
            ))
        }
    }

    // -----------------------------------------------------------------------
    // Repository management (delegates to admin_service)
    // -----------------------------------------------------------------------

    /// List all configured repositories.
    #[tool(
        description = "List all configured repositories with name, URL, enabled status, priority, last sync, and GPG settings."
    )]
    async fn list_repos(&self) -> Result<CallToolResult, McpError> {
        let repos = admin_service::list_repos(&self.state)
            .await
            .map_err(service_err_to_mcp)?;

        let json: Vec<serde_json::Value> = repos
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "name": r.name,
                    "url": r.url,
                    "enabled": r.enabled,
                    "priority": r.priority,
                    "last_sync": r.last_sync,
                    "gpg_check": r.gpg_check,
                })
            })
            .collect();

        let text = to_json_text(&json)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get details for a specific repository by name.
    #[tool(description = "Get full details for a specific repository by name.")]
    async fn get_repo(
        &self,
        Parameters(params): Parameters<RepoNameParams>,
    ) -> Result<CallToolResult, McpError> {
        let repo = admin_service::get_repo(&self.state, &params.name)
            .await
            .map_err(service_err_to_mcp)?;

        match repo {
            Some(r) => {
                let result = serde_json::json!({
                    "id": r.id,
                    "name": r.name,
                    "url": r.url,
                    "content_url": r.content_url,
                    "enabled": r.enabled,
                    "priority": r.priority,
                    "gpg_check": r.gpg_check,
                    "gpg_strict": r.gpg_strict,
                    "gpg_key_url": r.gpg_key_url,
                    "metadata_expire": r.metadata_expire,
                    "last_sync": r.last_sync,
                    "created_at": r.created_at,
                    "default_strategy": r.default_strategy,
                });
                let text = to_json_text(&result)?;
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            None => Err(McpError::invalid_params(
                format!("Repository '{}' not found", params.name),
                None,
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Federation peer management (delegates to admin_service)
    // -----------------------------------------------------------------------

    /// List all federation peers with health information.
    #[tool(
        description = "List all federation peers with endpoint, tier, last seen, success rate, and enabled status."
    )]
    async fn list_peers(&self) -> Result<CallToolResult, McpError> {
        let peers = admin_service::list_peers(&self.state)
            .await
            .map_err(service_err_to_mcp)?;

        let json: Vec<serde_json::Value> = peers
            .iter()
            .map(|p| {
                let total = p.success_count + p.failure_count;
                let success_rate = if total > 0 {
                    format!("{:.1}%", (p.success_count as f64 / total as f64) * 100.0)
                } else {
                    "N/A".to_string()
                };
                serde_json::json!({
                    "id": p.id,
                    "endpoint": p.endpoint,
                    "node_name": p.node_name,
                    "tier": p.tier,
                    "last_seen": p.last_seen,
                    "success_rate": success_rate,
                    "total_requests": total,
                    "consecutive_failures": p.consecutive_failures,
                    "enabled": p.is_enabled,
                })
            })
            .collect();

        let text = to_json_text(&json)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Add a federation peer by endpoint URL.
    ///
    /// The peer ID is derived from the SHA-256 hash of the endpoint.
    /// Returns an error if the peer already exists.
    #[tool(
        description = "Add a federation peer by endpoint URL. Peer ID is derived from SHA-256 of the endpoint. Returns an error if the peer already exists."
    )]
    async fn add_peer(
        &self,
        Parameters(params): Parameters<AddPeerParams>,
    ) -> Result<CallToolResult, McpError> {
        let input = AddPeerInput {
            endpoint: params.endpoint,
            tier: params.tier,
            node_name: None,
        };

        let (peer_id, peer) = admin_service::add_peer(&self.state, input)
            .await
            .map_err(service_err_to_mcp)?;

        let result = serde_json::json!({
            "id": peer_id,
            "endpoint": peer.endpoint,
            "tier": peer.tier,
        });
        let text = to_json_text(&result)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Delete a federation peer by its SHA-256 hash ID.
    #[tool(
        description = "Delete a federation peer by its SHA-256 hash ID. Returns success or error if not found."
    )]
    async fn delete_peer(
        &self,
        Parameters(params): Parameters<PeerIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let deleted = admin_service::delete_peer(&self.state, &params.peer_id)
            .await
            .map_err(service_err_to_mcp)?;

        if deleted {
            let result = serde_json::json!({"status": "deleted", "peer_id": params.peer_id});
            let text = to_json_text(&result)?;
            Ok(CallToolResult::success(vec![Content::text(text)]))
        } else {
            Err(McpError::invalid_params(
                format!("Peer with ID '{}' not found", params.peer_id),
                None,
            ))
        }
    }

    // -----------------------------------------------------------------------
    // Audit log (delegates to admin_service)
    // -----------------------------------------------------------------------

    /// Query the admin audit log. Returns recent API operations with timing
    /// and (for writes) request/response bodies.
    #[tool(
        description = "Query admin audit log. Supports filters: limit, action prefix, since timestamp, token_name."
    )]
    async fn query_audit_log(
        &self,
        Parameters(params): Parameters<QueryAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        let entries = admin_service::query_audit(
            &self.state,
            params.limit,
            params.action,
            params.since,
            params.token_name,
        )
        .await
        .map_err(service_err_to_mcp)?;

        let text = to_json_text(&entries)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Purge old audit log entries. Deletes entries older than the given date.
    ///
    /// **Not idempotent** -- deleted entries cannot be recovered.
    #[tool(
        description = "Delete audit log entries older than a given ISO 8601 date. NOT reversible."
    )]
    async fn purge_audit_log(
        &self,
        Parameters(params): Parameters<PurgeAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        let deleted = admin_service::purge_audit(&self.state, &params.before)
            .await
            .map_err(service_err_to_mcp)?;

        let result = serde_json::json!({
            "deleted": deleted,
            "before": params.before,
        });
        let text = to_json_text(&result)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    // -----------------------------------------------------------------------
    // Test data (delegates to admin_service)
    // -----------------------------------------------------------------------

    /// List recent test runs with optional filtering.
    #[tool(
        description = "List recent test runs with optional filtering by suite, distro, and status. Returns newest first with cursor-based pagination."
    )]
    async fn test_list_runs(
        &self,
        Parameters(params): Parameters<TestListRunsParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(20).min(100);
        let runs = admin_service::list_test_runs(
            &self.state,
            limit,
            params.cursor,
            params.suite,
            params.distro,
            params.status,
        )
        .await
        .map_err(service_err_to_mcp)?;
        let text = to_json_text(&runs)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get full details for a test run including all test result summaries.
    #[tool(description = "Get full details for a test run including all test result summaries.")]
    async fn test_get_run(
        &self,
        Parameters(params): Parameters<TestGetRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let detail = admin_service::get_test_run_detail(&self.state, params.run_id)
            .await
            .map_err(service_err_to_mcp)?;
        let text = to_json_text(&detail)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get a single test result with all execution steps and their logs.
    #[tool(description = "Get a single test result with all execution steps and their logs.")]
    async fn test_get_test(
        &self,
        Parameters(params): Parameters<TestGetTestParams>,
    ) -> Result<CallToolResult, McpError> {
        let detail = admin_service::get_test_detail(&self.state, params.run_id, params.test_id)
            .await
            .map_err(service_err_to_mcp)?;
        let text = to_json_text(&detail)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get test execution logs, optionally filtered by stream and step index.
    #[tool(
        description = "Get test execution logs, optionally filtered by stream (stdout/stderr) and step index."
    )]
    async fn test_get_logs(
        &self,
        Parameters(params): Parameters<TestGetLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let logs = admin_service::get_test_logs(
            &self.state,
            params.run_id,
            params.test_id,
            params.stream,
            params.step_index,
        )
        .await
        .map_err(service_err_to_mcp)?;
        let text = to_json_text(&logs)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get aggregate test health: total runs, recent activity, and pass/fail
    /// summary.
    #[tool(
        description = "Get aggregate test health: total runs, recent activity, and pass/fail summary."
    )]
    async fn test_health(&self) -> Result<CallToolResult, McpError> {
        let health = admin_service::test_health(&self.state)
            .await
            .map_err(service_err_to_mcp)?;
        let text = to_json_text(&health)?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    // -----------------------------------------------------------------------
    // Canonical mapping
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Chunk garbage collection
    // -----------------------------------------------------------------------

    /// Garbage collect orphaned chunks from local disk and R2.
    ///
    /// Finds chunks not referenced by any converted package and deletes
    /// them.  Use `dry_run = true` to preview without deleting.
    #[tool(
        description = "Garbage collect orphaned chunks from local disk and R2. Finds chunks not referenced by any converted package and deletes them. Use dry_run=true to preview."
    )]
    async fn chunk_gc(
        &self,
        Parameters(params): Parameters<ChunkGcParams>,
    ) -> Result<CallToolResult, McpError> {
        let dry_run = params.dry_run.unwrap_or(false);
        let grace_period_secs: u64 = 3600; // 1 hour grace period

        let state = self.state.read().await;
        let db_path = state.config.db_path.clone();
        let objects_dir = state.config.chunk_dir.join("objects");
        let r2_store = state.r2_store.clone();
        drop(state);

        let gc_result = crate::server::chunk_gc::run_chunk_gc(
            &db_path,
            &objects_dir,
            r2_store,
            dry_run,
            grace_period_secs,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let text = to_json_text(&serde_json::json!({
            "dry_run": dry_run,
            "referenced": gc_result.referenced,
            "local_scanned": gc_result.local_scanned,
            "r2_scanned": gc_result.r2_scanned,
            "local_deleted": gc_result.local_deleted,
            "r2_deleted": gc_result.r2_deleted,
            "local_bytes_freed": gc_result.local_bytes_freed,
            "r2_bytes_freed": gc_result.r2_bytes_freed,
        }))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    // -----------------------------------------------------------------------
    // Canonical mapping
    // -----------------------------------------------------------------------

    /// Rebuild the canonical package mapping from all indexed distros.
    ///
    /// Runs auto-discovery and curated rules to create cross-distro name
    /// equivalences.
    #[tool(
        description = "Rebuild the canonical package mapping from all indexed distros. Runs auto-discovery and curated rules to create cross-distro name equivalences."
    )]
    async fn canonical_rebuild(&self) -> Result<CallToolResult, McpError> {
        let state = self.state.read().await;
        let db_path = state.config.db_path.clone();
        drop(state);

        let config = crate::server::config::CanonicalSection {
            rules_dir: "data/canonical-rules".to_string(),
            ..Default::default()
        };

        let count = tokio::task::spawn_blocking(move || {
            crate::server::canonical_job::rebuild_canonical_map(&db_path, &config)
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let text = to_json_text(&serde_json::json!({
            "status": "ok",
            "new_mappings": count,
        }))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler implementation
// ---------------------------------------------------------------------------

impl ServerHandler for RemiMcpServer {
    fn get_info(&self) -> ServerInfo {
        conary_core::mcp::server_info(
            "remi-mcp",
            env!("CARGO_PKG_VERSION"),
            "Remi MCP server -- manage CI workflows, inspect runs, \
             trigger builds, sync mirrors, manage admin tokens, \
             list/inspect repositories, manage federation peers, \
             query/purge the admin audit log, and inspect test \
             run data and health.",
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
        assert_eq!(tools.len(), 23, "Expected 23 MCP tools");
    }
}
