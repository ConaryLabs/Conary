// apps/remi/src/server/mcp.rs
//! MCP (Model Context Protocol) server for LLM agent integration.
//!
//! Exposes Remi admin operations as MCP tools so that LLM agents can manage
//! tokens, repositories, federation peers, audit data,
//! test harness state, chunk garbage collection, and canonical mappings
//! through a standardised protocol.
//!
//! The MCP endpoint is mounted on the external admin router at `/mcp` and
//! sits behind the same Bearer-token auth middleware as other admin endpoints.
//!
//! DB-touching tools delegate to [`crate::server::admin_service`] so that
//! business logic is shared with the HTTP admin handlers.

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;

use conary_mcp::{server_info, to_json_text};
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
use crate::server::admin_service::{self, AddPeerInput, ChunkGcReport, ServiceError};
use crate::server::r2_durability::{DEFAULT_BACKFILL_CONCURRENCY, R2DurabilityMode};

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
        ServiceError::StorageCapacity(error) => McpError::invalid_request(error.to_string(), None),
        ServiceError::Internal(msg) => McpError::internal_error(msg, None),
    }
}

/// Render a [`ChunkGcReport`] as the `chunk_gc` tool's output object.
///
/// Going through [`serde_json::Value`] keeps the tool's established key order,
/// which is independent of the report struct's field order.
fn chunk_gc_tool_output(report: &ChunkGcReport) -> Result<serde_json::Value, McpError> {
    serde_json::to_value(report)
        .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))
}

/// MCP server instance that wraps Remi admin operations as tools.
///
/// Each MCP request gets a fresh `RemiMcpServer` clone, while all requests
/// share the same `Arc<RwLock<ServerState>>` for Remi state.
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
// Parameter structs for tools that accept arguments
// ---------------------------------------------------------------------------

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
    /// Pinned SHA-256 TLS certificate fingerprint for HTTPS peers.
    #[serde(default)]
    pub tls_fingerprint: Option<String>,
}

/// Parameters for operations on a specific peer.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeerIdParams {
    /// Peer ID (endpoint hash for HTTP peers, TLS fingerprint for HTTPS peers).
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

/// Parameters for the R2 durability inventory and backfill tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct R2DurabilityParams {
    /// Read-only plan or explicit apply. Defaults to plan.
    #[serde(default)]
    pub mode: Option<R2DurabilityMode>,
    /// Maximum concurrent R2 PUT requests. Defaults to 16 and cannot exceed 64.
    #[serde(default)]
    pub concurrency: Option<usize>,
}

// ---------------------------------------------------------------------------
// MCP tool definitions
// ---------------------------------------------------------------------------

#[tool_router]
impl RemiMcpServer {
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Create a new admin API token. Returns the plaintext token once.
    ///
    /// The plaintext token is only shown in this response -- store it
    /// securely. Subsequent `list_tokens` calls only return metadata.
    #[tool(
        description = "Create a new admin API token. Returns the plaintext token once -- store it securely. Risk: high. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Delete an admin API token by ID.
    #[tool(
        description = "Delete an admin API token by ID. Returns success or error if not found. Risk: high/destructive. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
    )]
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
            Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        description = "List all configured repositories with name, URL, enabled status, priority, distinct refresh timestamps, parser, and ecosystem-native trust policy."
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
                    "last_checked_at": r.last_checked_at,
                    "last_changed_at": r.last_changed_at,
                    "last_validated_at": r.last_validated_at,
                    "last_published_at": r.last_published_at,
                    "package_format": r.package_format,
                    "parser": r.parser_config,
                    "trust": r.trust_policy,
                })
            })
            .collect();

        let text = to_json_text(&json)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
                    "package_format": r.package_format,
                    "parser": r.parser_config,
                    "trust": r.trust_policy,
                    "metadata_expire": r.metadata_expire,
                    "last_checked_at": r.last_checked_at,
                    "last_changed_at": r.last_changed_at,
                    "last_validated_at": r.last_validated_at,
                    "last_published_at": r.last_published_at,
                    "created_at": r.created_at,
                    "default_strategy": r.default_strategy,
                });
                let text = to_json_text(&result)?;
                Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Add a federation peer by endpoint URL.
    ///
    /// HTTPS peers require a pinned TLS certificate fingerprint, which becomes
    /// the peer ID. HTTP peers use a hash of the endpoint.
    /// Returns an error if the peer already exists.
    #[tool(
        description = "Add a federation peer by endpoint URL. HTTPS peers require a pinned TLS certificate fingerprint, which becomes the peer ID. HTTP peers use a SHA-256 hash of the endpoint. Returns an error if the peer already exists. Risk: high. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
    )]
    async fn add_peer(
        &self,
        Parameters(params): Parameters<AddPeerParams>,
    ) -> Result<CallToolResult, McpError> {
        let input = AddPeerInput {
            endpoint: params.endpoint,
            tier: params.tier,
            node_name: None,
            tls_fingerprint: params.tls_fingerprint,
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Delete a federation peer by its peer ID.
    #[tool(
        description = "Delete a federation peer by its peer ID. Returns success or error if not found. Risk: high/destructive. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
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
            Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Purge old audit log entries. Deletes entries older than the given date.
    ///
    /// **Not idempotent** -- deleted entries cannot be recovered.
    #[tool(
        description = "Delete audit log entries older than a given ISO 8601 date. NOT reversible. Risk: destructive. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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
    /// them.  Omitted `dry_run` defaults to preview mode; set `dry_run = false`
    /// only after confirmation.
    #[tool(
        description = "Garbage collect orphaned chunks from local disk and R2. Finds chunks not referenced by any converted package and deletes them when dry_run=false. Omitted dry_run defaults to preview mode. Risk: destructive. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
    )]
    async fn chunk_gc(
        &self,
        Parameters(params): Parameters<ChunkGcParams>,
    ) -> Result<CallToolResult, McpError> {
        let dry_run = params.dry_run.unwrap_or(true);

        let report = admin_service::run_chunk_gc_op(&self.state, dry_run)
            .await
            .map_err(service_err_to_mcp)?;

        let text = to_json_text(&chunk_gc_tool_output(&report)?)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Inventory local and R2 chunk durability or apply an exact backfill.
    #[tool(
        description = "Inventory exact local and R2 chunk counts, bytes, and published-object completeness. Omitted mode defaults to a read-only plan; mode=apply verifies local SHA-256 before uploading only missing objects and rechecks R2 afterward. Risk: high. Requires plan-then-apply confirmation."
    )]
    async fn r2_durability(
        &self,
        Parameters(params): Parameters<R2DurabilityParams>,
    ) -> Result<CallToolResult, McpError> {
        let report = admin_service::run_r2_durability_op(
            &self.state,
            params.mode.unwrap_or(R2DurabilityMode::Plan),
            params.concurrency.unwrap_or(DEFAULT_BACKFILL_CONCURRENCY),
        )
        .await
        .map_err(service_err_to_mcp)?;
        let text = to_json_text(&report)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    // -----------------------------------------------------------------------
    // Canonical mapping
    // -----------------------------------------------------------------------

    /// Rebuild the canonical package mapping from exact versioned contracts.
    ///
    /// AppStream may enrich an existing exact mapping with metadata. Repology
    /// discovery data never creates or ranks a mutation-authoritative mapping.
    #[tool(
        description = "Rebuild canonical package mappings from exact versioned contracts, then attach non-authoritative AppStream metadata to already-mapped packages. Repology discovery data cannot create or rank mappings. Risk: medium. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
    )]
    async fn canonical_rebuild(&self) -> Result<CallToolResult, McpError> {
        let state = self.state.read().await;
        let db_path = state.config.db_path.clone();
        let config = state.canonical_config.clone();
        let database_writer = state.database_writer.clone();
        let publication_coordinator = state.publication_coordinator.clone();
        drop(state);
        let _publication_guard = publication_coordinator.lock_owned().await;

        let count = tokio::task::spawn_blocking(move || {
            crate::server::canonical_job::rebuild_canonical_map(&db_path, &config, &database_writer)
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        crate::server::universe_publish::publish_current_universe_from_state(&self.state)
            .await
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;

        let text = to_json_text(&serde_json::json!({
            "status": "ok",
            "new_mappings": count,
        }))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    /// Trigger an immediate Repology + AppStream fetch and rebuild cycle.
    ///
    /// Populates discovery caches and rebuilds exact canonical contracts.
    /// Every source and rebuild phase returns its typed persistence outcome;
    /// two failed sources can never masquerade as a successful zero-row fetch.
    #[tool(
        description = "Refresh Repology and AppStream discovery caches, then rebuild exact canonical contracts. AppStream may enrich an existing exact contract mapping; neither source may establish mapping or ranking authority. Returns typed source persistence and rebuild outcomes. Risk: medium. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface."
    )]
    async fn canonical_fetch(&self) -> Result<CallToolResult, McpError> {
        let state = self.state.read().await;
        let db_path = state.config.db_path.clone();
        let config = state.canonical_config.clone();
        let database_writer = state.database_writer.clone();
        let publication_coordinator = state.publication_coordinator.clone();
        drop(state);
        let _publication_guard = publication_coordinator.lock_owned().await;

        let report =
            crate::server::canonical_fetch::run_canonical_cycle(&db_path, &config, database_writer)
                .await;
        let report = if matches!(
            report.rebuild,
            crate::server::canonical_fetch::CanonicalRebuildOutcome::Completed { .. }
        ) {
            match crate::server::universe_publish::publish_current_universe_from_state(&self.state)
                .await
            {
                Ok(_) => report,
                Err(error) => report.with_publication_failure(&error),
            }
        } else {
            report
        };
        crate::server::publication_scheduler::record_canonical_readiness(&self.state, &report)
            .await;
        let text = to_json_text(&report)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler implementation
// ---------------------------------------------------------------------------

impl ServerHandler for RemiMcpServer {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    fn get_info(&self) -> ServerInfo {
        server_info(
            "remi-mcp",
            env!("CARGO_PKG_VERSION"),
            "Remi MCP server -- manage admin tokens, list and inspect \
             repositories, manage federation peers, query and purge \
             the admin audit log, inspect test run data and health, \
             garbage collect chunks, and maintain canonical mappings.",
        )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let mut tools = self.tool_router.list_all();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        std::future::ready(Ok(ListToolsResult {
            tools,
            ..Default::default()
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, McpError>> + Send + '_ {
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
        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("test server state"),
        ));
        let server = RemiMcpServer::new(state);

        let info = server.get_info();
        assert_eq!(info.server_info.name, "remi-mcp");
    }

    #[test]
    fn mcp_server_supports_only_the_modern_protocol_version() {
        let config = crate::server::ServerConfig::default();
        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("test server state"),
        ));
        let server = RemiMcpServer::new(state);

        assert_eq!(
            server.supported_protocol_versions().as_ref(),
            [ProtocolVersion::V_2026_07_28]
        );
    }

    #[test]
    fn chunk_gc_tool_output_matches_the_published_report_object() {
        let report = ChunkGcReport {
            dry_run: true,
            referenced: 1,
            local_scanned: 2,
            r2_scanned: 3,
            local_deleted: 4,
            r2_deleted: 5,
            local_bytes_freed: 6,
            r2_bytes_freed: 7,
        };

        let rendered = to_json_text(&chunk_gc_tool_output(&report).unwrap()).unwrap();
        let expected = to_json_text(&serde_json::json!({
            "dry_run": true,
            "referenced": 1,
            "local_scanned": 2,
            "r2_scanned": 3,
            "local_deleted": 4,
            "r2_deleted": 5,
            "local_bytes_freed": 6,
            "r2_bytes_freed": 7,
        }))
        .unwrap();

        assert_eq!(rendered, expected);
    }

    #[test]
    fn mcp_tool_catalog_records_context_budget_debt() {
        let tools = RemiMcpServer::tool_router().list_all();
        assert!(
            tools.len() <= 20,
            "Remi has {} MCP tools; split read-only/admin/mutation surfaces or document progressive discovery before adding more",
            tools.len()
        );
    }

    #[test]
    fn high_risk_tools_are_named_for_confirmation_review() {
        let tools = RemiMcpServer::tool_router().list_all();
        let names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
        assert!(names.iter().any(|name| name.contains("token")));
        assert!(names.iter().any(|name| name.contains("audit")));
    }

    #[test]
    fn high_risk_tool_descriptions_require_contract_confirmation() {
        let tools = RemiMcpServer::tool_router().list_all();
        for name in [
            "create_token",
            "delete_token",
            "add_peer",
            "delete_peer",
            "purge_audit_log",
            "chunk_gc",
            "r2_durability",
            "canonical_rebuild",
            "canonical_fetch",
        ] {
            let tool = tools
                .iter()
                .find(|tool| tool.name.as_ref() == name)
                .unwrap_or_else(|| panic!("missing high-risk tool: {name}"));
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(
                description.contains("Risk:"),
                "{name} should classify mutation risk"
            );
            assert!(
                description.contains("plan-then-apply confirmation"),
                "{name} should require contract confirmation"
            );
        }
    }

    #[test]
    fn test_mcp_tool_list_excludes_legacy_ci_bridge_tools() {
        let router = RemiMcpServer::tool_router();
        let tool_names: Vec<String> = router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();

        for forbidden in [
            "ci_list_workflows",
            "ci_list_runs",
            "ci_get_run",
            "ci_get_logs",
            "ci_dispatch",
            "ci_mirror_sync",
        ] {
            assert!(
                !tool_names.iter().any(|name| name == forbidden),
                "legacy Forgejo MCP tool should be absent: {forbidden}"
            );
        }
    }
}
