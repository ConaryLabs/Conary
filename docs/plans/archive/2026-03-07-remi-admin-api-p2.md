# Remi Admin API P2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add rate limiting and audit logging to the Remi external admin API (:8082).

**Architecture:** Rate limiting uses `governor` crate with three in-memory token buckets (read/write/auth-fail) applied as axum middleware on the external admin router. Audit logging uses a new `admin_audit_log` SQLite table (migration v48) with an axum middleware that captures request/response details for write operations and logs all requests asynchronously via `spawn_blocking`.

**Tech Stack:** Rust, axum, governor, rusqlite, tokio, serde

**Design doc:** `docs/plans/2026-03-07-admin-api-p2-design.md`

---

### Task 1: DB Migration — audit_log table

**Files:**
- Modify: `conary-core/src/db/schema.rs`
- Modify: `conary-core/src/db/migrations.rs`

**Step 1: Add migration v48**

In `conary-core/src/db/migrations.rs`, add after `migrate_v47`:

```rust
/// Version 48 - Admin audit log
///
/// Creates the admin_audit_log table for tracking admin API operations.
pub fn migrate_v48(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS admin_audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL DEFAULT (datetime('now')),
            token_name TEXT,
            action TEXT NOT NULL,
            method TEXT NOT NULL,
            path TEXT NOT NULL,
            status_code INTEGER NOT NULL,
            request_body TEXT,
            response_body TEXT,
            source_ip TEXT,
            duration_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON admin_audit_log(timestamp);
        CREATE INDEX IF NOT EXISTS idx_audit_log_action ON admin_audit_log(action);",
    )?;

    info!("Schema version 48 applied successfully (admin_audit_log table)");
    Ok(())
}
```

**Step 2: Update schema.rs**

In `conary-core/src/db/schema.rs`:
- Change `SCHEMA_VERSION` from 47 to 48
- Add dispatch: `48 => migrations::migrate_v48(conn),`

**Step 3: Build and test**

Run: `cargo build --features server`
Run: `cargo test -p conary-core`
Expected: All pass.

**Step 4: Commit**

```bash
git add conary-core/src/db/schema.rs conary-core/src/db/migrations.rs
git commit -m "feat(db): add admin_audit_log table (migration v48)"
```

---

### Task 2: Audit Log Model

**Files:**
- Create: `conary-core/src/db/models/audit_log.rs`
- Modify: `conary-core/src/db/models/mod.rs`

**Step 1: Create audit_log model**

Create `conary-core/src/db/models/audit_log.rs`:

```rust
// conary-core/src/db/models/audit_log.rs

//! Admin audit log model - tracks admin API operations

use crate::error::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

/// A single audit log entry.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub timestamp: String,
    pub token_name: Option<String>,
    pub action: String,
    pub method: String,
    pub path: String,
    pub status_code: i32,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub source_ip: Option<String>,
    pub duration_ms: Option<i64>,
}

/// Insert a new audit log entry.
pub fn insert(
    conn: &Connection,
    token_name: Option<&str>,
    action: &str,
    method: &str,
    path: &str,
    status_code: i32,
    request_body: Option<&str>,
    response_body: Option<&str>,
    source_ip: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO admin_audit_log \
         (token_name, action, method, path, status_code, request_body, response_body, source_ip, duration_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![token_name, action, method, path, status_code, request_body, response_body, source_ip, duration_ms],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Query audit log entries with optional filters.
///
/// Filters:
/// - `limit`: Max entries to return (default 50, max 500)
/// - `action`: Filter by action prefix (e.g., "repo" matches "repo.create", "repo.delete")
/// - `since`: Only entries after this ISO 8601 timestamp
/// - `token_name`: Filter by token name
pub fn query(
    conn: &Connection,
    limit: Option<i64>,
    action: Option<&str>,
    since: Option<&str>,
    token_name: Option<&str>,
) -> Result<Vec<AuditEntry>> {
    let limit = limit.unwrap_or(50).min(500);

    let mut sql = String::from(
        "SELECT id, timestamp, token_name, action, method, path, status_code, \
         request_body, response_body, source_ip, duration_ms \
         FROM admin_audit_log WHERE 1=1"
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(action) = action {
        sql.push_str(&format!(" AND action LIKE ?{param_idx}"));
        param_values.push(Box::new(format!("{action}%")));
        param_idx += 1;
    }
    if let Some(since) = since {
        sql.push_str(&format!(" AND timestamp >= ?{param_idx}"));
        param_values.push(Box::new(since.to_string()));
        param_idx += 1;
    }
    if let Some(name) = token_name {
        sql.push_str(&format!(" AND token_name = ?{param_idx}"));
        param_values.push(Box::new(name.to_string()));
        param_idx += 1;
    }
    let _ = param_idx; // suppress unused warning

    sql.push_str(&format!(" ORDER BY id DESC LIMIT {limit}"));

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let entries = stmt.query_map(params_ref.as_slice(), |row| {
        Ok(AuditEntry {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            token_name: row.get(2)?,
            action: row.get(3)?,
            method: row.get(4)?,
            path: row.get(5)?,
            status_code: row.get(6)?,
            request_body: row.get(7)?,
            response_body: row.get(8)?,
            source_ip: row.get(9)?,
            duration_ms: row.get(10)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(entries)
}

/// Delete audit log entries older than the given timestamp.
///
/// Returns the number of entries deleted.
pub fn purge(conn: &Connection, before: &str) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM admin_audit_log WHERE timestamp < ?1",
        [before],
    )?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;").unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_query() {
        let conn = test_conn();
        let id = insert(
            &conn,
            Some("test-admin"),
            "token.create",
            "POST",
            "/v1/admin/tokens",
            201,
            Some(r#"{"name":"new-token"}"#),
            Some(r#"{"id":1}"#),
            Some("127.0.0.1"),
            Some(42),
        ).unwrap();
        assert!(id > 0);

        let entries = query(&conn, Some(10), None, None, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "token.create");
        assert_eq!(entries[0].token_name, Some("test-admin".to_string()));
        assert_eq!(entries[0].status_code, 201);
        assert_eq!(entries[0].duration_ms, Some(42));
    }

    #[test]
    fn test_query_filters() {
        let conn = test_conn();
        insert(&conn, Some("admin"), "token.create", "POST", "/v1/admin/tokens", 201, None, None, None, None).unwrap();
        insert(&conn, Some("admin"), "repo.create", "POST", "/v1/admin/repos", 201, None, None, None, None).unwrap();
        insert(&conn, Some("ci-reader"), "ci.list", "GET", "/v1/admin/ci/workflows", 200, None, None, None, None).unwrap();

        // Filter by action prefix
        let entries = query(&conn, None, Some("repo"), None, None).unwrap();
        assert_eq!(entries.len(), 1);

        // Filter by token_name
        let entries = query(&conn, None, None, None, Some("ci-reader")).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_purge() {
        let conn = test_conn();
        // Insert with explicit old timestamp
        conn.execute(
            "INSERT INTO admin_audit_log (timestamp, action, method, path, status_code) \
             VALUES ('2020-01-01T00:00:00', 'old.action', 'GET', '/old', 200)",
            [],
        ).unwrap();
        insert(&conn, None, "new.action", "GET", "/new", 200, None, None, None, None).unwrap();

        let deleted = purge(&conn, "2025-01-01T00:00:00").unwrap();
        assert_eq!(deleted, 1);

        let remaining = query(&conn, None, None, None, None).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].action, "new.action");
    }
}
```

**Step 2: Register the module**

In `conary-core/src/db/models/mod.rs`, add:
```rust
pub mod audit_log;
```

**Step 3: Build and test**

Run: `cargo test -p conary-core`
Expected: All pass including 3 new audit_log tests.

**Step 4: Commit**

```bash
git add conary-core/src/db/models/audit_log.rs conary-core/src/db/models/mod.rs
git commit -m "feat(db): add audit_log model with insert, query, and purge"
```

---

### Task 3: Rate Limiting Middleware

**Files:**
- Create: `conary-server/src/server/rate_limit.rs`
- Modify: `conary-server/src/server/mod.rs`
- Modify: `conary-server/Cargo.toml`

**Step 1: Add governor dependency**

In `conary-server/Cargo.toml`, add:
```toml
governor = "0.8"
```

**Step 2: Create rate_limit module**

Create `conary-server/src/server/rate_limit.rs`:

```rust
// conary-server/src/server/rate_limit.rs
//! Rate limiting middleware for the external admin API.
//!
//! Three separate token buckets per source IP:
//! - Read (GET): default 60/min
//! - Write (POST/PUT/DELETE): default 10/min
//! - Auth failure: default 5/min (applied in auth middleware on 401)

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::{Quota, RateLimiter, clock::DefaultClock, state::keyed::DashMapStateStore};
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::server::auth::json_error;

/// Keyed rate limiter (keyed by IP address).
pub type KeyedLimiter = RateLimiter<IpAddr, DashMapStateStore<IpAddr>, DefaultClock>;

/// Rate limiter set for the admin API.
pub struct AdminRateLimiters {
    /// Rate limiter for read operations (GET)
    pub read: Arc<KeyedLimiter>,
    /// Rate limiter for write operations (POST/PUT/DELETE)
    pub write: Arc<KeyedLimiter>,
    /// Rate limiter for auth failures (applied separately)
    pub auth_fail: Arc<KeyedLimiter>,
}

impl AdminRateLimiters {
    /// Create a new set of rate limiters with the given per-minute limits.
    pub fn new(read_rpm: u32, write_rpm: u32, auth_fail_rpm: u32) -> Self {
        Self {
            read: Arc::new(Self::make_limiter(read_rpm)),
            write: Arc::new(Self::make_limiter(write_rpm)),
            auth_fail: Arc::new(Self::make_limiter(auth_fail_rpm)),
        }
    }

    fn make_limiter(rpm: u32) -> KeyedLimiter {
        let quota = Quota::per_minute(NonZeroU32::new(rpm).unwrap_or(NonZeroU32::new(1).unwrap()));
        RateLimiter::keyed(quota)
    }
}

/// Extract client IP from the request.
///
/// Tries ConnectInfo first, falls back to 127.0.0.1.
fn extract_ip(request: &Request<Body>) -> IpAddr {
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or_else(|| IpAddr::from([127, 0, 0, 1]))
}

/// Rate limiting middleware for the external admin API.
///
/// Checks the read or write bucket depending on the HTTP method.
/// Returns 429 with Retry-After header if the limit is exceeded.
pub async fn rate_limit_middleware(
    State(limiters): State<Arc<AdminRateLimiters>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let ip = extract_ip(&request);
    let method = request.method().clone();

    let limiter = if method == axum::http::Method::GET || method == axum::http::Method::HEAD {
        &limiters.read
    } else {
        &limiters.write
    };

    if limiter.check_key(&ip).is_err() {
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            [
                ("content-type", "application/json"),
                ("retry-after", "60"),
            ],
            r#"{"error":"Rate limit exceeded","code":"RATE_LIMITED"}"#,
        )
            .into_response();
    }

    next.run(request).await
}

/// Record an auth failure for rate limiting purposes.
///
/// Called from the auth middleware when a 401 is returned.
/// Returns true if the auth failure rate limit is also exceeded
/// (in which case the caller should return 429 instead of 401).
pub fn check_auth_failure(limiters: &AdminRateLimiters, ip: IpAddr) -> bool {
    limiters.auth_fail.check_key(&ip).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limiter_creation() {
        let limiters = AdminRateLimiters::new(60, 10, 5);
        let ip = IpAddr::from([1, 2, 3, 4]);

        // First request should pass
        assert!(limiters.read.check_key(&ip).is_ok());
        assert!(limiters.write.check_key(&ip).is_ok());
    }

    #[test]
    fn test_write_limiter_exhaustion() {
        // 2 per minute -- should exhaust quickly
        let limiters = AdminRateLimiters::new(60, 2, 5);
        let ip = IpAddr::from([10, 0, 0, 1]);

        // First 2 should pass
        assert!(limiters.write.check_key(&ip).is_ok());
        assert!(limiters.write.check_key(&ip).is_ok());

        // Third should be rate limited
        assert!(limiters.write.check_key(&ip).is_err());
    }

    #[test]
    fn test_different_ips_independent() {
        let limiters = AdminRateLimiters::new(60, 1, 5);
        let ip1 = IpAddr::from([10, 0, 0, 1]);
        let ip2 = IpAddr::from([10, 0, 0, 2]);

        // Exhaust ip1's write limit
        assert!(limiters.write.check_key(&ip1).is_ok());
        assert!(limiters.write.check_key(&ip1).is_err());

        // ip2 should still be fine
        assert!(limiters.write.check_key(&ip2).is_ok());
    }

    #[test]
    fn test_auth_failure_check() {
        let limiters = AdminRateLimiters::new(60, 10, 2);
        let ip = IpAddr::from([192, 168, 1, 1]);

        // First 2 auth failures OK
        assert!(!check_auth_failure(&limiters, ip));
        assert!(!check_auth_failure(&limiters, ip));

        // Third should trigger rate limit
        assert!(check_auth_failure(&limiters, ip));
    }
}
```

**Step 3: Register module and add to ServerState**

In `conary-server/src/server/mod.rs`:
- Add `pub mod rate_limit;`

**Step 4: Build and test**

Run: `cargo build --features server`
Run: `cargo test --features server -p conary-server`
Expected: All pass including 4 new rate_limit tests.

**Step 5: Commit**

```bash
git add conary-server/Cargo.toml conary-server/src/server/rate_limit.rs conary-server/src/server/mod.rs
git commit -m "feat(server): add rate limiting middleware with per-IP token buckets"
```

---

### Task 4: Audit Log Middleware

**Files:**
- Create: `conary-server/src/server/audit.rs`
- Modify: `conary-server/src/server/mod.rs`

**Context:** The audit middleware wraps requests to capture timing, method, path, token name, and (for writes) request/response bodies. It runs after auth so it can access the token name from extensions. It uses `spawn_blocking` to write the log entry asynchronously.

**Step 1: Create audit module**

Create `conary-server/src/server/audit.rs`:

```rust
// conary-server/src/server/audit.rs
//! Audit logging middleware for the external admin API.
//!
//! Captures all admin API requests with timing, token identity, and
//! (for write operations) request/response bodies.

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::auth::TokenScopes;

/// Derive a semantic action name from method + path.
///
/// Examples:
/// - POST /v1/admin/tokens -> "token.create"
/// - GET /v1/admin/repos -> "repo.list"
/// - DELETE /v1/admin/federation/peers/abc123 -> "federation.peer.delete"
fn derive_action(method: &str, path: &str) -> String {
    // Strip the /v1/admin/ prefix
    let rest = path
        .strip_prefix("/v1/admin/")
        .unwrap_or(path);

    // Map known path patterns to semantic actions
    let resource = if rest.starts_with("tokens") {
        "token"
    } else if rest.starts_with("ci/mirror-sync") {
        "ci.mirror_sync"
    } else if rest.starts_with("ci/workflows") && rest.contains("/dispatch") {
        "ci.dispatch"
    } else if rest.starts_with("ci/") {
        "ci"
    } else if rest.starts_with("repos") {
        if rest.contains("/sync") {
            "repo.sync"
        } else {
            "repo"
        }
    } else if rest.starts_with("federation/config") {
        "federation.config"
    } else if rest.starts_with("federation/peers") {
        "federation.peer"
    } else if rest.starts_with("audit") {
        "audit"
    } else if rest.starts_with("events") {
        "events"
    } else {
        "unknown"
    };

    let verb = match method {
        "GET" => "read",
        "POST" => "create",
        "PUT" => "update",
        "DELETE" => "delete",
        _ => "unknown",
    };

    // Special cases where the resource already includes the verb
    if resource.ends_with("dispatch") || resource.ends_with("mirror_sync") || resource.ends_with("sync") {
        return resource.to_string();
    }

    format!("{resource}.{verb}")
}

/// Audit logging middleware.
///
/// Captures request details, passes to the handler, then logs the result
/// asynchronously. For write operations (POST/PUT/DELETE), also captures
/// request and response bodies.
pub async fn audit_middleware(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let start = Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let is_write = matches!(method.as_str(), "POST" | "PUT" | "DELETE");

    // Extract token name from extensions (set by auth middleware)
    let token_name = request
        .extensions()
        .get::<TokenScopes>()
        .map(|_| {
            // TokenScopes doesn't carry the name — we'll get it from the
            // token_name extension if the auth middleware stores it.
            // For now, extract from the scopes string as a fallback.
            // TODO: Store token name in auth middleware extensions.
            None::<String>
        })
        .unwrap_or(None);

    // For write operations, capture the request body
    let (request, request_body) = if is_write {
        let (parts, body) = request.into_parts();
        match axum::body::to_bytes(body, 64 * 1024).await {
            Ok(bytes) => {
                let body_str = String::from_utf8_lossy(&bytes).to_string();
                let new_body = Body::from(bytes);
                (Request::from_parts(parts, new_body), Some(body_str))
            }
            Err(_) => {
                let new_body = Body::empty();
                (Request::from_parts(parts, new_body), None)
            }
        }
    } else {
        (request, None)
    };

    // Run the actual handler
    let response = next.run(request).await;
    let duration_ms = start.elapsed().as_millis() as i64;
    let status_code = response.status().as_u16() as i32;

    // For write operations, capture the response body
    let (response, response_body) = if is_write {
        let (parts, body) = response.into_parts();
        match axum::body::to_bytes(body, 64 * 1024).await {
            Ok(bytes) => {
                let body_str = String::from_utf8_lossy(&bytes).to_string();
                let new_body = Body::from(bytes);
                (Response::from_parts(parts, new_body), Some(body_str))
            }
            Err(_) => {
                let new_body = Body::empty();
                (Response::from_parts(parts, new_body), None)
            }
        }
    } else {
        (response, None)
    };

    let action = derive_action(&method, &path);

    // Log asynchronously -- don't block the response
    let db_path = {
        let s = state.read().await;
        s.config.db_path.clone()
    };

    let token_name_owned = token_name;
    tokio::task::spawn_blocking(move || {
        if let Ok(conn) = conary_core::db::open(&db_path) {
            if let Err(e) = conary_core::db::models::audit_log::insert(
                &conn,
                token_name_owned.as_deref(),
                &action,
                &method,
                &path,
                status_code,
                request_body.as_deref(),
                response_body.as_deref(),
                None, // source_ip -- requires ConnectInfo, added later
                Some(duration_ms),
            ) {
                tracing::warn!("Failed to write audit log: {e}");
            }
        }
    });

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_action_tokens() {
        assert_eq!(derive_action("POST", "/v1/admin/tokens"), "token.create");
        assert_eq!(derive_action("GET", "/v1/admin/tokens"), "token.read");
        assert_eq!(derive_action("DELETE", "/v1/admin/tokens/5"), "token.delete");
    }

    #[test]
    fn test_derive_action_ci() {
        assert_eq!(derive_action("GET", "/v1/admin/ci/workflows"), "ci.read");
        assert_eq!(derive_action("POST", "/v1/admin/ci/workflows/ci.yaml/dispatch"), "ci.dispatch");
        assert_eq!(derive_action("POST", "/v1/admin/ci/mirror-sync"), "ci.mirror_sync");
    }

    #[test]
    fn test_derive_action_repos() {
        assert_eq!(derive_action("GET", "/v1/admin/repos"), "repo.read");
        assert_eq!(derive_action("POST", "/v1/admin/repos"), "repo.create");
        assert_eq!(derive_action("PUT", "/v1/admin/repos/fedora"), "repo.update");
        assert_eq!(derive_action("DELETE", "/v1/admin/repos/fedora"), "repo.delete");
        assert_eq!(derive_action("POST", "/v1/admin/repos/fedora/sync"), "repo.sync");
    }

    #[test]
    fn test_derive_action_federation() {
        assert_eq!(derive_action("GET", "/v1/admin/federation/peers"), "federation.peer.read");
        assert_eq!(derive_action("POST", "/v1/admin/federation/peers"), "federation.peer.create");
        assert_eq!(derive_action("DELETE", "/v1/admin/federation/peers/abc"), "federation.peer.delete");
        assert_eq!(derive_action("GET", "/v1/admin/federation/config"), "federation.config.read");
        assert_eq!(derive_action("PUT", "/v1/admin/federation/config"), "federation.config.update");
    }
}
```

**Step 2: Register module**

In `conary-server/src/server/mod.rs`, add:
```rust
pub mod audit;
```

**Step 3: Build and test**

Run: `cargo build --features server`
Run: `cargo test --features server -p conary-server`
Expected: All pass including 4 new audit tests.

**Step 4: Commit**

```bash
git add conary-server/src/server/audit.rs conary-server/src/server/mod.rs
git commit -m "feat(server): add audit logging middleware with action derivation"
```

---

### Task 5: Wire Rate Limiting and Audit into Router

**Files:**
- Modify: `conary-server/src/server/config.rs`
- Modify: `conary-server/src/server/routes.rs`
- Modify: `conary-server/src/server/mod.rs`

**Context:** Add rate limiting config fields to `AdminSection`, create `AdminRateLimiters` during server startup, and apply both middleware layers to the external admin router. Layer order (outermost to innermost): rate_limit -> auth -> audit -> handlers.

**Step 1: Add config fields**

In `conary-server/src/server/config.rs`, add to `AdminSection`:

```rust
    /// Rate limit for read operations (GET), requests per minute per IP
    #[serde(default = "default_admin_read_rpm")]
    pub rate_limit_read_rpm: u32,

    /// Rate limit for write operations (POST/PUT/DELETE), requests per minute per IP
    #[serde(default = "default_admin_write_rpm")]
    pub rate_limit_write_rpm: u32,

    /// Rate limit for auth failures, attempts per minute per IP
    #[serde(default = "default_admin_auth_fail_rpm")]
    pub rate_limit_auth_fail_rpm: u32,

    /// Audit log retention in days (for display only; purge via API)
    #[serde(default = "default_audit_retention_days")]
    pub audit_retention_days: u32,
```

Add default functions:
```rust
fn default_admin_read_rpm() -> u32 { 60 }
fn default_admin_write_rpm() -> u32 { 10 }
fn default_admin_auth_fail_rpm() -> u32 { 5 }
fn default_audit_retention_days() -> u32 { 30 }
```

Update the `Default` impl for `AdminSection` to include these fields.

**Step 2: Wire into external admin router**

In `conary-server/src/server/routes.rs`, in `create_external_admin_router()`:

1. Create the rate limiters from config:
```rust
    let admin_config = {
        let s = state.read().blocking_lock(); // Can't use this -- we're not in async context
    };
```

Actually, since `create_external_admin_router` is called during server setup (not in an async context necessarily), we need to pass the config values in. The simplest approach: create the `AdminRateLimiters` outside the router function and pass it in, OR read the config directly from the state synchronously.

Better approach: create the limiters in `create_external_admin_router` using default values, and make the function accept optional config:

```rust
pub fn create_external_admin_router(state: Arc<RwLock<ServerState>>) -> Router {
    // Create rate limiters (using defaults -- config is read at startup)
    let limiters = Arc::new(crate::server::rate_limit::AdminRateLimiters::new(60, 10, 5));
```

Even better: pass the admin config as a parameter. Let's look at how the function is called and adjust accordingly. The implementer should check the call site.

The key wiring: apply layers in order (outermost first):

```rust
    // Auth-protected routes with audit logging
    let protected = Router::new()
        // ... all routes ...
        // Audit middleware (innermost -- runs after auth, captures token info)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::server::audit::audit_middleware,
        ))
        // Auth middleware
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::server::auth::auth_middleware,
        ));

    // Rate limiting wraps everything (including unprotected routes)
    unprotected.merge(protected)
        .layer(middleware::from_fn_with_state(
            limiters,
            crate::server::rate_limit::rate_limit_middleware,
        ))
        .with_state(state)
```

**Step 3: Build and test**

Run: `cargo build --features server`
Run: `cargo test --features server -p conary-server`
Expected: All pass.

**Step 4: Commit**

```bash
git add conary-server/src/server/config.rs conary-server/src/server/routes.rs conary-server/src/server/mod.rs
git commit -m "feat(server): wire rate limiting and audit middleware into admin router"
```

---

### Task 6: Audit Log Endpoints and MCP Tools

**Files:**
- Modify: `conary-server/src/server/handlers/admin.rs`
- Modify: `conary-server/src/server/routes.rs`
- Modify: `conary-server/src/server/mcp.rs`

**Step 1: Add audit log REST handlers**

In `admin.rs`, add:

```rust
// ========== Audit Log ==========

/// Query parameters for the audit log endpoint.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub action: Option<String>,
    pub since: Option<String>,
    pub token_name: Option<String>,
}

/// Query parameters for purging audit entries.
#[derive(Debug, Deserialize)]
pub struct PurgeQuery {
    pub before: String,
}

/// GET /v1/admin/audit
pub async fn query_audit(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<AuditQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "admin") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::audit_log::query(
            &conn,
            query.limit,
            query.action.as_deref(),
            query.since.as_deref(),
            query.token_name.as_deref(),
        )
    })
    .await;
    match result {
        Ok(Ok(entries)) => Json(entries).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to query audit log: {e}");
            json_error(500, "Failed to query audit log", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error querying audit: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/audit
pub async fn purge_audit(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<PurgeQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, "admin") {
        return err;
    }
    let db_path = { state.read().await.config.db_path.clone() };
    let before = query.before.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        conary_core::db::models::audit_log::purge(&conn, &before)
    })
    .await;
    match result {
        Ok(Ok(deleted)) => Json(serde_json::json!({"deleted": deleted, "before": query.before})).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to purge audit log: {e}");
            json_error(500, "Failed to purge audit log", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error purging audit: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}
```

**Step 2: Add routes**

In `routes.rs`, add to the `protected` router:
```rust
        .route("/v1/admin/audit", get(admin::query_audit))
        .route("/v1/admin/audit", delete(admin::purge_audit))
```

**Step 3: Add MCP tools**

In `mcp.rs`, add parameter structs:
```rust
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
```

Add tools in the `#[tool_router]` block:
```rust
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
            serde_json::to_string_pretty(&result).unwrap(),
        )]))
    }

    /// Purge old audit log entries. Deletes entries older than the given date.
    ///
    /// **Not idempotent** — deleted entries cannot be recovered.
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
            })).unwrap(),
        )]))
    }
```

Update `test_mcp_tool_count` to assert 16 tools (14 + 2 new audit tools).

Update `get_info()` instructions to mention audit log.

**Step 4: Build and test**

Run: `cargo build --features server`
Run: `cargo test --features server -p conary-server`
Expected: All pass.

**Step 5: Commit**

```bash
git add conary-server/src/server/handlers/admin.rs conary-server/src/server/routes.rs conary-server/src/server/mcp.rs
git commit -m "feat(server): add audit log endpoints and MCP tools"
```

---

### Task 7: Update OpenAPI Spec

**Files:**
- Modify: `conary-server/src/server/handlers/openapi.rs`

**Step 1: Add audit endpoints to spec**

Add to the paths object:

```json
"/v1/admin/audit": {
    "get": {
        "operationId": "queryAudit",
        "summary": "Query audit log",
        "description": "Returns recent admin API operations. Supports filtering by action, token, and time range. Write operations include request/response bodies.",
        "security": [{"bearerAuth": []}],
        "parameters": [
            {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 50}},
            {"name": "action", "in": "query", "schema": {"type": "string"}, "description": "Filter by action prefix"},
            {"name": "since", "in": "query", "schema": {"type": "string"}, "description": "ISO 8601 timestamp"},
            {"name": "token_name", "in": "query", "schema": {"type": "string"}}
        ],
        "responses": {
            "200": {"description": "Array of audit log entries"}
        }
    },
    "delete": {
        "operationId": "purgeAudit",
        "summary": "Purge old audit entries",
        "description": "Delete audit log entries older than the specified date. NOT reversible.",
        "security": [{"bearerAuth": []}],
        "parameters": [
            {"name": "before", "in": "query", "required": true, "schema": {"type": "string"}, "description": "ISO 8601 date cutoff"}
        ],
        "responses": {
            "200": {"description": "Number of entries deleted"}
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
git commit -m "feat(server): add audit endpoints to OpenAPI spec"
```

---

### Task 8: Clippy and Build Verification

**Step 1:** `cargo build --features server`
**Step 2:** `cargo clippy --features server -- -D warnings`
**Step 3:** `cargo test --features server`
**Step 4:** `cargo build && cargo test` (default features)

All must pass. Fix any issues.

---

### Task 9: Documentation Updates

**Files:**
- Modify: `CLAUDE.md` — bump schema v47 -> v48
- Modify: `.claude/rules/db.md` — bump schema v47 -> v48, add AuditEntry type
- Modify: `.claude/rules/server.md` — add rate_limit.rs, audit.rs, audit endpoints

**Commit:**
```bash
git commit -m "docs: update project docs for P2 admin API (rate limiting + audit log)"
```
