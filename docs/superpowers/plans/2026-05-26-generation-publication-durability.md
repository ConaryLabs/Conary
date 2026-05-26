# Generation Publication Durability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Plan B from the preview invariant hardening milestone: committed package DB state must leave durable, queryable publication debt, and successful generation publication must flush current DB state into a durably selected generation.

**Architecture:** Add a DB-backed `generation_publications` debt ledger in `conary-core`, then route install, batch install, remove, manual publish, and recovery through one publication contract. B1 owns publication semantics, `system generation publish`, status/history visibility, and durable `/conary/current` parent sync; B2 broadens the durable filesystem helper to generation metadata, operation records, GC protection, and live-root journal-adjacent writes.

**Tech Stack:** Rust, rusqlite migrations/models, clap subcommands, existing `TransactionEngine` lock, composefs/EROFS generation builder, tempfile-based unit/integration tests, docs-audit scripts.

---

## Scope

This plan implements `docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md`.

It is split into two implementation slices:

- **B1:** DB publication debt model, publication helper, package-mutation integration, parameterless `system generation publish`, pending-status surfaces, recovery guardrails, and durable `/conary/current` sync.
- **B2:** shared durable filesystem helper sweep, generation metadata/signature/pending-marker durability, operation-record durability, live-root parent sync, and generation GC coordination.

The plan intentionally does not add daemon-side automatic retries or a new dedicated conaryd publication endpoint. Existing conaryd history/status surfaces must not falsely report publication-pending changesets as fully published.

## Review-Tightened Decisions

- `generation_publications` is a **publication debt ledger**, not a queue of independent historical changeset rebuilds.
- Generation publication builds from current DB state (`Trove::list_all(conn)`). It never tries to replay only one historical changeset.
- The primary retry command is `conary system generation publish`. Optional `--changeset <id>` is an assertion/filter over pending debt, not a request to build that exact historical state.
- A successful current-DB publication marks every covered pending recoverable debt complete.
- B1 must make `/conary/current` parent-directory sync durable. The `CurrentPublished` phase cannot be deferred to B2.
- Package-mutation publication must build an inactive generation, durably select it with `/conary/current`, then mark the matching DB state active.
- Post-commit publication failure keeps exit code `0` because the package DB mutation committed, but it must leave machine-readable `needs_publication` state.
- The implementation plan must stage exact paths only. Do not `git add docs apps crates` or other broad directories.

## File Structure

- Create `crates/conary-core/src/db/models/generation_publication.rs`: typed publication phase/status enums and CRUD/query helpers for the `generation_publications` table.
- Modify `crates/conary-core/src/db/models/mod.rs`: export `GenerationPublication`, `GenerationPublicationPhase`, and `GenerationPublicationStatus`.
- Modify `crates/conary-core/src/db/migrations/v41_current.rs`: add schema v69 migration and tests for the publication table.
- Modify `crates/conary-core/src/db/schema.rs`: bump `SCHEMA_VERSION` and add `migrate_v69`.
- Create `crates/conary-core/src/filesystem/durable.rs`: durable parent-directory sync and atomic filesystem helpers.
- Modify `crates/conary-core/src/filesystem/mod.rs`: export durable helper functions.
- Modify `crates/conary-core/src/generation/mount.rs`: make `update_current_symlink` fsync the `/conary` parent directory after rename.
- Modify `apps/conary/src/commands/composefs_ops.rs`: split current generation build/publish work so package mutation uses inactive state creation and DB active marking happens after durable current selection.
- Create `apps/conary/src/commands/generation/publication.rs`: command-level publication helper that creates/updates debt rows, publishes current DB state, marks covered debts complete, and exposes CLI status formatting helpers.
- Modify `apps/conary/src/commands/generation/mod.rs`: export `publication`.
- Modify `apps/conary/src/commands/generation/commands.rs`: add `cmd_generation_publish` and `cmd_generation_pending`, and wire recovery/status behavior.
- Modify `apps/conary/src/cli/generation.rs`: add `Publish` and `Pending` subcommands.
- Modify `apps/conary/src/command_risk.rs` and `apps/conary/src/dispatch.rs`: classify `system generation publish` as live-mutation gated, classify `system generation pending` as read-only, and dispatch both subcommands.
- Modify `apps/conary/src/commands/install/mod.rs`, `apps/conary/src/commands/install/batch.rs`, and `apps/conary/src/commands/remove.rs`: replace stringly deferred rebuild handling with the shared publication helper.
- Modify `apps/conary/src/commands/changeset_metadata.rs`: type or validate generation deferred-follow-up metadata and suppress legacy `system generation build` retry hints.
- Modify `apps/conary/src/commands/query/history.rs`: mark changesets with pending/failed publication debt.
- Modify `crates/conary-core/src/transaction/recovery.rs`: check publication debt before accepting `/conary/current` as fully recovered.
- Modify `apps/conary/src/commands/generation/commands.rs`: protect incomplete publication generations in GC.
- Modify `apps/conaryd/src/daemon/routes.rs`, `apps/conaryd/src/daemon/routes/query.rs`, `apps/conaryd/src/daemon/routes/system.rs`, and `apps/conaryd/src/daemon/routes/transactions.rs`: add `publication_status` to changeset/transaction JSON responses and `pending_publications` to system summary/state responses so existing surfaces expose publication debt instead of implying full publication.
- Modify `crates/conary-core/src/generation/metadata.rs`, `apps/conary/src/commands/operation_records.rs`, and `apps/conary/src/commands/live_root.rs` during B2 for durable filesystem coverage.
- Modify docs-audit metadata if this plan changes active docs.

---

### Task 1: Publication Debt Schema And Model

**Files:**
- Create: `crates/conary-core/src/db/models/generation_publication.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`
- Modify: `crates/conary-core/src/db/schema.rs`

- [ ] **Step 1: Write model parsing and query tests first**

Add the new model file with the path comment, imports, enum definitions, and tests before wiring it into the build:

```rust
// conary-core/src/db/models/generation_publication.rs

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::str::FromStr;
use strum_macros::{AsRefStr, Display, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum GenerationPublicationPhase {
    PendingBuild,
    Building,
    ArtifactReady,
    CurrentPublished,
    ActiveMarked,
}

impl GenerationPublicationPhase {
    pub fn as_str(self) -> &'static str {
        self.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum GenerationPublicationStatus {
    Pending,
    Running,
    Failed,
    Complete,
    Abandoned,
}

impl GenerationPublicationStatus {
    pub fn as_str(self) -> &'static str {
        self.as_ref()
    }

    pub fn is_recoverable(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationPublication {
    pub id: Option<i64>,
    pub trigger_changeset_id: Option<i64>,
    pub published_through_changeset_id: Option<i64>,
    pub tx_uuid: Option<String>,
    pub db_path: String,
    pub runtime_root: String,
    pub phase: GenerationPublicationPhase,
    pub status: GenerationPublicationStatus,
    pub state_number: Option<i64>,
    pub generation_number: Option<i64>,
    pub summary: String,
    pub last_error: Option<String>,
    pub retry_count: i64,
    pub recoverable: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub completed_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_and_status_reject_unknown_values() {
        assert_eq!(
            GenerationPublicationPhase::from_str("artifact_ready").unwrap(),
            GenerationPublicationPhase::ArtifactReady
        );
        assert!(GenerationPublicationPhase::from_str("current_renamed").is_err());
        assert_eq!(
            GenerationPublicationStatus::from_str("failed").unwrap(),
            GenerationPublicationStatus::Failed
        );
        assert!(GenerationPublicationStatus::from_str("mystery").is_err());
    }
}
```

In `crates/conary-core/src/db/models/mod.rs`, add a private module declaration so the unit test compiles before the type is exported:

```rust
mod generation_publication;
```

- [ ] **Step 2: Run the focused enum test**

Run:

```bash
cargo test -p conary-core generation_publication::tests::phase_and_status_reject_unknown_values
```

Expected: pass.

- [ ] **Step 3: Implement schema migration v69**

In `crates/conary-core/src/db/schema.rs`, change:

```rust
pub const SCHEMA_VERSION: i32 = 68;
```

to:

```rust
pub const SCHEMA_VERSION: i32 = 69;
```

Add the migration arm:

```rust
68 => migrations::migrate_v68(conn),
69 => migrations::migrate_v69(conn),
```

In `crates/conary-core/src/db/migrations/v41_current.rs`, append:

```rust
/// Version 69: Generation publication debt ledger
pub fn migrate_v69(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 69");

    conn.execute_batch(
        "
        CREATE TABLE generation_publications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trigger_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            published_through_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            tx_uuid TEXT,
            db_path TEXT NOT NULL,
            runtime_root TEXT NOT NULL,
            phase TEXT NOT NULL CHECK(phase IN (
                'pending_build',
                'building',
                'artifact_ready',
                'current_published',
                'active_marked'
            )),
            status TEXT NOT NULL CHECK(status IN (
                'pending',
                'running',
                'failed',
                'complete',
                'abandoned'
            )),
            state_number INTEGER,
            generation_number INTEGER,
            summary TEXT NOT NULL,
            last_error TEXT,
            retry_count INTEGER NOT NULL DEFAULT 0,
            recoverable INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            completed_at TEXT
        );

        CREATE INDEX idx_generation_publications_status
            ON generation_publications(status);
        CREATE INDEX idx_generation_publications_trigger_changeset
            ON generation_publications(trigger_changeset_id);
        CREATE INDEX idx_generation_publications_generation
            ON generation_publications(generation_number);
        CREATE INDEX idx_generation_publications_recoverable
            ON generation_publications(recoverable, status);
        ",
    )?;

    info!("Schema version 69 applied successfully (generation publication debt ledger)");
    Ok(())
}
```

- [ ] **Step 4: Add migration tests**

In the existing `#[cfg(test)]` module in `v41_current.rs`, add:

```rust
#[test]
fn test_migrate_v69_adds_generation_publications_table() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    let version: i32 = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, crate::db::schema::SCHEMA_VERSION);

    conn.execute(
        "INSERT INTO changesets (description, status)
         VALUES ('Install fixture', 'applied')",
        [],
    )
    .unwrap();
    let changeset_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO generation_publications (
            trigger_changeset_id, db_path, runtime_root, phase, status, summary
         ) VALUES (?1, ?2, ?3, 'pending_build', 'pending', ?4)",
        (changeset_id, "/tmp/conary.db", "/tmp/conary", "Install fixture"),
    )
    .unwrap();

    let phase: String = conn
        .query_row("SELECT phase FROM generation_publications", [], |row| row.get(0))
        .unwrap();
    assert_eq!(phase, "pending_build");
}

#[test]
fn test_generation_publications_reject_unknown_phase() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    let err = conn
        .execute(
            "INSERT INTO generation_publications (
                db_path, runtime_root, phase, status, summary
             ) VALUES (?1, ?2, 'current_renamed', 'pending', ?3)",
            ("/tmp/conary.db", "/tmp/conary", "bad phase"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("CHECK"));
}
```

- [ ] **Step 5: Export and implement model CRUD helpers**

In `crates/conary-core/src/db/models/mod.rs`, replace the private module entry from Step 1 with:

```rust
mod generation_publication;
pub use generation_publication::{
    GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus,
};
```

In `generation_publication.rs`, add the methods used by later tasks:

```rust
impl GenerationPublication {
    const COLUMNS: &'static str = "id, trigger_changeset_id, published_through_changeset_id, \
        tx_uuid, db_path, runtime_root, phase, status, state_number, generation_number, \
        summary, last_error, retry_count, recoverable, created_at, updated_at, completed_at";

    pub fn create_pending(
        conn: &Connection,
        trigger_changeset_id: Option<i64>,
        tx_uuid: Option<&str>,
        db_path: &str,
        runtime_root: &str,
        summary: &str,
    ) -> Result<Self> {
        conn.execute(
            "INSERT INTO generation_publications (
                trigger_changeset_id, tx_uuid, db_path, runtime_root, phase, status, summary
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                trigger_changeset_id,
                tx_uuid,
                db_path,
                runtime_root,
                GenerationPublicationPhase::PendingBuild.as_str(),
                GenerationPublicationStatus::Pending.as_str(),
                summary,
            ],
        )?;
        Self::find_by_id(conn, conn.last_insert_rowid())?
            .ok_or_else(|| crate::error::Error::InternalError("inserted publication row not found".to_string()))
    }

    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM generation_publications WHERE id = ?1", Self::COLUMNS);
        conn.prepare(&sql)?
            .query_row([id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn pending_recoverable(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM generation_publications
             WHERE recoverable = 1 AND status IN ('pending', 'running', 'failed')
             ORDER BY id ASC",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn pending_for_changeset(conn: &Connection, changeset_id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM generation_publications
             WHERE trigger_changeset_id = ?1
               AND recoverable = 1
               AND status IN ('pending', 'running', 'failed')
             ORDER BY id DESC LIMIT 1",
            Self::COLUMNS
        );
        conn.prepare(&sql)?
            .query_row([changeset_id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn high_water_changeset_id(conn: &Connection) -> Result<Option<i64>> {
        conn.query_row("SELECT MAX(id) FROM changesets", [], |row| row.get(0))
            .map_err(Into::into)
    }

    pub fn mark_failed(&self, conn: &Connection, message: &str) -> Result<()> {
        let id = self.id.ok_or_else(|| crate::error::Error::MissingId("publication id missing".to_string()))?;
        conn.execute(
            "UPDATE generation_publications
             SET status = 'failed',
                 last_error = ?1,
                 retry_count = retry_count + 1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![message, id],
        )?;
        Ok(())
    }

    pub fn set_phase(
        &self,
        conn: &Connection,
        phase: GenerationPublicationPhase,
        status: GenerationPublicationStatus,
        state_number: Option<i64>,
        generation_number: Option<i64>,
    ) -> Result<()> {
        let id = self.id.ok_or_else(|| crate::error::Error::MissingId("publication id missing".to_string()))?;
        conn.execute(
            "UPDATE generation_publications
             SET phase = ?1,
                 status = ?2,
                 state_number = COALESCE(?3, state_number),
                 generation_number = COALESCE(?4, generation_number),
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?5",
            params![phase.as_str(), status.as_str(), state_number, generation_number, id],
        )?;
        Ok(())
    }

    pub fn mark_complete_through(
        conn: &Connection,
        high_water_changeset_id: Option<i64>,
        state_number: i64,
        generation_number: i64,
    ) -> Result<usize> {
        let rows = conn.execute(
            "UPDATE generation_publications
             SET status = 'complete',
                 phase = 'active_marked',
                 published_through_changeset_id = ?1,
                 state_number = COALESCE(state_number, ?2),
                 generation_number = COALESCE(generation_number, ?3),
                 recoverable = 0,
                 completed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE recoverable = 1
               AND status IN ('pending', 'running', 'failed')
               AND (?1 IS NULL OR trigger_changeset_id IS NULL OR trigger_changeset_id <= ?1)",
            params![high_water_changeset_id, state_number, generation_number],
        )?;
        Ok(rows)
    }

    pub fn protected_generation_numbers(conn: &Connection) -> Result<Vec<i64>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT generation_number
             FROM generation_publications
             WHERE recoverable = 1
               AND status IN ('pending', 'running', 'failed')
               AND generation_number IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let phase_raw: String = row.get(6)?;
        let status_raw: String = row.get(7)?;
        let phase = phase_raw.parse::<GenerationPublicationPhase>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let status = status_raw.parse::<GenerationPublicationStatus>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let recoverable: i32 = row.get(13)?;
        Ok(Self {
            id: Some(row.get(0)?),
            trigger_changeset_id: row.get(1)?,
            published_through_changeset_id: row.get(2)?,
            tx_uuid: row.get(3)?,
            db_path: row.get(4)?,
            runtime_root: row.get(5)?,
            phase,
            status,
            state_number: row.get(8)?,
            generation_number: row.get(9)?,
            summary: row.get(10)?,
            last_error: row.get(11)?,
            retry_count: row.get(12)?,
            recoverable: recoverable != 0,
            created_at: row.get(14)?,
            updated_at: row.get(15)?,
            completed_at: row.get(16)?,
        })
    }
}
```

- [ ] **Step 6: Add model behavior tests**

Add these tests to `generation_publication.rs`:

```rust
#[test]
fn create_pending_and_mark_complete_sweeps_covered_debts() {
    let (_tmp, conn) = crate::db::testing::create_test_db();

    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
        [],
    )
    .unwrap();
    let cs_a = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('B', 'applied')",
        [],
    )
    .unwrap();
    let cs_b = conn.last_insert_rowid();

    let a = GenerationPublication::create_pending(
        &conn,
        Some(cs_a),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "A",
    )
    .unwrap();
    let b = GenerationPublication::create_pending(
        &conn,
        Some(cs_b),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "B",
    )
    .unwrap();
    a.mark_failed(&conn, "forced").unwrap();
    b.set_phase(
        &conn,
        GenerationPublicationPhase::ArtifactReady,
        GenerationPublicationStatus::Running,
        Some(7),
        Some(7),
    )
    .unwrap();

    let completed = GenerationPublication::mark_complete_through(&conn, Some(cs_b), 7, 7).unwrap();
    assert_eq!(completed, 2);
    assert!(GenerationPublication::pending_recoverable(&conn).unwrap().is_empty());
}

#[test]
fn pending_for_changeset_finds_recoverable_debt_only() {
    let (_tmp, conn) = crate::db::testing::create_test_db();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
        [],
    )
    .unwrap();
    let cs_a = conn.last_insert_rowid();

    let debt = GenerationPublication::create_pending(
        &conn,
        Some(cs_a),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "A",
    )
    .unwrap();
    assert_eq!(
        GenerationPublication::pending_for_changeset(&conn, cs_a)
            .unwrap()
            .unwrap()
            .id,
        debt.id
    );

    GenerationPublication::mark_complete_through(&conn, Some(cs_a), 1, 1).unwrap();
    assert!(GenerationPublication::pending_for_changeset(&conn, cs_a).unwrap().is_none());
}
```

- [ ] **Step 7: Run focused core tests**

Run:

```bash
cargo test -p conary-core generation_publication
cargo test -p conary-core test_migrate_v69
```

Expected: all tests pass.

- [ ] **Step 8: Commit Task 1**

Review the exact changed files:

```bash
git diff --name-only
```

Commit only Task 1 files:

```bash
git add crates/conary-core/src/db/models/generation_publication.rs \
        crates/conary-core/src/db/models/mod.rs \
        crates/conary-core/src/db/migrations/v41_current.rs \
        crates/conary-core/src/db/schema.rs
git commit -m "feat(core): add generation publication debt model"
```

---

### Task 2: Durable Current Symlink Publication

**Files:**
- Create: `crates/conary-core/src/filesystem/durable.rs`
- Modify: `crates/conary-core/src/filesystem/mod.rs`
- Modify: `crates/conary-core/src/generation/mount.rs`

- [ ] **Step 1: Add durable filesystem helper tests**

Create `crates/conary-core/src/filesystem/durable.rs`:

```rust
// conary-core/src/filesystem/durable.rs

use crate::{Error, Result};
use std::fs::OpenOptions;
use std::path::Path;

pub fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        Error::IoError(format!("Path has no parent directory: {}", path.display()))
    })?;
    let dir = OpenOptions::new().read(true).open(parent).map_err(|error| {
        Error::IoError(format!(
            "Failed to open parent directory {} for sync: {error}",
            parent.display()
        ))
    })?;
    dir.sync_all().map_err(|error| {
        Error::IoError(format!(
            "Failed to sync parent directory {}: {error}",
            parent.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::sync_parent_directory;
    use tempfile::TempDir;

    #[test]
    fn sync_parent_directory_succeeds_for_existing_parent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("current");
        std::fs::write(&path, b"fixture").unwrap();
        sync_parent_directory(&path).unwrap();
    }

    #[test]
    fn sync_parent_directory_rejects_path_without_parent() {
        let error = sync_parent_directory(std::path::Path::new("current"))
            .expect_err("relative path without parent should fail");
        assert!(error.to_string().contains("no parent directory"));
    }
}
```

- [ ] **Step 2: Export the durable helper**

In `crates/conary-core/src/filesystem/mod.rs`, add:

```rust
pub mod durable;
```

- [ ] **Step 3: Update `update_current_symlink` to sync parent**

In `crates/conary-core/src/generation/mount.rs`, after `std::fs::rename(&tmp_link, &link)` succeeds, add:

```rust
    crate::filesystem::durable::sync_parent_directory(&link)?;
```

Keep the existing log line after the sync. This makes the log correspond to a durable selection rather than just a successful rename.

- [ ] **Step 4: Strengthen current symlink tests**

In the existing `mount.rs` tests for `update_current_symlink`, add assertions that the temp symlink is gone after a successful update:

```rust
assert!(
    !tmp.path().join("current.tmp").exists(),
    "successful update must not leave a stale temp symlink"
);
```

Add the assertion to both the single-update and idempotent-update tests.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p conary-core filesystem::durable
cargo test -p conary-core generation::mount::tests::update_current_symlink
```

Expected: all tests pass.

- [ ] **Step 6: Commit Task 2**

```bash
git add crates/conary-core/src/filesystem/durable.rs \
        crates/conary-core/src/filesystem/mod.rs \
        crates/conary-core/src/generation/mount.rs
git commit -m "fix(generation): durably sync current generation link"
```

---

### Task 3: Publication Helper And Composefs Activation Refactor

**Files:**
- Create: `apps/conary/src/commands/generation/publication.rs`
- Modify: `apps/conary/src/commands/generation/mod.rs`
- Modify: `apps/conary/src/commands/composefs_ops.rs`

- [ ] **Step 1: Add publication helper module shell and tests**

Create `apps/conary/src/commands/generation/publication.rs`:

```rust
// apps/conary/src/commands/generation/publication.rs

use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{
    GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus, SystemState,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub(crate) struct PublicationRequest<'a> {
    pub db_path: &'a str,
    pub summary: &'a str,
    pub trigger_changeset_id: Option<i64>,
    pub tx_uuid: Option<&'a str>,
    pub prev_etc_snapshot: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicationOutcome {
    pub generation_number: Option<i64>,
    pub state_number: Option<i64>,
    pub needs_publication: bool,
    pub retry_command: Option<String>,
    pub completed_debts: usize,
}

impl PublicationOutcome {
    pub(crate) fn retry_command() -> String {
        "conary --allow-live-system-mutation system generation publish".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::PublicationOutcome;

    #[test]
    fn retry_command_uses_parameterless_publish() {
        assert_eq!(
            PublicationOutcome::retry_command(),
            "conary --allow-live-system-mutation system generation publish"
        );
    }
}
```

In `apps/conary/src/commands/generation/mod.rs`, add:

```rust
pub(crate) mod publication;
```

- [ ] **Step 2: Run the shell test**

```bash
cargo test -p conary generation::publication::tests::retry_command_uses_parameterless_publish
```

Expected: pass.

- [ ] **Step 3: Split composefs build from current-link activation**

In `apps/conary/src/commands/composefs_ops.rs`, add this result type near `rebuild_and_mount`:

```rust
#[derive(Debug)]
pub(crate) struct BuiltGeneration {
    pub generation_number: i64,
    pub state_number: i64,
}
```

Extract the body of `rebuild_and_mount` into a new function that builds and finalizes the artifact without updating `/conary/current` or marking the DB state active:

```rust
pub(crate) fn build_generation_for_publication(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    prev_etc_snapshot: Option<HashMap<String, String>>,
) -> anyhow::Result<BuiltGeneration> {
    if let Some(error) = forced_generation_rebuild_failure() {
        return Err(error);
    }

    let runtime_root = runtime_root_for_db_path(db_path);
    let current_gen = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap_or(None)
        .unwrap_or(0);

    let prev_etc = resolve_previous_etc_snapshot(conn, current_gen, prev_etc_snapshot)?;
    let generations_dir = runtime_root.generations_dir();
    let boot_root = boot_root_for_generation_build(&runtime_root);
    let (gen_num, build_result) =
        conary_core::generation::builder::build_generation_from_db_with_boot_root_and_activation(
            conn,
            &generations_dir,
            summary,
            &boot_root,
            conary_core::generation::builder::GenerationActivation::Inactive,
        )
        .map_err(|e| anyhow::anyhow!("Failed to build EROFS generation: {e}"))?;

    apply_etc_merge_for_generation(conn, &runtime_root, gen_num, &prev_etc)?;

    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_none() {
        let gen_dir = generations_dir.join(gen_num.to_string());
        enable_generation_rootfs_verity(&gen_dir, &build_result.image_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to enable fs-verity on generation {gen_num} image {}: {e}",
                build_result.image_path.display()
            )
        })?;
    }

    Ok(BuiltGeneration {
        generation_number: gen_num,
        state_number: gen_num,
    })
}
```

During extraction, move the current `prev_etc` computation into:

```rust
fn resolve_previous_etc_snapshot(
    conn: &Connection,
    current_gen: i64,
    prev_etc_snapshot: Option<HashMap<String, String>>,
) -> anyhow::Result<HashMap<String, String>> {
    match prev_etc_snapshot {
        Some(snapshot) => Ok(snapshot),
        None => {
            if let Some(base_num) = current_base_generation_for_merge(conn, current_gen)? {
                let base_etc = collect_etc_files_for_state(conn, base_num)?;
                if base_etc.is_empty() {
                    let has_resolvable_troves: bool = conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM state_members sm \
                             JOIN troves t ON t.name = sm.trove_name \
                                 AND t.version = sm.trove_version \
                                 AND (sm.architecture IS NULL \
                                      OR t.architecture IS NULL \
                                      OR sm.architecture = t.architecture) \
                             JOIN system_states ss ON sm.state_id = ss.id \
                             WHERE ss.state_number = ?1)",
                            [base_num],
                            |row| row.get(0),
                        )
                        .unwrap_or(false);
                    if has_resolvable_troves {
                        Ok(base_etc)
                    } else {
                        collect_etc_files(conn)
                    }
                } else {
                    Ok(base_etc)
                }
            } else {
                collect_etc_files(conn)
            }
        }
    }
}
```

Move the `/etc` merge action loop into:

```rust
fn apply_etc_merge_for_generation(
    conn: &Connection,
    runtime_root: &ConaryRuntimeRoot,
    gen_num: i64,
    prev_etc: &HashMap<String, String>,
) -> anyhow::Result<()> {
    let upper_dir = runtime_root.etc_state_dir().join(gen_num.to_string());
    std::fs::create_dir_all(&upper_dir)?;

    let new_etc = collect_etc_files(conn)?;
    let merge_plan = etc_merge::plan_etc_merge(prev_etc, &new_etc, &upper_dir)
        .map_err(|e| anyhow::anyhow!("Failed to plan /etc merge: {e}"))?;

    for (rel_path, action) in &merge_plan.actions {
        match action {
            MergeAction::AcceptPackage => {
                let upper_file = upper_dir.join(rel_path);
                if upper_file.exists() {
                    std::fs::remove_file(&upper_file).with_context(|| {
                        format!("failed to remove upper layer copy {}", upper_file.display())
                    })?;
                }
            }
            MergeAction::Conflict { .. }
            | MergeAction::OrphanedUserFile
            | MergeAction::KeepUser
            | MergeAction::NewFromPackage
            | MergeAction::Unchanged => {}
        }
    }

    if merge_plan.has_conflicts() {
        warn!(
            count = merge_plan.conflicts().len(),
            "Generation {gen_num} has /etc merge conflicts that need manual resolution"
        );
    }

    Ok(())
}
```

Move the existing warning branches into this helper unchanged. Preserve behavior and keep `AcceptPackage` removal errors as returned errors instead of logging and continuing.

- [ ] **Step 4: Add explicit current-link and DB-active helpers**

In `composefs_ops.rs`, add:

```rust
pub(crate) fn publish_generation_link(db_path: &str, gen_num: i64) -> anyhow::Result<()> {
    let runtime_root = runtime_root_for_db_path(db_path);
    conary_core::generation::mount::update_current_symlink(runtime_root.root(), gen_num)
        .map_err(|e| anyhow::anyhow!("Failed to update current symlink: {e}"))
}

pub(crate) fn mark_generation_state_active(conn: &Connection, gen_num: i64) -> anyhow::Result<()> {
    let state = SystemState::find_by_number(conn, gen_num)
        .map_err(|e| anyhow::anyhow!("Failed to load system state {gen_num}: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("System state {gen_num} not found after generation build"))?;
    state
        .set_active(conn)
        .map_err(|e| anyhow::anyhow!("Failed to mark system state {gen_num} active: {e}"))
}
```

Add `SystemState` to imports:

```rust
use conary_core::db::models::{FileEntry, SystemState};
```

- [ ] **Step 5: Keep `rebuild_and_mount` as compatibility wrapper with correct ordering**

Replace the old `rebuild_and_mount` body with:

```rust
pub fn rebuild_and_mount(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    prev_etc_snapshot: Option<HashMap<String, String>>,
) -> anyhow::Result<i64> {
    let built = build_generation_for_publication(conn, db_path, summary, prev_etc_snapshot)?;
    publish_generation_link(db_path, built.generation_number)?;
    mark_generation_state_active(conn, built.state_number)?;

    info!(
        "Generation {} built and selected for next boot",
        built.generation_number
    );
    Ok(built.generation_number)
}
```

This preserves existing callers while fixing the activation order.

- [ ] **Step 6: Implement `publish_current_db_state`**

In `generation/publication.rs`, add:

```rust
pub(crate) fn publish_current_db_state(
    conn: &Connection,
    request: PublicationRequest<'_>,
) -> Result<PublicationOutcome> {
    let runtime_root = ConaryRuntimeRoot::from_db_path(request.db_path.into());
    let runtime_root_display = runtime_root.root().display().to_string();
    let high_water = GenerationPublication::high_water_changeset_id(conn)?;
    let debt = GenerationPublication::create_pending(
        conn,
        request.trigger_changeset_id,
        request.tx_uuid,
        request.db_path,
        &runtime_root_display,
        request.summary,
    )?;

    let publish_result = (|| -> Result<BuiltForPublication> {
        debt.set_phase(
            conn,
            GenerationPublicationPhase::Building,
            GenerationPublicationStatus::Running,
            None,
            None,
        )?;
        let built = crate::commands::composefs_ops::build_generation_for_publication(
            conn,
            request.db_path,
            request.summary,
            request.prev_etc_snapshot,
        )?;
        debt.set_phase(
            conn,
            GenerationPublicationPhase::ArtifactReady,
            GenerationPublicationStatus::Running,
            Some(built.state_number),
            Some(built.generation_number),
        )?;
        crate::commands::composefs_ops::publish_generation_link(
            request.db_path,
            built.generation_number,
        )?;
        debt.set_phase(
            conn,
            GenerationPublicationPhase::CurrentPublished,
            GenerationPublicationStatus::Running,
            Some(built.state_number),
            Some(built.generation_number),
        )?;
        crate::commands::composefs_ops::mark_generation_state_active(conn, built.state_number)?;
        Ok(BuiltForPublication {
            state_number: built.state_number,
            generation_number: built.generation_number,
        })
    })();

    match publish_result {
        Ok(built) => {
            let completed = GenerationPublication::mark_complete_through(
                conn,
                high_water,
                built.state_number,
                built.generation_number,
            )?;
            Ok(PublicationOutcome {
                generation_number: Some(built.generation_number),
                state_number: Some(built.state_number),
                needs_publication: false,
                retry_command: None,
                completed_debts: completed,
            })
        }
        Err(error) => {
            debt.mark_failed(conn, &error.to_string())?;
            Ok(PublicationOutcome {
                generation_number: None,
                state_number: None,
                needs_publication: true,
                retry_command: Some(PublicationOutcome::retry_command()),
                completed_debts: 0,
            })
        }
    }
}

#[derive(Debug)]
struct BuiltForPublication {
    state_number: i64,
    generation_number: i64,
}
```

The helper returns `Ok(PublicationOutcome { needs_publication: true, .. })` after a post-commit publication failure because the package DB commit already happened. It returns `Err` only if Conary cannot record debt/failure state.

- [ ] **Step 7: Add a unit test for sweep semantics**

In `generation/publication.rs`, add a test that uses the core model without building an artifact:

```rust
#[test]
fn successful_publication_completion_sweeps_prior_debts() {
    let (_tmp, conn) = conary_core::db::testing::create_test_db();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
        [],
    )
    .unwrap();
    let cs_a = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('B', 'applied')",
        [],
    )
    .unwrap();
    let cs_b = conn.last_insert_rowid();

    let first = GenerationPublication::create_pending(
        &conn,
        Some(cs_a),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "A",
    )
    .unwrap();
    first.mark_failed(&conn, "forced").unwrap();
    GenerationPublication::create_pending(
        &conn,
        Some(cs_b),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "B",
    )
    .unwrap();

    let completed = GenerationPublication::mark_complete_through(&conn, Some(cs_b), 2, 2).unwrap();
    assert_eq!(completed, 2);
    assert!(GenerationPublication::pending_recoverable(&conn).unwrap().is_empty());
}
```

- [ ] **Step 8: Run focused tests**

Run:

```bash
cargo test -p conary generation::publication
cargo test -p conary composefs_ops
```

Expected: all tests pass.

- [ ] **Step 9: Commit Task 3**

```bash
git add apps/conary/src/commands/generation/publication.rs \
        apps/conary/src/commands/generation/mod.rs \
        apps/conary/src/commands/composefs_ops.rs
git commit -m "feat(generation): publish current db state through debt ledger"
```

---

### Task 4: Package Mutation Integration

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/changeset_metadata.rs`

- [ ] **Step 1: Add a shared warning formatter**

In `apps/conary/src/commands/generation/publication.rs`, add:

```rust
pub(crate) fn warn_if_publication_pending(changeset_id: i64, outcome: &PublicationOutcome) {
    if !outcome.needs_publication {
        return;
    }
    let retry = outcome
        .retry_command
        .as_deref()
        .unwrap_or("conary --allow-live-system-mutation system generation publish");
    tracing::warn!(
        changeset_id,
        retry,
        "Package mutation committed, but generation publication is pending"
    );
    eprintln!(
        "WARNING: package mutation committed, but generation publication is pending for changeset {changeset_id}.\nRun: {retry}"
    );
}
```

- [ ] **Step 2: Type/validate deferred generation follow-up metadata**

In `apps/conary/src/commands/changeset_metadata.rs`, add enum wrappers while preserving the JSON envelope:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredFollowUpKind {
    GenerationPublication,
    LegacyGenerationRebuild,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredFollowUpStatus {
    Pending,
    Failed,
    Complete,
    Other,
}

pub(crate) fn classify_deferred_follow_up(follow_up: &DeferredFollowUp) -> (DeferredFollowUpKind, DeferredFollowUpStatus) {
    let kind = match follow_up.kind.as_str() {
        "generation_publication" => DeferredFollowUpKind::GenerationPublication,
        "generation_rebuild" => DeferredFollowUpKind::LegacyGenerationRebuild,
        _ => DeferredFollowUpKind::Other,
    };
    let status = match follow_up.status.as_str() {
        "pending" => DeferredFollowUpStatus::Pending,
        "failed" => DeferredFollowUpStatus::Failed,
        "complete" => DeferredFollowUpStatus::Complete,
        _ => DeferredFollowUpStatus::Other,
    };
    (kind, status)
}

pub(crate) fn publication_deferred_follow_up(message: String) -> DeferredFollowUp {
    DeferredFollowUp {
        kind: "generation_publication".to_string(),
        status: "pending".to_string(),
        message,
        retry_command: Some(
            "conary --allow-live-system-mutation system generation publish".to_string(),
        ),
    }
}
```

Add tests:

```rust
#[test]
fn publication_deferred_follow_up_uses_publish_retry() {
    let follow_up = publication_deferred_follow_up("forced".to_string());
    assert_eq!(follow_up.kind, "generation_publication");
    assert_eq!(follow_up.status, "pending");
    assert_eq!(
        follow_up.retry_command.as_deref(),
        Some("conary --allow-live-system-mutation system generation publish")
    );
}

#[test]
fn classify_legacy_generation_rebuild_follow_up() {
    let follow_up = DeferredFollowUp {
        kind: "generation_rebuild".to_string(),
        status: "failed".to_string(),
        message: "old failure".to_string(),
        retry_command: Some("conary system generation build --summary retry".to_string()),
    };
    assert_eq!(
        classify_deferred_follow_up(&follow_up),
        (DeferredFollowUpKind::LegacyGenerationRebuild, DeferredFollowUpStatus::Failed)
    );
}
```

Export these helpers from `apps/conary/src/commands/mod.rs`.

- [ ] **Step 3: Integrate single install**

In `apps/conary/src/commands/install/mod.rs`, replace the post-commit `rebuild_and_mount` block with:

```rust
let post_commit_result = (|| -> Result<()> {
    let outcome = crate::commands::generation::publication::publish_current_db_state(
        conn,
        crate::commands::generation::publication::PublicationRequest {
            db_path: ctx.db_path,
            summary: &tx_description,
            trigger_changeset_id: Some(changeset_id),
            tx_uuid: changeset.tx_uuid.as_deref(),
            prev_etc_snapshot: Some(prev_etc),
        },
    )?;
    if outcome.needs_publication {
        crate::commands::append_deferred_follow_up_metadata(
            conn,
            changeset_id,
            crate::commands::publication_deferred_follow_up(
                "generation publication is pending".to_string(),
            ),
        )?;
        crate::commands::generation::publication::warn_if_publication_pending(
            changeset_id,
            &outcome,
        );
    }
    changeset.update_status(conn, ChangesetStatus::Applied)?;
    Ok(())
})();
```

Keep the existing `ctx.defer_generation` early return, but create visible publication debt before returning:

```rust
if ctx.defer_generation && ctx.execution_path == PackageExecutionPath::GenerationAware {
    let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(ctx.db_path.into());
    conary_core::db::models::GenerationPublication::create_pending(
        conn,
        Some(changeset_id),
        changeset.tx_uuid.as_deref(),
        ctx.db_path,
        &runtime_root.root().display().to_string(),
        &tx_description,
    )?;
    crate::commands::append_deferred_follow_up_metadata(
        conn,
        changeset_id,
        crate::commands::publication_deferred_follow_up(
            "generation publication is pending".to_string(),
        ),
    )?;
    changeset.update_status(conn, ChangesetStatus::Applied)?;
    engine.release_lock();
    return Ok(InstallTransactionResult { changeset_id });
}
```

- [ ] **Step 4: Integrate batch install**

In `apps/conary/src/commands/install/batch.rs`, replace the deferred generation rebuild metadata block with the same helper shape:

```rust
let outcome = crate::commands::generation::publication::publish_current_db_state(
    conn,
    crate::commands::generation::publication::PublicationRequest {
        db_path,
        summary,
        trigger_changeset_id: Some(changeset_id),
        tx_uuid: changeset.tx_uuid.as_deref(),
        prev_etc_snapshot,
    },
)?;
if outcome.needs_publication {
    crate::commands::append_deferred_follow_up_metadata(
        conn,
        changeset_id,
        crate::commands::publication_deferred_follow_up(
            "generation publication is pending".to_string(),
        ),
    )?;
    crate::commands::generation::publication::warn_if_publication_pending(changeset_id, &outcome);
}
```

Adapt this snippet at the existing `batch.rs` call site by passing the caller's existing `conn`, `db_path`, `summary`, `changeset_id`, `changeset.tx_uuid`, and `prev_etc_snapshot` values. Keep `PublicationRequest` unchanged.

- [ ] **Step 5: Integrate remove**

In `apps/conary/src/commands/remove.rs`, replace the post-commit `rebuild_and_mount` error branch with `publish_current_db_state`:

```rust
let outcome = crate::commands::generation::publication::publish_current_db_state(
    &conn,
    crate::commands::generation::publication::PublicationRequest {
        db_path,
        summary: &format!("Remove {}", package_name),
        trigger_changeset_id: Some(remove_changeset_id),
        tx_uuid: changeset.tx_uuid.as_deref(),
        prev_etc_snapshot: Some(prev_etc),
    },
)?;
if outcome.needs_publication {
    crate::commands::append_deferred_follow_up_metadata(
        &conn,
        remove_changeset_id,
        crate::commands::publication_deferred_follow_up(
            "generation publication is pending".to_string(),
        ),
    )?;
    crate::commands::generation::publication::warn_if_publication_pending(
        remove_changeset_id,
        &outcome,
    );
}
changeset.update_status(&conn, conary_core::db::models::ChangesetStatus::Applied)?;
```

- [ ] **Step 6: Update forced-failure tests**

Rename `BatchInstaller::record_generation_rebuild_failure` to `record_generation_publication_pending`. The helper must create both a `generation_publications` row and the envelope-preserving deferred follow-up entry:

```rust
fn record_generation_publication_pending(
    conn: &Connection,
    changeset_id: i64,
    db_path: &str,
    summary: &str,
    error: anyhow::Error,
) -> Result<()> {
    let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.into());
    let debt = conary_core::db::models::GenerationPublication::create_pending(
        conn,
        Some(changeset_id),
        None,
        db_path,
        &runtime_root.root().display().to_string(),
        summary,
    )?;
    debt.mark_failed(conn, &error.to_string())?;
    crate::commands::append_deferred_follow_up_metadata(
        conn,
        changeset_id,
        crate::commands::publication_deferred_follow_up(error.to_string()),
    )
}
```

Update `generation_rebuild_failure_records_deferred_follow_up_for_applied_batch` to:

```rust
#[test]
fn generation_publication_failure_records_debt_for_applied_batch() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut changeset = Changeset::new("Batch install: fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    BatchInstaller::record_generation_publication_pending(
        &conn,
        changeset_id,
        db_path.to_str().unwrap(),
        "Batch install: fixture",
        anyhow::anyhow!("composefs build failed"),
    )
    .unwrap();

    let changeset = Changeset::find_by_id(&conn, changeset_id)
        .unwrap()
        .expect("changeset should exist");
    assert_eq!(changeset.status, ChangesetStatus::Applied);
let deferred = crate::commands::deferred_follow_up(changeset.metadata.as_deref());
assert_eq!(deferred.len(), 1);
assert_eq!(deferred[0].kind, "generation_publication");
assert_eq!(deferred[0].status, "pending");
assert!(deferred[0].message.contains("composefs build failed"));
assert_eq!(
    deferred[0].retry_command.as_deref(),
    Some("conary --allow-live-system-mutation system generation publish")
);
let debts = conary_core::db::models::GenerationPublication::pending_recoverable(&conn).unwrap();
assert_eq!(debts.len(), 1);
assert_eq!(debts[0].status, conary_core::db::models::GenerationPublicationStatus::Failed);
}
```

Add a source-order regression in `apps/conary/src/commands/install/mod.rs` proving post-commit publication failure does not convert the already-committed install into an error:

```rust
#[test]
fn install_exits_zero_and_records_publication_debt_when_generation_publish_fails() {
    let source = include_str!("mod.rs");
    let commit_pos = source
        .find("tx.commit()?")
        .expect("install transaction should commit before publication");
    let publish_pos = source
        .find("publish_current_db_state")
        .expect("install path should publish after commit");
    let result_pos = source
        .find("Ok(InstallTransactionResult { changeset_id })")
        .expect("install path should return success after post-commit handling");
    assert!(commit_pos < publish_pos);
    assert!(publish_pos < result_pos);
    assert!(source.contains("outcome.needs_publication"));
    assert!(source.contains("append_deferred_follow_up_metadata"));
}
```

- [ ] **Step 7: Add current-DB sweep regression**

Add a unit test in `generation/publication.rs` or an integration test in the install module:

```rust
#[test]
fn later_successful_publication_completes_prior_publication_debt() {
    let (_tmp, conn) = conary_core::db::testing::create_test_db();
    conn.execute("INSERT INTO changesets (description, status) VALUES ('A', 'applied')", []).unwrap();
    let cs_a = conn.last_insert_rowid();
    conn.execute("INSERT INTO changesets (description, status) VALUES ('B', 'applied')", []).unwrap();
    let cs_b = conn.last_insert_rowid();

    let debt_a = GenerationPublication::create_pending(&conn, Some(cs_a), None, "/tmp/db", "/tmp/root", "A").unwrap();
    debt_a.mark_failed(&conn, "forced").unwrap();
    GenerationPublication::create_pending(&conn, Some(cs_b), None, "/tmp/db", "/tmp/root", "B").unwrap();

    GenerationPublication::mark_complete_through(&conn, Some(cs_b), 3, 3).unwrap();
    assert!(GenerationPublication::pending_recoverable(&conn).unwrap().is_empty());
}
```

This test proves the model behavior even before full CLI install fixtures are extended.

- [ ] **Step 8: Run focused tests**

```bash
cargo test -p conary changeset_metadata
cargo test -p conary generation::publication
cargo test -p conary install::batch
cargo test -p conary remove
```

Expected: all tests pass, and the batch output includes `generation_publication_failure_records_debt_for_applied_batch`.

- [ ] **Step 9: Commit Task 4**

```bash
git add apps/conary/src/commands/install/mod.rs \
        apps/conary/src/commands/install/batch.rs \
        apps/conary/src/commands/remove.rs \
        apps/conary/src/commands/changeset_metadata.rs \
        apps/conary/src/commands/mod.rs \
        apps/conary/src/commands/generation/publication.rs
git commit -m "feat(conary): record publication debt after package mutations"
```

---

### Task 5: CLI Publish, Pending, And History Surfaces

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `apps/conary/src/commands/generation/publication.rs`
- Modify: `apps/conary/src/commands/query/history.rs`

- [ ] **Step 1: Add CLI variants**

In `apps/conary/src/cli/generation.rs`, add:

```rust
    /// Publish committed DB state into the selected generation.
    Publish {
        /// Assert that this pending changeset is covered by the publication.
        #[arg(long)]
        changeset: Option<i64>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show generation publication debt that still needs operator attention.
    Pending {
        #[command(flatten)]
        db: DbArgs,
    },
```

- [ ] **Step 2: Classify command risk**

In `apps/conary/src/command_risk.rs`, update `classify_generation`:

```rust
        cli::GenerationCommands::Publish { .. } => policy(
            "conary system generation publish",
            CommandRisk::AlwaysLive,
            false,
        ),
        cli::GenerationCommands::Pending { .. } => {
            read_only("conary system generation pending")
        }
```

Add unit tests:

```rust
#[test]
fn classify_generation_publish_as_always_live() {
    let policy = policy(&["conary", "system", "generation", "publish"]);
    assert_eq!(policy.risk, CommandRisk::AlwaysLive);
    assert!(policy.requires_ack());
}

#[test]
fn classify_generation_pending_as_read_only() {
    let policy = policy(&["conary", "system", "generation", "pending"]);
    assert_eq!(policy.risk, CommandRisk::ReadOnly);
    assert!(!policy.requires_ack());
}
```

- [ ] **Step 3: Add command implementations**

In `apps/conary/src/commands/generation/commands.rs`, add:

```rust
pub fn cmd_generation_publish(db_path: &str, changeset: Option<i64>) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    if let Some(changeset_id) = changeset
        && conary_core::db::models::GenerationPublication::pending_for_changeset(&conn, changeset_id)?.is_none()
    {
        return Err(anyhow!(
            "No pending generation publication debt found for changeset {changeset_id}"
        ));
    }

    let debts = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)?;
    if debts.is_empty() {
        println!("Generation publication is already current.");
        return Ok(());
    }

    let runtime_root = runtime_root_for_generation_db_path(db_path);
    let mut engine = conary_core::transaction::TransactionEngine::new(
        conary_core::transaction::TransactionConfig::from_paths(
            runtime_root.root().to_path_buf(),
            runtime_root.db_path().to_path_buf(),
        ),
    )?;
    engine.begin()?;
    let result = crate::commands::generation::publication::publish_current_db_state(
        &conn,
        crate::commands::generation::publication::PublicationRequest {
            db_path,
            summary: "Retry pending generation publication",
            trigger_changeset_id: changeset,
            tx_uuid: None,
            prev_etc_snapshot: None,
        },
    );
    engine.release_lock();

    let outcome = result?;
    if outcome.needs_publication {
        return Err(anyhow!(
            "Generation publication is still pending. Retry with: {}",
            outcome.retry_command.unwrap_or_else(crate::commands::generation::publication::PublicationOutcome::retry_command)
        ));
    }

    println!(
        "Generation publication complete: generation {} selected.",
        outcome.generation_number.unwrap_or_default()
    );
    Ok(())
}

pub fn cmd_generation_pending(db_path: &str) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    let debts = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)?;
    if debts.is_empty() {
        println!("No pending generation publication debt.");
        return Ok(());
    }

    println!("Pending generation publication debt:");
    for debt in debts {
        let id = debt.id.unwrap_or_default();
        let changeset = debt
            .trigger_changeset_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  [{id}] changeset={changeset} status={} phase={} generation={} state={} retry=\"{}\"",
            debt.status.as_str(),
            debt.phase.as_str(),
            debt.generation_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
            debt.state_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
            crate::commands::generation::publication::PublicationOutcome::retry_command()
        );
    }
    Ok(())
}
```

- [ ] **Step 4: Dispatch the new commands**

In `apps/conary/src/dispatch.rs`, add match arms:

```rust
            cli::GenerationCommands::Publish { changeset, db } => {
                require_live_mutation(
                    allow_live_system_mutation,
                    Cow::Borrowed("conary system generation publish"),
                    LiveMutationClass::AlwaysLive,
                    false,
                )?;
                commands::generation::commands::cmd_generation_publish(&db.db_path, changeset)
            }
            cli::GenerationCommands::Pending { db } => {
                commands::generation::commands::cmd_generation_pending(&db.db_path)
            }
```

- [ ] **Step 5: Mark history lines with publication status**

In `apps/conary/src/commands/query/history.rs`, add a helper:

```rust
fn publication_marker_for_changeset(
    publications: &[conary_core::db::models::GenerationPublication],
    changeset_id: Option<i64>,
) -> &'static str {
    let Some(changeset_id) = changeset_id else {
        return "";
    };
    publications
        .iter()
        .find(|publication| publication.trigger_changeset_id == Some(changeset_id))
        .map(|publication| match publication.status {
            conary_core::db::models::GenerationPublicationStatus::Failed => " [publication-failed]",
            conary_core::db::models::GenerationPublicationStatus::Pending
            | conary_core::db::models::GenerationPublicationStatus::Running => " [publication-pending]",
            conary_core::db::models::GenerationPublicationStatus::Complete
            | conary_core::db::models::GenerationPublicationStatus::Abandoned => "",
        })
        .unwrap_or("")
}
```

Update `format_deferred_follow_up_lines` so legacy `generation_rebuild` metadata never prints the old `system generation build` retry hint:

```rust
fn deferred_retry_hint(follow_up: &crate::commands::DeferredFollowUp) -> String {
    let (kind, _) = crate::commands::classify_deferred_follow_up(follow_up);
    match kind {
        crate::commands::DeferredFollowUpKind::GenerationPublication
        | crate::commands::DeferredFollowUpKind::LegacyGenerationRebuild => {
            " Retry: conary --allow-live-system-mutation system generation publish.".to_string()
        }
        crate::commands::DeferredFollowUpKind::Other => follow_up
            .retry_command
            .as_ref()
            .map(|command| format!(" Retry: {command}."))
            .unwrap_or_default(),
    }
}
```

Then use `deferred_retry_hint(&follow_up)` inside `format_deferred_follow_up_lines` instead of formatting `follow_up.retry_command` directly.

Update `cmd_history` to fetch pending recoverable debts and append the marker when printing. Keep the existing `[deferred]` marker for non-publication follow-ups.

Add tests:

```rust
#[test]
fn publication_marker_marks_failed_debt() {
    let publication = conary_core::db::models::GenerationPublication {
        id: Some(1),
        trigger_changeset_id: Some(8),
        published_through_changeset_id: None,
        tx_uuid: None,
        db_path: "/tmp/db".to_string(),
        runtime_root: "/tmp/root".to_string(),
        phase: conary_core::db::models::GenerationPublicationPhase::PendingBuild,
        status: conary_core::db::models::GenerationPublicationStatus::Failed,
        state_number: None,
        generation_number: None,
        summary: "fixture".to_string(),
        last_error: Some("forced".to_string()),
        retry_count: 1,
        recoverable: true,
        created_at: None,
        updated_at: None,
        completed_at: None,
    };
    assert_eq!(
        publication_marker_for_changeset(&[publication], Some(8)),
        " [publication-failed]"
    );
}

#[test]
fn legacy_generation_rebuild_deferred_line_uses_publish_retry() {
    let warning = crate::commands::DeferredFollowUp {
        kind: "generation_rebuild".to_string(),
        status: "failed".to_string(),
        message: "root is not self-contained".to_string(),
        retry_command: Some(
            "conary --allow-live-system-mutation system generation build --summary retry"
                .to_string(),
        ),
    };
    let mut changeset = conary_core::db::models::Changeset::new("Install fixture".to_string());
    changeset.id = Some(8);
    changeset.metadata = Some(
        crate::commands::metadata_with_deferred_follow_up(Vec::new(), vec![warning]).unwrap(),
    );
    let details = format_deferred_follow_up_lines(&changeset);
    assert_eq!(details.len(), 1);
    assert!(details[0].contains("system generation publish"));
    assert!(!details[0].contains("system generation build"));
}
```

- [ ] **Step 6: Run CLI surface tests**

```bash
cargo test -p conary command_risk::tests::classify_generation
cargo test -p conary generation::commands::tests
cargo test -p conary query::history::tests
cargo build -p conary
```

Expected: all pass.

- [ ] **Step 7: Commit Task 5**

```bash
git add apps/conary/src/cli/generation.rs \
        apps/conary/src/command_risk.rs \
        apps/conary/src/dispatch.rs \
        apps/conary/src/commands/generation/commands.rs \
        apps/conary/src/commands/generation/publication.rs \
        apps/conary/src/commands/query/history.rs
git commit -m "feat(cli): expose generation publication debt"
```

---

### Task 6: Recovery Guardrails

**Files:**
- Modify: `crates/conary-core/src/transaction/recovery.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`

- [ ] **Step 1: Add core recovery debt check helper**

In `crates/conary-core/src/transaction/recovery.rs`, add:

```rust
fn pending_publication_debt(conn: &Connection) -> Result<Vec<GenerationPublication>> {
    GenerationPublication::pending_recoverable(conn)
}
```

Update imports:

```rust
use crate::db::models::{
    GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus, SystemState,
};
```

- [ ] **Step 2: Guard selected-generation success path**

At the start of `recover_with_policy`, before accepting `current_generation`, load pending debt:

```rust
let pending_debt = pending_publication_debt(conn)?;
```

When selected artifact validation succeeds and `policy == RecoveryScanPolicy::SelectedGenerationOnly`, replace the immediate return with:

```rust
if !pending_debt.is_empty() {
    if pending_debt.iter().all(|debt| {
        debt.generation_number == Some(current_num)
            && debt.phase == GenerationPublicationPhase::CurrentPublished
    }) {
        mark_generation_state_active_if_present(conn, current_num)?;
        let completed = GenerationPublication::mark_complete_through(
            conn,
            GenerationPublication::high_water_changeset_id(conn)?,
            current_num,
            current_num,
        )?;
        tracing::info!(
            completed,
            "Recovery completed publication debt for durably selected generation {current_num}"
        );
        return Ok(());
    }
    return Err(crate::Error::RecoveryFailed(format!(
        "Pending generation publication debt exists; run `conary --allow-live-system-mutation system generation publish` before accepting selected generation {current_num} as recovered"
    )));
}
return mark_generation_state_active_if_present(conn, current_num);
```

This closes the current short-circuit without trying to rebuild full current DB state inside `conary-core`.

- [ ] **Step 3: Keep boot-selection recovery available**

For `RecoveryScanPolicy::SelectedOrLatestArtifact`, do not fail just because pending debt exists. Add a warning before the mount/scan logic:

```rust
if !pending_debt.is_empty() {
    tracing::warn!(
        count = pending_debt.len(),
        "Boot-selection recovery found pending generation publication debt; booting a valid published generation and leaving debt visible for later publish retry"
    );
}
```

Do not mark pending debts complete in this policy unless the selected current generation matches debt at `CurrentPublished` and active marking succeeds.

- [ ] **Step 4: Add recovery debt query tests**

Add tests in `recovery.rs`:

```rust
#[test]
fn pending_publication_debt_reads_recoverable_rows() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let db_path = root.join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();

    conn.execute(
        "INSERT INTO generation_publications (
            db_path, runtime_root, phase, status, summary
         ) VALUES (?1, ?2, 'pending_build', 'failed', 'fixture')",
        (db_path.display().to_string(), root.display().to_string()),
    )
    .unwrap();

    let debts = pending_publication_debt(&conn).unwrap();
    assert_eq!(debts.len(), 1);
    assert_eq!(
        debts[0].status,
        GenerationPublicationStatus::Failed
    );
}
```

- [ ] **Step 5: Add source-order recovery regressions**

Add these tests in `recovery.rs` to pin the short-circuit fix and the `CurrentPublished` completion branch:

```rust
#[test]
fn recovery_checks_publication_debt_before_accepting_selected_current() {
    let source = include_str!("recovery.rs");
    let debt_pos = source
        .find("pending_publication_debt(conn)?")
        .expect("recovery should load publication debt");
    let current_pos = source
        .find("current_generation(&self.config.root)")
        .expect("recovery should inspect selected current generation");
    assert!(
        debt_pos < current_pos,
        "publication debt must be loaded before selected generation can short-circuit recovery"
    );
    assert!(source.contains("Pending generation publication debt exists"));
    assert!(source.contains("system generation publish"));
}

#[test]
fn recovery_completes_current_published_debt_branch() {
    let source = include_str!("recovery.rs");
    assert!(source.contains("GenerationPublicationPhase::CurrentPublished"));
    assert!(source.contains("GenerationPublication::mark_complete_through"));
    assert!(source.contains("Recovery completed publication debt for durably selected generation"));
}
```

- [ ] **Step 6: Run recovery tests**

```bash
cargo test -p conary-core transaction::recovery
```

Expected: all recovery tests pass.

- [ ] **Step 7: Commit Task 6**

```bash
git add crates/conary-core/src/transaction/recovery.rs \
        apps/conary/src/commands/generation/commands.rs
git commit -m "fix(recovery): honor pending generation publication debt"
```

---

### Task 7: Daemon Truthfulness And Generation GC Protection

**Files:**
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Modify: `apps/conaryd/src/daemon/routes/query.rs`
- Modify: `apps/conaryd/src/daemon/routes/system.rs`
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`

- [ ] **Step 1: Add publication status to conaryd history entries**

In `apps/conaryd/src/daemon/routes.rs`, extend `HistoryEntry`:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    // existing fields
    pub publication_status: Option<String>,
}
```

Add helper:

```rust
fn publication_status_for_changeset(
    publications: &[conary_core::db::models::GenerationPublication],
    changeset_id: Option<i64>,
) -> Option<String> {
    let changeset_id = changeset_id?;
    publications
        .iter()
        .find(|publication| publication.trigger_changeset_id == Some(changeset_id))
        .map(|publication| publication.status.as_str().to_string())
}
```

Do not make `From<&Changeset>` lie by setting `publication_status: None` for all callers. Prefer a new constructor:

```rust
impl HistoryEntry {
    fn from_changeset_with_publication(
        cs: &Changeset,
        publications: &[conary_core::db::models::GenerationPublication],
    ) -> Self {
        let mut entry = Self::from(cs);
        entry.publication_status = publication_status_for_changeset(publications, cs.id);
        entry
    }
}
```

- [ ] **Step 2: Update `/v1/history`**

In `apps/conaryd/src/daemon/routes/query.rs`, update `history_handler`:

```rust
let (changesets, publications) = run_db_query(&state, |conn| {
    Ok((
        Changeset::list_all(conn)?,
        conary_core::db::models::GenerationPublication::pending_recoverable(conn)?,
    ))
})
.await?;
let history: Vec<HistoryEntry> = changesets
    .iter()
    .map(|changeset| HistoryEntry::from_changeset_with_publication(changeset, &publications))
    .collect();
```

- [ ] **Step 3: Update transaction and system-state surfaces minimally**

For `/v1/transactions`, `/v1/transactions/{id}`, and `/v1/system/states`, add a field named `publication_status` or `pending_publications` only where the response already maps to DB changesets or system state. If a response is daemon-job-only and has no DB changeset, leave the field absent and add a test proving it does not claim publication success.

Use this query when a DB connection is available:

```rust
let pending_publications =
    conary_core::db::models::GenerationPublication::pending_recoverable(conn)?.len();
```

- [ ] **Step 4: Protect incomplete publication generations from GC**

In `apps/conary/src/commands/generation/commands.rs`, after loading `gc_roots`, add:

```rust
let publication_roots =
    conary_core::db::models::GenerationPublication::protected_generation_numbers(&conn)?;
```

Add them to `keep_set`:

```rust
for root in &publication_roots {
    keep_set.insert(*root);
}
```

When removing `pending_numbers`, skip any generation protected by publication debt:

```rust
if keep_set.contains(gen_number) {
    info!("Keeping pending generation {gen_number} because publication debt references it");
    continue;
}
```

- [ ] **Step 5: Add tests**

Add a conaryd helper test near `HistoryEntry` tests in `apps/conaryd/src/daemon/routes.rs`:

```rust
#[test]
fn history_publication_status_matches_changeset_debt() {
    let publications = vec![conary_core::db::models::GenerationPublication {
        id: Some(1),
        trigger_changeset_id: Some(42),
        published_through_changeset_id: None,
        tx_uuid: None,
        db_path: "/tmp/conary.db".to_string(),
        runtime_root: "/tmp/conary".to_string(),
        phase: conary_core::db::models::GenerationPublicationPhase::PendingBuild,
        status: conary_core::db::models::GenerationPublicationStatus::Failed,
        state_number: None,
        generation_number: None,
        summary: "fixture".to_string(),
        last_error: Some("forced".to_string()),
        retry_count: 1,
        recoverable: true,
        created_at: None,
        updated_at: None,
        completed_at: None,
    }];
    assert_eq!(
        publication_status_for_changeset(&publications, Some(42)),
        Some("failed".to_string())
    );
    assert_eq!(publication_status_for_changeset(&publications, Some(7)), None);
}
```

Add a GC unit test in `generation/commands.rs`:

```rust
#[test]
fn generation_gc_keeps_generation_referenced_by_publication_debt() {
    let source = include_str!("commands.rs");
    let query_pos = source
        .find("GenerationPublication::protected_generation_numbers")
        .expect("generation GC should query publication-protected roots");
    let insert_pos = source
        .find("for root in &publication_roots")
        .expect("generation GC should add publication roots to keep_set");
    let remove_pos = source
        .find("for gen_number in &pending_numbers")
        .expect("generation GC should evaluate pending generations for deletion");
    assert!(query_pos < remove_pos);
    assert!(insert_pos < remove_pos);
    assert!(source.contains("publication debt references it"));
}
```

- [ ] **Step 6: Run daemon and GC tests**

```bash
cargo test -p conary generation::commands::tests::generation_gc
cargo test -p conaryd history
cargo test -p conaryd transactions
cargo test -p conaryd system
```

Expected: all tests pass.

- [ ] **Step 7: Commit Task 7**

```bash
git add apps/conaryd/src/daemon/routes.rs \
        apps/conaryd/src/daemon/routes/query.rs \
        apps/conaryd/src/daemon/routes/system.rs \
        apps/conaryd/src/daemon/routes/transactions.rs \
        apps/conary/src/commands/generation/commands.rs
git commit -m "fix(api): surface pending generation publication state"
```

---

### Task 8: B2 Durable Filesystem Sweep

**Files:**
- Modify: `crates/conary-core/src/filesystem/durable.rs`
- Modify: `crates/conary-core/src/generation/metadata.rs`
- Modify: `apps/conary/src/commands/operation_records.rs`
- Modify: `apps/conary/src/commands/live_root.rs`

- [ ] **Step 1: Extend durable helpers**

In `crates/conary-core/src/filesystem/durable.rs`, add:

```rust
use serde::Serialize;
use std::fs::File;
use std::io::Write;

pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    sync_parent_directory(path)
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| Error::InternalError(format!("failed to serialize JSON: {error}")))?;
    write_file_atomic(path, &bytes)
}

pub fn remove_file_and_sync_parent(path: &Path) -> Result<()> {
    std::fs::remove_file(path)?;
    sync_parent_directory(path)
}
```

Add tests:

```rust
#[test]
fn write_json_atomic_writes_pretty_json_and_syncs_parent() {
    #[derive(serde::Serialize)]
    struct Fixture {
        name: &'static str,
    }
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("record.json");
    write_json_atomic(&path, &Fixture { name: "fixture" }).unwrap();
    let raw = std::fs::read_to_string(path).unwrap();
    assert!(raw.contains("\"fixture\""));
}
```

- [ ] **Step 2: Make generation metadata/signature/pending marker durable**

In `crates/conary-core/src/generation/metadata.rs`, replace manual temp-write/rename blocks with `write_json_atomic` or `write_file_atomic`. For pending marker removal, use `remove_file_and_sync_parent` and ignore `NotFound` only:

```rust
match crate::filesystem::durable::remove_file_and_sync_parent(&pending_path) {
    Ok(()) => Ok(()),
    Err(crate::Error::IoError(message)) if message.contains("No such file") => Ok(()),
    Err(error) => Err(error),
}
```

Preserve existing signature behavior. Do not remove signing.

- [ ] **Step 3: Make operation records durable**

In `apps/conary/src/commands/operation_records.rs`, replace `write_json_record` with:

```rust
pub fn write_json_record<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    conary_core::filesystem::durable::write_json_atomic(path, value)
        .map_err(|error| anyhow::anyhow!("{error}"))
}
```

Add a test:

```rust
#[test]
fn write_json_record_uses_atomic_tmp_path() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Fixture {
        value: String,
    }
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("record.json");
    write_json_record(&path, &Fixture { value: "ok".to_string() }).unwrap();
    assert!(!temp.path().join("record.json.tmp").exists());
    let loaded: Fixture = load_json_record(&path).unwrap();
    assert_eq!(loaded.value, "ok");
}
```

- [ ] **Step 4: Add parent sync in live-root target mutations**

In `apps/conary/src/commands/live_root.rs`, reuse its existing `sync_parent_directory` if present; otherwise call `conary_core::filesystem::durable::sync_parent_directory`. After every target `rename`, backup move, directory create, file remove, and directory remove that recovery depends on, call parent sync.

Use this common pattern:

```rust
fs::rename(&temp, &target)?;
sync_parent_directory(&target)?;
```

For removal:

```rust
fs::remove_file(&target)?;
sync_parent_directory(&target)?;
```

For directory creation:

```rust
fs::create_dir_all(&target)?;
sync_parent_directory(&target)?;
```

- [ ] **Step 5: Add B2 durability tests**

Add focused tests in the owning modules:

```rust
#[test]
fn operation_record_write_leaves_no_tmp_file() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Fixture {
        value: String,
    }
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("record.json");
    write_json_record(&path, &Fixture { value: "ok".to_string() }).unwrap();
    assert!(!temp.path().join("record.json.tmp").exists());
    let loaded: Fixture = load_json_record(&path).unwrap();
    assert_eq!(loaded.value, "ok");
}
```

For metadata:

```rust
#[test]
fn generation_metadata_write_leaves_no_tmp_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let metadata = GenerationMetadata {
        generation: 1,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(1),
        cas_objects_referenced: Some(0),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        created_at: "2026-05-26T00:00:00Z".to_string(),
        package_count: 0,
        kernel_version: None,
        summary: "fixture".to_string(),
    };
    metadata.write_to(temp.path()).unwrap();
    assert!(!temp.path().join(".conary-gen.json.tmp").exists());
}
```

For live root, add a structural regression test if direct fsync injection is impractical:

```rust
#[test]
fn live_root_install_syncs_target_parent_after_rename() {
    let source = include_str!("live_root.rs");
    assert!(
        source.contains("sync_parent_directory(&target)")
            || source.contains("durable::sync_parent_directory(&target)"),
        "live-root target rename path must sync target parent"
    );
}
```

- [ ] **Step 6: Run B2 tests**

```bash
cargo test -p conary-core filesystem::durable
cargo test -p conary-core generation::metadata
cargo test -p conary operation_records
cargo test -p conary live_root
```

Expected: all tests pass.

- [ ] **Step 7: Commit Task 8**

```bash
git add crates/conary-core/src/filesystem/durable.rs \
        crates/conary-core/src/generation/metadata.rs \
        apps/conary/src/commands/operation_records.rs \
        apps/conary/src/commands/live_root.rs
git commit -m "fix(fs): harden durable generation publication writes"
```

---

### Task 9: Documentation, Audit Metadata, And Final Verification

**Files:**
- Modify: `docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md`
- Modify: `docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `README.md`

- [ ] **Step 1: Update Plan B status in specs**

In `docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md`, update status to name the implementation commit range after all B1/B2 commits land:

```markdown
**Status:** Implemented in `<commit-range>`; archive when umbrella Plan C is split out or deferred
```

In the umbrella spec, update Plan B status from design-split-out to implemented:

```markdown
**Status:** Plan A implemented in `2e294320`; Plan B implemented in `<commit-range>`; Plan C remains open
```

- [ ] **Step 2: Update operator-facing docs**

In `README.md`, under `### System Generations`, add this paragraph after the generation command examples:

```markdown
When a package mutation commits but generation publication fails, Conary exits
successfully for the package transaction and records pending publication debt.
Run `conary system generation pending` to inspect it and
`conary --allow-live-system-mutation system generation publish` to retry
publication of the current DB state.
```

Do not broaden release claims or imply Plan C truth-check automation exists.

- [ ] **Step 3: Regenerate docs-audit inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 4: Update docs-audit ledger and summary**

Add or update ledger rows for every active doc changed. The Plan B implementation row must mention:

```text
generation-publication; recovery; crash-consistency; durable-fsync; implementation
```

Use evidence sources that include the spec, plan, new model, publication helper, recovery, and current-link helper files.

- [ ] **Step 5: Run focused final checks**

```bash
cargo test -p conary-core generation_publication
cargo test -p conary generation::publication
cargo test -p conary command_risk::tests::classify_generation
cargo test -p conary query::history::tests
cargo test -p conaryd history
```

Expected: all tests pass.

- [ ] **Step 6: Run workspace verification**

Run the full required gate:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
git diff --check
```

Expected: all pass. If Clippy finds unrelated pre-existing warnings, stop and report the exact warnings before changing unrelated files.

- [ ] **Step 7: Commit docs/status updates**

Review exact changed files:

```bash
git diff --name-only
```

Commit only the implementation-status docs and audit metadata:

```bash
git add docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md \
        docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md \
        README.md \
        docs/superpowers/documentation-accuracy-audit-inventory.tsv \
        docs/superpowers/documentation-accuracy-audit-ledger.tsv \
        docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: record generation publication durability status"
```

---

## Final Acceptance

Plan B is complete only when all of these are true:

- `generation_publications` exists at schema v69 and rejects unknown phase/status values.
- Package mutation publication builds current DB state and records debt on post-commit failure.
- `conary system generation publish` is the primary retry command and is idempotent when no debt exists.
- Optional `--changeset` never claims to replay historical changeset state.
- A later successful publication completes earlier covered publication debts.
- `/conary/current` rename is parent-directory synced before DB state is marked active.
- Recovery does not accept a valid selected generation as fully recovered while blocking publication debt exists.
- `system generation pending`, `system history`, and existing conaryd history/status surfaces expose pending/failed publication state.
- Generation GC preserves generations referenced by incomplete publication debt.
- Generation metadata, operation records, and live-root journal-adjacent writes have the B2 durable sync coverage.
- The full verification gate in Task 9 passes.

## Suggested `/goal` Prompt For Execution

When starting implementation, use:

```text
/goal Implement Conary Plan B generation publication durability from docs/superpowers/plans/2026-05-26-generation-publication-durability.md. Read AGENTS.md, docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md, apps/conary/src/commands/composefs_ops.rs, apps/conary/src/commands/install/mod.rs, apps/conary/src/commands/install/batch.rs, apps/conary/src/commands/remove.rs, crates/conary-core/src/generation/builder.rs, crates/conary-core/src/generation/mount.rs, crates/conary-core/src/transaction/recovery.rs, and crates/conary-core/src/db/models/state.rs first. Implement the plan task-by-task with exact-path commits, keep publication as a current-DB debt sweep rather than historical changeset replay, and stop only when the focused tests plus the full final gate pass.
```
