// conary-server/src/server/admin_service.rs
//! Shared business logic for admin operations.
//!
//! This module extracts the common `spawn_blocking` + `db::open_fast` pattern
//! from the admin HTTP handlers into reusable async functions.  Handlers become
//! thin wrappers: check scopes, call a service function, map errors to HTTP
//! responses, and publish SSE events where appropriate.
//!
//! The service layer is also the integration point for MCP tool handlers,
//! which need the same business logic without HTTP framing.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use conary_core::db::models::Repository;
use conary_core::db::models::admin_token::AdminToken;
use conary_core::db::models::audit_log::AuditEntry;
use conary_core::db::models::federation_peer::FederationPeer;

use crate::server::ServerState;
use crate::server::auth::{generate_token, hash_token, validate_scopes};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by service-layer operations.
///
/// Handlers map these to HTTP status codes; MCP tools map them to tool errors.
#[derive(Debug)]
pub enum ServiceError {
    /// Client sent invalid input (400).
    BadRequest(String),
    /// Requested resource does not exist (404).
    NotFound(String),
    /// A uniqueness constraint was violated (409).
    Conflict(String),
    /// An internal failure -- DB error, join error, etc. (500).
    Internal(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(msg) => write!(f, "bad request: {msg}"),
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::Conflict(msg) => write!(f, "conflict: {msg}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for ServiceError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read `db_path` from shared server state.
async fn db_path(state: &Arc<RwLock<ServerState>>) -> PathBuf {
    state.read().await.config.db_path.clone()
}

/// Run a blocking closure on the Tokio blocking pool and flatten the
/// `JoinError` / `conary_core::Error` nesting into `ServiceError`.
async fn blocking<F, T>(f: F) -> Result<T, ServiceError>
where
    F: FnOnce() -> conary_core::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(val)) => Ok(val),
        Ok(Err(e)) => Err(ServiceError::Internal(e.to_string())),
        Err(e) => Err(ServiceError::Internal(format!("task join error: {e}"))),
    }
}

// ---------------------------------------------------------------------------
// Token operations
// ---------------------------------------------------------------------------

/// The result of creating a new admin token.
pub struct CreatedToken {
    pub id: i64,
    pub raw_token: String,
    pub name: String,
    pub scopes: String,
}

/// Create a new admin API token.
///
/// Validates the name (1-128 chars after trimming) and scopes, generates a
/// random token, hashes it, and inserts a row into `admin_tokens`.
pub async fn create_token(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
    scopes: Option<&str>,
) -> Result<CreatedToken, ServiceError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(ServiceError::BadRequest(
            "Token name must be 1-128 characters".to_string(),
        ));
    }

    let scopes_str = scopes.unwrap_or("admin").to_string();
    if let Err(invalid) = validate_scopes(&scopes_str) {
        return Err(ServiceError::BadRequest(format!(
            "Invalid scope: '{invalid}'"
        )));
    }

    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);
    let db = db_path(state).await;

    let name_owned = name.to_string();
    let scopes_clone = scopes_str.clone();
    let id = blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::admin_token::create(&conn, &name_owned, &token_hash, &scopes_clone)
    })
    .await?;

    Ok(CreatedToken {
        id,
        raw_token,
        name: name.to_string(),
        scopes: scopes_str,
    })
}

/// List all admin API tokens (hashes redacted).
pub async fn list_tokens(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Vec<AdminToken>, ServiceError> {
    let db = db_path(state).await;
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::admin_token::list(&conn)
    })
    .await
}

/// Delete an admin token by ID.  Returns `true` if a row was deleted.
pub async fn delete_token(state: &Arc<RwLock<ServerState>>, id: i64) -> Result<bool, ServiceError> {
    let db = db_path(state).await;
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::admin_token::delete(&conn, id)
    })
    .await
}

// ---------------------------------------------------------------------------
// Federation peer operations
// ---------------------------------------------------------------------------

/// Input for adding a new federation peer.
pub struct AddPeerInput {
    pub endpoint: String,
    pub tier: Option<String>,
    pub node_name: Option<String>,
}

/// List all federation peers.
pub async fn list_peers(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Vec<FederationPeer>, ServiceError> {
    let db = db_path(state).await;
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::federation_peer::list(&conn)
    })
    .await
}

/// Add a federation peer.  Returns the generated peer ID on success.
///
/// Validates the endpoint URL and tier, generates an ID from the URL hash,
/// and inserts via the `federation_peer` model.
pub async fn add_peer(
    state: &Arc<RwLock<ServerState>>,
    input: AddPeerInput,
) -> Result<(String, FederationPeer), ServiceError> {
    let endpoint = input.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return Err(ServiceError::BadRequest(
            "Endpoint must not be empty".to_string(),
        ));
    }
    if url::Url::parse(&endpoint).is_err() {
        return Err(ServiceError::BadRequest("Invalid endpoint URL".to_string()));
    }

    let tier = input.tier.unwrap_or_else(|| "leaf".to_string());
    if !["leaf", "cell_hub", "region_hub"].contains(&tier.as_str()) {
        return Err(ServiceError::BadRequest(
            "Tier must be one of: leaf, cell_hub, region_hub".to_string(),
        ));
    }

    let peer_id = conary_core::hash::sha256(endpoint.as_bytes());
    let node_name = input.node_name;
    let db = db_path(state).await;

    let peer_id_clone = peer_id.clone();
    let endpoint_clone = endpoint.clone();
    let tier_clone = tier.clone();
    let node_name_clone = node_name.clone();

    let result = blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::federation_peer::insert(
            &conn,
            &peer_id_clone,
            &endpoint_clone,
            node_name_clone.as_deref(),
            &tier_clone,
        )?;
        // Read back the inserted row to get DB-generated defaults (timestamps, etc.)
        conary_core::db::models::federation_peer::find_by_id(&conn, &peer_id_clone)
    })
    .await;

    match result {
        Ok(Some(peer)) => Ok((peer_id, peer)),
        Ok(None) => Err(ServiceError::Internal(
            "Peer inserted but not found on read-back".to_string(),
        )),
        Err(ServiceError::Internal(msg)) if msg.contains("UNIQUE constraint") => Err(
            ServiceError::Conflict("Peer with this endpoint already exists".to_string()),
        ),
        Err(e) => Err(e),
    }
}

/// Delete a federation peer by ID.  Returns `true` if a row was deleted.
pub async fn delete_peer(state: &Arc<RwLock<ServerState>>, id: &str) -> Result<bool, ServiceError> {
    let db = db_path(state).await;
    let id_owned = id.to_string();
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::federation_peer::delete(&conn, &id_owned)
    })
    .await
}

/// Get a single federation peer by ID.
pub async fn get_peer(
    state: &Arc<RwLock<ServerState>>,
    id: &str,
) -> Result<Option<FederationPeer>, ServiceError> {
    let db = db_path(state).await;
    let id_owned = id.to_string();
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::federation_peer::find_by_id(&conn, &id_owned)
    })
    .await
}

// ---------------------------------------------------------------------------
// Audit operations
// ---------------------------------------------------------------------------

/// Query the admin audit log with optional filters.
pub async fn query_audit(
    state: &Arc<RwLock<ServerState>>,
    limit: Option<i64>,
    action: Option<String>,
    since: Option<String>,
    token_name: Option<String>,
) -> Result<Vec<AuditEntry>, ServiceError> {
    let db = db_path(state).await;
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::audit_log::query(
            &conn,
            limit,
            action.as_deref(),
            since.as_deref(),
            token_name.as_deref(),
        )
    })
    .await
}

/// Purge audit log entries older than `before`.  Returns the number deleted.
///
/// The `before` string must be a valid date in `YYYY-MM-DD` format.
/// Invalid dates are rejected before reaching SQL.
pub async fn purge_audit(
    state: &Arc<RwLock<ServerState>>,
    before: &str,
) -> Result<usize, ServiceError> {
    // Validate date format before passing to SQL
    if chrono::NaiveDate::parse_from_str(before, "%Y-%m-%d").is_err() {
        return Err(ServiceError::BadRequest(
            "Invalid date format: expected YYYY-MM-DD".to_string(),
        ));
    }

    let db = db_path(state).await;
    let before_owned = before.to_string();
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::audit_log::purge(&conn, &before_owned)
    })
    .await
}

// ---------------------------------------------------------------------------
// Repository operations
// ---------------------------------------------------------------------------

/// List all configured repositories.
pub async fn list_repos(state: &Arc<RwLock<ServerState>>) -> Result<Vec<Repository>, ServiceError> {
    let db = db_path(state).await;
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        Repository::list_all(&conn)
    })
    .await
}

/// Get a single repository by name.
pub async fn get_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
) -> Result<Option<Repository>, ServiceError> {
    let db = db_path(state).await;
    let name_owned = name.to_string();
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        Repository::find_by_name(&conn, &name_owned)
    })
    .await
}

/// Input for creating a new repository.
pub struct CreateRepoInput {
    pub name: String,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub gpg_check: bool,
    pub metadata_expire: i32,
}

/// Create a new repository.
pub async fn create_repo(
    state: &Arc<RwLock<ServerState>>,
    input: CreateRepoInput,
) -> Result<Repository, ServiceError> {
    let db = db_path(state).await;
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        let mut repo = Repository::new(input.name, input.url);
        repo.content_url = input.content_url;
        repo.enabled = input.enabled;
        repo.priority = input.priority;
        repo.gpg_check = input.gpg_check;
        repo.metadata_expire = input.metadata_expire;
        repo.insert(&conn)?;
        Ok(repo)
    })
    .await
}

/// Input for updating a repository.
pub struct UpdateRepoInput {
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub gpg_check: Option<bool>,
    pub metadata_expire: Option<i32>,
}

/// Update an existing repository by name.  Returns `None` if not found.
pub async fn update_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
    input: UpdateRepoInput,
) -> Result<Option<Repository>, ServiceError> {
    let db = db_path(state).await;
    let name_owned = name.to_string();
    blocking(move || {
        let mut conn = conary_core::db::open_fast(&db)?;
        let tx = conn.transaction()?;
        let repo = Repository::find_by_name(&tx, &name_owned)?;
        let mut repo = match repo {
            Some(r) => r,
            None => return Ok(None),
        };
        repo.url = input.url;
        repo.content_url = input.content_url;
        if let Some(enabled) = input.enabled {
            repo.enabled = enabled;
        }
        if let Some(priority) = input.priority {
            repo.priority = priority;
        }
        if let Some(gpg_check) = input.gpg_check {
            repo.gpg_check = gpg_check;
        }
        if let Some(metadata_expire) = input.metadata_expire {
            repo.metadata_expire = metadata_expire;
        }
        repo.update(&tx)?;
        tx.commit()?;
        Ok(Some(repo))
    })
    .await
}

/// Delete a repository by name.  Returns `true` if deleted, `false` if not found.
pub async fn delete_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
) -> Result<bool, ServiceError> {
    let db = db_path(state).await;
    let name_owned = name.to_string();
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        let repo = Repository::find_by_name(&conn, &name_owned)?;
        match repo {
            Some(r) => {
                let id = r.id.ok_or_else(|| {
                    conary_core::Error::MissingId("Repository has no ID".to_string())
                })?;
                Repository::delete(&conn, id)?;
                Ok(true)
            }
            None => Ok(false),
        }
    })
    .await
}

/// Check whether a repository exists by name.
pub async fn repo_exists(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
) -> Result<bool, ServiceError> {
    let repo = get_repo(state, name).await?;
    Ok(repo.is_some())
}
