# Admin API Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate duplication between admin handlers and MCP tools, extract shared infrastructure, and reduce per-request overhead.

**Architecture:** Eight refactoring tasks ordered by dependency. Foundation tasks (db::open_fast, Scope enum, federation_peer model, Forgejo client, rate limiter extraction) have no dependencies and can run in any order. Then split admin.rs into domain files, extract a shared service layer, and wire MCP to call the service layer instead of reimplementing business logic.

**Tech Stack:** Rust 1.93, axum, governor, rmcp, rusqlite, tokio

---

## Task Dependency Graph

```
Independent (any order):
  Task 1: db::open_fast()
  Task 2: Scope enum
  Task 3: Federation peer model
  Task 4: Forgejo client module
  Task 5: Rate limiters out of RwLock + Governor cleanup

Sequential:
  Task 6: Split admin.rs into domain files
  Task 7: Extract admin service layer (depends on 3, 4, 6)
  Task 8: Wire MCP to service layer (depends on 7)
```

---

### Task 1: Add `db::open_fast()` — Skip Migrations on Hot Path

**Files:**
- Modify: `conary-core/src/db/mod.rs:72-94`
- Test: existing tests in `conary-core/src/db/mod.rs` (bottom of file)

**Context:** Every `db::open()` call runs `schema::migrate()`, which checks `PRAGMA user_version` and logs at info level. The admin API calls `db::open()` 3-4 times per request (auth middleware, handler, audit middleware, background touch). The schema is guaranteed correct after server startup.

**Step 1: Add `open_fast` function**

In `conary-core/src/db/mod.rs`, after the existing `open()` function (line 94), add:

```rust
/// Open an existing database WITHOUT running migrations.
///
/// Use this on hot paths (e.g., per-request in a server) where the schema
/// is known-good from a prior `open()` or `init()` call. Sets the same
/// PRAGMAs as `open()` but skips the `schema::migrate()` check.
pub fn open_fast(path: impl AsRef<Path>) -> Result<Connection> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(Error::DatabaseNotFound(path.to_string_lossy().to_string()));
    }

    let conn = Connection::open(path)?;

    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 5000;
        ",
    )?;

    Ok(conn)
}
```

**Step 2: Write a test**

In the `#[cfg(test)]` block of `conary-core/src/db/mod.rs`, add:

```rust
#[test]
fn test_open_fast_skips_migration() {
    // Create a temp DB with init (runs migrations)
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    init(&db_path).unwrap();

    // open_fast should succeed and return a valid connection
    let conn = open_fast(&db_path).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, schema::SCHEMA_VERSION as i32);
}
```

**Step 3: Run tests**

Run: `cargo test -p conary-core -- open_fast`
Expected: PASS

**Step 4: Replace `db::open` with `db::open_fast` in server hot paths**

In these files, change `conary_core::db::open(` to `conary_core::db::open_fast(`:

- `conary-server/src/server/auth.rs` — token lookup (line ~111) and background touch (line ~152)
- `conary-server/src/server/audit.rs` — fire-and-forget audit write (line ~147)
- `conary-server/src/server/handlers/admin.rs` — ALL `spawn_blocking` blocks (28+ occurrences)
- `conary-server/src/server/mcp.rs` — ALL `spawn_blocking` blocks (10 occurrences)

Keep `db::open()` (with migrations) in `run_server_from_config()` startup path — that's the one place where migration checking is correct.

**Step 5: Run full test suite**

Run: `cargo test --features server`
Expected: All pass

**Step 6: Commit**

```bash
git add conary-core/src/db/mod.rs conary-server/src/server/auth.rs conary-server/src/server/audit.rs conary-server/src/server/handlers/admin.rs conary-server/src/server/mcp.rs
git commit -m "perf(db): add open_fast() to skip migrations on server hot paths"
```

---

### Task 2: Replace String Scopes with Scope Enum

**Files:**
- Modify: `conary-server/src/server/auth.rs:23-65`
- Modify: `conary-server/src/server/handlers/admin.rs` (all `check_scope` calls)
- Modify: `conary-server/src/server/mcp.rs` (scope validation in create_token)
- Test: `conary-server/src/server/auth.rs` (existing test block)

**Context:** Scope names are string literals scattered across files. `VALID_SCOPES` already exists as a `&[&str]` — replace with a proper enum to get compile-time safety.

**Step 1: Define the Scope enum in auth.rs**

Replace the `VALID_SCOPES` const and `validate_scopes` function (lines 44-65) with:

```rust
/// Token scope for the admin API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Admin,
    CiRead,
    CiTrigger,
    ReposRead,
    ReposWrite,
    FederationRead,
    FederationWrite,
}

impl Scope {
    /// All valid scopes.
    pub const ALL: &[Scope] = &[
        Scope::Admin,
        Scope::CiRead,
        Scope::CiTrigger,
        Scope::ReposRead,
        Scope::ReposWrite,
        Scope::FederationRead,
        Scope::FederationWrite,
    ];

    /// String representation used in DB and API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Admin => "admin",
            Scope::CiRead => "ci:read",
            Scope::CiTrigger => "ci:trigger",
            Scope::ReposRead => "repos:read",
            Scope::ReposWrite => "repos:write",
            Scope::FederationRead => "federation:read",
            Scope::FederationWrite => "federation:write",
        }
    }

    /// Parse a scope string. Returns None for unknown scopes.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "admin" => Some(Scope::Admin),
            "ci:read" => Some(Scope::CiRead),
            "ci:trigger" => Some(Scope::CiTrigger),
            "repos:read" => Some(Scope::ReposRead),
            "repos:write" => Some(Scope::ReposWrite),
            "federation:read" => Some(Scope::FederationRead),
            "federation:write" => Some(Scope::FederationWrite),
            _ => None,
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validate that all scopes in a comma-separated string are valid.
/// Returns Err with the first invalid scope found.
pub fn validate_scopes(scopes: &str) -> Result<(), String> {
    for scope in scopes.split(',') {
        if Scope::parse(scope).is_none() {
            return Err(scope.trim().to_string());
        }
    }
    Ok(())
}
```

**Step 2: Update `has_scope` to accept `Scope` enum**

Replace `has_scope` (lines 35-41):

```rust
impl TokenScopes {
    /// Check if this token has the required scope.
    ///
    /// The Admin scope grants access to everything.
    pub fn has_scope(&self, required: Scope) -> bool {
        let required_str = required.as_str();
        self.0.split(',').any(|s| {
            let t = s.trim();
            t == "admin" || t == required_str
        })
    }
}
```

**Step 3: Update all `check_scope` / `has_scope` call sites in admin.rs**

Find every `has_scope("...")` call and replace the string with the enum variant:
- `has_scope("admin")` → `has_scope(Scope::Admin)`
- `has_scope("ci:read")` → `has_scope(Scope::CiRead)`
- `has_scope("ci:trigger")` → `has_scope(Scope::CiTrigger)`
- `has_scope("repos:read")` → `has_scope(Scope::ReposRead)`
- `has_scope("repos:write")` → `has_scope(Scope::ReposWrite)`
- `has_scope("federation:read")` → `has_scope(Scope::FederationRead)`
- `has_scope("federation:write")` → `has_scope(Scope::FederationWrite)`

Add `use crate::server::auth::Scope;` at the top of `admin.rs`.

**Step 4: Remove `VALID_SCOPES` const** (it's replaced by `Scope::ALL`)

**Step 5: Update tests in auth.rs**

Replace the string-based test calls with enum variants. Update `test_validate_scopes_valid` and `test_validate_scopes_invalid` to still work (they use string input which is still correct).

**Step 6: Run tests and clippy**

Run: `cargo test --features server` and `cargo clippy --features server -- -D warnings`
Expected: All pass

**Step 7: Commit**

```bash
git add conary-server/src/server/auth.rs conary-server/src/server/handlers/admin.rs conary-server/src/server/mcp.rs
git commit -m "refactor(server): replace string scopes with Scope enum"
```

---

### Task 3: Create Federation Peer DB Model

**Files:**
- Create: `conary-core/src/db/models/federation_peer.rs`
- Modify: `conary-core/src/db/models/mod.rs` — add `pub mod federation_peer;`
- Test: inline in `federation_peer.rs`

**Context:** Raw SQL for `federation_peers` table is duplicated in `admin.rs` (lines 1062, 1150, 1223, 1276) and `mcp.rs` (lines 548, 631, 670). Extract into a model following the `admin_token.rs` and `audit_log.rs` patterns.

**Step 1: Create the model file**

Create `conary-core/src/db/models/federation_peer.rs`:

```rust
// conary-core/src/db/models/federation_peer.rs

//! Federation peer model - manages peer entries in the federation_peers table

use crate::error::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

/// A federation peer entry.
#[derive(Debug, Clone, Serialize)]
pub struct FederationPeer {
    pub id: String,
    pub name: String,
    pub url: String,
    pub tier: String,
    pub is_enabled: bool,
    pub region: Option<String>,
    pub cell: Option<String>,
    pub public_key: Option<String>,
    pub last_seen: Option<String>,
    pub added_at: String,
    pub notes: Option<String>,
}

/// List all federation peers, ordered by name.
pub fn list(conn: &Connection) -> Result<Vec<FederationPeer>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, url, tier, is_enabled, region, cell, public_key, \
         last_seen, added_at, notes \
         FROM federation_peers ORDER BY name",
    )?;
    let peers = stmt
        .query_map([], |row| {
            Ok(FederationPeer {
                id: row.get(0)?,
                name: row.get(1)?,
                url: row.get(2)?,
                tier: row.get(3)?,
                is_enabled: row.get(4)?,
                region: row.get(5)?,
                cell: row.get(6)?,
                public_key: row.get(7)?,
                last_seen: row.get(8)?,
                added_at: row.get(9)?,
                notes: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(peers)
}

/// Find a federation peer by ID.
pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<FederationPeer>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, url, tier, is_enabled, region, cell, public_key, \
         last_seen, added_at, notes \
         FROM federation_peers WHERE id = ?1",
    )?;
    let peer = stmt
        .query_row([id], |row| {
            Ok(FederationPeer {
                id: row.get(0)?,
                name: row.get(1)?,
                url: row.get(2)?,
                tier: row.get(3)?,
                is_enabled: row.get(4)?,
                region: row.get(5)?,
                cell: row.get(6)?,
                public_key: row.get(7)?,
                last_seen: row.get(8)?,
                added_at: row.get(9)?,
                notes: row.get(10)?,
            })
        })
        .optional()?;
    Ok(peer)
}

/// Insert a new federation peer.
#[allow(clippy::too_many_arguments)]
pub fn insert(
    conn: &Connection,
    id: &str,
    name: &str,
    url: &str,
    tier: &str,
    region: Option<&str>,
    cell: Option<&str>,
    public_key: Option<&str>,
    notes: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO federation_peers (id, name, url, tier, region, cell, public_key, notes) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, name, url, tier, region, cell, public_key, notes],
    )?;
    Ok(())
}

/// Delete a federation peer by ID. Returns true if a row was deleted.
pub fn delete(conn: &Connection, id: &str) -> Result<bool> {
    let deleted = conn.execute("DELETE FROM federation_peers WHERE id = ?1", [id])?;
    Ok(deleted > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;
    use rusqlite::OptionalExtension;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_list() {
        let conn = test_conn();
        insert(&conn, "peer-1", "node-a", "http://a:8080", "leaf", None, None, None, None).unwrap();
        insert(&conn, "peer-2", "node-b", "http://b:8080", "cell_hub", Some("us-east"), None, None, None).unwrap();

        let peers = list(&conn).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].name, "node-a"); // ordered by name
        assert_eq!(peers[1].tier, "cell_hub");
        assert_eq!(peers[1].region, Some("us-east".to_string()));
    }

    #[test]
    fn test_find_by_id() {
        let conn = test_conn();
        insert(&conn, "peer-1", "node-a", "http://a:8080", "leaf", None, None, None, None).unwrap();

        let found = find_by_id(&conn, "peer-1").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "node-a");

        let not_found = find_by_id(&conn, "nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_delete() {
        let conn = test_conn();
        insert(&conn, "peer-1", "node-a", "http://a:8080", "leaf", None, None, None, None).unwrap();

        assert!(delete(&conn, "peer-1").unwrap());
        assert!(!delete(&conn, "peer-1").unwrap()); // already deleted

        let peers = list(&conn).unwrap();
        assert!(peers.is_empty());
    }
}
```

**Step 2: Register in mod.rs**

In `conary-core/src/db/models/mod.rs`, add `pub mod federation_peer;` after `pub mod audit_log;`.

**Step 3: Run tests**

Run: `cargo test -p conary-core federation_peer`
Expected: 3 tests pass

**Step 4: Commit**

```bash
git add conary-core/src/db/models/federation_peer.rs conary-core/src/db/models/mod.rs
git commit -m "feat(db): add federation_peer model to replace raw SQL in handlers"
```

---

### Task 4: Extract Forgejo Client Module

**Files:**
- Create: `conary-server/src/server/forgejo.rs`
- Modify: `conary-server/src/server/mod.rs` — add `pub mod forgejo;`
- Modify: `conary-server/src/server/handlers/admin.rs` — remove forgejo_get/forgejo_post, import from forgejo module
- Modify: `conary-server/src/server/mcp.rs` — remove forgejo_get/forgejo_post, import from forgejo module
- Test: inline in `forgejo.rs`

**Context:** `forgejo_get()` and `forgejo_post()` are duplicated in both `admin.rs` (lines 240-362) and `mcp.rs` (lines 76-151). Additionally, `ci_get_logs()` in `admin.rs` (line 422) has an inlined variant for text responses.

**Step 1: Create `conary-server/src/server/forgejo.rs`**

```rust
// conary-server/src/server/forgejo.rs

//! Forgejo API client for CI proxy operations.
//!
//! Shared between admin HTTP handlers and MCP tools.

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;

/// Error from a Forgejo API call.
#[derive(Debug)]
pub struct ForgejoError {
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for ForgejoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Extract Forgejo config from server state.
///
/// Returns (base_url, token, http_client) or error if not configured.
async fn get_config(
    state: &Arc<RwLock<ServerState>>,
) -> Result<(String, String, reqwest::Client), ForgejoError> {
    let s = state.read().await;
    let url = s.forgejo_url.clone().ok_or_else(|| ForgejoError {
        status: None,
        message: "Forgejo URL not configured".to_string(),
    })?;
    let token = s.forgejo_token.clone().ok_or_else(|| ForgejoError {
        status: None,
        message: "Forgejo token not configured".to_string(),
    })?;
    let client = s.http_client.clone();
    Ok((url, token, client))
}

/// Build the full Forgejo API URL from a path.
fn api_url(base: &str, path: &str) -> String {
    format!("{}/api/v1{}", base.trim_end_matches('/'), path)
}

/// GET a Forgejo API path, returning the JSON response text.
pub async fn get(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
) -> Result<String, ForgejoError> {
    let (base, token, client) = get_config(state).await?;
    let url = api_url(&base, path);

    let resp = client
        .get(&url)
        .header("Authorization", format!("token {token}"))
        .send()
        .await
        .map_err(|e| ForgejoError {
            status: None,
            message: format!("Forgejo request failed: {e}"),
        })?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(ForgejoError {
            status: Some(status),
            message: format!("Forgejo returned {status}"),
        });
    }

    resp.text().await.map_err(|e| ForgejoError {
        status: Some(status),
        message: format!("Failed to read Forgejo response: {e}"),
    })
}

/// GET a Forgejo API path, returning plain text (for logs).
pub async fn get_text(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
) -> Result<String, ForgejoError> {
    // Same as get() — Forgejo returns text for log endpoints
    get(state, path).await
}

/// POST to a Forgejo API path with an optional JSON body.
///
/// Returns the response text, or empty string for 204 No Content.
pub async fn post(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<String, ForgejoError> {
    let (base, token, client) = get_config(state).await?;
    let url = api_url(&base, path);

    let mut req = client
        .post(&url)
        .header("Authorization", format!("token {token}"));

    if let Some(json) = body {
        req = req.json(json);
    }

    let resp = req.send().await.map_err(|e| ForgejoError {
        status: None,
        message: format!("Forgejo request failed: {e}"),
    })?;

    let status = resp.status().as_u16();

    // 204 No Content is success with no body
    if status == 204 {
        return Ok(String::new());
    }

    if !resp.status().is_success() {
        return Err(ForgejoError {
            status: Some(status),
            message: format!("Forgejo returned {status}"),
        });
    }

    resp.text().await.map_err(|e| ForgejoError {
        status: Some(status),
        message: format!("Failed to read Forgejo response: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_url() {
        assert_eq!(
            api_url("http://forge.example.com:3000", "/repos/peter/Conary/actions/workflows"),
            "http://forge.example.com:3000/api/v1/repos/peter/Conary/actions/workflows"
        );
        // Trailing slash stripped
        assert_eq!(
            api_url("http://forge.example.com:3000/", "/repos/foo"),
            "http://forge.example.com:3000/api/v1/repos/foo"
        );
    }
}
```

**Step 2: Register in mod.rs**

Add `pub mod forgejo;` in `conary-server/src/server/mod.rs`.

**Step 3: Replace in admin.rs**

Delete the `forgejo_get` and `forgejo_post` helper functions from `admin.rs` (lines ~240-362). In each CI handler, replace calls:
- `forgejo_get(&state, "/repos/peter/Conary/...").await` → `crate::server::forgejo::get(&state, "/repos/peter/Conary/...").await`
- `forgejo_post(&state, "/repos/peter/Conary/...", body).await` → `crate::server::forgejo::post(&state, "/repos/peter/Conary/...", body).await`

Map errors: `ForgejoError` → axum Response using status code if available, falling back to 502.

For `ci_get_logs` specifically, replace the inlined proxy logic with `crate::server::forgejo::get_text(&state, &path).await`.

**Step 4: Replace in mcp.rs**

Delete the `forgejo_get` and `forgejo_post` methods from `impl RemiMcpServer` (lines ~74-151). Replace calls:
- `self.forgejo_get(path).await` → `crate::server::forgejo::get(&self.state, path).await`
- `self.forgejo_post(path, body).await` → `crate::server::forgejo::post(&self.state, path, body).await`

Map errors: `ForgejoError` → `McpError` using `McpError::internal_error`.

**Step 5: Run tests and clippy**

Run: `cargo test --features server` and `cargo clippy --features server -- -D warnings`
Expected: All pass

**Step 6: Commit**

```bash
git add conary-server/src/server/forgejo.rs conary-server/src/server/mod.rs conary-server/src/server/handlers/admin.rs conary-server/src/server/mcp.rs
git commit -m "refactor(server): extract Forgejo client into shared module"
```

---

### Task 5: Move Rate Limiters Out of RwLock + Governor Cleanup

**Files:**
- Modify: `conary-server/src/server/mod.rs` — move `rate_limiters` out of `ServerState`
- Modify: `conary-server/src/server/routes.rs` — pass limiters as separate state layer
- Modify: `conary-server/src/server/rate_limit.rs` — update middleware to extract from own State
- Modify: `conary-server/src/server/auth.rs` — extract limiters from new location

**Context:** `rate_limiters` is set once at startup, never mutated, but every request acquires `state.read().await` to clone the `Option<Arc<AdminRateLimiters>>`. Moving it out of the RwLock eliminates this overhead. Additionally, the governor DashMap entries never expire — add periodic cleanup.

**Step 1: Extract rate limiters into a separate Extension**

In `conary-server/src/server/mod.rs`:

1. Remove `rate_limiters` from `ServerState` struct.
2. At startup in `run_server_from_config()`, instead of `state.write().await.rate_limiters = Some(limiters)`, store the `Arc<AdminRateLimiters>` separately.

In `conary-server/src/server/routes.rs`:

1. Accept `Option<Arc<AdminRateLimiters>>` as a parameter to `create_external_admin_router()`.
2. Store it as an axum Extension on the router.

In `conary-server/src/server/rate_limit.rs`:

1. Change `rate_limit_middleware` to extract `Option<Extension<Arc<AdminRateLimiters>>>` instead of reading from `ServerState`.

In `conary-server/src/server/auth.rs`:

1. Change auth middleware to extract `Option<Extension<Arc<AdminRateLimiters>>>` instead of reading from `ServerState`.

**Step 2: Add governor cleanup task**

In `conary-server/src/server/mod.rs`, in the `run_server_from_config()` function where background tasks are spawned, add a periodic cleanup:

```rust
// Periodic rate limiter cleanup (every 5 minutes)
if let Some(ref limiters) = rate_limiters {
    let l = limiters.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            l.read.retain_recent();
            l.write.retain_recent();
            l.auth_fail.retain_recent();
        }
    });
}
```

Note: Check if `governor::DefaultKeyedRateLimiter` exposes `retain_recent()`. If not, this cleanup is not possible with the current governor API and should be skipped. In that case, just document the limitation with a comment.

**Step 3: Run tests and clippy**

Run: `cargo test --features server` and `cargo clippy --features server -- -D warnings`
Expected: All pass

**Step 4: Commit**

```bash
git add conary-server/src/server/mod.rs conary-server/src/server/routes.rs conary-server/src/server/rate_limit.rs conary-server/src/server/auth.rs
git commit -m "perf(server): move rate limiters out of RwLock, add governor cleanup"
```

---

### Task 6: Split admin.rs into Domain Files

**Files:**
- Modify: `conary-server/src/server/handlers/admin.rs` — keep as a barrel re-export
- Create: `conary-server/src/server/handlers/admin/tokens.rs`
- Create: `conary-server/src/server/handlers/admin/ci.rs`
- Create: `conary-server/src/server/handlers/admin/repos.rs`
- Create: `conary-server/src/server/handlers/admin/federation.rs`
- Create: `conary-server/src/server/handlers/admin/audit.rs`
- Create: `conary-server/src/server/handlers/admin/events.rs`

**Context:** `admin.rs` is 1,968 lines with 5+ distinct domains. Split into focused files while keeping the same public API (routes.rs import paths stay the same).

**Step 1: Convert admin.rs from a file to a directory module**

1. Create `conary-server/src/server/handlers/admin/` directory.
2. Move `admin.rs` to `admin/mod.rs`.

**Step 2: Extract each domain into its own file**

For each domain, move the functions and their supporting types (request/response structs) into the domain file. Keep `mod.rs` as a barrel that re-exports everything:

```rust
// conary-server/src/server/handlers/admin/mod.rs

mod tokens;
mod ci;
mod repos;
mod federation;
mod audit;
mod events;

// Re-export all handler functions so routes.rs doesn't change
pub use tokens::*;
pub use ci::*;
pub use repos::*;
pub use federation::*;
pub use audit::*;
pub use events::*;
```

Domain file contents (move from mod.rs):
- `tokens.rs`: `create_token`, `list_tokens`, `delete_token`, `CreateTokenRequest`
- `ci.rs`: `ci_list_workflows`, `ci_list_runs`, `ci_get_run`, `ci_get_logs`, `ci_dispatch`, `ci_mirror_sync` (uses `crate::server::forgejo`)
- `repos.rs`: `list_repos`, `create_repo`, `get_repo`, `update_repo`, `delete_repo`, `sync_repo`, `RepoRequest`, `RepoResponse`
- `federation.rs`: `list_peers`, `add_peer`, `delete_peer`, `peer_health`, `get_federation_config`, `update_federation_config`, `AddPeerRequest`, `PeerResponse`
- `audit.rs`: `query_audit`, `purge_audit`, `AuditQuery`, `PurgeQuery`
- `events.rs`: `sse_events`, `EventsQuery`, `AdminEvent`

Each file needs:
```rust
// conary-server/src/server/handlers/admin/<domain>.rs
```

And shared imports like `use crate::server::auth::Scope;`, `use crate::server::ServerState;`, etc.

**Step 3: Move tests**

The tests section of admin.rs (~lines 1520-1968) should be split into the respective domain files. Each domain file gets its own `#[cfg(test)] mod tests { ... }` block with only the tests relevant to that domain.

**Step 4: Verify routes.rs doesn't need changes**

Since `admin/mod.rs` re-exports everything with `pub use *`, the import path `crate::server::handlers::admin::create_token` etc. should remain valid.

**Step 5: Run tests and clippy**

Run: `cargo test --features server` and `cargo clippy --features server -- -D warnings`
Expected: All pass

**Step 6: Commit**

```bash
git add conary-server/src/server/handlers/admin/
git commit -m "refactor(server): split admin.rs into domain files (tokens, ci, repos, federation, audit, events)"
```

---

### Task 7: Extract Admin Service Layer

**Files:**
- Create: `conary-server/src/server/admin_service.rs`
- Modify: `conary-server/src/server/mod.rs` — add `pub mod admin_service;`
- Modify: `conary-server/src/server/handlers/admin/tokens.rs` — delegate to service
- Modify: `conary-server/src/server/handlers/admin/repos.rs` — delegate to service
- Modify: `conary-server/src/server/handlers/admin/federation.rs` — delegate to service
- Modify: `conary-server/src/server/handlers/admin/audit.rs` — delegate to service

**Context:** MCP tools and admin handlers duplicate business logic. Extract shared service functions that both callers use. The service layer handles DB access, validation, and business rules. HTTP handlers wrap results into HTTP responses. MCP tools wrap results into MCP responses.

**Step 1: Create the service module**

Create `conary-server/src/server/admin_service.rs` with a service error type and shared business logic:

```rust
// conary-server/src/server/admin_service.rs

//! Shared business logic for admin operations.
//!
//! Called by both HTTP handlers (admin/*.rs) and MCP tools (mcp.rs).
//! Handles DB access, validation, and business rules.
//! Callers map ServiceError to their own response types.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::auth::{generate_token, hash_token, validate_scopes};

/// Error from a service operation.
#[derive(Debug)]
pub enum ServiceError {
    /// Client error (400-level)
    BadRequest(String),
    /// Not found (404)
    NotFound(String),
    /// Internal error (500-level)
    Internal(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::BadRequest(msg) => write!(f, "Bad request: {msg}"),
            ServiceError::NotFound(msg) => write!(f, "Not found: {msg}"),
            ServiceError::Internal(msg) => write!(f, "Internal error: {msg}"),
        }
    }
}

/// Extract db_path from server state.
async fn db_path(state: &Arc<RwLock<ServerState>>) -> PathBuf {
    state.read().await.config.db_path.clone()
}

// -- Token operations --

pub struct CreatedToken {
    pub id: i64,
    pub raw_token: String,
    pub name: String,
    pub scopes: String,
}

pub async fn create_token(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
    scopes: &str,
) -> Result<CreatedToken, ServiceError> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 {
        return Err(ServiceError::BadRequest(
            "Token name must be 1-128 characters".to_string(),
        ));
    }

    let scopes = if scopes.is_empty() { "admin" } else { scopes };
    if let Err(invalid) = validate_scopes(scopes) {
        return Err(ServiceError::BadRequest(format!(
            "Invalid scope: '{invalid}'"
        )));
    }
    let scopes = scopes.to_string();

    let raw = generate_token();
    let hash = hash_token(&raw);
    let db = db_path(state).await;
    let name_for_db = name.clone();
    let scopes_for_db = scopes.clone();
    let hash_for_db = hash.clone();

    let id = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::admin_token::create(&conn, &name_for_db, &hash_for_db, &scopes_for_db)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))?;

    Ok(CreatedToken { id, raw_token: raw, name, scopes })
}

pub async fn list_tokens(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Vec<conary_core::db::models::admin_token::AdminToken>, ServiceError> {
    let db = db_path(state).await;
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::admin_token::list(&conn)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

pub async fn delete_token(
    state: &Arc<RwLock<ServerState>>,
    token_id: i64,
) -> Result<bool, ServiceError> {
    let db = db_path(state).await;
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::admin_token::delete(&conn, token_id)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

// -- Federation peer operations --

pub async fn list_peers(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Vec<conary_core::db::models::federation_peer::FederationPeer>, ServiceError> {
    let db = db_path(state).await;
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::federation_peer::list(&conn)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

pub struct AddPeerInput {
    pub name: String,
    pub url: String,
    pub tier: String,
    pub region: Option<String>,
    pub cell: Option<String>,
    pub public_key: Option<String>,
    pub notes: Option<String>,
}

pub async fn add_peer(
    state: &Arc<RwLock<ServerState>>,
    input: AddPeerInput,
) -> Result<String, ServiceError> {
    // Validate tier
    if !["leaf", "cell_hub", "region_hub"].contains(&input.tier.as_str()) {
        return Err(ServiceError::BadRequest(format!(
            "Invalid tier '{}'. Must be: leaf, cell_hub, region_hub",
            input.tier
        )));
    }

    // Generate deterministic ID from URL
    let id = conary_core::hash::sha256(input.url.as_bytes())[..16].to_string();
    let db = db_path(state).await;

    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::federation_peer::insert(
            &conn,
            &id,
            &input.name,
            &input.url,
            &input.tier,
            input.region.as_deref(),
            input.cell.as_deref(),
            input.public_key.as_deref(),
            input.notes.as_deref(),
        )?;
        Ok::<_, conary_core::error::Error>(id)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

pub async fn delete_peer(
    state: &Arc<RwLock<ServerState>>,
    peer_id: &str,
) -> Result<bool, ServiceError> {
    let db = db_path(state).await;
    let id = peer_id.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::federation_peer::delete(&conn, &id)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

// -- Audit operations --

pub async fn query_audit(
    state: &Arc<RwLock<ServerState>>,
    limit: Option<i64>,
    action: Option<&str>,
    since: Option<&str>,
    token_name: Option<&str>,
) -> Result<Vec<conary_core::db::models::audit_log::AuditEntry>, ServiceError> {
    let db = db_path(state).await;
    let action = action.map(|s| s.to_string());
    let since = since.map(|s| s.to_string());
    let token_name = token_name.map(|s| s.to_string());
    tokio::task::spawn_blocking(move || {
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
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

pub async fn purge_audit(
    state: &Arc<RwLock<ServerState>>,
    before: &str,
) -> Result<usize, ServiceError> {
    let db = db_path(state).await;
    let before = before.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::audit_log::purge(&conn, &before)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

// -- Repo operations --
// Note: repo operations delegate to the existing Repository model which
// already has proper CRUD functions. These service functions standardize
// the spawn_blocking + db::open_fast pattern.

pub async fn list_repos(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Vec<conary_core::db::models::Repository>, ServiceError> {
    let db = db_path(state).await;
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::Repository::list_all(&conn)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}

pub async fn get_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
) -> Result<Option<conary_core::db::models::Repository>, ServiceError> {
    let db = db_path(state).await;
    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db)?;
        conary_core::db::models::Repository::find_by_name(&conn, &name)
    })
    .await
    .map_err(|e| ServiceError::Internal(format!("Task join error: {e}")))?
    .map_err(|e| ServiceError::Internal(format!("DB error: {e}")))
}
```

**Step 2: Register in mod.rs**

Add `pub mod admin_service;` in `conary-server/src/server/mod.rs`.

**Step 3: Refactor admin handler files to use service layer**

For each domain file (tokens.rs, federation.rs, audit.rs, repos.rs), replace the inline `spawn_blocking + db::open` pattern with a call to the corresponding service function. The handler becomes a thin wrapper that:
1. Checks scopes
2. Calls the service function
3. Maps the result to an HTTP response

Example for `tokens.rs`:
```rust
pub async fn create_token(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes_ext: Option<Extension<TokenScopes>>,
    Json(body): Json<CreateTokenRequest>,
) -> Response {
    if let Some(resp) = check_scope(&scopes_ext, Scope::Admin) { return resp; }

    match crate::server::admin_service::create_token(&state, &body.name, &body.scopes.unwrap_or_default()).await {
        Ok(created) => {
            // Publish SSE event, return JSON response
            ...
        }
        Err(ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(ServiceError::NotFound(msg)) => json_error(404, &msg, "NOT_FOUND"),
        Err(ServiceError::Internal(msg)) => json_error(500, &msg, "INTERNAL_ERROR"),
    }
}
```

**Step 4: Run tests and clippy**

Run: `cargo test --features server` and `cargo clippy --features server -- -D warnings`
Expected: All pass

**Step 5: Commit**

```bash
git add conary-server/src/server/admin_service.rs conary-server/src/server/mod.rs conary-server/src/server/handlers/admin/
git commit -m "refactor(server): extract admin service layer for shared business logic"
```

---

### Task 8: Wire MCP Tools to Service Layer

**Files:**
- Modify: `conary-server/src/server/mcp.rs` — replace business logic with service calls

**Context:** MCP tools currently reimplement all business logic. With the service layer from Task 7, each MCP tool becomes a thin wrapper: call the service function, map the result to `CallToolResult`.

**Step 1: Refactor all MCP tools to use service layer**

For each MCP tool, replace the `spawn_blocking + db::open + raw SQL/model call` pattern with a call to the corresponding service function. Example:

Before (create_token, ~50 lines):
```rust
async fn create_token(&self, params) -> Result<CallToolResult, McpError> {
    // inline validation, generate_token, hash_token, spawn_blocking, db::open, admin_token::create
}
```

After (~15 lines):
```rust
async fn create_token(&self, Parameters(params): Parameters<CreateTokenParams>) -> Result<CallToolResult, McpError> {
    let scopes = params.scopes.as_deref().unwrap_or("admin");
    match crate::server::admin_service::create_token(&self.state, &params.name, scopes).await {
        Ok(created) => {
            let json = serde_json::json!({
                "id": created.id,
                "token": created.raw_token,
                "name": created.name,
                "scopes": created.scopes,
            });
            Ok(CallToolResult::success(vec![Content::text(json.to_string())]))
        }
        Err(e) => Err(McpError::internal_error(e.to_string(), None)),
    }
}
```

Tools to refactor:
- `list_tokens` → `admin_service::list_tokens`
- `create_token` → `admin_service::create_token`
- `delete_token` → `admin_service::delete_token`
- `list_repos` → `admin_service::list_repos`
- `get_repo` → `admin_service::get_repo`
- `list_peers` → `admin_service::list_peers`
- `add_peer` → `admin_service::add_peer`
- `delete_peer` → `admin_service::delete_peer`
- `query_audit_log` → `admin_service::query_audit`
- `purge_audit_log` → `admin_service::purge_audit`

CI tools (`ci_list_workflows`, `ci_list_runs`, `ci_get_run`, `ci_get_logs`, `ci_dispatch`, `ci_mirror_sync`) already use the shared Forgejo client from Task 4, so those are already deduplicated.

**Step 2: Remove `validate_path_param` from mcp.rs**

With business logic in the service layer, MCP tools no longer need their own `validate_path_param`. Remove it and use the shared `is_valid_path_param` from `handlers/mod.rs` (or the service layer validates internally).

**Step 3: Remove unused imports**

Clean up imports in `mcp.rs` — `generate_token`, `hash_token`, `db::open` calls should all be gone now.

**Step 4: Run tests and clippy**

Run: `cargo test --features server` and `cargo clippy --features server -- -D warnings`
Expected: All pass

**Step 5: Commit**

```bash
git add conary-server/src/server/mcp.rs
git commit -m "refactor(server): wire MCP tools to shared service layer, remove duplication"
```

---

## Summary

| Task | What | Key Files | Depends On |
|------|------|-----------|------------|
| 1 | `db::open_fast()` | conary-core/src/db/mod.rs | — |
| 2 | Scope enum | auth.rs, admin handlers | — |
| 3 | Federation peer model | models/federation_peer.rs | — |
| 4 | Forgejo client module | server/forgejo.rs | — |
| 5 | Rate limiters out of RwLock | mod.rs, routes.rs, rate_limit.rs, auth.rs | — |
| 6 | Split admin.rs | handlers/admin/ directory | — |
| 7 | Admin service layer | admin_service.rs, admin handlers | 3, 4, 6 |
| 8 | Wire MCP to service | mcp.rs | 7 |

Tasks 1-5 are independent. Tasks 6-8 must be sequential.
