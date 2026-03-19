# Canonical Registry Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Remi the single source of truth for canonical package mappings. Wire up Repology + AppStream data sources, add a 4-phase rebuild pipeline, and have clients fetch the canonical map from Remi instead of bundling local YAML rules.

**Architecture:** Two server-side cycles (daily external fetch + post-sync rebuild) feed a 4-phase pipeline (curated rules > Repology > AppStream > auto-discovery). Clients pull the finished map via `GET /v1/canonical/map` with ETag caching.

**Tech Stack:** Rust 1.94, rusqlite, axum, reqwest, tokio, serde, conary-core/conary-server workspace

**Spec:** `docs/superpowers/specs/2026-03-19-canonical-registry-redesign.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `conary-core/src/db/schema.rs` | Modify | Bump SCHEMA_VERSION to 53 |
| `conary-core/src/db/migrations.rs` | Modify | Add `migrate_v53()` with 4 new tables |
| `conary-core/src/db/models/metadata.rs` | Create | `ServerMetadata` and `ClientMetadata` models |
| `conary-core/src/db/models/repology_cache.rs` | Create | `RepologyCacheEntry` model |
| `conary-core/src/db/models/appstream_cache.rs` | Create | `AppstreamCacheEntry` model |
| `conary-core/src/db/models/mod.rs` | Modify | Register + re-export new models |
| `conary-core/src/canonical/repology.rs` | Modify | Add cache write/read methods |
| `conary-core/src/canonical/appstream.rs` | Modify | Add fetch-from-URL + cache write/read |
| `conary-core/src/canonical/client.rs` | Create | HTTP fetch + ETag + local DB ingestion |
| `conary-core/src/canonical/mod.rs` | Modify | Register `client` module |
| `conary-server/src/server/config.rs` | Modify | Add `CanonicalSection` to `RemiConfig` |
| `conary-server/src/server/canonical_job.rs` | Rewrite | 4-phase pipeline + debounce |
| `conary-server/src/server/canonical_fetch.rs` | Create | Daily background Repology + AppStream fetch |
| `conary-server/src/server/handlers/canonical.rs` | Modify | ETag support on `canonical_map()` |
| `conary-server/src/server/mod.rs` | Modify | Register module, spawn background job |
| `conary-server/src/server/mcp.rs` | Modify | Add `canonical_fetch` tool, update `canonical_rebuild` |
| `src/commands/registry.rs` | Rewrite | Remi fetch first, local YAML fallback |
| `src/commands/repo.rs` | Modify | Fetch canonical map after sync |

---

### Task 1: Schema Migration v53

**Files:**
- Modify: `conary-core/src/db/schema.rs:14` and `:138`
- Modify: `conary-core/src/db/migrations.rs` (append after `migrate_v52`)

- [ ] **Step 1: Write the migration test**

Add to `conary-core/src/db/migrations.rs` test module:

```rust
#[test]
fn test_migrate_v53_cache_tables() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    // Verify repology_cache
    conn.execute(
        "INSERT INTO repology_cache (project_name, distro, distro_name, version, status, fetched_at)
         VALUES ('python', 'arch', 'python', '3.12.0', 'newest', '2026-03-19')",
        [],
    ).unwrap();

    // Verify appstream_cache
    conn.execute(
        "INSERT INTO appstream_cache (appstream_id, pkgname, display_name, summary, distro, fetched_at)
         VALUES ('org.mozilla.firefox', 'firefox', 'Firefox', 'Web Browser', 'fedora', '2026-03-19')",
        [],
    ).unwrap();

    // Verify server_metadata seeded
    let version: String = conn.query_row(
        "SELECT value FROM server_metadata WHERE key = 'canonical_map_version'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(version, "0");

    // Verify client_metadata
    conn.execute(
        "INSERT INTO client_metadata (key, value) VALUES ('etag', 'test')",
        [],
    ).unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_migrate_v53 -- --nocapture`
Expected: FAIL — `migrate_v53` doesn't exist

- [ ] **Step 3: Write the migration**

In `conary-core/src/db/migrations.rs`, add after `migrate_v52`:

```rust
pub fn migrate_v53(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS repology_cache (
            project_name TEXT NOT NULL,
            distro TEXT NOT NULL,
            distro_name TEXT NOT NULL,
            version TEXT,
            status TEXT,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (project_name, distro)
        );

        CREATE TABLE IF NOT EXISTS appstream_cache (
            appstream_id TEXT NOT NULL,
            pkgname TEXT NOT NULL,
            display_name TEXT,
            summary TEXT,
            distro TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (appstream_id, distro)
        );

        CREATE TABLE IF NOT EXISTS server_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        INSERT OR IGNORE INTO server_metadata (key, value)
            VALUES ('canonical_map_version', '0');

        CREATE TABLE IF NOT EXISTS client_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;

    info!("Schema version 53 applied successfully (canonical cache tables, metadata)");
    Ok(())
}
```

In `conary-core/src/db/schema.rs`:
- Line 14: change `52` to `53`
- Line 138: add `53 => migrations::migrate_v53(conn),` before the `_` arm

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p conary-core test_migrate_v53 -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/db/schema.rs conary-core/src/db/migrations.rs
git commit -m "feat(db): add schema v53 with canonical cache tables and metadata"
```

---

### Task 2: DB Models for Cache and Metadata Tables

**Files:**
- Create: `conary-core/src/db/models/metadata.rs`
- Create: `conary-core/src/db/models/repology_cache.rs`
- Create: `conary-core/src/db/models/appstream_cache.rs`
- Modify: `conary-core/src/db/models/mod.rs`

- [ ] **Step 1: Create `metadata.rs`**

```rust
// conary-core/src/db/models/metadata.rs

use rusqlite::{Connection, OptionalExtension};

/// Get a value from server_metadata (or client_metadata — same schema).
pub fn get_metadata(conn: &Connection, table: &str, key: &str) -> rusqlite::Result<Option<String>> {
    let sql = format!("SELECT value FROM {table} WHERE key = ?1");
    conn.query_row(&sql, [key], |row| row.get(0))
        .optional()
}

/// Set a value in server_metadata or client_metadata (upsert).
pub fn set_metadata(conn: &Connection, table: &str, key: &str, value: &str) -> rusqlite::Result<()> {
    let sql = format!("INSERT OR REPLACE INTO {table} (key, value) VALUES (?1, ?2)");
    conn.execute(&sql, rusqlite::params![key, value])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn test_metadata_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        assert_eq!(get_metadata(&conn, "server_metadata", "missing").unwrap(), None);

        set_metadata(&conn, "server_metadata", "test_key", "test_value").unwrap();
        assert_eq!(
            get_metadata(&conn, "server_metadata", "test_key").unwrap(),
            Some("test_value".to_string())
        );

        // Upsert overwrites
        set_metadata(&conn, "server_metadata", "test_key", "new_value").unwrap();
        assert_eq!(
            get_metadata(&conn, "server_metadata", "test_key").unwrap(),
            Some("new_value".to_string())
        );
    }

    #[test]
    fn test_client_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        set_metadata(&conn, "client_metadata", "etag", "W/\"v5\"").unwrap();
        assert_eq!(
            get_metadata(&conn, "client_metadata", "etag").unwrap(),
            Some("W/\"v5\"".to_string())
        );
    }
}
```

- [ ] **Step 2: Create `repology_cache.rs`**

```rust
// conary-core/src/db/models/repology_cache.rs

use rusqlite::{Connection, params};

/// A cached Repology project → distro mapping.
#[derive(Debug, Clone)]
pub struct RepologyCacheEntry {
    pub project_name: String,
    pub distro: String,
    pub distro_name: String,
    pub version: Option<String>,
    pub status: Option<String>,
    pub fetched_at: String,
}

impl RepologyCacheEntry {
    pub fn insert_or_replace(conn: &Connection, entry: &RepologyCacheEntry) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO repology_cache
             (project_name, distro, distro_name, version, status, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.project_name,
                entry.distro,
                entry.distro_name,
                entry.version,
                entry.status,
                entry.fetched_at,
            ],
        )?;
        Ok(())
    }

    /// Read all cache entries, optionally filtered by acceptable statuses.
    pub fn find_all(conn: &Connection) -> rusqlite::Result<Vec<RepologyCacheEntry>> {
        let mut stmt = conn.prepare(
            "SELECT project_name, distro, distro_name, version, status, fetched_at
             FROM repology_cache",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RepologyCacheEntry {
                project_name: row.get(0)?,
                distro: row.get(1)?,
                distro_name: row.get(2)?,
                version: row.get(3)?,
                status: row.get(4)?,
                fetched_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Clear all cache entries.
    pub fn clear_all(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute("DELETE FROM repology_cache", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn test_repology_cache_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let entry = RepologyCacheEntry {
            project_name: "python".into(),
            distro: "arch".into(),
            distro_name: "python".into(),
            version: Some("3.12.0".into()),
            status: Some("newest".into()),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        };
        RepologyCacheEntry::insert_or_replace(&conn, &entry).unwrap();

        let all = RepologyCacheEntry::find_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].project_name, "python");
        assert_eq!(all[0].distro_name, "python");
    }
}
```

- [ ] **Step 3: Create `appstream_cache.rs`**

```rust
// conary-core/src/db/models/appstream_cache.rs

use rusqlite::{Connection, params};

/// A cached AppStream component.
#[derive(Debug, Clone)]
pub struct AppstreamCacheEntry {
    pub appstream_id: String,
    pub pkgname: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub distro: String,
    pub fetched_at: String,
}

impl AppstreamCacheEntry {
    pub fn insert_or_replace(conn: &Connection, entry: &AppstreamCacheEntry) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO appstream_cache
             (appstream_id, pkgname, display_name, summary, distro, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.appstream_id,
                entry.pkgname,
                entry.display_name,
                entry.summary,
                entry.distro,
                entry.fetched_at,
            ],
        )?;
        Ok(())
    }

    pub fn find_all(conn: &Connection) -> rusqlite::Result<Vec<AppstreamCacheEntry>> {
        let mut stmt = conn.prepare(
            "SELECT appstream_id, pkgname, display_name, summary, distro, fetched_at
             FROM appstream_cache",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AppstreamCacheEntry {
                appstream_id: row.get(0)?,
                pkgname: row.get(1)?,
                display_name: row.get(2)?,
                summary: row.get(3)?,
                distro: row.get(4)?,
                fetched_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn clear_all(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute("DELETE FROM appstream_cache", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn test_appstream_cache_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let entry = AppstreamCacheEntry {
            appstream_id: "org.mozilla.firefox".into(),
            pkgname: "firefox".into(),
            display_name: Some("Firefox".into()),
            summary: Some("Web Browser".into()),
            distro: "fedora".into(),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        };
        AppstreamCacheEntry::insert_or_replace(&conn, &entry).unwrap();

        let all = AppstreamCacheEntry::find_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].pkgname, "firefox");
    }
}
```

- [ ] **Step 4: Register models in `mod.rs`**

In `conary-core/src/db/models/mod.rs`, add to the module declarations (around line 24):

```rust
mod appstream_cache;
mod metadata;
mod repology_cache;
```

And to the re-exports (around line 59):

```rust
pub use appstream_cache::AppstreamCacheEntry;
pub use metadata::{get_metadata, set_metadata};
pub use repology_cache::RepologyCacheEntry;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p conary-core metadata repology_cache appstream_cache -- --nocapture`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/db/models/
git commit -m "feat(db): add models for repology_cache, appstream_cache, and metadata tables"
```

---

### Task 3: Repology Cache Write/Read Methods

**Files:**
- Modify: `conary-core/src/canonical/repology.rs`

- [ ] **Step 1: Write test for cache persistence**

Add to existing test module in `repology.rs`:

```rust
#[test]
fn test_cache_repology_projects() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();

    let projects = vec![RepologyProject {
        name: "python".into(),
        implementations: vec![
            RepologyImplementation {
                repo: "fedora_43".into(),
                visiblename: "python3".into(),
                version: "3.12.0".into(),
                status: "newest".into(),
            },
            RepologyImplementation {
                repo: "arch".into(),
                visiblename: "python".into(),
                version: "3.12.0".into(),
                status: "newest".into(),
            },
        ],
    }];

    let count = cache_projects_to_db(&conn, &projects).unwrap();
    assert_eq!(count, 2);

    let entries = crate::db::models::RepologyCacheEntry::find_all(&conn).unwrap();
    assert_eq!(entries.len(), 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_cache_repology -- --nocapture`
Expected: FAIL — `cache_projects_to_db` not defined

- [ ] **Step 3: Update User-Agent constant**

In `repology.rs` line 175, change:

```rust
const USER_AGENT: &str = "conary/0.6.0 (https://conary.io; canonical-registry-sync)";
```

This satisfies Repology's requirement for a descriptive User-Agent with contact URL. Repology blocks non-compliant clients.

- [ ] **Step 4: Implement `cache_projects_to_db`**

Add to `repology.rs` after the `sync_to_db` method:

```rust
/// Write a batch of Repology projects to the `repology_cache` table.
/// Maps Repology repo IDs to Conary distro names, skipping unrecognised repos.
/// Returns the number of cache entries written.
pub fn cache_projects_to_db(conn: &Connection, projects: &[RepologyProject]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut count = 0;

    for project in projects {
        for imp in &project.implementations {
            let Some(distro) = repo_to_distro(&imp.repo) else {
                continue;
            };
            let entry = crate::db::models::RepologyCacheEntry {
                project_name: project.name.clone(),
                distro,
                distro_name: imp.visiblename.clone(),
                version: Some(imp.version.clone()),
                status: Some(imp.status.clone()),
                fetched_at: now.clone(),
            };
            crate::db::models::RepologyCacheEntry::insert_or_replace(&tx, &entry)?;
            count += 1;
        }
    }

    tx.commit()?;
    Ok(count)
}
```

Add `use rusqlite::Connection;` to imports if not already present.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p conary-core test_cache_repology -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/canonical/repology.rs
git commit -m "feat(canonical): add Repology cache persistence to repology_cache table"
```

---

### Task 4: AppStream Fetch-from-URL and Cache Methods

**Files:**
- Modify: `conary-core/src/canonical/appstream.rs`

- [ ] **Step 1: Write test for AppStream cache persistence**

Add to existing test module in `appstream.rs`:

```rust
#[test]
fn test_cache_appstream_components() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();

    let components = vec![
        AppStreamComponent {
            id: "org.mozilla.firefox".into(),
            pkgname: "firefox".into(),
            name: "Firefox".into(),
            summary: "Web Browser".into(),
        },
    ];

    let count = cache_components_to_db(&conn, &components, "fedora").unwrap();
    assert_eq!(count, 1);

    let entries = crate::db::models::AppstreamCacheEntry::find_all(&conn).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].pkgname, "firefox");
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p conary-core test_cache_appstream -- --nocapture`

- [ ] **Step 3: Implement `cache_components_to_db`**

Add to `appstream.rs`:

```rust
/// Write parsed AppStream components to the appstream_cache table.
/// Write parsed AppStream components to the appstream_cache table.
/// `pkgname` is always present (components without it are dropped at parse time).
pub fn cache_components_to_db(
    conn: &rusqlite::Connection,
    components: &[AppStreamComponent],
    distro: &str,
) -> crate::error::Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut count = 0;

    for component in components {
        let entry = crate::db::models::AppstreamCacheEntry {
            appstream_id: component.id.clone(),
            pkgname: component.pkgname.clone(),
            display_name: Some(component.name.clone()),
            summary: Some(component.summary.clone()),
            distro: distro.to_string(),
            fetched_at: now.clone(),
        };
        crate::db::models::AppstreamCacheEntry::insert_or_replace(&tx, &entry)?;
        count += 1;
    }

    tx.commit()?;
    Ok(count)
}
```

- [ ] **Step 4: Run test, verify passes**

Run: `cargo test -p conary-core test_cache_appstream -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/canonical/appstream.rs
git commit -m "feat(canonical): add AppStream cache persistence to appstream_cache table"
```

---

### Task 5: Canonical Client Module (HTTP Fetch + ETag + Ingestion)

**Files:**
- Create: `conary-core/src/canonical/client.rs`
- Modify: `conary-core/src/canonical/mod.rs`

- [ ] **Step 1: Write test for map ingestion (no HTTP, just the DB part)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingest_canonical_map_response() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();

        let json = r#"{
            "version": 5,
            "generated_at": "2026-03-19T00:00:00Z",
            "entries": [
                {
                    "canonical": "python",
                    "implementations": {"fedora": "python3", "arch": "python"}
                },
                {
                    "canonical": "curl",
                    "implementations": {"fedora": "curl", "arch": "curl", "ubuntu": "curl"}
                }
            ]
        }"#;

        let count = ingest_canonical_map_json(&conn, json).unwrap();
        assert_eq!(count, 2);

        let pkg = crate::db::models::CanonicalPackage::find_by_name(&conn, "python").unwrap();
        assert!(pkg.is_some());
    }
}
```

- [ ] **Step 2: Run test, verify fails**

Run: `cargo test -p conary-core test_ingest_canonical_map -- --nocapture`

- [ ] **Step 3: Implement `client.rs`**

```rust
// conary-core/src/canonical/client.rs

//! Client-side canonical map fetching from Remi.
//!
//! Uses `reqwest::blocking` for CLI commands. Server-side callers must wrap
//! in `tokio::task::spawn_blocking` if called from async context.

use crate::db::models::{CanonicalPackage, PackageImplementation, get_metadata, set_metadata};
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
struct CanonicalMapResponse {
    version: u32,
    #[allow(dead_code)]
    generated_at: String,
    entries: Vec<CanonicalMapEntry>,
}

#[derive(Debug, Deserialize)]
struct CanonicalMapEntry {
    canonical: String,
    implementations: BTreeMap<String, String>,
}

/// Fetch the canonical map from a Remi endpoint.
/// Returns Ok(Some(count)) if new data was fetched, Ok(None) if 304, Err on failure.
pub fn fetch_canonical_map(conn: &Connection, endpoint: &str) -> Result<Option<usize>> {
    let url = format!("{}/v1/canonical/map", endpoint.trim_end_matches('/'));
    let etag = get_metadata(conn, "client_metadata", "canonical_etag")
        .unwrap_or(None);

    let client = reqwest::blocking::Client::builder()
        .user_agent("conary/0.6.0 (https://conary.io)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| Error::DownloadError(e.to_string()))?;

    let mut request = client.get(&url);
    if let Some(ref etag_val) = etag {
        request = request.header("If-None-Match", etag_val.as_str());
    }

    let response = request.send().map_err(|e| Error::DownloadError(e.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }

    if !response.status().is_success() {
        return Err(Error::DownloadError(format!(
            "canonical map fetch failed: HTTP {}",
            response.status()
        )));
    }

    let new_etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let body = response.text().map_err(|e| Error::DownloadError(e.to_string()))?;
    let count = ingest_canonical_map_json(conn, &body)?;

    if let Some(etag_val) = new_etag {
        let _ = set_metadata(conn, "client_metadata", "canonical_etag", &etag_val);
    }

    Ok(Some(count))
}

/// Parse a canonical map JSON response and replace the local canonical DB.
pub fn ingest_canonical_map_json(conn: &Connection, json: &str) -> Result<usize> {
    let map: CanonicalMapResponse =
        serde_json::from_str(json).map_err(|e| Error::ParseError(e.to_string()))?;

    let tx = conn.unchecked_transaction()?;

    // Full replace — clear existing data
    tx.execute("DELETE FROM package_implementations", [])?;
    tx.execute("DELETE FROM canonical_packages", [])?;

    let mut count = 0;
    for entry in &map.entries {
        let mut canonical = CanonicalPackage::new(entry.canonical.clone(), "package".to_string());
        let id = canonical.insert_or_ignore(&tx)?;
        let canonical_id = match id {
            Some(cid) => cid,
            None => continue,
        };

        for (distro, distro_name) in &entry.implementations {
            let mut imp = PackageImplementation::new(
                canonical_id,
                distro.clone(),
                distro_name.clone(),
                "server".to_string(),
            );
            imp.insert_or_ignore(&tx)?;
        }
        count += 1;
    }

    tx.commit()?;
    Ok(count)
}
```

- [ ] **Step 4: Register in `canonical/mod.rs`**

Add `pub mod client;` to `conary-core/src/canonical/mod.rs`.

- [ ] **Step 5: Run test, verify passes**

Run: `cargo test -p conary-core test_ingest_canonical_map -- --nocapture`

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/canonical/client.rs conary-core/src/canonical/mod.rs
git commit -m "feat(canonical): add client module for fetching canonical map from Remi"
```

---

### Task 6: Server Config — Add `[canonical]` Section

**Files:**
- Modify: `conary-server/src/server/config.rs`

- [ ] **Step 1: Add `CanonicalSection` struct and register in `RemiConfig`**

In `config.rs`, add the struct:

```rust
/// Canonical registry settings
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CanonicalSection {
    /// Hours between Repology/AppStream fetch cycles (default: 24)
    pub fetch_interval_hours: u64,
    /// Minutes of cooldown between rebuilds (default: 5)
    pub rebuild_cooldown_minutes: u64,
    /// Max Repology projects to fetch per cycle (default: 5000)
    pub repology_batch_size: usize,
    /// Path to curated rules directory
    pub rules_dir: String,
}

impl Default for CanonicalSection {
    fn default() -> Self {
        Self {
            fetch_interval_hours: 24,
            rebuild_cooldown_minutes: 5,
            repology_batch_size: 5000,
            rules_dir: "/usr/share/conary/canonical-rules".to_string(),
        }
    }
}
```

In `RemiConfig` struct (around line 69), add:

```rust
    /// Canonical registry settings
    #[serde(default)]
    pub canonical: CanonicalSection,
```

- [ ] **Step 2: Build to verify no compilation errors**

Run: `cargo build --features server`

- [ ] **Step 3: Commit**

```bash
git add conary-server/src/server/config.rs
git commit -m "feat(server): add [canonical] config section to RemiConfig"
```

---

### Task 7: Rewrite `canonical_job.rs` — 4-Phase Pipeline + Debounce

**Files:**
- Rewrite: `conary-server/src/server/canonical_job.rs`
- Modify: `conary-core/src/canonical/sync.rs` (extract Phase 4 auto-discovery into a standalone function callable from the pipeline, e.g., `run_auto_discovery(conn, skip_existing: bool)`)

- [ ] **Step 1: Write test for debounce logic**

```rust
#[test]
fn test_should_rebuild_respects_cooldown() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();

    // No previous rebuild — should proceed
    assert!(should_rebuild(&conn, 5));

    // Record a rebuild
    record_rebuild_timestamp(&conn).unwrap();

    // Immediately after — should skip (within 5 min cooldown)
    assert!(!should_rebuild(&conn, 5));
}
```

- [ ] **Step 2: Run test, verify fails**

Run: `cargo test -p conary-server test_should_rebuild -- --nocapture`

- [ ] **Step 3: Implement the rewritten `canonical_job.rs`**

Full rewrite with:
- `should_rebuild(conn, cooldown_minutes) -> bool` — checks `server_metadata.last_canonical_rebuild`
- `record_rebuild_timestamp(conn)` — writes current time
- `bump_map_version(conn)` — increments `canonical_map_version`
- `rebuild_canonical_map(db_path, config) -> Result<u64>` — returns count of new mappings. 4-phase pipeline:
  - Phase 1: load curated rules from `config.rules_dir`, insert with `source='curated'`
  - Phase 2: read `repology_cache`, group by project, insert with `source='repology'` (filter by acceptable status)
  - Phase 3: read `appstream_cache`, enrich `appstream_id` on existing entries, insert new with `source='appstream'`
  - Phase 4: call existing `build_repo_package_list` + `ingest_canonical_mappings` for auto-discovery
  - Bump version counter after all phases

Each phase commits independently using `INSERT OR IGNORE` for first-match-wins.

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-server canonical_job -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git add conary-server/src/server/canonical_job.rs
git commit -m "refactor(server): rewrite canonical_job with 4-phase pipeline and debounce"
```

---

### Task 8: Wire Post-Sync Rebuild on Remi Server

**Files:**
- Modify: `conary-server/src/server/admin_service.rs`

This is one of the two core server-side cycles described in the spec: after a successful repo sync on Remi, trigger a canonical rebuild with debounce.

- [ ] **Step 1: Add rebuild call after successful repo sync**

In `admin_service.rs`, after `sync_repository` succeeds, add:

```rust
// Trigger canonical rebuild if cooldown has elapsed
if canonical_job::should_rebuild(&conn, config.canonical.rebuild_cooldown_minutes) {
    match canonical_job::rebuild_canonical_map(&db_path, &config.canonical) {
        Ok(result) => tracing::info!("Post-sync canonical rebuild: {} new mappings", result),
        Err(e) => tracing::warn!("Post-sync canonical rebuild failed: {}", e),
    }
}
```

This uses the debounce functions from Task 7 (`should_rebuild`, `record_rebuild_timestamp`).

- [ ] **Step 2: Build**

Run: `cargo build --features server`

- [ ] **Step 3: Commit**

```bash
git add conary-server/src/server/admin_service.rs
git commit -m "feat(server): trigger canonical rebuild after repo sync with debounce"
```

---

### Task 9: ETag Support on `/v1/canonical/map`


**Files:**
- Modify: `conary-server/src/server/handlers/canonical.rs`

- [ ] **Step 1: Modify `canonical_map()` to read version from DB and add ETag header**

Replace the hardcoded `version: 1` with a DB read from `server_metadata`. Add `If-None-Match` check returning 304 when matched. Add `ETag` response header.

Key changes:
- Read `canonical_map_version` from `server_metadata` table
- Set `CanonicalMapResponse.version` from the DB value
- Compute `ETag` as `W/"v{version}"`
- Check incoming `If-None-Match` header, return 304 if matched
- Add `ETag` header to 200 response

- [ ] **Step 2: Build and run existing tests**

Run: `cargo test -p conary-server canonical -- --nocapture`

- [ ] **Step 3: Commit**

```bash
git add conary-server/src/server/handlers/canonical.rs
git commit -m "feat(server): add ETag support to canonical map endpoint"
```

---

### Task 10: Daily Background Fetch Job (`canonical_fetch.rs`)

**Files:**
- Create: `conary-server/src/server/canonical_fetch.rs`
- Modify: `conary-server/src/server/mod.rs`

- [ ] **Step 1: Create `canonical_fetch.rs`**

Implement:
- `fetch_repology_data(db_path, batch_size) -> Result<usize>` — uses existing `RepologyClient` to paginate through projects, calls `cache_projects_to_db`. Sleeps 1 second between API calls.
- `fetch_appstream_data(db_path) -> Result<usize>` — fetches Fedora (via repomd.xml parsing) and Ubuntu (direct DEP-11 URL) catalogs, decompresses, parses, calls `cache_components_to_db`.
- `spawn_canonical_fetch_loop(config, db_path)` — starts the tokio interval loop (60s initial delay, then `fetch_interval_hours` interval). Calls fetch functions then triggers rebuild.

- [ ] **Step 2: Register in `mod.rs` and spawn at server boot**

In `conary-server/src/server/mod.rs`:
- Add `pub mod canonical_fetch;`
- In `run_server_from_config()`, after the metadata refresh spawn block, add spawn for the canonical fetch loop

- [ ] **Step 3: Build**

Run: `cargo build --features server`

- [ ] **Step 4: Commit**

```bash
git add conary-server/src/server/canonical_fetch.rs conary-server/src/server/mod.rs
git commit -m "feat(server): add daily canonical fetch background job (Repology + AppStream)"
```

---

### Task 11: MCP Tools Update

**Files:**
- Modify: `conary-server/src/server/mcp.rs`

- [ ] **Step 1: Update `canonical_rebuild` to use config-driven rules dir**

Change the hardcoded `"data/canonical-rules"` to read from the `CanonicalSection` config.

- [ ] **Step 2: Add `canonical_fetch` MCP tool**

```rust
#[tool(description = "Trigger immediate Repology + AppStream fetch cycle. Populates the cache used by canonical_rebuild.")]
async fn canonical_fetch(&self) -> Result<CallToolResult, McpError> {
    // spawn_blocking to call fetch_repology_data + fetch_appstream_data
}
```

- [ ] **Step 3: Build**

Run: `cargo build --features server`

- [ ] **Step 4: Commit**

```bash
git add conary-server/src/server/mcp.rs
git commit -m "feat(server): add canonical_fetch MCP tool, update canonical_rebuild config"
```

---

### Task 12: Client-Side `registry update` — Remi Fetch with Local Fallback

**Files:**
- Rewrite: `src/commands/registry.rs`

- [ ] **Step 1: Write test for fallback behavior**

```rust
#[test]
fn test_registry_update_falls_back_to_local() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db_str = db_path.to_str().unwrap();
    conary_core::db::init(db_str).unwrap();

    // No repos configured, should fall back to local files (and succeed)
    let result = cmd_registry_update(db_str);
    assert!(result.is_ok());
}
```

- [ ] **Step 2: Rewrite `cmd_registry_update`**

New flow:
1. Open DB, list repos by priority
2. For each repo: derive Remi endpoint (strip path, append `/v1/canonical/map`)
3. Call `conary_core::canonical::client::fetch_canonical_map()`
4. On success: print count, return
5. On all failures: fall back to local YAML rules (existing logic)

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary registry -- --nocapture`

- [ ] **Step 4: Commit**

```bash
git add src/commands/registry.rs
git commit -m "feat(cli): registry update fetches from Remi, falls back to local YAML"
```

---

### Task 13: Client-Side `repo sync` — Auto-Fetch Canonical Map

**Files:**
- Modify: `src/commands/repo.rs`

- [ ] **Step 1: Add canonical map fetch after sync completes**

In `cmd_repo_sync()`, after the sync loop succeeds (around line 270, before the failure check), add:

```rust
// Best-effort canonical map sync from Remi
if let Some(repo) = repos.first() {
    let endpoint = derive_remi_endpoint(&repo.url);
    match conary_core::canonical::client::fetch_canonical_map(&conn, &endpoint) {
        Ok(Some(n)) => tracing::info!("Canonical map updated: {n} entries"),
        Ok(None) => tracing::debug!("Canonical map is current (304)"),
        Err(e) => tracing::debug!("Canonical map fetch skipped: {e}"),
    }
}
```

Add a helper `derive_remi_endpoint(url: &str) -> String` that strips the path and returns the base URL.

- [ ] **Step 2: Build and test**

Run: `cargo build && cargo test repo_sync -- --nocapture`

- [ ] **Step 3: Commit**

```bash
git add src/commands/repo.rs
git commit -m "feat(cli): auto-fetch canonical map from Remi after repo sync"
```

---

### Task 14: Integration Test — Cross-Distro Canonical Mapping

**Files:**
- Modify: `tests/integration/remi/manifests/phase4-group-e.toml` (already exists with T230-T249)

- [ ] **Step 1: Update T233 to test server-fetched canonical data**

T233 currently tests `registry update` with local rules. Replace the `registry update` step with a `repo sync` that automatically fetches the canonical map from Remi, then verify `canonical show` works. The test already exists — this modifies its steps.

- [ ] **Step 2: Sync manifest to Forge and run**

```bash
rsync -az tests/integration/remi/manifests/phase4-group-e.toml peter@forge.conarylabs.com:~/Conary/tests/integration/remi/manifests/phase4-group-e.toml
```

- [ ] **Step 3: Commit**

```bash
git add tests/integration/remi/manifests/phase4-group-e.toml
git commit -m "test(integration): update T233 to use server-fetched canonical data"
```

---

### Task 15: Final Build + Clippy + Full Test Suite

- [ ] **Step 1: Build both profiles**

```bash
cargo build
cargo build --features server
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -- -D warnings
cargo clippy --features server -- -D warnings
```

- [ ] **Step 3: Run unit tests**

```bash
cargo test
cargo test --features server
```

- [ ] **Step 4: Fix any issues**

- [ ] **Step 5: Final commit if needed**

```bash
git add -A
git commit -m "chore: fix clippy warnings and test issues from canonical redesign"
```
