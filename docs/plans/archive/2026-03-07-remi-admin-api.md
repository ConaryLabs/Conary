# Remi Admin API Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a token-authenticated external admin API to Remi for CI monitoring, repo management, federation control, SSE event streaming, with an MCP endpoint for direct LLM agent integration and an OpenAPI spec for discoverability.

**Architecture:** New listener on :8082 with bearer token auth middleware, reusing the existing admin router pattern. Tokens stored in SQLite `admin_tokens` table (SHA-256 hashed). CI endpoints proxy to Forgejo API with server-side credential injection. SSE via `tokio::sync::broadcast`. MCP endpoint via `rmcp` crate wraps the same handler functions — no logic duplication.

**Tech Stack:** Rust 1.93, axum (existing), tokio, reqwest (existing), serde, sha2, rusqlite, rmcp (MCP SDK)

---

## Context for Implementer

### Codebase Layout

- **Server crate:** `conary-server/` (feature-gated: `--features server`)
- **Core crate:** `conary-core/` (shared library)
- **Build:** `cargo build --features server` / `cargo test --features server`
- **Clippy:** `cargo clippy --features server -- -D warnings`

### Key Files You'll Touch

| File | What's There |
|------|-------------|
| `conary-server/src/server/mod.rs` | `ServerConfig`, `ServerState`, `run_server_from_config()` |
| `conary-server/src/server/routes.rs` | `create_admin_router()`, middleware stack |
| `conary-server/src/server/config.rs` | `RemiConfig`, `ServerSection`, TOML parsing |
| `conary-server/src/server/security.rs` | `RateLimiter`, `BanList` |
| `conary-server/src/server/handlers/mod.rs` | Handler module declarations, shared utilities |
| `conary-core/src/db/schema.rs` | `SCHEMA_VERSION`, migration dispatch |
| `conary-core/src/db/migrations.rs` | Migration functions |

### Existing Patterns to Follow

**File headers:** Every `.rs` file starts with `// path/to/file.rs`

**Handler signature:**
```rust
pub async fn handler_name(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(param): Path<String>,
) -> Response
```

**State type:** `Arc<RwLock<ServerState>>` — read lock via `state.read().await`

**DB access:** Synchronous `conary_core::db::open(&db_path)` — no connection pool

**Error responses:** `(StatusCode::BAD_REQUEST, "message").into_response()`

**JSON responses:** `Json(value).into_response()` or `handlers::json_response(json_string, cache_seconds)`

**Tests:** In-file `#[cfg(test)] mod tests { ... }`, use `#[tokio::test]` for async

---

### Task 1: Database Migration — `admin_tokens` Table

**Files:**
- Modify: `conary-core/src/db/schema.rs`
- Modify: `conary-core/src/db/migrations.rs`
- Create: `conary-core/src/db/models/admin_token.rs`
- Modify: `conary-core/src/db/models/mod.rs`

**Step 1: Write the failing test**

Add to `conary-core/src/db/models/admin_token.rs`:

```rust
// conary-core/src/db/models/admin_token.rs

//! Admin API token management

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

/// An admin API token record (never includes the raw token)
#[derive(Debug, Clone, Serialize)]
pub struct AdminToken {
    pub id: i64,
    pub name: String,
    pub token_hash: String,
    pub scopes: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// Create a new admin token. Returns the row ID.
pub fn create(conn: &Connection, name: &str, token_hash: &str, scopes: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO admin_tokens (name, token_hash, scopes) VALUES (?1, ?2, ?3)",
        [name, token_hash, scopes],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Look up a token by its hash. Returns None if not found.
pub fn find_by_hash(conn: &Connection, token_hash: &str) -> Result<Option<AdminToken>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, token_hash, scopes, created_at, last_used_at
         FROM admin_tokens WHERE token_hash = ?1",
    )?;
    let result = stmt
        .query_row([token_hash], |row| {
            Ok(AdminToken {
                id: row.get(0)?,
                name: row.get(1)?,
                token_hash: row.get(2)?,
                scopes: row.get(3)?,
                created_at: row.get(4)?,
                last_used_at: row.get(5)?,
            })
        })
        .optional()?;
    Ok(result)
}

/// List all tokens (without hashes — for display only)
pub fn list(conn: &Connection) -> Result<Vec<AdminToken>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, '', scopes, created_at, last_used_at FROM admin_tokens ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AdminToken {
            id: row.get(0)?,
            name: row.get(1)?,
            token_hash: row.get(2)?,
            scopes: row.get(3)?,
            created_at: row.get(4)?,
            last_used_at: row.get(5)?,
        })
    })?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// Delete a token by ID. Returns true if a row was deleted.
pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    let count = conn.execute("DELETE FROM admin_tokens WHERE id = ?1", [id])?;
    Ok(count > 0)
}

/// Update last_used_at to current time
pub fn touch(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE admin_tokens SET last_used_at = datetime('now') WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_create_and_find_by_hash() {
        let conn = test_db();
        let id = create(&conn, "ci-bot", "abc123hash", "ci:read,ci:trigger").unwrap();
        assert!(id > 0);

        let token = find_by_hash(&conn, "abc123hash").unwrap().unwrap();
        assert_eq!(token.name, "ci-bot");
        assert_eq!(token.scopes, "ci:read,ci:trigger");
        assert!(token.last_used_at.is_none());
    }

    #[test]
    fn test_find_by_hash_not_found() {
        let conn = test_db();
        assert!(find_by_hash(&conn, "nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_list_tokens() {
        let conn = test_db();
        create(&conn, "token1", "hash1", "admin").unwrap();
        create(&conn, "token2", "hash2", "ci:read").unwrap();

        let tokens = list(&conn).unwrap();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].name, "token1");
        assert_eq!(tokens[1].name, "token2");
        // Hashes should be empty in list output
        assert!(tokens[0].token_hash.is_empty());
    }

    #[test]
    fn test_delete_token() {
        let conn = test_db();
        let id = create(&conn, "disposable", "hash", "admin").unwrap();
        assert!(delete(&conn, id).unwrap());
        assert!(!delete(&conn, id).unwrap()); // Already deleted
        assert!(find_by_hash(&conn, "hash").unwrap().is_none());
    }

    #[test]
    fn test_touch_updates_last_used() {
        let conn = test_db();
        let id = create(&conn, "bot", "hash", "admin").unwrap();
        assert!(find_by_hash(&conn, "hash").unwrap().unwrap().last_used_at.is_none());

        touch(&conn, id).unwrap();
        assert!(find_by_hash(&conn, "hash").unwrap().unwrap().last_used_at.is_some());
    }
}
```

**Step 2: Add the migration**

In `conary-core/src/db/migrations.rs`, add:

```rust
pub fn migrate_v47(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS admin_tokens (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            scopes TEXT NOT NULL DEFAULT 'admin',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_admin_tokens_hash ON admin_tokens(token_hash);",
    )?;
    Ok(())
}
```

In `conary-core/src/db/schema.rs`:
- Change `SCHEMA_VERSION` from `46` to `47`
- Add dispatch: `46 => migrations::migrate_v47(conn)?`

In `conary-core/src/db/models/mod.rs`:
- Add `pub mod admin_token;`

**Step 3: Run tests to verify**

Run: `cargo test --features server -p conary-core -- admin_token`
Expected: All 5 tests pass

**Step 4: Commit**

```bash
git add conary-core/src/db/models/admin_token.rs conary-core/src/db/schema.rs conary-core/src/db/migrations.rs conary-core/src/db/models/mod.rs
git commit -m "feat(db): add admin_tokens table (migration v47)"
```

---

### Task 2: Token Hashing and Auth Utilities

**Files:**
- Create: `conary-server/src/server/auth.rs`
- Modify: `conary-server/src/server/mod.rs` (add `pub mod auth;`)

This module handles token generation, hashing, and the axum auth middleware extractor.

**Step 1: Write the module with tests**

Create `conary-server/src/server/auth.rs`:

```rust
// conary-server/src/server/auth.rs

//! Bearer token authentication for the external admin API

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use conary_core::db::models::admin_token;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use super::ServerState;

/// Hash a raw token string to its storage form (hex-encoded SHA-256)
pub fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generate a cryptographically random token (32 bytes, hex-encoded = 64 chars)
pub fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    hex::encode(bytes)
}

/// JSON error response for auth failures
#[derive(Serialize)]
struct AuthError {
    error: String,
    code: String,
}

fn auth_error(status: StatusCode, message: &str, code: &str) -> Response {
    let body = AuthError {
        error: message.to_string(),
        code: code.to_string(),
    };
    (status, axum::Json(body)).into_response()
}

/// Extract bearer token from Authorization header
fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Auth middleware for the external admin API.
///
/// Validates the bearer token against the DB, updates last_used_at,
/// and stores the token's scopes in request extensions.
pub async fn auth_middleware(
    State(state): State<Arc<RwLock<ServerState>>>,
    mut request: Request,
    next: Next,
) -> Response {
    let raw_token = match extract_bearer(request.headers()) {
        Some(t) => t.to_string(),
        None => {
            return auth_error(
                StatusCode::UNAUTHORIZED,
                "Missing or malformed Authorization header",
                "UNAUTHORIZED",
            );
        }
    };

    let token_hash = hash_token(&raw_token);
    let db_path = {
        let state_guard = state.read().await;
        state_guard.config.db_path.clone()
    };

    // DB lookup in blocking context
    let lookup_result = {
        let path = db_path.clone();
        let hash = token_hash.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<admin_token::AdminToken>, String> {
            let conn = conary_core::db::open(&path).map_err(|e| e.to_string())?;
            admin_token::find_by_hash(&conn, &hash).map_err(|e| e.to_string())
        })
        .await
    };

    match lookup_result {
        Ok(Ok(Some(token))) => {
            // Update last_used_at in background (fire-and-forget)
            let token_id = token.id;
            let path = db_path;
            tokio::task::spawn_blocking(move || {
                if let Ok(conn) = conary_core::db::open(&path) {
                    let _ = admin_token::touch(&conn, token_id);
                }
            });

            // Store scopes in request extensions for handlers to check
            request.extensions_mut().insert(TokenScopes(token.scopes));
            next.run(request).await
        }
        Ok(Ok(None)) => {
            warn!("Admin API: invalid token attempted");
            auth_error(StatusCode::UNAUTHORIZED, "Invalid token", "UNAUTHORIZED")
        }
        Ok(Err(e)) => {
            tracing::error!("Admin API auth DB error: {}", e);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "INTERNAL_ERROR",
            )
        }
        Err(e) => {
            tracing::error!("Admin API auth task error: {}", e);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "INTERNAL_ERROR",
            )
        }
    }
}

/// Scopes attached to a validated request (stored in extensions)
#[derive(Clone, Debug)]
pub struct TokenScopes(pub String);

impl TokenScopes {
    /// Check if this token has a specific scope (or "admin" which grants all)
    pub fn has_scope(&self, required: &str) -> bool {
        if self.0.contains("admin") {
            return true;
        }
        self.0.split(',').any(|s| s.trim() == required)
    }
}

/// Helper to check scope from a request's extensions. Returns error response if insufficient.
#[allow(clippy::result_large_err)]
pub fn require_scope(extensions: &axum::http::Extensions, scope: &str) -> Result<(), Response> {
    match extensions.get::<TokenScopes>() {
        Some(scopes) if scopes.has_scope(scope) => Ok(()),
        Some(_) => Err(auth_error(
            StatusCode::FORBIDDEN,
            &format!("Requires scope: {scope}"),
            "INSUFFICIENT_SCOPE",
        )),
        None => Err(auth_error(
            StatusCode::UNAUTHORIZED,
            "Not authenticated",
            "UNAUTHORIZED",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_token_deterministic() {
        let h1 = hash_token("my-secret-token");
        let h2 = hash_token("my-secret-token");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_hash_token_different_inputs() {
        assert_ne!(hash_token("token-a"), hash_token("token-b"));
    }

    #[test]
    fn test_generate_token_length() {
        let token = generate_token();
        assert_eq!(token.len(), 64); // 32 bytes hex
    }

    #[test]
    fn test_generate_token_unique() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2);
    }

    #[test]
    fn test_token_scopes_admin_grants_all() {
        let scopes = TokenScopes("admin".to_string());
        assert!(scopes.has_scope("ci:read"));
        assert!(scopes.has_scope("repos:write"));
        assert!(scopes.has_scope("anything"));
    }

    #[test]
    fn test_token_scopes_specific() {
        let scopes = TokenScopes("ci:read,ci:trigger".to_string());
        assert!(scopes.has_scope("ci:read"));
        assert!(scopes.has_scope("ci:trigger"));
        assert!(!scopes.has_scope("repos:write"));
        assert!(!scopes.has_scope("admin"));
    }

    #[test]
    fn test_extract_bearer_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer mytoken123".parse().unwrap());
        assert_eq!(extract_bearer(&headers), Some("mytoken123"));
    }

    #[test]
    fn test_extract_bearer_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_bearer(&headers), None);
    }

    #[test]
    fn test_extract_bearer_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Basic abc123".parse().unwrap());
        assert_eq!(extract_bearer(&headers), None);
    }
}
```

**Step 2: Register the module**

In `conary-server/src/server/mod.rs`, add `pub mod auth;` in the module declarations section (around line 17-40).

**Step 3: Add dependencies**

In `conary-server/Cargo.toml`, ensure these dependencies exist:
- `sha2` (for hashing)
- `rand` (for token generation)
- `hex` (for hex encoding)

Check what's already there — `reqwest`, `serde`, `tokio` are already present.

Run: `cargo build --features server`

**Step 4: Run tests**

Run: `cargo test --features server -p conary-server -- auth`
Expected: All 9 tests pass

**Step 5: Commit**

```bash
git add conary-server/src/server/auth.rs conary-server/src/server/mod.rs conary-server/Cargo.toml
git commit -m "feat(server): add token auth module with hashing, generation, and middleware"
```

---

### Task 3: External Admin Router and Config

**Files:**
- Modify: `conary-server/src/server/config.rs` — add `AdminSection` with `external_bind`, `forgejo_url`
- Modify: `conary-server/src/server/mod.rs` — add `external_admin_bind` to `ServerConfig`, third listener
- Modify: `conary-server/src/server/routes.rs` — add `create_external_admin_router()`

**Step 1: Add AdminSection to config**

In `conary-server/src/server/config.rs`, add a new section to `RemiConfig`:

```rust
/// Admin API settings (external, token-authenticated)
#[serde(default)]
pub admin: AdminSection,
```

Add the section struct:

```rust
/// External admin API configuration
#[derive(Debug, Deserialize)]
pub struct AdminSection {
    /// External admin API bind address (token-authenticated)
    #[serde(default = "default_external_admin_bind")]
    pub external_bind: String,

    /// Enable external admin API
    #[serde(default)]
    pub enabled: bool,

    /// Forgejo instance URL for CI proxy
    #[serde(default)]
    pub forgejo_url: Option<String>,

    /// Forgejo API token (for proxying CI requests)
    #[serde(default)]
    pub forgejo_token: Option<String>,

    /// Bootstrap token from environment (REMI_ADMIN_TOKEN)
    #[serde(default)]
    pub bootstrap_token: Option<String>,
}

impl Default for AdminSection {
    fn default() -> Self {
        Self {
            external_bind: default_external_admin_bind(),
            enabled: false,
            forgejo_url: None,
            forgejo_token: None,
            bootstrap_token: None,
        }
    }
}

fn default_external_admin_bind() -> String {
    "0.0.0.0:8082".to_string()
}
```

Add an accessor to `RemiConfig`:

```rust
pub fn external_admin_bind_addr(&self) -> Result<SocketAddr> {
    self.admin
        .external_bind
        .parse()
        .with_context(|| format!("Invalid external admin bind: {}", self.admin.external_bind))
}
```

**Step 2: Add fields to ServerState**

In `conary-server/src/server/mod.rs`, add to `ServerState`:

```rust
/// Forgejo URL for CI proxy (None = CI proxy disabled)
pub forgejo_url: Option<String>,
/// Forgejo API token (injected server-side into proxied requests)
pub forgejo_token: Option<String>,
/// Broadcast channel for SSE admin events
pub admin_events: tokio::sync::broadcast::Sender<AdminEvent>,
```

Add the event type near `ServerState`:

```rust
/// An admin event for SSE streaming
#[derive(Clone, Debug, serde::Serialize)]
pub struct AdminEvent {
    pub event_type: String,
    pub data: serde_json::Value,
    pub timestamp: String,
}
```

Initialize in `ServerState::with_options()`:

```rust
let (admin_events, _) = tokio::sync::broadcast::channel(1024);
// ... in the Self { } block:
forgejo_url: None,
forgejo_token: None,
admin_events,
```

**Step 3: Create external admin router**

In `conary-server/src/server/routes.rs`, add:

```rust
/// Create the external admin router (token-authenticated, public-facing)
pub fn create_external_admin_router(state: Arc<RwLock<ServerState>>) -> Router {
    Router::new()
        .route("/health", get(|| async { "OK" }))
        // Token management (P0)
        .route("/v1/admin/tokens", post(admin::create_token))
        .route("/v1/admin/tokens", get(admin::list_tokens))
        .route("/v1/admin/tokens/:id", delete(admin::delete_token))
        // CI proxy (P0)
        .route("/v1/admin/ci/workflows", get(admin::ci_list_workflows))
        .route(
            "/v1/admin/ci/workflows/:name/runs",
            get(admin::ci_list_runs),
        )
        .route("/v1/admin/ci/runs/:id", get(admin::ci_get_run))
        .route("/v1/admin/ci/runs/:id/logs", get(admin::ci_get_logs))
        .route(
            "/v1/admin/ci/workflows/:name/dispatch",
            post(admin::ci_dispatch),
        )
        .route("/v1/admin/ci/mirror-sync", post(admin::ci_mirror_sync))
        // SSE events (P1)
        .route("/v1/admin/events", get(admin::sse_events))
        // Auth middleware wraps everything except /health
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::server::auth::auth_middleware,
        ))
        .with_state(state)
}
```

**Step 4: Bind the third listener**

In `conary-server/src/server/mod.rs` `run_server_from_config()`, after existing listener setup:

```rust
// External admin API (token-authenticated)
if remi_config.admin.enabled {
    let external_admin_bind = remi_config.external_admin_bind_addr()?;
    let external_admin_app = create_external_admin_router(state.clone());
    let external_admin_listener =
        tokio::net::TcpListener::bind(external_admin_bind).await?;
    tracing::info!("  External admin API: {}", external_admin_bind);

    // Set Forgejo config
    {
        let mut state_w = state.write().await;
        state_w.forgejo_url = remi_config.admin.forgejo_url.clone();
        state_w.forgejo_token = remi_config.admin.forgejo_token.clone();
    }

    // Bootstrap token from env
    if let Some(ref bootstrap) = remi_config.admin.bootstrap_token {
        let db_path = server_config.db_path.clone();
        let hash = crate::server::auth::hash_token(bootstrap);
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = conary_core::db::open(&db_path) {
                if conary_core::db::models::admin_token::find_by_hash(&conn, &hash)
                    .unwrap_or(None)
                    .is_none()
                {
                    let _ = conary_core::db::models::admin_token::create(
                        &conn, "bootstrap", &hash, "admin",
                    );
                    tracing::info!("  Bootstrap admin token created");
                }
            }
        })
        .await?;
    }

    // Also check REMI_ADMIN_TOKEN env var
    if let Ok(env_token) = std::env::var("REMI_ADMIN_TOKEN") {
        let db_path = server_config.db_path.clone();
        let hash = crate::server::auth::hash_token(&env_token);
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = conary_core::db::open(&db_path) {
                if conary_core::db::models::admin_token::find_by_hash(&conn, &hash)
                    .unwrap_or(None)
                    .is_none()
                {
                    let _ = conary_core::db::models::admin_token::create(
                        &conn, "env-bootstrap", &hash, "admin",
                    );
                    tracing::info!("  Admin token created from REMI_ADMIN_TOKEN env var");
                }
            }
        })
        .await?;
    }

    // Add to tokio::select!
    // (modify existing select to include third branch)
}
```

Update the `tokio::select!` block to conditionally include the third listener. The cleanest approach: move it into a helper or use `Option<TcpListener>` with a conditional future.

**Step 5: Build and verify**

Run: `cargo build --features server`
Expected: Compiles (handlers module `admin` doesn't exist yet — add a stub)

**Step 6: Commit**

```bash
git add conary-server/src/server/config.rs conary-server/src/server/mod.rs conary-server/src/server/routes.rs
git commit -m "feat(server): add external admin router with config and third listener"
```

---

### Task 4: Token Management Handlers

**Files:**
- Create: `conary-server/src/server/handlers/admin.rs`
- Modify: `conary-server/src/server/handlers/mod.rs`

**Step 1: Implement token CRUD handlers**

Create `conary-server/src/server/handlers/admin.rs`:

```rust
// conary-server/src/server/handlers/admin.rs

//! Handlers for the external admin API

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use conary_core::db::models::admin_token;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::auth::{generate_token, hash_token, require_scope};
use crate::server::ServerState;

// ========== Token Management ==========

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
    pub scopes: Option<String>,
}

#[derive(Serialize)]
pub struct CreateTokenResponse {
    pub id: i64,
    pub name: String,
    pub token: String, // Plaintext — shown only once
    pub scopes: String,
}

pub async fn create_token(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
    Json(body): Json<CreateTokenRequest>,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "admin") {
        return e;
    }

    if body.name.is_empty() || body.name.len() > 128 {
        return (StatusCode::BAD_REQUEST, "Name must be 1-128 characters").into_response();
    }

    let scopes = body.scopes.unwrap_or_else(|| "admin".to_string());
    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);

    let db_path = {
        let s = state.read().await;
        s.config.db_path.clone()
    };

    let name = body.name.clone();
    let scopes_clone = scopes.clone();
    let hash = token_hash.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        admin_token::create(&conn, &name, &hash, &scopes_clone)
    })
    .await;

    match result {
        Ok(Ok(id)) => Json(CreateTokenResponse {
            id,
            name: body.name,
            token: raw_token,
            scopes,
        })
        .into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to create token: {}", e);
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create token", "INTERNAL_ERROR")
        }
        Err(e) => {
            tracing::error!("Token creation task failed: {}", e);
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "Internal error", "INTERNAL_ERROR")
        }
    }
}

pub async fn list_tokens(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "admin") {
        return e;
    }

    let db_path = {
        let s = state.read().await;
        s.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        admin_token::list(&conn)
    })
    .await;

    match result {
        Ok(Ok(tokens)) => Json(tokens).into_response(),
        _ => error_json(StatusCode::INTERNAL_SERVER_ERROR, "Failed to list tokens", "INTERNAL_ERROR"),
    }
}

pub async fn delete_token(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
    Path(id): Path<i64>,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "admin") {
        return e;
    }

    let db_path = {
        let s = state.read().await;
        s.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        admin_token::delete(&conn, id)
    })
    .await;

    match result {
        Ok(Ok(true)) => (StatusCode::NO_CONTENT, "").into_response(),
        Ok(Ok(false)) => error_json(StatusCode::NOT_FOUND, "Token not found", "NOT_FOUND"),
        _ => error_json(StatusCode::INTERNAL_SERVER_ERROR, "Failed to delete token", "INTERNAL_ERROR"),
    }
}

// ========== Helpers ==========

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    code: String,
}

fn error_json(status: StatusCode, message: &str, code: &str) -> Response {
    (
        status,
        Json(ErrorBody {
            error: message.to_string(),
            code: code.to_string(),
        }),
    )
        .into_response()
}
```

**Step 2: Register in handlers/mod.rs**

Add `pub mod admin;` to `conary-server/src/server/handlers/mod.rs`.

**Step 3: Build**

Run: `cargo build --features server`
Expected: Compiles (CI handlers are stubs/unimplemented for now)

**Step 4: Commit**

```bash
git add conary-server/src/server/handlers/admin.rs conary-server/src/server/handlers/mod.rs
git commit -m "feat(server): add token management handlers (create, list, delete)"
```

---

### Task 5: CI Proxy Handlers

**Files:**
- Modify: `conary-server/src/server/handlers/admin.rs`

Add the CI proxy handlers to the existing admin.rs file.

**Step 1: Add CI proxy functions**

Append to `conary-server/src/server/handlers/admin.rs`:

```rust
// ========== CI Proxy (Forgejo) ==========

/// Proxy a GET request to Forgejo API
async fn forgejo_get(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
) -> Result<serde_json::Value, Response> {
    let (url, token, client) = {
        let s = state.read().await;
        let url = s.forgejo_url.as_ref().ok_or_else(|| {
            error_json(
                StatusCode::SERVICE_UNAVAILABLE,
                "Forgejo not configured",
                "UPSTREAM_ERROR",
            )
        })?;
        let token = s.forgejo_token.clone().unwrap_or_default();
        (format!("{}/api/v1{}", url.trim_end_matches('/'), path), token, s.http_client.clone())
    };

    let resp = client
        .get(&url)
        .header("Authorization", format!("token {token}"))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Forgejo proxy error: {}", e);
            error_json(StatusCode::BAD_GATEWAY, "Forgejo unreachable", "UPSTREAM_ERROR")
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!("Forgejo returned {}: {}", status, body);
        return Err(error_json(
            StatusCode::BAD_GATEWAY,
            &format!("Forgejo returned {status}"),
            "UPSTREAM_ERROR",
        ));
    }

    resp.json().await.map_err(|e| {
        tracing::error!("Forgejo response parse error: {}", e);
        error_json(StatusCode::BAD_GATEWAY, "Invalid Forgejo response", "UPSTREAM_ERROR")
    })
}

/// Proxy a POST request to Forgejo API
async fn forgejo_post(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, Response> {
    let (url, token, client) = {
        let s = state.read().await;
        let url = s.forgejo_url.as_ref().ok_or_else(|| {
            error_json(StatusCode::SERVICE_UNAVAILABLE, "Forgejo not configured", "UPSTREAM_ERROR")
        })?;
        let token = s.forgejo_token.clone().unwrap_or_default();
        (format!("{}/api/v1{}", url.trim_end_matches('/'), path), token, s.http_client.clone())
    };

    let resp = client
        .post(&url)
        .header("Authorization", format!("token {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Forgejo proxy error: {}", e);
            error_json(StatusCode::BAD_GATEWAY, "Forgejo unreachable", "UPSTREAM_ERROR")
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(error_json(
            StatusCode::BAD_GATEWAY,
            &format!("Forgejo returned {status}"),
            "UPSTREAM_ERROR",
        ));
    }

    resp.json().await.map_err(|e| {
        error_json(StatusCode::BAD_GATEWAY, "Invalid Forgejo response", "UPSTREAM_ERROR")
    })
}

pub async fn ci_list_workflows(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "ci:read") {
        return e;
    }
    match forgejo_get(&state, "/repos/peter/Conary/actions/workflows").await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

pub async fn ci_list_runs(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
    Path(name): Path<String>,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "ci:read") {
        return e;
    }
    let path = format!("/repos/peter/Conary/actions/workflows/{name}/runs");
    match forgejo_get(&state, &path).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

pub async fn ci_get_run(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
    Path(id): Path<i64>,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "ci:read") {
        return e;
    }
    let path = format!("/repos/peter/Conary/actions/runs/{id}");
    match forgejo_get(&state, &path).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

pub async fn ci_get_logs(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
    Path(id): Path<i64>,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "ci:read") {
        return e;
    }
    // Forgejo serves logs as plain text at this endpoint
    let (url, token, client) = {
        let s = state.read().await;
        let base = match s.forgejo_url.as_ref() {
            Some(u) => u.clone(),
            None => return error_json(StatusCode::SERVICE_UNAVAILABLE, "Forgejo not configured", "UPSTREAM_ERROR"),
        };
        let token = s.forgejo_token.clone().unwrap_or_default();
        (
            format!("{}/api/v1/repos/peter/Conary/actions/runs/{id}/logs", base.trim_end_matches('/')),
            token,
            s.http_client.clone(),
        )
    };

    let resp = client
        .get(&url)
        .header("Authorization", format!("token {token}"))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body = r.text().await.unwrap_or_default();
            (StatusCode::OK, [("content-type", "text/plain")], body).into_response()
        }
        Ok(r) => error_json(
            StatusCode::BAD_GATEWAY,
            &format!("Forgejo returned {}", r.status().as_u16()),
            "UPSTREAM_ERROR",
        ),
        Err(e) => {
            tracing::error!("Forgejo log fetch error: {}", e);
            error_json(StatusCode::BAD_GATEWAY, "Forgejo unreachable", "UPSTREAM_ERROR")
        }
    }
}

pub async fn ci_dispatch(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
    Path(name): Path<String>,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "ci:trigger") {
        return e;
    }
    let path = format!(
        "/repos/peter/Conary/actions/workflows/{name}/dispatches"
    );
    let body = serde_json::json!({"ref": "main"});
    match forgejo_post(&state, &path, body).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}

pub async fn ci_mirror_sync(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
) -> Response {
    if let Err(e) = require_scope(request.extensions(), "ci:trigger") {
        return e;
    }
    match forgejo_post(&state, "/repos/peter/Conary/mirror-sync", serde_json::json!({})).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => e,
    }
}
```

**Step 2: Build**

Run: `cargo build --features server`
Expected: Compiles

**Step 3: Commit**

```bash
git add conary-server/src/server/handlers/admin.rs
git commit -m "feat(server): add CI proxy handlers for Forgejo integration"
```

---

### Task 6: SSE Event Stream

**Files:**
- Modify: `conary-server/src/server/handlers/admin.rs`

**Step 1: Add SSE handler**

Append to `conary-server/src/server/handlers/admin.rs`:

```rust
// ========== SSE Event Stream ==========

use axum::response::sse::{Event, Sse};
use futures::stream::Stream;
use std::convert::Infallible;

#[derive(Deserialize)]
pub struct EventsQuery {
    pub filter: Option<String>,
}

pub async fn sse_events(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
    Query(query): Query<EventsQuery>,
) -> Response {
    // Any valid token can subscribe to events
    if request.extensions().get::<crate::server::auth::TokenScopes>().is_none() {
        return error_json(StatusCode::UNAUTHORIZED, "Not authenticated", "UNAUTHORIZED");
    }

    let filters: Option<Vec<String>> = query.filter.map(|f| {
        f.split(',').map(|s| s.trim().to_string()).collect()
    });

    let rx = {
        let s = state.read().await;
        s.admin_events.subscribe()
    };

    let stream = async_stream::stream! {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    // Apply filter if specified
                    if let Some(ref filters) = filters {
                        if !filters.iter().any(|f| f == &event.event_type) {
                            continue;
                        }
                    }
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok::<_, Infallible>(
                        Event::default()
                            .event(&event.event_type)
                            .data(data)
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("SSE client lagged by {} events", n);
                    yield Ok(Event::default().event("error").data(
                        format!("{{\"error\":\"Lagged by {n} events\",\"code\":\"LAGGED\"}}")
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(30))
                .text("ping"),
        )
        .into_response()
}
```

**Step 2: Add dependencies**

In `conary-server/Cargo.toml`, add:
- `async-stream` (for `async_stream::stream!` macro)
- `futures` (for `Stream` trait)

Check if already present first.

**Step 3: Build**

Run: `cargo build --features server`
Expected: Compiles

**Step 4: Commit**

```bash
git add conary-server/src/server/handlers/admin.rs conary-server/Cargo.toml
git commit -m "feat(server): add SSE event stream for admin API"
```

---

### Task 7: Event Publishing Integration

**Files:**
- Modify: `conary-server/src/server/handlers/packages.rs` (conversion events)
- Modify: `conary-server/src/server/handlers/chunks.rs` (cache events)

Add event publishing to existing operations so SSE clients get real-time updates.

**Step 1: Add helper to publish events**

In `conary-server/src/server/mod.rs`, add a convenience method:

```rust
impl ServerState {
    /// Publish an admin event to SSE subscribers
    pub fn publish_event(&self, event_type: &str, data: serde_json::Value) {
        let event = AdminEvent {
            event_type: event_type.to_string(),
            data,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        // Ignore error (no subscribers)
        let _ = self.admin_events.send(event);
    }
}
```

**Step 2: Add events to conversion handler**

In `conary-server/src/server/handlers/packages.rs` `trigger_conversion()`, after a successful conversion start:

```rust
state.publish_event("conversion", serde_json::json!({
    "action": "started",
    "distro": distro,
    "package": name,
}));
```

**Step 3: Add events to cache eviction**

In `conary-server/src/server/handlers/chunks.rs` `trigger_eviction()`, after eviction completes:

```rust
state.publish_event("cache", serde_json::json!({
    "action": "eviction_triggered",
}));
```

**Step 4: Build and test**

Run: `cargo build --features server`
Expected: Compiles

**Step 5: Commit**

```bash
git add conary-server/src/server/mod.rs conary-server/src/server/handlers/packages.rs conary-server/src/server/handlers/chunks.rs
git commit -m "feat(server): publish admin events from conversion and cache operations"
```

---

### Task 8: Integration Test — Admin API Smoke Test

**Files:**
- Create: `conary-server/src/server/handlers/admin_test.rs` (or in-file tests in admin.rs)

Since the admin API is heavily network-dependent (Forgejo proxy), we focus on testing:
1. Token CRUD through actual HTTP (axum test server)
2. Auth middleware rejection

**Step 1: Add integration-style tests to admin.rs**

Append to `conary-server/src/server/handlers/admin.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::Router;
    use tower::ServiceExt;

    /// Build a minimal test router with auth middleware and token endpoints
    async fn test_app() -> (Router, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        // Initialize DB with schema
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
            conary_core::db::schema::migrate(&conn).unwrap();
        }

        let mut config = crate::server::ServerConfig::default();
        config.db_path = db_path.clone();

        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));

        let app = Router::new()
            .route("/v1/admin/tokens", axum::routing::post(create_token))
            .route("/v1/admin/tokens", axum::routing::get(list_tokens))
            .route("/v1/admin/tokens/:id", axum::routing::delete(delete_token))
            .route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::server::auth::auth_middleware,
            ))
            .with_state(state);

        // Create a bootstrap token for tests
        let test_token = "test-admin-token-12345";
        let hash = crate::server::auth::hash_token(test_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            admin_token::create(&conn, "test-admin", &hash, "admin").unwrap();
        }

        (app, db_path)
    }

    #[tokio::test]
    async fn test_unauthenticated_request_rejected() {
        let (app, _db) = test_app().await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/tokens")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_invalid_token_rejected() {
        let (app, _db) = test_app().await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/tokens")
                    .header("Authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_valid_token_list_tokens() {
        let (app, _db) = test_app().await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/tokens")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_and_delete_token() {
        let (app, db_path) = test_app().await;

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/tokens")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"name":"new-token","scopes":"ci:read"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(created["name"], "new-token");
        assert_eq!(created["scopes"], "ci:read");
        assert!(!created["token"].as_str().unwrap().is_empty());

        // Delete
        let id = created["id"].as_i64().unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&format!("/v1/admin/tokens/{id}"))
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
}
```

**Step 2: Run tests**

Run: `cargo test --features server -p conary-server -- admin`
Expected: All 4 tests pass

**Step 3: Commit**

```bash
git add conary-server/src/server/handlers/admin.rs
git commit -m "test(server): add admin API integration tests for token CRUD and auth"
```

---

### Task 9: Clippy and Build Verification

**Files:**
- Various (fix any issues found)

**Step 1: Run clippy**

Run: `cargo clippy --features server -- -D warnings`
Expected: Clean (fix any warnings)

**Step 2: Run all tests**

Run: `cargo test --features server`
Expected: All tests pass

**Step 3: Run default build too**

Run: `cargo build && cargo test && cargo clippy -- -D warnings`
Expected: Clean (admin code is behind `--features server`)

**Step 4: Commit fixes if any**

```bash
git add -u
git commit -m "fix: resolve clippy warnings in admin API"
```

---

### Task 10: OpenAPI Spec Endpoint

**Files:**
- Create: `conary-server/src/server/handlers/openapi.rs`
- Modify: `conary-server/src/server/handlers/mod.rs`
- Modify: `conary-server/src/server/routes.rs`

Serve a hand-written OpenAPI 3.1 spec at `/v1/admin/openapi.json`. Hand-written rather than auto-generated because we want rich, LLM-friendly descriptions on every endpoint.

**Step 1: Create the OpenAPI spec handler**

Create `conary-server/src/server/handlers/openapi.rs`:

```rust
// conary-server/src/server/handlers/openapi.rs

//! OpenAPI 3.1 specification for the admin API

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Serve the OpenAPI spec as JSON
pub async fn openapi_spec() -> Response {
    let spec = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Remi Admin API",
            "description": "Administration API for the Remi package server. Manage CI/CD, repositories, federation peers, and admin tokens. Designed for both human and LLM agent consumption.",
            "version": env!("CARGO_PKG_VERSION"),
            "contact": { "name": "Conary Labs" }
        },
        "servers": [
            { "url": "https://packages.conary.io:8082", "description": "Production" }
        ],
        "security": [{ "bearerAuth": [] }],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Admin API token. Create via POST /v1/admin/tokens or REMI_ADMIN_TOKEN env var."
                }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string", "description": "Human-readable error message" },
                        "code": { "type": "string", "description": "Machine-readable error code (UNAUTHORIZED, INSUFFICIENT_SCOPE, NOT_FOUND, UPSTREAM_ERROR, INTERNAL_ERROR)" }
                    },
                    "required": ["error", "code"]
                },
                "Token": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "name": { "type": "string" },
                        "scopes": { "type": "string", "description": "Comma-separated scopes: admin, ci:read, ci:trigger, repos:read, repos:write, federation:read, federation:write" },
                        "created_at": { "type": "string", "format": "date-time" },
                        "last_used_at": { "type": ["string", "null"], "format": "date-time" }
                    }
                }
            }
        },
        "paths": {
            "/v1/admin/tokens": {
                "get": {
                    "operationId": "listTokens",
                    "summary": "List all admin API tokens",
                    "description": "Returns all tokens with their names, scopes, and last-used timestamps. Token hashes are never returned. Use this to audit which tokens exist and when they were last used.",
                    "tags": ["tokens"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": {
                        "200": { "description": "Array of tokens (without hashes)" },
                        "401": { "description": "Missing or invalid token" }
                    }
                },
                "post": {
                    "operationId": "createToken",
                    "summary": "Create a new admin API token",
                    "description": "Creates a new token and returns the plaintext value ONCE. Store it securely — it cannot be retrieved again. The 'admin' scope grants access to all endpoints.",
                    "tags": ["tokens"],
                    "security": [{ "bearerAuth": [] }],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["name"],
                                    "properties": {
                                        "name": { "type": "string", "description": "Human-readable label for this token (1-128 chars)" },
                                        "scopes": { "type": "string", "description": "Comma-separated scopes. Default: 'admin'. Options: admin, ci:read, ci:trigger, repos:read, repos:write, federation:read, federation:write" }
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": { "description": "Token created. The 'token' field contains the plaintext value — save it now." },
                        "401": { "description": "Missing or invalid token" }
                    }
                }
            },
            "/v1/admin/tokens/{id}": {
                "delete": {
                    "operationId": "deleteToken",
                    "summary": "Revoke an admin API token",
                    "description": "Permanently deletes a token. Any requests using this token will immediately fail with 401.",
                    "tags": ["tokens"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }],
                    "responses": {
                        "204": { "description": "Token deleted" },
                        "404": { "description": "Token not found" }
                    }
                }
            },
            "/v1/admin/ci/workflows": {
                "get": {
                    "operationId": "ciListWorkflows",
                    "summary": "List CI workflows",
                    "description": "Returns all CI/CD workflows from Forgejo. Use this to discover available workflow names before listing runs or triggering dispatches. Requires ci:read scope.",
                    "tags": ["ci"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": {
                        "200": { "description": "Workflow list from Forgejo" },
                        "502": { "description": "Forgejo unreachable or returned an error" }
                    }
                }
            },
            "/v1/admin/ci/workflows/{name}/runs": {
                "get": {
                    "operationId": "ciListRuns",
                    "summary": "List CI runs for a workflow",
                    "description": "Returns recent runs for a specific workflow. The 'name' is the workflow filename (e.g., 'ci.yaml'). Requires ci:read scope.",
                    "tags": ["ci"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Workflow filename (e.g., ci.yaml, integration.yaml, e2e.yaml)" }],
                    "responses": {
                        "200": { "description": "List of workflow runs with status, duration, and timestamps" },
                        "502": { "description": "Forgejo error" }
                    }
                }
            },
            "/v1/admin/ci/runs/{id}": {
                "get": {
                    "operationId": "ciGetRun",
                    "summary": "Get details for a specific CI run",
                    "description": "Returns full details for a run including job statuses. Use this after listing runs to inspect a specific one. Requires ci:read scope.",
                    "tags": ["ci"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }],
                    "responses": { "200": { "description": "Run details" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/runs/{id}/logs": {
                "get": {
                    "operationId": "ciGetLogs",
                    "summary": "Get logs for a CI run",
                    "description": "Returns raw log output as plain text. Useful for diagnosing build failures. Can be large — consider checking run status first. Requires ci:read scope.",
                    "tags": ["ci"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }],
                    "responses": { "200": { "description": "Plain text logs" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/workflows/{name}/dispatch": {
                "post": {
                    "operationId": "ciDispatch",
                    "summary": "Trigger a CI workflow run",
                    "description": "Dispatches a new run of the named workflow on the main branch. Use workflow names from ciListWorkflows (e.g., 'ci.yaml'). This is NOT idempotent — each call creates a new run. Requires ci:trigger scope.",
                    "tags": ["ci"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" } }],
                    "responses": { "200": { "description": "Workflow dispatched" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/mirror-sync": {
                "post": {
                    "operationId": "ciMirrorSync",
                    "summary": "Force GitHub mirror sync",
                    "description": "Forces Forgejo to sync its mirror of the GitHub repository immediately instead of waiting for the 10-minute poll interval. Useful after pushing to GitHub when you want CI to start quickly. Requires ci:trigger scope.",
                    "tags": ["ci"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "200": { "description": "Sync triggered" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/events": {
                "get": {
                    "operationId": "sseEvents",
                    "summary": "Subscribe to real-time admin events (SSE)",
                    "description": "Server-Sent Events stream of admin events. Filter by type with ?filter=ci,repo,federation,cache,conversion. Any valid token can subscribe. Connection stays open — events arrive as they happen.",
                    "tags": ["events"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{
                        "name": "filter",
                        "in": "query",
                        "required": false,
                        "schema": { "type": "string" },
                        "description": "Comma-separated event types to receive (ci, repo, federation, cache, conversion). Omit for all."
                    }],
                    "responses": { "200": { "description": "SSE event stream" } }
                }
            }
        }
    });

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string_pretty(&spec).unwrap_or_default(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_openapi_spec_is_valid_json() {
        let resp = openapi_spec().await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

**Step 2: Register the module and route**

In `conary-server/src/server/handlers/mod.rs`, add `pub mod openapi;`.

In `conary-server/src/server/routes.rs` `create_external_admin_router()`, add before the auth middleware layer:
```rust
// OpenAPI spec (no auth required — it's the discovery endpoint)
.route("/v1/admin/openapi.json", get(openapi::openapi_spec))
```

Note: This route must be added OUTSIDE the auth middleware layer so it's accessible without a token. The spec itself is not sensitive — it just describes what endpoints exist.

**Step 3: Build and test**

Run: `cargo test --features server -p conary-server -- openapi`
Expected: Test passes

**Step 4: Commit**

```bash
git add conary-server/src/server/handlers/openapi.rs conary-server/src/server/handlers/mod.rs conary-server/src/server/routes.rs
git commit -m "feat(server): add OpenAPI 3.1 spec endpoint with LLM-friendly descriptions"
```

---

### Task 11: MCP Server Endpoint

**Files:**
- Create: `conary-server/src/server/mcp.rs`
- Modify: `conary-server/src/server/mod.rs`
- Modify: `conary-server/src/server/routes.rs`
- Modify: `conary-server/Cargo.toml`

This is the MCP (Model Context Protocol) layer that wraps admin API operations as MCP tools. LLM agents (Claude Code, etc.) connect to this endpoint and discover/invoke tools directly.

**Step 1: Add rmcp dependency**

In `conary-server/Cargo.toml`, add:

```toml
rmcp = { version = "0.16", features = ["server", "transport-streamable-http"] }
schemars = "0.8"
```

Check current rmcp version on crates.io — use latest stable.

**Step 2: Create the MCP server module**

Create `conary-server/src/server/mcp.rs`:

```rust
// conary-server/src/server/mcp.rs

//! MCP (Model Context Protocol) server for LLM agent integration
//!
//! Exposes admin API operations as MCP tools that LLM agents can
//! discover and invoke. Uses the rmcp crate (official Rust MCP SDK).
//! All tools delegate to the same internal functions as the REST handlers.

use rmcp::{ServerHandler, model::*, tool, Error as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::ServerState;
use crate::server::auth;

/// MCP server instance holding shared state and an auth token
#[derive(Clone)]
pub struct RemiMcpServer {
    state: Arc<RwLock<ServerState>>,
    /// The bearer token for this MCP session (validated on connection)
    token_scopes: auth::TokenScopes,
}

impl RemiMcpServer {
    pub fn new(state: Arc<RwLock<ServerState>>, token_scopes: auth::TokenScopes) -> Self {
        Self { state, token_scopes }
    }

    fn check_scope(&self, scope: &str) -> Result<(), McpError> {
        if self.token_scopes.has_scope(scope) {
            Ok(())
        } else {
            Err(McpError::invalid_request(
                format!("Insufficient scope: requires '{scope}'"),
                None,
            ))
        }
    }

    /// Proxy GET to Forgejo and return the JSON as a string
    async fn forgejo_get_text(&self, path: &str) -> Result<String, McpError> {
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
            .map_err(|e| McpError::internal_error(format!("Response read error: {e}"), None))
    }
}

#[tool(tool_box)]
impl RemiMcpServer {
    /// List all admin API tokens. Shows names, scopes, and last-used timestamps.
    /// Use this to audit which tokens exist. Requires 'admin' scope.
    #[tool(name = "list_tokens")]
    async fn list_tokens(&self) -> Result<CallToolResult, McpError> {
        self.check_scope("admin")?;

        let db_path = {
            let s = self.state.read().await;
            s.config.db_path.clone()
        };

        let tokens = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path).map_err(|e| e.to_string())?;
            conary_core::db::models::admin_token::list(&conn).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e, None))?;

        let json = serde_json::to_string_pretty(&tokens)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// List all CI/CD workflows. Returns workflow names and file paths.
    /// Use the workflow name (e.g., 'ci.yaml') with other CI tools.
    /// Requires 'ci:read' scope.
    #[tool(name = "ci_list_workflows")]
    async fn ci_list_workflows(&self) -> Result<CallToolResult, McpError> {
        self.check_scope("ci:read")?;
        let text = self.forgejo_get_text("/repos/peter/Conary/actions/workflows").await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// List recent CI runs for a specific workflow.
    /// Shows run status, duration, and timestamps.
    /// Requires 'ci:read' scope.
    ///
    /// # Arguments
    /// * `workflow` - Workflow filename (e.g., "ci.yaml", "integration.yaml", "e2e.yaml")
    #[tool(name = "ci_list_runs")]
    async fn ci_list_runs(
        &self,
        #[tool(param, description = "Workflow filename (e.g., ci.yaml, integration.yaml, e2e.yaml)")]
        workflow: String,
    ) -> Result<CallToolResult, McpError> {
        self.check_scope("ci:read")?;
        let path = format!("/repos/peter/Conary/actions/workflows/{workflow}/runs");
        let text = self.forgejo_get_text(&path).await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get detailed information about a specific CI run, including job statuses.
    /// Use after ci_list_runs to inspect a particular run.
    /// Requires 'ci:read' scope.
    #[tool(name = "ci_get_run")]
    async fn ci_get_run(
        &self,
        #[tool(param, description = "Run ID (integer) from ci_list_runs results")]
        run_id: i64,
    ) -> Result<CallToolResult, McpError> {
        self.check_scope("ci:read")?;
        let path = format!("/repos/peter/Conary/actions/runs/{run_id}");
        let text = self.forgejo_get_text(&path).await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Get raw log output for a CI run. Useful for diagnosing build failures.
    /// Can be large — check run status first to confirm it's the run you want.
    /// Requires 'ci:read' scope.
    #[tool(name = "ci_get_logs")]
    async fn ci_get_logs(
        &self,
        #[tool(param, description = "Run ID (integer) to fetch logs for")]
        run_id: i64,
    ) -> Result<CallToolResult, McpError> {
        self.check_scope("ci:read")?;
        let path = format!("/repos/peter/Conary/actions/runs/{run_id}/logs");
        let text = self.forgejo_get_text(&path).await?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Trigger a new CI workflow run on the main branch.
    /// NOT idempotent — each call creates a new run.
    /// Requires 'ci:trigger' scope.
    #[tool(name = "ci_dispatch")]
    async fn ci_dispatch(
        &self,
        #[tool(param, description = "Workflow filename to dispatch (e.g., ci.yaml)")]
        workflow: String,
    ) -> Result<CallToolResult, McpError> {
        self.check_scope("ci:trigger")?;

        let (url, token, client) = {
            let s = self.state.read().await;
            let base = s.forgejo_url.as_ref().ok_or_else(|| {
                McpError::internal_error("Forgejo not configured", None)
            })?;
            let token = s.forgejo_token.clone().unwrap_or_default();
            (
                format!(
                    "{}/api/v1/repos/peter/Conary/actions/workflows/{workflow}/dispatches",
                    base.trim_end_matches('/')
                ),
                token,
                s.http_client.clone(),
            )
        };

        let resp = client
            .post(&url)
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({"ref": "main"}))
            .send()
            .await
            .map_err(|e| McpError::internal_error(format!("Forgejo unreachable: {e}"), None))?;

        if resp.status().is_success() {
            Ok(CallToolResult::success(vec![Content::text(
                format!("Workflow '{workflow}' dispatched successfully on main branch"),
            )]))
        } else {
            Err(McpError::internal_error(
                format!("Forgejo returned {}", resp.status()),
                None,
            ))
        }
    }

    /// Force Forgejo to sync its GitHub mirror immediately.
    /// Normally the mirror polls every 10 minutes — this triggers an immediate sync.
    /// Useful after pushing to GitHub when you want CI to start quickly.
    /// Requires 'ci:trigger' scope.
    #[tool(name = "ci_mirror_sync")]
    async fn ci_mirror_sync(&self) -> Result<CallToolResult, McpError> {
        self.check_scope("ci:trigger")?;

        let (url, token, client) = {
            let s = self.state.read().await;
            let base = s.forgejo_url.as_ref().ok_or_else(|| {
                McpError::internal_error("Forgejo not configured", None)
            })?;
            let token = s.forgejo_token.clone().unwrap_or_default();
            (
                format!("{}/api/v1/repos/peter/Conary/mirror-sync", base.trim_end_matches('/')),
                token,
                s.http_client.clone(),
            )
        };

        let resp = client
            .post(&url)
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| McpError::internal_error(format!("Forgejo unreachable: {e}"), None))?;

        if resp.status().is_success() {
            Ok(CallToolResult::success(vec![Content::text("Mirror sync triggered")]))
        } else {
            Err(McpError::internal_error(
                format!("Forgejo returned {}", resp.status()),
                None,
            ))
        }
    }
}

/// Implement the ServerHandler trait for MCP protocol negotiation
#[tool(tool_box)]
impl ServerHandler for RemiMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Remi Admin API — manage CI/CD pipelines, admin tokens, and server operations \
                 for the Conary package manager's Remi server. Available workflows: ci.yaml \
                 (build+test+clippy), integration.yaml (3-distro Podman matrix), e2e.yaml \
                 (deep end-to-end), remi-health.yaml (endpoint verification)."
                    .to_string(),
            ),
        }
    }
}
```

**Step 3: Wire up the MCP endpoint**

The MCP endpoint needs to authenticate the bearer token from the HTTP request, then create an `RemiMcpServer` instance with the validated scopes.

In `conary-server/src/server/routes.rs`, add the MCP route to `create_external_admin_router()`. The `rmcp` crate provides an axum integration for streamable HTTP transport:

```rust
// MCP endpoint (streamable HTTP transport)
.route("/mcp", any(mcp_handler))
```

The `mcp_handler` function:
```rust
async fn mcp_handler(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: axum::extract::Request,
) -> Response {
    // Validate bearer token
    let raw_token = match crate::server::auth::extract_bearer_from_headers(request.headers()) {
        Some(t) => t.to_string(),
        None => return (StatusCode::UNAUTHORIZED, "Bearer token required").into_response(),
    };

    let token_hash = crate::server::auth::hash_token(&raw_token);
    let db_path = state.read().await.config.db_path.clone();

    let scopes = match tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::admin_token::find_by_hash(&conn, &token_hash)
    }).await {
        Ok(Ok(Some(token))) => crate::server::auth::TokenScopes(token.scopes),
        _ => return (StatusCode::UNAUTHORIZED, "Invalid token").into_response(),
    };

    let mcp_server = crate::server::mcp::RemiMcpServer::new(state, scopes);

    // Delegate to rmcp's streamable HTTP handler
    rmcp::transport::streamable_http::axum_handler(mcp_server, request)
        .await
        .into_response()
}
```

Note: The exact rmcp axum integration API may differ — check `rmcp` docs at build time. The `#[tool]` macro from rmcp auto-generates the tool listing and JSON Schema for each tool's parameters.

**Step 4: Register the module**

In `conary-server/src/server/mod.rs`, add `pub mod mcp;`.

**Step 5: Build**

Run: `cargo build --features server`
Expected: Compiles. The rmcp `#[tool]` macro generates tool schemas at compile time.

**Step 6: Test manually**

After deploying, verify MCP discovery works:
```bash
# From local machine
claude mcp add remi-admin --transport http --url https://packages.conary.io:8082/mcp --header "Authorization: Bearer <token>"

# Then in Claude Code:
# "List the CI workflows"  →  calls ci_list_workflows tool
# "What's the latest CI run for ci.yaml?"  →  calls ci_list_runs tool
```

**Step 7: Commit**

```bash
git add conary-server/src/server/mcp.rs conary-server/src/server/mod.rs conary-server/src/server/routes.rs conary-server/Cargo.toml
git commit -m "feat(server): add MCP endpoint for LLM agent integration via rmcp"
```

---

### Task 12: Documentation Updates

**Files:**
- Modify: `CLAUDE.md` — bump schema version 46 → 47
- Modify: `.claude/rules/architecture.md` — add auth, admin, openapi, mcp modules
- Modify: `.claude/rules/server.md` — add auth module, admin handlers, MCP
- Modify: `.claude/rules/db.md` — bump schema version
- Modify: `.claude/rules/infrastructure.md` — add :8082 port, admin API + MCP info

**Step 1: Update CLAUDE.md**

Change `v46` to `v47` in the schema version line.

**Step 2: Update architecture.md**

Add to the conary-server table:
```
| `src/server/auth.rs` | Bearer token auth middleware |
| `src/server/handlers/admin.rs` | External admin API (tokens, CI proxy, SSE) |
| `src/server/handlers/openapi.rs` | OpenAPI 3.1 spec endpoint |
| `src/server/mcp.rs` | MCP server for LLM agent integration |
```

**Step 3: Update server.md**

Add to the Remi Server Key Types section:
```
- `AdminSection` -- external admin API config (bind, Forgejo URL/token)
- `TokenScopes` -- scope checking for auth middleware
- `AdminEvent` -- SSE event for admin subscribers
- `RemiMcpServer` -- MCP server exposing admin tools to LLM agents
```

Add to the Files section:
```
- `auth.rs` -- bearer token auth middleware, token hashing, scope checking
- `handlers/admin.rs` -- external admin API handlers (token CRUD, CI proxy, SSE)
- `handlers/openapi.rs` -- OpenAPI 3.1 specification endpoint
- `mcp.rs` -- MCP (Model Context Protocol) server for LLM agent tool integration
```

**Step 4: Update db.md**

Change schema version from v46 to v47.

**Step 5: Update infrastructure.md**

Add to the Remi section:
```
- **Admin API:** `:8082` (external, token-authenticated) — requires `[admin] enabled = true` in config
- **MCP endpoint:** `:8082/mcp` (Streamable HTTP transport) — connect with `claude mcp add`
- **OpenAPI spec:** `:8082/v1/admin/openapi.json` (no auth required)
```

**Step 6: Commit**

```bash
git add CLAUDE.md .claude/rules/architecture.md .claude/rules/server.md .claude/rules/db.md .claude/rules/infrastructure.md
git commit -m "docs: update rules and docs for admin API, MCP, and OpenAPI (schema v47)"
```

---

## Dependency Graph

```
Task 1 (DB migration) ──┐
                         ├── Task 3 (Config + Router) ──┐
Task 2 (Auth module) ────┘                              ├── Task 4 (Token handlers)
                                                        ├── Task 5 (CI handlers)
                                                        ├── Task 6 (SSE handler)
                                                        ├── Task 7 (Event publishing)
                                                        └── Task 10 (OpenAPI spec)
                                                                    │
                                                        Task 8 (Tests) ← depends on Tasks 4-6
                                                        Task 11 (MCP) ← depends on Tasks 4-7
                                                        Task 9 (Clippy) ← depends on all
                                                        Task 12 (Docs) ← depends on all
```

Tasks 1 and 2 are independent and can run in parallel.
Tasks 4, 5, 6, 7, 10 depend on Task 3 but are independent of each other.
Task 11 (MCP) depends on the REST handlers being done.
Tasks 8, 9, 12 are sequential cleanup.
