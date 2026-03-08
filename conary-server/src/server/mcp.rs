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
use crate::server::auth::{generate_token, hash_token};

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

    /// Create a new admin API token. Returns the plaintext token once.
    ///
    /// The plaintext token is only shown in this response — store it
    /// securely. Subsequent `list_tokens` calls only return metadata.
    #[tool(description = "Create a new admin API token. Returns the plaintext token once — store it securely.")]
    async fn create_token(
        &self,
        Parameters(params): Parameters<CreateTokenParams>,
    ) -> Result<CallToolResult, McpError> {
        let name = params.name.trim().to_string();
        if name.is_empty() || name.len() > 128 {
            return Err(McpError::invalid_params(
                "Token name must be 1-128 characters",
                None,
            ));
        }

        let scopes = params.scopes.unwrap_or_else(|| "admin".to_string());
        let raw_token = generate_token();
        let token_hash = hash_token(&raw_token);

        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let name_clone = name.clone();
        let scopes_clone = scopes.clone();
        let id = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            conary_core::db::models::admin_token::create(&conn, &name_clone, &token_hash, &scopes_clone)
                .map_err(|e| McpError::internal_error(format!("DB insert error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

        let result = serde_json::json!({
            "id": id,
            "name": name,
            "token": raw_token,
            "scopes": scopes,
        });
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Delete an admin API token by ID.
    #[tool(description = "Delete an admin API token by ID. Returns success or error if not found.")]
    async fn delete_token(
        &self,
        Parameters(params): Parameters<DeleteTokenParams>,
    ) -> Result<CallToolResult, McpError> {
        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let deleted = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            conary_core::db::models::admin_token::delete(&conn, params.token_id)
                .map_err(|e| McpError::internal_error(format!("DB delete error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

        if deleted {
            let result = serde_json::json!({"status": "deleted", "token_id": params.token_id});
            let text = serde_json::to_string_pretty(&result)
                .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;
            Ok(CallToolResult::success(vec![Content::text(text)]))
        } else {
            Err(McpError::invalid_params(
                format!("Token with ID {} not found", params.token_id),
                None,
            ))
        }
    }

    /// List all configured repositories.
    #[tool(description = "List all configured repositories with name, URL, enabled status, priority, last sync, and GPG settings.")]
    async fn list_repos(&self) -> Result<CallToolResult, McpError> {
        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let result = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            conary_core::db::models::Repository::list_all(&conn)
                .map_err(|e| McpError::internal_error(format!("DB query error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

        let repos: Vec<serde_json::Value> = result
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

        let text = serde_json::to_string_pretty(&repos)
            .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get details for a specific repository by name.
    #[tool(description = "Get full details for a specific repository by name.")]
    async fn get_repo(
        &self,
        Parameters(params): Parameters<RepoNameParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_path_param(&params.name, "name")?;

        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let name = params.name.clone();
        let repo = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            conary_core::db::models::Repository::find_by_name(&conn, &name)
                .map_err(|e| McpError::internal_error(format!("DB query error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

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
                let text = serde_json::to_string_pretty(&result)
                    .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            None => Err(McpError::invalid_params(
                format!("Repository '{}' not found", params.name),
                None,
            )),
        }
    }

    /// List all federation peers with health information.
    #[tool(description = "List all federation peers with endpoint, tier, last seen, success rate, and enabled status.")]
    async fn list_peers(&self) -> Result<CallToolResult, McpError> {
        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let result = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, endpoint, node_name, tier, last_seen,
                            success_count, failure_count, consecutive_failures, is_enabled
                     FROM federation_peers
                     ORDER BY tier, endpoint",
                )
                .map_err(|e| McpError::internal_error(format!("DB query error: {e}"), None))?;

            let rows = stmt
                .query_map([], |row| {
                    let success: i64 = row.get(5)?;
                    let failure: i64 = row.get(6)?;
                    let total = success + failure;
                    let rate = if total > 0 {
                        format!("{:.1}%", (success as f64 / total as f64) * 100.0)
                    } else {
                        "N/A".to_string()
                    };
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "endpoint": row.get::<_, String>(1)?,
                        "node_name": row.get::<_, Option<String>>(2)?,
                        "tier": row.get::<_, String>(3)?,
                        "last_seen": row.get::<_, String>(4)?,
                        "success_rate": rate,
                        "total_requests": total,
                        "consecutive_failures": row.get::<_, i64>(7)?,
                        "enabled": row.get::<_, bool>(8)?,
                    }))
                })
                .map_err(|e| McpError::internal_error(format!("DB query error: {e}"), None))?;

            let peers: Vec<serde_json::Value> = rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| McpError::internal_error(format!("DB row error: {e}"), None))?;

            Ok::<_, McpError>(peers)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Add a federation peer by endpoint URL.
    ///
    /// The peer ID is derived from the SHA-256 hash of the endpoint.
    /// Returns an error if the peer already exists.
    #[tool(description = "Add a federation peer by endpoint URL. Peer ID is derived from SHA-256 of the endpoint. Returns an error if the peer already exists.")]
    async fn add_peer(
        &self,
        Parameters(params): Parameters<AddPeerParams>,
    ) -> Result<CallToolResult, McpError> {
        // Validate endpoint URL
        let _url = url::Url::parse(&params.endpoint).map_err(|e| {
            McpError::invalid_params(format!("Invalid endpoint URL: {e}"), None)
        })?;

        let tier = params.tier.unwrap_or_else(|| "leaf".to_string());
        if !["leaf", "cell_hub", "region_hub"].contains(&tier.as_str()) {
            return Err(McpError::invalid_params(
                "Tier must be 'leaf', 'cell_hub', or 'region_hub'",
                None,
            ));
        }

        let peer_id = conary_core::hash::sha256(params.endpoint.as_bytes());

        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let endpoint = params.endpoint.clone();
        let tier_clone = tier.clone();
        let id_clone = peer_id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            conn.execute(
                "INSERT INTO federation_peers
                 (id, endpoint, tier, first_seen, last_seen,
                  latency_ms, success_count, failure_count, consecutive_failures, is_enabled)
                 VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 0, 0, 0, 0, 1)",
                rusqlite::params![id_clone, endpoint, tier_clone],
            )
            .map_err(|e| McpError::internal_error(format!("DB insert error: {e}"), None))?;
            Ok::<_, McpError>(())
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

        let result = serde_json::json!({
            "id": peer_id,
            "endpoint": params.endpoint,
            "tier": tier,
        });
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Delete a federation peer by its SHA-256 hash ID.
    #[tool(description = "Delete a federation peer by its SHA-256 hash ID. Returns success or error if not found.")]
    async fn delete_peer(
        &self,
        Parameters(params): Parameters<PeerIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let id = params.peer_id.clone();
        let affected = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB open error: {e}"), None))?;
            conn.execute(
                "DELETE FROM federation_peers WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(|e| McpError::internal_error(format!("DB delete error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))??;

        if affected > 0 {
            let result = serde_json::json!({"status": "deleted", "peer_id": params.peer_id});
            let text = serde_json::to_string_pretty(&result)
                .map_err(|e| McpError::internal_error(format!("Serialization error: {e}"), None))?;
            Ok(CallToolResult::success(vec![Content::text(text)]))
        } else {
            Err(McpError::invalid_params(
                format!("Peer with ID '{}' not found", params.peer_id),
                None,
            ))
        }
    }

    /// Query the admin audit log. Returns recent API operations with timing
    /// and (for writes) request/response bodies.
    #[tool(description = "Query admin audit log. Supports filters: limit, action prefix, since timestamp, token_name.")]
    async fn query_audit_log(
        &self,
        Parameters(params): Parameters<QueryAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        let db_path = { self.state.read().await.config.db_path.clone() };
        let result = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            conary_core::db::models::audit_log::query(
                &conn,
                params.limit,
                params.action.as_deref(),
                params.since.as_deref(),
                params.token_name.as_deref(),
            )
            .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )]))
    }

    /// Purge old audit log entries. Deletes entries older than the given date.
    ///
    /// **Not idempotent** -- deleted entries cannot be recovered.
    #[tool(description = "Delete audit log entries older than a given ISO 8601 date. NOT reversible.")]
    async fn purge_audit_log(
        &self,
        Parameters(params): Parameters<PurgeAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        let db_path = { self.state.read().await.config.db_path.clone() };
        let before = params.before.clone();
        let deleted = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            conary_core::db::models::audit_log::purge(&conn, &before)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&serde_json::json!({
                "deleted": deleted,
                "before": params.before,
            }))
            .unwrap_or_default(),
        )]))
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
             trigger builds, sync mirrors, manage admin tokens, \
             list/inspect repositories, manage federation peers, \
             and query/purge the admin audit log.",
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
        assert_eq!(tools.len(), 16, "Expected 16 MCP tools");
    }
}
