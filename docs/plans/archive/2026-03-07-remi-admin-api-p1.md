# Remi Admin API P1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add repository management, federation management, and missing MCP tools to the Remi Admin API.

**Architecture:** Extends the existing P0 admin API on :8082. Repo handlers use the existing `Repository` model in conary-core (full CRUD already exists). Federation handlers query the `federation_peers` DB table directly (like the existing federation directory endpoint). New MCP tools wrap the same DB operations. All endpoints follow the established pattern: scope-checked, `spawn_blocking` for SQLite, consistent JSON errors via `json_error()`.

**Tech Stack:** Rust, axum, rusqlite, rmcp (MCP), serde, tokio

**Design doc:** `docs/plans/2026-03-07-remi-admin-api-design.md`

---

### Task 1: Repository Management Handlers

**Files:**
- Modify: `conary-server/src/server/handlers/admin.rs`
- Modify: `conary-server/src/server/routes.rs`

**Context:** The `Repository` model in `conary-core/src/db/models/repository.rs` already has: `new()`, `insert()`, `find_by_name()`, `list_all()`, `update()`, `delete()`, `find_by_id()`. We just need thin handler wrappers.

**Step 1: Add request/response types and handlers to admin.rs**

Add these after the SSE section, before `#[cfg(test)]`:

```rust
// ========== Repository Management ==========

/// Request body for creating or updating a repository.
#[derive(Debug, Deserialize)]
pub struct RepoRequest {
    pub name: String,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub gpg_check: Option<bool>,
    pub metadata_expire: Option<i32>,
}

/// Response body for a repository.
#[derive(Debug, Serialize)]
pub struct RepoResponse {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub gpg_check: bool,
    pub metadata_expire: i32,
    pub last_sync: Option<String>,
    pub created_at: Option<String>,
}

impl From<conary_core::db::models::repository::Repository> for RepoResponse {
    fn from(r: conary_core::db::models::repository::Repository) -> Self {
        Self {
            id: r.id.unwrap_or(0),
            name: r.name,
            url: r.url,
            content_url: r.content_url,
            enabled: r.enabled,
            priority: r.priority,
            gpg_check: r.gpg_check,
            metadata_expire: r.metadata_expire,
            last_sync: r.last_sync,
            created_at: r.created_at,
        }
    }
}

/// GET /v1/admin/repos
pub async fn list_repos(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:read") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::repository::Repository::list_all(&conn)
    })
    .await;
    match result {
        Ok(Ok(repos)) => {
            let resp: Vec<RepoResponse> = repos.into_iter().map(RepoResponse::from).collect();
            Json(resp).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to list repos: {e}");
            json_error(500, "Failed to list repositories", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error listing repos: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/repos
pub async fn create_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<RepoRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }
    if body.name.trim().is_empty() || body.url.trim().is_empty() {
        return json_error(400, "Name and URL are required", "INVALID_INPUT");
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let mut repo = conary_core::db::models::repository::Repository::new(
            body.name.trim().to_string(),
            body.url.trim().to_string(),
        );
        if let Some(content_url) = body.content_url {
            repo.content_url = Some(content_url);
        }
        if let Some(enabled) = body.enabled {
            repo.enabled = enabled;
        }
        if let Some(priority) = body.priority {
            repo.priority = priority;
        }
        if let Some(gpg_check) = body.gpg_check {
            repo.gpg_check = gpg_check;
        }
        if let Some(expire) = body.metadata_expire {
            repo.metadata_expire = expire;
        }
        repo.insert(&conn)?;
        Ok::<_, conary_core::error::Error>(repo)
    })
    .await;
    match result {
        Ok(Ok(repo)) => {
            let s = state.read().await;
            s.publish_event("repo.created", serde_json::json!({"name": repo.name}));
            (StatusCode::CREATED, Json(RepoResponse::from(repo))).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to create repo: {e}");
            json_error(500, "Failed to create repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error creating repo: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/repos/:name
pub async fn get_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:read") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::repository::Repository::find_by_name(&conn, &name)
    })
    .await;
    match result {
        Ok(Ok(Some(repo))) => Json(RepoResponse::from(repo)).into_response(),
        Ok(Ok(None)) => json_error(404, "Repository not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to get repo: {e}");
            json_error(500, "Failed to get repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error getting repo: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// PUT /v1/admin/repos/:name
pub async fn update_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<RepoRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let mut repo = conary_core::db::models::repository::Repository::find_by_name(&conn, &name_clone)?
            .ok_or_else(|| conary_core::error::Error::MissingId("Repository not found".to_string()))?;
        repo.name = body.name.trim().to_string();
        repo.url = body.url.trim().to_string();
        repo.content_url = body.content_url;
        if let Some(enabled) = body.enabled {
            repo.enabled = enabled;
        }
        if let Some(priority) = body.priority {
            repo.priority = priority;
        }
        if let Some(gpg_check) = body.gpg_check {
            repo.gpg_check = gpg_check;
        }
        if let Some(expire) = body.metadata_expire {
            repo.metadata_expire = expire;
        }
        repo.update(&conn)?;
        Ok::<_, conary_core::error::Error>(repo)
    })
    .await;
    match result {
        Ok(Ok(repo)) => {
            let s = state.read().await;
            s.publish_event("repo.updated", serde_json::json!({"name": repo.name}));
            Json(RepoResponse::from(repo)).into_response()
        }
        Ok(Err(e)) if e.to_string().contains("not found") => {
            json_error(404, "Repository not found", "NOT_FOUND")
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to update repo: {e}");
            json_error(500, "Failed to update repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error updating repo: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/repos/:name
pub async fn delete_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let repo = conary_core::db::models::repository::Repository::find_by_name(&conn, &name_clone)?;
        match repo {
            Some(r) => {
                let id = r.id.unwrap();
                conary_core::db::models::repository::Repository::delete(&conn, id)?;
                Ok::<_, conary_core::error::Error>(true)
            }
            None => Ok(false),
        }
    })
    .await;
    match result {
        Ok(Ok(true)) => {
            let s = state.read().await;
            s.publish_event("repo.deleted", serde_json::json!({"name": name}));
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(false)) => json_error(404, "Repository not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to delete repo: {e}");
            json_error(500, "Failed to delete repository", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error deleting repo: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/repos/:name/sync
pub async fn sync_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "repos:write") {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::repository::Repository::find_by_name(&conn, &name)
    })
    .await;
    match result {
        Ok(Ok(Some(_repo))) => {
            // Sync is a stub for now -- actual sync requires the full repository
            // sync machinery which runs asynchronously. Publish the event and
            // return accepted.
            let s = state.read().await;
            s.publish_event("repo.sync_requested", serde_json::json!({"name": name}));
            (StatusCode::ACCEPTED, Json(serde_json::json!({"status": "sync_requested", "name": name}))).into_response()
        }
        Ok(Ok(None)) => json_error(404, "Repository not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to find repo for sync: {e}");
            json_error(500, "Failed to trigger sync", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error in sync: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}
```

**Step 2: Add routes to the external admin router**

In `routes.rs`, inside `create_external_admin_router()`, add to the `protected` router after the SSE route and before the MCP nest_service:

```rust
        // Repository management
        .route("/v1/admin/repos", get(admin::list_repos))
        .route("/v1/admin/repos", post(admin::create_repo))
        .route("/v1/admin/repos/:name", get(admin::get_repo))
        .route("/v1/admin/repos/:name", put(admin::update_repo))
        .route("/v1/admin/repos/:name", delete(admin::delete_repo))
        .route("/v1/admin/repos/:name/sync", post(admin::sync_repo))
```

Add `put` to the `use` imports at the top of `routes.rs` if not already there:
```rust
use axum::routing::{delete, get, post, put};
```

**Step 3: Build and test**

Run: `cargo build --features server`
Expected: Compiles cleanly.

Run: `cargo test --features server -p conary-server`
Expected: All existing tests pass.

**Step 4: Add unit tests**

Add to the `#[cfg(test)]` section of `admin.rs`:

```rust
    #[tokio::test]
    async fn test_repo_crud_lifecycle() {
        let (app, db_path) = test_app().await;

        // POST /v1/admin/repos - create
        let body = serde_json::json!({"name": "test-repo", "url": "https://example.com/repo"});
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Build fresh router for GET
        let mut config2 = crate::server::ServerConfig::default();
        config2.db_path = db_path.clone();
        config2.chunk_dir = db_path.parent().unwrap().join("chunks");
        config2.cache_dir = db_path.parent().unwrap().join("cache");
        let state2 = Arc::new(RwLock::new(crate::server::ServerState::new(config2)));
        let app2 = crate::server::routes::create_external_admin_router(state2);

        // GET /v1/admin/repos - list
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let repos: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(repos.as_array().unwrap().iter().any(|r| r["name"] == "test-repo"));
    }

    #[tokio::test]
    async fn test_repo_scope_enforcement() {
        let (app, db_path) = test_app().await;

        // Create a token with only ci:read scope
        let hash = crate::server::auth::hash_token("ci-only-token");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "ci-only", &hash, "ci:read").unwrap();
        }

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer ci-only-token")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
```

**Step 5: Run tests again**

Run: `cargo test --features server -p conary-server`
Expected: All tests pass including the two new ones.

**Step 6: Commit**

```bash
git add conary-server/src/server/handlers/admin.rs conary-server/src/server/routes.rs
git commit -m "feat(server): add repository management endpoints to admin API"
```

---

### Task 2: Federation Management Handlers

**Files:**
- Modify: `conary-server/src/server/handlers/admin.rs`
- Modify: `conary-server/src/server/routes.rs`

**Context:** The `federation_peers` table exists (migration v34) with columns: `id`, `endpoint`, `node_name`, `tier`, `first_seen`, `last_seen`, `latency_ms`, `success_count`, `failure_count`, `consecutive_failures`, `is_enabled`. Federation config is `FederationConfig` in `conary-server/src/federation/config.rs` (Serialize + Deserialize). Unlike repos, there's no model file — we query the DB directly (like the existing `federation::directory` handler does).

**Step 1: Add federation types and handlers to admin.rs**

Add after the repository management section:

```rust
// ========== Federation Management ==========

/// Response body for a federation peer.
#[derive(Debug, Serialize)]
pub struct PeerResponse {
    pub id: String,
    pub endpoint: String,
    pub node_name: Option<String>,
    pub tier: String,
    pub first_seen: String,
    pub last_seen: String,
    pub latency_ms: Option<f64>,
    pub success_count: i64,
    pub failure_count: i64,
    pub consecutive_failures: i64,
    pub is_enabled: bool,
}

/// Request body for adding a federation peer.
#[derive(Debug, Deserialize)]
pub struct AddPeerRequest {
    pub endpoint: String,
    pub tier: Option<String>,
    pub node_name: Option<String>,
}

/// GET /v1/admin/federation/peers
pub async fn list_peers(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:read") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let mut stmt = conn.prepare(
            "SELECT id, endpoint, node_name, tier, first_seen, last_seen, \
             latency_ms, success_count, failure_count, consecutive_failures, is_enabled \
             FROM federation_peers ORDER BY tier, endpoint"
        )?;
        let peers = stmt.query_map([], |row| {
            Ok(PeerResponse {
                id: row.get(0)?,
                endpoint: row.get(1)?,
                node_name: row.get(2)?,
                tier: row.get(3)?,
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
                latency_ms: row.get(6)?,
                success_count: row.get(7)?,
                failure_count: row.get(8)?,
                consecutive_failures: row.get(9)?,
                is_enabled: row.get::<_, i32>(10)? != 0,
            })
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok::<_, conary_core::error::Error>(peers)
    })
    .await;
    match result {
        Ok(Ok(peers)) => Json(peers).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to list federation peers: {e}");
            json_error(500, "Failed to list peers", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error listing peers: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/federation/peers
pub async fn add_peer(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<AddPeerRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:write") {
        return err;
    }
    let endpoint = body.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return json_error(400, "Endpoint URL is required", "INVALID_INPUT");
    }
    // Validate URL
    if url::Url::parse(&endpoint).is_err() {
        return json_error(400, "Invalid endpoint URL", "INVALID_INPUT");
    }
    let tier = body.tier.unwrap_or_else(|| "leaf".to_string());
    let node_name = body.node_name;
    let peer_id = conary_core::hash::sha256(endpoint.as_bytes());
    let db_path = { state.read().await.config.db_path.clone() };
    let peer_id_clone = peer_id.clone();
    let endpoint_clone = endpoint.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conn.execute(
            "INSERT OR REPLACE INTO federation_peers \
             (id, endpoint, node_name, tier, first_seen, last_seen, \
              latency_ms, success_count, failure_count, consecutive_failures, is_enabled) \
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'), NULL, 0, 0, 0, 1)",
            rusqlite::params![peer_id_clone, endpoint_clone, node_name, tier],
        )?;
        Ok::<_, conary_core::error::Error>(())
    })
    .await;
    match result {
        Ok(Ok(())) => {
            let s = state.read().await;
            s.publish_event("federation.peer_added", serde_json::json!({"endpoint": endpoint, "id": peer_id}));
            (StatusCode::CREATED, Json(serde_json::json!({"id": peer_id, "endpoint": endpoint, "tier": tier}))).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to add federation peer: {e}");
            json_error(500, "Failed to add peer", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error adding peer: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/federation/peers/:id
pub async fn delete_peer(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:write") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let id_clone = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let changed = conn.execute("DELETE FROM federation_peers WHERE id = ?1", [&id_clone])?;
        Ok::<_, conary_core::error::Error>(changed > 0)
    })
    .await;
    match result {
        Ok(Ok(true)) => {
            let s = state.read().await;
            s.publish_event("federation.peer_removed", serde_json::json!({"id": id}));
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(false)) => json_error(404, "Peer not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to delete federation peer: {e}");
            json_error(500, "Failed to delete peer", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error deleting peer: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/federation/peers/:id/health
pub async fn peer_health(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:read") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let mut stmt = conn.prepare(
            "SELECT id, endpoint, node_name, tier, first_seen, last_seen, \
             latency_ms, success_count, failure_count, consecutive_failures, is_enabled \
             FROM federation_peers WHERE id = ?1"
        )?;
        let peer = stmt.query_row([&id], |row| {
            Ok(PeerResponse {
                id: row.get(0)?,
                endpoint: row.get(1)?,
                node_name: row.get(2)?,
                tier: row.get(3)?,
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
                latency_ms: row.get(6)?,
                success_count: row.get(7)?,
                failure_count: row.get(8)?,
                consecutive_failures: row.get(9)?,
                is_enabled: row.get::<_, i32>(10)? != 0,
            })
        }).optional()?;
        Ok::<_, conary_core::error::Error>(peer)
    })
    .await;
    match result {
        Ok(Ok(Some(peer))) => {
            let total = peer.success_count + peer.failure_count;
            let success_rate = if total > 0 { peer.success_count as f64 / total as f64 } else { 0.0 };
            Json(serde_json::json!({
                "peer": peer,
                "health": {
                    "success_rate": success_rate,
                    "total_requests": total,
                    "status": if peer.consecutive_failures > 5 { "unhealthy" }
                              else if peer.consecutive_failures > 0 { "degraded" }
                              else { "healthy" }
                }
            })).into_response()
        }
        Ok(Ok(None)) => json_error(404, "Peer not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to get peer health: {e}");
            json_error(500, "Failed to get peer health", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error getting peer health: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/federation/config
pub async fn get_federation_config(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:read") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        // Read federation config from metadata table if it exists,
        // otherwise return defaults
        let config_json: Option<String> = conn
            .prepare("SELECT value FROM metadata WHERE key = 'federation_config'")
            .ok()
            .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)).ok());
        match config_json {
            Some(json) => {
                let config: serde_json::Value = serde_json::from_str(&json)
                    .unwrap_or_else(|_| serde_json::to_value(crate::federation::config::FederationConfig::default()).unwrap());
                Ok::<_, conary_core::error::Error>(config)
            }
            None => {
                let config = crate::federation::config::FederationConfig::default();
                Ok(serde_json::to_value(config).unwrap())
            }
        }
    })
    .await;
    match result {
        Ok(Ok(config)) => Json(config).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to get federation config: {e}");
            json_error(500, "Failed to get federation config", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// PUT /v1/admin/federation/config
pub async fn update_federation_config(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "federation:write") {
        return err;
    }
    // Validate that it's a valid FederationConfig
    let _config: crate::federation::config::FederationConfig = match serde_json::from_value(body.clone()) {
        Ok(c) => c,
        Err(e) => return json_error(400, &format!("Invalid federation config: {e}"), "INVALID_INPUT"),
    };
    let db_path = { state.read().await.config.db_path.clone() };
    let config_json = serde_json::to_string(&body).unwrap();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('federation_config', ?1)",
            [&config_json],
        )?;
        Ok::<_, conary_core::error::Error>(())
    })
    .await;
    match result {
        Ok(Ok(())) => {
            let s = state.read().await;
            s.publish_event("federation.config_updated", serde_json::json!({}));
            Json(body).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to update federation config: {e}");
            json_error(500, "Failed to update federation config", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}
```

Add `use rusqlite::OptionalExtension;` to the imports at the top of `admin.rs`.

**Step 2: Add federation routes to the external admin router**

In `routes.rs`, add to the `protected` router after the repo routes:

```rust
        // Federation management
        .route("/v1/admin/federation/peers", get(admin::list_peers))
        .route("/v1/admin/federation/peers", post(admin::add_peer))
        .route("/v1/admin/federation/peers/:id", delete(admin::delete_peer))
        .route("/v1/admin/federation/peers/:id/health", get(admin::peer_health))
        .route("/v1/admin/federation/config", get(admin::get_federation_config))
        .route("/v1/admin/federation/config", put(admin::update_federation_config))
```

**Step 3: Build and test**

Run: `cargo build --features server`
Expected: Compiles cleanly.

Run: `cargo test --features server -p conary-server`
Expected: All tests pass.

**Step 4: Add federation unit tests**

Add to `#[cfg(test)]` in `admin.rs`:

```rust
    #[tokio::test]
    async fn test_federation_peer_lifecycle() {
        let (app, db_path) = test_app().await;

        // POST /v1/admin/federation/peers - add peer
        let body = serde_json::json!({"endpoint": "https://peer1.example.com:7891", "tier": "leaf"});
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/federation/peers")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let peer: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let peer_id = peer["id"].as_str().unwrap().to_string();

        // Build fresh router for GET
        let mut config2 = crate::server::ServerConfig::default();
        config2.db_path = db_path.clone();
        config2.chunk_dir = db_path.parent().unwrap().join("chunks");
        config2.cache_dir = db_path.parent().unwrap().join("cache");
        let state2 = Arc::new(RwLock::new(crate::server::ServerState::new(config2)));
        let app2 = crate::server::routes::create_external_admin_router(state2);

        // GET /v1/admin/federation/peers - list
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/federation/peers")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let peers: Vec<serde_json::Value> = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0]["endpoint"], "https://peer1.example.com:7891");

        // Build fresh router for DELETE
        let mut config3 = crate::server::ServerConfig::default();
        config3.db_path = db_path.clone();
        config3.chunk_dir = db_path.parent().unwrap().join("chunks");
        config3.cache_dir = db_path.parent().unwrap().join("cache");
        let state3 = Arc::new(RwLock::new(crate::server::ServerState::new(config3)));
        let app3 = crate::server::routes::create_external_admin_router(state3);

        // DELETE /v1/admin/federation/peers/:id
        let resp = app3
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(&format!("/v1/admin/federation/peers/{peer_id}"))
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
```

**Step 5: Run tests**

Run: `cargo test --features server -p conary-server`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add conary-server/src/server/handlers/admin.rs conary-server/src/server/routes.rs
git commit -m "feat(server): add federation management endpoints to admin API"
```

---

### Task 3: Missing MCP Tools (create_token, delete_token)

**Files:**
- Modify: `conary-server/src/server/mcp.rs`

**Context:** The existing MCP server has 7 tools. We need to add `create_token` and `delete_token` to match the design doc. We also add repo and federation tools to cover the P1 endpoints. The `sse_subscribe` tool from the design doc is intentionally omitted — SSE is a streaming protocol and MCP tools are request-response; the design doc notes it would need to be "converted to polling" which adds complexity for minimal value.

**Step 1: Add parameter structs for new tools**

Add after the existing `RunIdParams`:

```rust
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

/// Parameters for listing repositories.
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
```

**Step 2: Add MCP tool implementations**

Add these inside the `#[tool_router] impl RemiMcpServer` block, after `list_tokens`:

```rust
    /// Create a new admin API token. Returns the plaintext token exactly once.
    ///
    /// **Not idempotent** — every call creates a new token.
    #[tool(description = "Create a new admin API token. Returns plaintext token once. NOT idempotent.")]
    async fn create_token(
        &self,
        Parameters(params): Parameters<CreateTokenParams>,
    ) -> Result<CallToolResult, McpError> {
        let name = params.name.trim().to_string();
        if name.is_empty() || name.len() > 128 {
            return Err(McpError::invalid_params("Token name must be 1-128 characters", None));
        }
        let scopes = params.scopes.unwrap_or_else(|| "admin".to_string());
        let raw_token = crate::server::auth::generate_token();
        let token_hash = crate::server::auth::hash_token(&raw_token);

        let db_path = { self.state.read().await.config.db_path.clone() };
        let name_clone = name.clone();
        let scopes_clone = scopes.clone();
        let id = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            conary_core::db::models::admin_token::create(&conn, &name_clone, &token_hash, &scopes_clone)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        let result = serde_json::json!({
            "id": id,
            "name": name,
            "token": raw_token,
            "scopes": scopes,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap(),
        )]))
    }

    /// Delete (revoke) an admin API token by ID.
    #[tool(description = "Delete an admin API token by its numeric ID.")]
    async fn delete_token(
        &self,
        Parameters(params): Parameters<DeleteTokenParams>,
    ) -> Result<CallToolResult, McpError> {
        let db_path = { self.state.read().await.config.db_path.clone() };
        let deleted = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            conary_core::db::models::admin_token::delete(&conn, params.token_id)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        if deleted {
            Ok(CallToolResult::success(vec![Content::text(
                format!(r#"{{"deleted": true, "id": {}}}"#, params.token_id),
            )]))
        } else {
            Err(McpError::invalid_params(
                format!("Token with ID {} not found", params.token_id),
                None,
            ))
        }
    }

    /// List all configured repositories with their sync status.
    #[tool(description = "List all configured repositories with sync status and config.")]
    async fn list_repos(&self) -> Result<CallToolResult, McpError> {
        let db_path = { self.state.read().await.config.db_path.clone() };
        let result = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            conary_core::db::models::repository::Repository::list_all(&conn)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        let repos: Vec<serde_json::Value> = result.into_iter().map(|r| {
            serde_json::json!({
                "id": r.id, "name": r.name, "url": r.url,
                "enabled": r.enabled, "priority": r.priority,
                "last_sync": r.last_sync, "gpg_check": r.gpg_check,
            })
        }).collect();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&repos).unwrap(),
        )]))
    }

    /// Get details for a specific repository by name.
    #[tool(description = "Get details for a specific repository by name.")]
    async fn get_repo(
        &self,
        Parameters(params): Parameters<RepoNameParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_path_param(&params.name, "repo name")?;
        let db_path = { self.state.read().await.config.db_path.clone() };
        let name = params.name.clone();
        let result = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            conary_core::db::models::repository::Repository::find_by_name(&conn, &name)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        match result {
            Some(r) => {
                let resp = serde_json::json!({
                    "id": r.id, "name": r.name, "url": r.url,
                    "content_url": r.content_url, "enabled": r.enabled,
                    "priority": r.priority, "gpg_check": r.gpg_check,
                    "metadata_expire": r.metadata_expire,
                    "last_sync": r.last_sync, "created_at": r.created_at,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&resp).unwrap(),
                )]))
            }
            None => Err(McpError::invalid_params(
                format!("Repository '{}' not found", params.name),
                None,
            )),
        }
    }

    /// List all federation peers with health information.
    #[tool(description = "List all federation peers with health status, latency, and success rates.")]
    async fn list_peers(&self) -> Result<CallToolResult, McpError> {
        let db_path = { self.state.read().await.config.db_path.clone() };
        let result = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            let mut stmt = conn.prepare(
                "SELECT id, endpoint, node_name, tier, last_seen, \
                 success_count, failure_count, consecutive_failures, is_enabled \
                 FROM federation_peers ORDER BY tier, endpoint"
            ).map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            let peers = stmt.query_map([], |row| {
                let success: i64 = row.get(5)?;
                let failure: i64 = row.get(6)?;
                let total = success + failure;
                let rate = if total > 0 { success as f64 / total as f64 } else { 0.0 };
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "endpoint": row.get::<_, String>(1)?,
                    "node_name": row.get::<_, Option<String>>(2)?,
                    "tier": row.get::<_, String>(3)?,
                    "last_seen": row.get::<_, String>(4)?,
                    "success_rate": rate,
                    "total_requests": total,
                    "consecutive_failures": row.get::<_, i64>(7)?,
                    "enabled": row.get::<_, i32>(8)? != 0,
                }))
            }).map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            Ok::<_, McpError>(peers)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap(),
        )]))
    }

    /// Add a new federation peer.
    ///
    /// **Not idempotent** — re-adding an existing endpoint replaces its record.
    #[tool(description = "Add a federation peer by endpoint URL. Re-adding replaces the existing record.")]
    async fn add_peer(
        &self,
        Parameters(params): Parameters<AddPeerParams>,
    ) -> Result<CallToolResult, McpError> {
        let endpoint = params.endpoint.trim().to_string();
        if endpoint.is_empty() {
            return Err(McpError::invalid_params("Endpoint URL is required", None));
        }
        if url::Url::parse(&endpoint).is_err() {
            return Err(McpError::invalid_params("Invalid endpoint URL", None));
        }
        let tier = params.tier.unwrap_or_else(|| "leaf".to_string());
        let peer_id = conary_core::hash::sha256(endpoint.as_bytes());
        let db_path = { self.state.read().await.config.db_path.clone() };
        let pid = peer_id.clone();
        let ep = endpoint.clone();
        let t = tier.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)
                .map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            conn.execute(
                "INSERT OR REPLACE INTO federation_peers \
                 (id, endpoint, node_name, tier, first_seen, last_seen, \
                  latency_ms, success_count, failure_count, consecutive_failures, is_enabled) \
                 VALUES (?1, ?2, NULL, ?3, datetime('now'), datetime('now'), NULL, 0, 0, 0, 1)",
                rusqlite::params![pid, ep, t],
            ).map_err(|e| McpError::internal_error(format!("DB error: {e}"), None))?;
            Ok::<_, McpError>(())
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task error: {e}"), None))??;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&serde_json::json!({
                "id": peer_id, "endpoint": endpoint, "tier": tier
            })).unwrap(),
        )]))
    }
```

**Step 3: Update ServerHandler instructions**

Update the `get_info()` method to include repo and federation tools:

```rust
        .with_instructions(
            "Remi MCP server -- manage CI workflows, inspect runs, \
             trigger builds, sync mirrors, manage admin tokens, \
             list/inspect repositories, and manage federation peers.",
        )
```

**Step 4: Update the tool count test**

Change the assertion in `test_mcp_tool_count`:

```rust
        assert_eq!(tools.len(), 14, "Expected 14 MCP tools");
```

(7 existing + 2 token tools + 2 repo tools + 3 federation tools = 14)

**Step 5: Build and test**

Run: `cargo build --features server`
Expected: Compiles cleanly.

Run: `cargo test --features server -p conary-server`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add conary-server/src/server/mcp.rs
git commit -m "feat(server): add token, repo, and federation MCP tools"
```

---

### Task 4: Update OpenAPI Spec

**Files:**
- Modify: `conary-server/src/server/handlers/openapi.rs`

**Context:** The existing OpenAPI spec covers tokens, CI, and SSE. We need to add the repo and federation endpoint descriptions.

**Step 1: Add repo and federation paths to the OpenAPI spec**

In the `openapi_spec()` function, add these paths to the `"paths"` object in the JSON, after the `/v1/admin/events` entry:

```rust
            "/v1/admin/repos": {
                "get": {
                    "operationId": "listRepos",
                    "summary": "List configured repositories",
                    "description": "Returns all repositories with sync status. Use this to see what package sources are configured.",
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "Array of repository objects"}
                    }
                },
                "post": {
                    "operationId": "createRepo",
                    "summary": "Add a repository",
                    "description": "Add a new package repository. Requires name and URL at minimum.",
                    "security": [{"bearerAuth": []}],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["name", "url"],
                                    "properties": {
                                        "name": {"type": "string"},
                                        "url": {"type": "string"},
                                        "content_url": {"type": "string"},
                                        "enabled": {"type": "boolean"},
                                        "priority": {"type": "integer"},
                                        "gpg_check": {"type": "boolean"},
                                        "metadata_expire": {"type": "integer"}
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "201": {"description": "Repository created"}
                    }
                }
            },
            "/v1/admin/repos/{name}": {
                "get": {
                    "operationId": "getRepo",
                    "summary": "Get repository details",
                    "parameters": [{"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "Repository details"},
                        "404": {"description": "Repository not found"}
                    }
                },
                "put": {
                    "operationId": "updateRepo",
                    "summary": "Update repository config",
                    "parameters": [{"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "security": [{"bearerAuth": []}],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["name", "url"],
                                    "properties": {
                                        "name": {"type": "string"},
                                        "url": {"type": "string"},
                                        "enabled": {"type": "boolean"},
                                        "priority": {"type": "integer"}
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {"description": "Repository updated"}
                    }
                },
                "delete": {
                    "operationId": "deleteRepo",
                    "summary": "Remove repository",
                    "parameters": [{"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "204": {"description": "Repository deleted"},
                        "404": {"description": "Repository not found"}
                    }
                }
            },
            "/v1/admin/repos/{name}/sync": {
                "post": {
                    "operationId": "syncRepo",
                    "summary": "Trigger manual repository sync",
                    "parameters": [{"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "202": {"description": "Sync requested"},
                        "404": {"description": "Repository not found"}
                    }
                }
            },
            "/v1/admin/federation/peers": {
                "get": {
                    "operationId": "listPeers",
                    "summary": "List federation peers with health",
                    "description": "Returns all federation peers with latency, success rates, and health status.",
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "Array of peer objects with health data"}
                    }
                },
                "post": {
                    "operationId": "addPeer",
                    "summary": "Add a federation peer",
                    "security": [{"bearerAuth": []}],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["endpoint"],
                                    "properties": {
                                        "endpoint": {"type": "string"},
                                        "tier": {"type": "string", "enum": ["leaf", "cell_hub", "region_hub"]},
                                        "node_name": {"type": "string"}
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "201": {"description": "Peer added"}
                    }
                }
            },
            "/v1/admin/federation/peers/{id}": {
                "delete": {
                    "operationId": "deletePeer",
                    "summary": "Remove a federation peer",
                    "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "204": {"description": "Peer removed"},
                        "404": {"description": "Peer not found"}
                    }
                }
            },
            "/v1/admin/federation/peers/{id}/health": {
                "get": {
                    "operationId": "peerHealth",
                    "summary": "Get detailed peer health",
                    "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "Peer details with health metrics"},
                        "404": {"description": "Peer not found"}
                    }
                }
            },
            "/v1/admin/federation/config": {
                "get": {
                    "operationId": "getFederationConfig",
                    "summary": "Get federation configuration",
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "Federation configuration object"}
                    }
                },
                "put": {
                    "operationId": "updateFederationConfig",
                    "summary": "Update federation configuration",
                    "security": [{"bearerAuth": []}],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {"type": "object"}
                            }
                        }
                    },
                    "responses": {
                        "200": {"description": "Updated federation configuration"}
                    }
                }
            }
```

**Step 2: Build and test**

Run: `cargo build --features server`
Run: `cargo test --features server -p conary-server`
Expected: All pass.

**Step 3: Commit**

```bash
git add conary-server/src/server/handlers/openapi.rs
git commit -m "feat(server): add repo and federation endpoints to OpenAPI spec"
```

---

### Task 5: Clippy and Build Verification

**Files:** None (verification only)

**Step 1: Full build**

Run: `cargo build --features server`
Expected: Compiles cleanly.

**Step 2: Clippy**

Run: `cargo clippy --features server -- -D warnings`
Expected: Zero warnings. Fix any that arise.

**Step 3: All tests**

Run: `cargo test --features server`
Expected: All tests pass.

**Step 4: Default feature build**

Run: `cargo build && cargo test`
Expected: Client-only build compiles and tests pass.

---

### Task 6: Documentation Updates

**Files:**
- Modify: `.claude/rules/server.md`
- Modify: `.claude/rules/architecture.md`
- Modify: `docs/plans/2026-03-07-remi-admin-api-design.md`

**Step 1: Update server.md**

Add to Remi Server Key Types section:
- `RepoRequest` / `RepoResponse` -- admin API repo management types
- `PeerResponse` / `AddPeerRequest` -- admin API federation peer types

Add to Invariants:
- Repo management uses existing `Repository` model CRUD (no new DB schema needed)
- Federation peer management queries `federation_peers` table directly

**Step 2: Update design doc**

Mark P1 items as implemented in the phasing table.

**Step 3: Commit**

```bash
git add .claude/rules/server.md .claude/rules/architecture.md docs/plans/2026-03-07-remi-admin-api-design.md
git commit -m "docs: update server rules and design doc for P1 admin API endpoints"
```
