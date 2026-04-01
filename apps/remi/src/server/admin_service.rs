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

use conary_core::db::models::Repository;
use conary_core::db::models::admin_token::AdminToken;
use conary_core::db::models::audit_log::AuditEntry;
use conary_core::db::models::federation_peer::FederationPeer;
use serde::{Deserialize, Serialize};

use crate::federation::{Peer, PeerTier};
use crate::server::ServerState;
use crate::server::auth::{generate_token, hash_token, validate_scopes};
use crate::server::test_db;

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
    validate_external_url(&input.url).await?;
    if let Some(ref content_url) = input.content_url
        && !content_url.trim().is_empty()
    {
        validate_external_url(content_url).await?;
    }

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

/// Result of a repository metadata refresh.
#[derive(Debug, Clone)]
pub struct RepoRefreshResult {
    pub name: String,
    pub packages_synced: usize,
    pub skipped: bool,
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

    let db = db_path(state).await;
    let name_owned = name.to_string();
    blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        let repo = Repository::find_by_name(&conn, &name_owned)?;
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
        repo.update(&conn)?;
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

/// Synchronize a single repository by name.
///
/// Returns `Ok(None)` if the repository does not exist.
///
/// # Deadlock risk (documented)
///
/// This function uses `Handle::block_on()` inside `spawn_blocking` to call
/// async operations (`sync_repository`, `maybe_fetch_gpg_key`) while holding
/// a `rusqlite::Connection` (which is `!Send` and cannot cross `.await`).
///
/// If the Tokio blocking thread pool is saturated, `block_on` will wait for
/// an async task that itself may need a blocking thread, creating a potential
/// deadlock. In practice this is safe because:
/// 1. `sync_repo` is called infrequently (admin API, background refresh)
/// 2. The default blocking pool (512 threads) is never near saturation
/// 3. Restructuring would require `sync_repository` to not take `&Connection`
///
/// If this becomes a problem, increase `max_blocking_threads` in the Tokio
/// runtime builder or restructure `sync_repository` to separate DB and HTTP.
pub async fn sync_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
    force: bool,
) -> Result<Option<RepoRefreshResult>, ServiceError> {
    let db = db_path(state).await;
    let name_owned = name.to_string();
    blocking_anyhow(move || {
        let handle = tokio::runtime::Handle::current();
        let conn = conary_core::db::open_fast(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
        let keyring_dir = conary_core::db::paths::keyring_dir(&db.display().to_string());
        let mut repo = match Repository::find_by_name(&conn, &name_owned)
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            Some(repo) => repo,
            None => return Ok(None),
        };

        if !force && !conary_core::repository::needs_sync(&repo) {
            return Ok(Some(RepoRefreshResult {
                name: repo.name,
                packages_synced: 0,
                skipped: true,
            }));
        }

        if repo.gpg_check
            && let Err(e) = handle.block_on(conary_core::repository::maybe_fetch_gpg_key(
                &repo,
                &keyring_dir,
            ))
        {
            tracing::warn!("Failed to fetch GPG key for repo {}: {e}", repo.name);
        }

        let packages_synced = handle
            .block_on(conary_core::repository::sync_repository(&conn, &mut repo))
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(Some(RepoRefreshResult {
            name: repo.name,
            packages_synced,
            skipped: false,
        }))
    })
    .await
}

/// Synchronize all enabled repositories.
///
/// See [`sync_repo`] for the `Handle::block_on` deadlock risk documentation.
pub async fn refresh_repositories(
    state: &Arc<RwLock<ServerState>>,
    force: bool,
) -> Result<Vec<RepoRefreshResult>, ServiceError> {
    let db = db_path(state).await;
    let results = blocking_anyhow(move || {
        let handle = tokio::runtime::Handle::current();
        let conn = conary_core::db::open_fast(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
        let keyring_dir = conary_core::db::paths::keyring_dir(&db.display().to_string());
        let repos = Repository::list_enabled(&conn).map_err(|e| anyhow::anyhow!("{e}"))?;

        let mut refreshed = Vec::new();

        for mut repo in repos {
            if !force && !conary_core::repository::needs_sync(&repo) {
                refreshed.push(RepoRefreshResult {
                    name: repo.name,
                    packages_synced: 0,
                    skipped: true,
                });
                continue;
            }

            if repo.gpg_check {
                let _ = handle.block_on(conary_core::repository::maybe_fetch_gpg_key(
                    &repo,
                    &keyring_dir,
                ));
            }

            let packages_synced = handle
                .block_on(conary_core::repository::sync_repository(&conn, &mut repo))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            refreshed.push(RepoRefreshResult {
                name: repo.name,
                packages_synced,
                skipped: false,
            });
        }

        Ok(refreshed)
    })
    .await?;

    // After successful sync, trigger canonical rebuild if cooldown elapsed.
    // Failures here are non-fatal -- the sync result is returned regardless.
    {
        let db_path = db_path(state).await;
        let canonical_cfg = state.read().await.canonical_config.clone();
        blocking(move || {
            let conn = crate::server::open_runtime_db(&db_path)?;
            if crate::server::canonical_job::should_rebuild(
                &conn,
                canonical_cfg.rebuild_cooldown_minutes,
            ) {
                match crate::server::canonical_job::rebuild_canonical_map(&db_path, &canonical_cfg)
                {
                    Ok(count) => {
                        tracing::info!("Post-sync canonical rebuild: {count} new mappings")
                    }
                    Err(e) => tracing::warn!("Post-sync canonical rebuild failed: {e}"),
                }
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|e| tracing::warn!("Post-sync canonical rebuild task failed: {e}"));
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Test data types
// ---------------------------------------------------------------------------

/// Input for pushing a test result with its steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushTestResultData {
    pub test_id: String,
    pub name: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub message: Option<String>,
    pub attempt: Option<i32>,
    pub steps: Vec<PushStepData>,
}

/// A single step within a [`PushTestResultData`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushStepData {
    pub step_type: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

/// A test run together with all its results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRunDetail {
    pub run: test_db::TestRun,
    pub results: Vec<test_db::TestResult>,
}

/// A single test result together with its steps and their logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDetail {
    pub result: test_db::TestResult,
    pub steps: Vec<TestStepWithLogs>,
}

/// A test step paired with its log entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestStepWithLogs {
    pub step: test_db::TestStep,
    pub logs: Vec<test_db::TestLog>,
}

/// Summary returned by [`test_health`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestHealthSummary {
    pub total_runs: u64,
    pub recent_runs: Vec<test_db::TestRun>,
    pub last_status: Option<String>,
}

// ---------------------------------------------------------------------------
// Test data operations
// ---------------------------------------------------------------------------

/// Create a new test run in the test data database.
pub async fn create_test_run(
    state: &Arc<RwLock<ServerState>>,
    suite: String,
    distro: String,
    phase: u32,
    triggered_by: Option<String>,
    source_commit: Option<String>,
) -> Result<test_db::TestRun, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        test_db::TestRun::create(
            &conn,
            &suite,
            &distro,
            i32::try_from(phase).unwrap_or(i32::MAX),
            triggered_by.as_deref(),
            source_commit.as_deref(),
        )
    })
    .await
}

/// Update the status (and optionally the aggregate counts) of a test run.
pub async fn update_test_run_status(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    status: String,
    total: Option<u32>,
    passed: Option<u32>,
    failed: Option<u32>,
    skipped: Option<u32>,
) -> Result<(), ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        test_db::TestRun::update_status(&conn, run_id, &status)?;
        if let Some(t) = total {
            test_db::TestRun::update_counts(
                &conn,
                run_id,
                i32::try_from(t).unwrap_or(i32::MAX),
                i32::try_from(passed.unwrap_or(0)).unwrap_or(0),
                i32::try_from(failed.unwrap_or(0)).unwrap_or(0),
                i32::try_from(skipped.unwrap_or(0)).unwrap_or(0),
            )?;
        }
        Ok(())
    })
    .await
}

/// Push a test result (with steps and logs) into an existing run.
pub async fn push_test_result(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    data: PushTestResultData,
) -> Result<(), ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;

        // Verify the run exists
        test_db::TestRun::find_by_id(&conn, run_id)?
            .ok_or_else(|| anyhow::anyhow!("test run {run_id} not found"))?;

        let result = test_db::TestResult::insert(
            &conn,
            &test_db::NewTestResult {
                run_id,
                test_id: &data.test_id,
                name: &data.name,
                status: &data.status,
                duration_ms: data.duration_ms,
                message: data.message.as_deref(),
                attempt: data.attempt.unwrap_or(1),
            },
        )?;

        for (idx, step_data) in data.steps.iter().enumerate() {
            let step = test_db::TestStep::insert(
                &conn,
                result.id,
                i32::try_from(idx).unwrap_or(i32::MAX),
                &step_data.step_type,
                step_data.command.as_deref(),
                step_data.exit_code,
                step_data.duration_ms,
            )?;

            if let Some(ref stdout) = step_data.stdout {
                test_db::TestLog::insert(&conn, step.id, "stdout", stdout)?;
            }
            if let Some(ref stderr) = step_data.stderr {
                test_db::TestLog::insert(&conn, step.id, "stderr", stderr)?;
            }
        }

        Ok(())
    })
    .await
}

/// List test runs with optional filters and cursor-based pagination.
///
/// Filters are pushed into the SQL WHERE clause so that `limit` applies
/// after filtering, not before.  The previous post-query `.retain()`
/// approach could return fewer rows than `limit` even when matching rows
/// existed beyond the first page.
pub async fn list_test_runs(
    state: &Arc<RwLock<ServerState>>,
    limit: u32,
    cursor: Option<i64>,
    suite: Option<String>,
    distro: Option<String>,
    status: Option<String>,
) -> Result<Vec<test_db::TestRun>, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let runs = test_db::TestRun::list_filtered(
            &conn,
            cursor,
            limit,
            suite.as_deref(),
            distro.as_deref(),
            status.as_deref(),
        )?;

        Ok(runs)
    })
    .await
}

/// Get a test run with all its results.
pub async fn get_test_run_detail(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
) -> Result<TestRunDetail, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let run = test_db::TestRun::find_by_id(&conn, run_id)?
            .ok_or_else(|| NotFoundError(format!("test run {run_id} not found")))?;
        let results = test_db::TestResult::find_by_run(&conn, run_id)?;
        Ok(TestRunDetail { run, results })
    })
    .await
}

/// Get a single test result with its steps and logs.
pub async fn get_test_detail(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    test_id: String,
) -> Result<TestDetail, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let result = test_db::TestResult::find_by_run_and_test(&conn, run_id, &test_id)?
            .ok_or_else(|| NotFoundError(format!("test {test_id} not found in run {run_id}")))?;

        let steps = test_db::TestStep::find_by_result(&conn, result.id)?;
        let mut steps_with_logs = Vec::with_capacity(steps.len());
        for step in steps {
            let logs = test_db::TestLog::find_by_step(&conn, step.id)?;
            steps_with_logs.push(TestStepWithLogs { step, logs });
        }

        Ok(TestDetail {
            result,
            steps: steps_with_logs,
        })
    })
    .await
}

/// Get log entries for a specific test, optionally filtered by stream or step.
pub async fn get_test_logs(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    test_id: String,
    stream: Option<String>,
    step_index: Option<u32>,
) -> Result<Vec<test_db::TestLog>, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let result = test_db::TestResult::find_by_run_and_test(&conn, run_id, &test_id)?
            .ok_or_else(|| NotFoundError(format!("test {test_id} not found in run {run_id}")))?;

        let steps = test_db::TestStep::find_by_result(&conn, result.id)?;
        let mut all_logs = Vec::new();

        for step in &steps {
            // Filter by step_index if specified
            if let Some(idx) = step_index
                && step.step_index != i32::try_from(idx).unwrap_or(i32::MAX)
            {
                continue;
            }
            let logs = test_db::TestLog::find_by_step(&conn, step.id)?;
            all_logs.extend(logs);
        }

        // Filter by stream if specified
        if let Some(ref s) = stream {
            all_logs.retain(|l| l.stream == *s);
        }

        Ok(all_logs)
    })
    .await
}

/// Return a health summary of recent test activity.
pub async fn test_health(
    state: &Arc<RwLock<ServerState>>,
) -> Result<TestHealthSummary, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let recent_runs = test_db::TestRun::list(&conn, None, 5)?;
        let total_runs: u64 = conn
            .query_row("SELECT COUNT(*) FROM test_runs", [], |r| {
                r.get::<_, i64>(0).map(|v| v as u64)
            })
            .map_err(|e| anyhow::anyhow!("Failed to count test runs: {e}"))?;
        let last_status = recent_runs.first().map(|r| r.status.clone());

        Ok(TestHealthSummary {
            total_runs,
            recent_runs,
            last_status,
        })
    })
    .await
}

/// Delete test runs older than `older_than_days` days.  Returns the number
/// of runs removed (children are CASCADE-deleted).
pub async fn test_gc(
    state: &Arc<RwLock<ServerState>>,
    older_than_days: u32,
) -> Result<u64, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        test_db::gc(&conn, older_than_days)
    })
    .await
}
