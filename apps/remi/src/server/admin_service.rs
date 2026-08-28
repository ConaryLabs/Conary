// apps/remi/src/server/admin_service.rs
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
use std::{net::IpAddr, str::FromStr};
use tokio::sync::RwLock;

use conary_core::db::models::admin_token::AdminToken;
use conary_core::db::models::audit_log::AuditEntry;
use conary_core::db::models::federation_peer::FederationPeer;
use conary_core::db::models::{RemiActiveProfileRevision, Repository, RepositoryOwnership};
use conary_core::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use rusqlite::TransactionBehavior;

use crate::federation::{Peer, PeerTier};
use crate::server::ServerState;
use crate::server::auth::{generate_token, hash_token, validate_scopes};
use crate::server::r2_durability::{
    MAX_BACKFILL_CONCURRENCY, R2DurabilityMode, R2DurabilityReport, run_r2_durability,
};

mod profile_refresh;
mod publication;
mod refresh;
mod repository_policy;
mod test_data;

pub(crate) use refresh::refresh_repositories_uncoordinated;
pub use refresh::{
    RepoRefreshBatch, RepoRefreshBatchState, RepoRefreshFailure, RepoRefreshFailureKind,
    RepoRefreshResult, sync_repo,
};
pub(crate) use refresh::{refresh_profile_repositories, refresh_repositories};
pub use repository_policy::NativeSourcePolicyInput;
use repository_policy::apply_native_source_contract;
pub use test_data::{
    PushStepData, PushTestResultData, TestDetail, TestHealthSummary, TestRunDetail,
    TestStepWithLogs, create_test_run, get_test_detail, get_test_logs, get_test_run_detail,
    list_test_runs, push_test_result, test_gc, test_health, update_test_run_status,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by service-layer operations.
///
/// Handlers map these to HTTP status codes; MCP tools map them to tool errors.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// Client sent invalid input (400).
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Requested resource does not exist (404).
    #[error("not found: {0}")]
    NotFound(String),
    /// A uniqueness constraint was violated (409).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Immutable catalog construction lacks structurally required scratch space.
    #[error(transparent)]
    StorageCapacity(#[from] conary_core::repository::catalog::CatalogScratchCapacityError),
    /// An internal failure -- DB error, join error, etc. (500).
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Error conversions
// ---------------------------------------------------------------------------

impl From<conary_core::Error> for ServiceError {
    fn from(e: conary_core::Error) -> Self {
        match &e {
            conary_core::Error::NotFound(_) => ServiceError::NotFound(e.to_string()),
            conary_core::Error::ConflictError(_) | conary_core::Error::AlreadyExists(_) => {
                ServiceError::Conflict(e.to_string())
            }
            conary_core::Error::ParseError(_) | conary_core::Error::ConfigError(_) => {
                ServiceError::BadRequest(e.to_string())
            }
            conary_core::Error::CatalogScratchCapacity(error) => {
                ServiceError::StorageCapacity(error.clone())
            }
            _ => ServiceError::Internal(e.to_string()),
        }
    }
}

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
        Ok(Err(e)) => Err(ServiceError::from(e)),
        Err(e) => Err(ServiceError::Internal(format!("task join error: {e}"))),
    }
}

/// Typed not-found error for use inside `blocking_anyhow` closures.
///
/// Downcast by `blocking_anyhow` to produce `ServiceError::NotFound` instead
/// of `ServiceError::Internal`. Use this instead of `anyhow::anyhow!("... not
/// found ...")` string-matching heuristics.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct NotFoundError(String);

/// Like [`blocking`] but for closures that return `anyhow::Result`.
///
/// The test data module uses anyhow rather than `conary_core::Error`, so
/// we need a parallel helper.  If the returned error is a [`NotFoundError`],
/// it maps to `ServiceError::NotFound`; all other errors map to
/// `ServiceError::Internal`.
async fn blocking_anyhow<F, T>(f: F) -> Result<T, ServiceError>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(val)) => Ok(val),
        Ok(Err(e)) => {
            if let Some(nf) = e.downcast_ref::<NotFoundError>() {
                Err(ServiceError::NotFound(nf.0.clone()))
            } else {
                Err(ServiceError::Internal(e.to_string()))
            }
        }
        Err(e) => Err(ServiceError::Internal(format!("task join error: {e}"))),
    }
}

/// Read `test_db_path` from shared server state, returning `ServiceError`
/// if not configured.
async fn test_db_path(state: &Arc<RwLock<ServerState>>) -> Result<String, ServiceError> {
    state
        .read()
        .await
        .test_db_path
        .clone()
        .ok_or_else(|| ServiceError::Internal("test_db_path not configured".to_string()))
}

/// Validate that a stored external URL cannot target local or cloud-metadata services.
async fn validate_external_url(url_str: &str) -> Result<(), ServiceError> {
    let parsed = url::Url::parse(url_str.trim())
        .map_err(|e| ServiceError::BadRequest(format!("Invalid URL '{url_str}': {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(ServiceError::BadRequest(format!(
                "Only http:// and https:// URLs are allowed, got {scheme}://"
            )));
        }
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| ServiceError::BadRequest("URL has no host".to_string()))?;

    validate_external_host(host)?;

    if let Ok(ip) = IpAddr::from_str(host) {
        validate_external_ip(&ip)?;
    }

    let port = parsed
        .port()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let resolved_addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ServiceError::BadRequest(format!("Failed to resolve '{host}': {e}")))?
        .collect();

    if resolved_addrs.is_empty() {
        return Err(ServiceError::BadRequest(format!(
            "DNS resolution for '{host}' returned no addresses"
        )));
    }

    for addr in resolved_addrs {
        validate_external_ip(&addr.ip())?;
    }

    Ok(())
}

fn validate_external_host(host: &str) -> Result<(), ServiceError> {
    let lower_host = host.to_ascii_lowercase();
    if lower_host == "localhost"
        || lower_host.ends_with(".localhost")
        || lower_host == "127.0.0.1"
        || lower_host == "::1"
        || lower_host == "0.0.0.0"
    {
        return Err(ServiceError::BadRequest(
            "URLs targeting localhost are not allowed".to_string(),
        ));
    }

    if lower_host == "metadata.google.internal" {
        return Err(ServiceError::BadRequest(
            "Cloud metadata endpoints are not allowed".to_string(),
        ));
    }

    Ok(())
}

fn validate_external_ip(ip: &IpAddr) -> Result<(), ServiceError> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() {
                return Err(ServiceError::BadRequest(format!(
                    "URLs targeting private or link-local IPs are not allowed: {ip}"
                )));
            }
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            let is_unique_local = (segments[0] & 0xfe00) == 0xfc00;
            let is_link_local = (segments[0] & 0xffc0) == 0xfe80;
            if v6.is_loopback() || v6.is_unspecified() || is_unique_local || is_link_local {
                return Err(ServiceError::BadRequest(format!(
                    "URLs targeting private or link-local IPs are not allowed: {ip}"
                )));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Chunk garbage collection
// ---------------------------------------------------------------------------

/// Chunks touched more recently than this are kept even when unreferenced, so
/// an in-flight conversion cannot lose the chunks it is still writing.
const CHUNK_GC_GRACE_PERIOD_SECS: u64 = 3600;

/// Outcome of a chunk garbage-collection run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChunkGcReport {
    pub dry_run: bool,
    pub referenced: usize,
    pub local_scanned: usize,
    pub r2_scanned: usize,
    pub local_deleted: usize,
    pub r2_deleted: usize,
    pub local_bytes_freed: u64,
    pub r2_bytes_freed: u64,
}

/// Garbage collect chunks that no converted package references.
///
/// With `dry_run` set the run only reports what it would remove.  This is the
/// single implementation behind both the HTTP admin route and the MCP tool.
pub async fn run_chunk_gc_op(
    state: &Arc<RwLock<ServerState>>,
    dry_run: bool,
) -> Result<ChunkGcReport, ServiceError> {
    let (db_path, objects_dir, r2_store) = {
        let state = state.read().await;
        (
            state.config.db_path.clone(),
            state.config.chunk_dir.join("objects"),
            state.r2_store.clone(),
        )
    };

    let result = crate::server::chunk_gc::run_chunk_gc(
        &db_path,
        &objects_dir,
        r2_store,
        dry_run,
        CHUNK_GC_GRACE_PERIOD_SECS,
    )
    .await
    .map_err(|e| ServiceError::Internal(e.to_string()))?;

    Ok(ChunkGcReport {
        dry_run,
        referenced: result.referenced,
        local_scanned: result.local_scanned,
        r2_scanned: result.r2_scanned,
        local_deleted: result.local_deleted,
        r2_deleted: result.r2_deleted,
        local_bytes_freed: result.local_bytes_freed,
        r2_bytes_freed: result.r2_bytes_freed,
    })
}

/// Inventory or backfill the R2 durable chunk tier.
///
/// This is the single implementation behind the HTTP admin route and MCP
/// tool. Plan mode is read-only; apply mode verifies every local object before
/// upload and reports completion from a fresh R2 inventory.
pub async fn run_r2_durability_op(
    state: &Arc<RwLock<ServerState>>,
    mode: R2DurabilityMode,
    concurrency: usize,
) -> Result<R2DurabilityReport, ServiceError> {
    if !(1..=MAX_BACKFILL_CONCURRENCY).contains(&concurrency) {
        return Err(ServiceError::BadRequest(format!(
            "R2 backfill concurrency must be between 1 and {MAX_BACKFILL_CONCURRENCY}"
        )));
    }
    let (db_path, objects_dir, r2_store) = {
        let state = state.read().await;
        (
            state.config.db_path.clone(),
            state.config.chunk_dir.join("objects"),
            state.r2_store.clone(),
        )
    };
    let r2_store = r2_store.ok_or_else(|| {
        ServiceError::BadRequest("R2 storage is not configured for this Remi instance".to_string())
    })?;

    run_r2_durability(&db_path, &objects_dir, r2_store, mode, concurrency)
        .await
        .map_err(|error| ServiceError::Internal(error.to_string()))
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
    pub tls_fingerprint: Option<String>,
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
/// Validates the endpoint URL and tier, derives the peer ID, and inserts via
/// the `federation_peer` model. HTTPS peers must include a pinned TLS
/// certificate fingerprint so the stored peer ID is certificate-bound.
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
    validate_external_url(&endpoint).await?;

    let tier = input.tier.unwrap_or_else(|| "leaf".to_string());
    if !["leaf", "cell_hub", "region_hub"].contains(&tier.as_str()) {
        return Err(ServiceError::BadRequest(
            "Tier must be one of: leaf, cell_hub, region_hub".to_string(),
        ));
    }

    let peer_tier = match tier.as_str() {
        "cell_hub" => PeerTier::CellHub,
        "region_hub" => PeerTier::RegionHub,
        _ => PeerTier::Leaf,
    };
    let peer = Peer::from_endpoint_with_fingerprint(
        &endpoint,
        peer_tier,
        input.tls_fingerprint.as_deref(),
    )
    .map_err(ServiceError::from)?;
    let peer_id = peer.id.clone();
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
    pub metadata_expire: i32,
    pub parser: RepositoryParserConfig,
    pub trust: Option<RepositoryTrustPolicy>,
    pub native_source: Option<NativeSourcePolicyInput>,
}

/// Create a new repository.
pub async fn create_repo(
    state: &Arc<RwLock<ServerState>>,
    input: CreateRepoInput,
) -> Result<Repository, ServiceError> {
    validate_external_url(&input.url).await?;
    if let Some(ref content_url) = input.content_url
        && !content_url.trim().is_empty()
    {
        validate_external_url(content_url).await?;
    }
    if let Some(trust) = input.trust.as_ref() {
        validate_repository_trust(trust).await?;
    }

    let _publication_guard = publication::guard(state).await;
    let db = db_path(state).await;
    let database_writer = state.read().await.database_writer.clone();
    blocking(move || {
        database_writer.execute(|| {
            let mut conn = conary_core::db::open_fast(&db)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut repo = Repository::new(input.name, input.url);
            repo.content_url = input.content_url;
            repo.enabled = input.enabled;
            repo.priority = input.priority;
            repo.metadata_expire = input.metadata_expire;
            repo.set_parser_config(input.parser)?;
            repo.trust_policy = input.trust;
            apply_native_source_contract(&mut repo, input.native_source)?;
            repo.insert(&tx)?;
            retire_native_profile_pointer(&tx, &repo)?;
            tx.commit()?;
            Ok(repo)
        })
    })
    .await
}

/// Input for updating a repository.
pub struct UpdateRepoInput {
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub metadata_expire: Option<i32>,
    pub parser: RepositoryParserConfig,
    pub trust: Option<RepositoryTrustPolicy>,
    pub native_source: Option<NativeSourcePolicyInput>,
}

/// Update an existing repository by name.  Returns `None` if not found.
pub async fn update_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
    input: UpdateRepoInput,
) -> Result<Option<Repository>, ServiceError> {
    validate_external_url(&input.url).await?;
    if let Some(ref content_url) = input.content_url
        && !content_url.trim().is_empty()
    {
        validate_external_url(content_url).await?;
    }
    if let Some(trust) = input.trust.as_ref() {
        validate_repository_trust(trust).await?;
    }

    let _publication_guard = publication::guard(state).await;
    let db = db_path(state).await;
    let database_writer = state.read().await.database_writer.clone();
    let name_owned = name.to_string();
    blocking(move || {
        database_writer.execute(|| {
            let mut conn = conary_core::db::open_fast(&db)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let repo = Repository::find_by_name(&tx, &name_owned)?;
            let mut repo = match repo {
                Some(r) => r,
                None => return Ok(None),
            };
            if repo.managed_by == RepositoryOwnership::RemiConfig {
                return Err(conary_core::Error::ConflictError(format!(
                    "repository '{}' is owned by the Remi repository manifest",
                    repo.name
                )));
            }
            let previous = repo.clone();
            let source_changed = repo.url != input.url
                || repo.content_url != input.content_url
                || repo.parser_config.as_ref() != Some(&input.parser)
                || repo.trust_policy != input.trust;
            repo.url = input.url;
            repo.content_url = input.content_url;
            if let Some(enabled) = input.enabled {
                repo.enabled = enabled;
            }
            if let Some(priority) = input.priority {
                repo.priority = priority;
            }
            if let Some(metadata_expire) = input.metadata_expire {
                repo.metadata_expire = metadata_expire;
            }
            repo.set_parser_config(input.parser)?;
            repo.trust_policy = input.trust;
            apply_native_source_contract(&mut repo, input.native_source)?;
            let source_changed = source_changed
                || repo.source_policy != previous.source_policy
                || repo.repository_identity != previous.repository_identity
                || repo.stream_binding_sha256 != previous.stream_binding_sha256
                || repo.pinned_snapshot != previous.pinned_snapshot;
            let catalog_binding_changed = source_changed
                || repo.enabled != previous.enabled
                || repo.priority != previous.priority;
            if source_changed {
                let repository_id = repo.id.ok_or_else(|| {
                    conary_core::Error::MissingId("Repository has no ID".to_string())
                })?;
                Repository::delete(&tx, repository_id)?;
                repo.id = None;
                repo.last_checked_at = None;
                repo.last_changed_at = None;
                repo.last_validated_at = None;
                repo.last_published_at = None;
                repo.created_at = None;
                repo.insert(&tx)?;
            } else {
                repo.update(&tx)?;
            }
            if catalog_binding_changed {
                retire_native_profile_pointer(&tx, &previous)?;
                retire_native_profile_pointer(&tx, &repo)?;
            }
            tx.commit()?;
            Ok(Some(repo))
        })
    })
    .await
}

/// Delete a repository by name.  Returns `true` if deleted, `false` if not found.
pub async fn delete_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
) -> Result<bool, ServiceError> {
    let _publication_guard = publication::guard(state).await;
    let db = db_path(state).await;
    let database_writer = state.read().await.database_writer.clone();
    let name_owned = name.to_string();
    blocking(move || {
        database_writer.execute(|| {
            let mut conn = conary_core::db::open_fast(&db)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let repo = Repository::find_by_name(&tx, &name_owned)?;
            let deleted = match repo {
                Some(r) => {
                    if r.managed_by == RepositoryOwnership::RemiConfig {
                        return Err(conary_core::Error::ConflictError(format!(
                            "repository '{}' is owned by the Remi repository manifest",
                            r.name
                        )));
                    }
                    let id = r.id.ok_or_else(|| {
                        conary_core::Error::MissingId("Repository has no ID".to_string())
                    })?;
                    Repository::delete(&tx, id)?;
                    retire_native_profile_pointer(&tx, &r)?;
                    true
                }
                None => false,
            };
            tx.commit()?;
            Ok(deleted)
        })
    })
    .await
}

fn retire_native_profile_pointer(
    conn: &rusqlite::Connection,
    repository: &Repository,
) -> conary_core::Result<()> {
    if repository.enabled
        && profile_refresh::is_native_profile_repository(repository)
        && let Some(source_profile) = repository.source_profile.as_deref()
    {
        RemiActiveProfileRevision::retire(conn, source_profile)?;
    }
    Ok(())
}

/// Check whether a repository exists by name.
pub async fn repo_exists(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
) -> Result<bool, ServiceError> {
    let repo = get_repo(state, name).await?;
    Ok(repo.is_some())
}

async fn validate_repository_trust(policy: &RepositoryTrustPolicy) -> Result<(), ServiceError> {
    policy.validate().map_err(ServiceError::from)?;
    if let RepositoryTrustPolicy::Rpm {
        metadata: RpmMetadataAuthority::Metalink { url },
        ..
    } = policy
    {
        validate_external_url(url).await?;
    }
    if let RepositoryTrustPolicy::Arch { keyring, .. } = policy {
        validate_external_url(&keyring.url).await?;
    }
    for root in repository_trust_roots(policy) {
        validate_external_url(&root.url).await?;
    }
    Ok(())
}

fn repository_trust_roots(policy: &RepositoryTrustPolicy) -> Vec<&OpenPgpTrustRoot> {
    match policy {
        RepositoryTrustPolicy::Debian { release_keys } => release_keys.iter().collect(),
        RepositoryTrustPolicy::Rpm {
            metadata,
            package_keys,
        } => match metadata {
            RpmMetadataAuthority::OpenPgp { keys } => keys.iter().chain(package_keys).collect(),
            RpmMetadataAuthority::Metalink { .. } => package_keys.iter().collect(),
        },
        RepositoryTrustPolicy::Arch { .. } | RepositoryTrustPolicy::Eopkg { .. } => Vec::new(),
    }
}
