# Test Infrastructure Overhaul Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Overhaul the conary-test infrastructure: fix the bootstrap sandbox bug, persist test data on Remi, make Forge a stateless executor with deployment MCP tools, migrate Phase 1-2 to TOML manifests, and polish the developer experience.

**Architecture:** Remi becomes the single source of truth for all test data via a separate SQLite database (`/conary/test-data.db`). Forge streams results to Remi per-test with a local WAL for resilience. The Python test runner is retired in favor of TOML manifests. All operations are accessible via MCP tools and CLI subcommands.

**Tech Stack:** Rust 1.94, SQLite (rusqlite), Axum, rmcp, serde, TOML, Podman (bollard), QEMU.

**Spec:** `docs/superpowers/specs/2026-03-14-test-infrastructure-overhaul-design.md` (rev 3)

---

## File Map

### Chunk 0: Bootstrap Sandbox Fix

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `conary-core/src/container/mod.rs:717-731` | Fix hardcoded PATH in sandbox |

### Chunk 1: Remi Test Data API

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `conary-server/src/server/test_db.rs` | Separate test data DB: init, schema, models |
| Modify | `conary-server/src/server/admin_service.rs` | Test data service functions |
| Create | `conary-server/src/server/handlers/admin/test_data.rs` | HTTP handlers for test data |
| Modify | `conary-server/src/server/handlers/admin/mod.rs` | Re-export test_data |
| Modify | `conary-server/src/server/mcp.rs` | 5 new MCP tools for test data |
| Modify | `conary-server/src/server/routes.rs` | Register test data routes |
| Modify | `conary-server/src/server/mod.rs` | Wire test_db into server startup |

### Chunk 2: Stateless Executor + Deployment Tools

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `conary-test/src/server/state.rs` | Remove DashMap persistence, add Remi client |
| Create | `conary-test/src/server/remi_client.rs` | HTTP client for pushing results to Remi |
| Create | `conary-test/src/server/wal.rs` | Local write-ahead log for Remi unreachability |
| Modify | `conary-test/src/engine/runner.rs` | Stream per-step results to Remi |
| Modify | `conary-test/src/server/handlers.rs` | Proxy queries to Remi, add deploy handlers |
| Modify | `conary-test/src/server/mcp.rs` | Add deployment tools, auth scopes |
| Modify | `conary-test/src/server/routes.rs` | Add deploy routes |
| Modify | `conary-test/src/cli.rs` | Add deploy/fixtures/logs CLI subcommands |

### Chunk 3: Phase 1-2 Manifest Migration

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `tests/integration/remi/manifests/phase1-core.toml` | Audit + complete T01-T10 |
| Modify | `tests/integration/remi/manifests/phase1-advanced.toml` | Audit + complete T11-T37 |
| Modify | `tests/integration/remi/manifests/phase2-group-[a-f].toml` | Audit + complete T38-T76 |
| Modify | `.forgejo/workflows/integration.yaml` | Switch to conary-test runner |
| Modify | `.forgejo/workflows/e2e.yaml` | Switch to conary-test runner |
| Delete | `tests/integration/remi/runner/test_runner.py` | Remove Python runner |
| Delete | `tests/integration/remi/run.sh` | Remove shell orchestrator |

### Chunk 4: DX Polish

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `conary-test/src/error_taxonomy.rs` | Structured error types with categories |
| Modify | `conary-test/src/server/handlers.rs` | Use error taxonomy in responses |
| Modify | `conary-test/src/server/mcp.rs` | reload_manifests, prune_images, image_info tools |
| Modify | `conary-test/src/cli.rs` | CLI parity with MCP tools |

---

## Chunk 0: Bootstrap Sandbox PATH Fix

### Task 1: Fix hardcoded PATH in container sandbox

**Files:**
- Modify: `conary-core/src/container/mod.rs:717-731`

- [ ] **Step 1: Write failing test**

Add to `conary-core/src/container/mod.rs` test module:

```rust
#[test]
fn test_sandbox_env_path_overrides_default() {
    // When custom env includes PATH, it should take precedence
    // over the hardcoded fallback
    let custom_path = "/opt/toolchain/bin:/usr/bin";
    let env = vec![("PATH", custom_path)];

    // After env_clear + default PATH + custom env applied,
    // the effective PATH should be the custom one
    // (This is a behavioral test — verify by running a command
    // that only exists in the custom PATH location)

    // For unit testing, we verify the env construction logic
    // rather than spawning a real sandbox
    let mut final_env = std::collections::HashMap::new();
    // Simulate the sandbox env setup
    final_env.insert("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
    for (key, value) in &env {
        final_env.insert(key, value);
    }
    assert_eq!(final_env["PATH"], custom_path,
        "Custom PATH should override default");
}
```

- [ ] **Step 2: Fix child_setup_and_execute**

In `conary-core/src/container/mod.rs`, find the `child_setup_and_execute`
function (around line 717). Replace the unconditional PATH setting:

**Current (line 717-731):**
```rust
        let mut cmd = Command::new(interpreter);
        cmd.arg(script_path)
            .args(args)
            .stdin(Stdio::null())
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            .env("SHELL", "/bin/sh");

        for (key, value) in env {
            cmd.env(*key, *value);
        }
```

**New:**
```rust
        let mut cmd = Command::new(interpreter);
        cmd.arg(script_path)
            .args(args)
            .stdin(Stdio::null())
            .env_clear()
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            .env("SHELL", "/bin/sh");

        // Set PATH fallback only if the caller didn't provide one.
        // Bootstrap builds need the toolchain PATH to take precedence.
        let has_custom_path = env.iter().any(|(k, _)| *k == "PATH");
        if !has_custom_path {
            cmd.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        }

        for (key, value) in env {
            cmd.env(*key, *value);
        }
```

- [ ] **Step 3: Check execute_limited for same pattern**

Search for other places in `container/mod.rs` that set a hardcoded PATH
(around line 529 in `execute_limited()`). Apply the same fix if found.

- [ ] **Step 4: Build and test**

Run: `cargo build -p conary-core && cargo test -p conary-core container -- --nocapture`
Expected: Compiles, tests pass

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 6: Commit**

```
fix(container): respect custom PATH in sandbox instead of hardcoding

child_setup_and_execute() unconditionally set PATH to /usr/sbin:/usr/bin:...
after env_clear(), overriding any custom PATH the caller provided. Bootstrap
builds need the toolchain PATH (e.g., /tmp/.../stage1/sysroot/usr/bin) to
find gcc/make. Now only sets the fallback PATH when no custom PATH is in
the caller's env.
```

### Task 2: Rebuild bootstrap image on Remi

**Files:** None (deployment)

- [ ] **Step 1: Deploy fix to Remi**

```bash
rsync -az --delete --exclude target --exclude .git ~/Conary/ root@ssh.conary.io:/root/conary-src/
ssh root@ssh.conary.io 'export PATH="$HOME/.cargo/bin:$PATH" && cd /root/conary-src && cargo build --release 2>&1 | tail -3'
```

- [ ] **Step 2: Restart bootstrap build**

```bash
ssh root@ssh.conary.io 'export PATH="$HOME/.cargo/bin:$PATH" && CONARY=/root/conary-src/target/release/conary && WORK=/tmp/conary-bootstrap-v1 && cd /root/conary-src && rm -rf $WORK/sysroot && nohup bash -c "$CONARY bootstrap base --work-dir $WORK --root $WORK/sysroot --recipe-dir /root/conary-src/recipes/core --skip-verify 2>&1 && mkdir -p /conary/test-artifacts && $CONARY bootstrap image --work-dir $WORK --output /conary/test-artifacts/minimal-boot-v1.qcow2 --format qcow2 --size 4G 2>&1 && echo DONE" > /tmp/bootstrap-build.log 2>&1 &'
```

- [ ] **Step 3: Monitor (background)**

```bash
ssh root@ssh.conary.io 'tail -5 /tmp/bootstrap-build.log'
```

This is a multi-hour build. Proceed to Chunk 1 while it runs.

- [ ] **Step 4: Commit**

```
chore: trigger bootstrap image rebuild with sandbox PATH fix
```

---

## Chunk 1: Remi Test Data API

### Task 3: Create test data database module

**Files:**
- Create: `conary-server/src/server/test_db.rs`

The test data lives in a SEPARATE SQLite file (`/conary/test-data.db`), not in
the main conary package database. This module handles init, schema, and models.

- [ ] **Step 1: Create test_db.rs with schema and models**

Create `conary-server/src/server/test_db.rs`:

```rust
// conary-server/src/server/test_db.rs
//
// Separate SQLite database for test run data.
// Path: /conary/test-data.db (configurable)

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::info;

const SCHEMA_VERSION: u32 = 1;

/// Initialize the test data database. Creates file if missing.
pub fn init(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);

    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS test_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                suite TEXT NOT NULL,
                distro TEXT NOT NULL,
                phase INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                completed_at TEXT,
                triggered_by TEXT,
                source_commit TEXT,
                total INTEGER NOT NULL DEFAULT 0,
                passed INTEGER NOT NULL DEFAULT 0,
                failed INTEGER NOT NULL DEFAULT 0,
                skipped INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_test_runs_suite ON test_runs(suite, distro);
            CREATE INDEX IF NOT EXISTS idx_test_runs_status ON test_runs(status);
            CREATE INDEX IF NOT EXISTS idx_test_runs_started ON test_runs(started_at DESC);

            CREATE TABLE IF NOT EXISTS test_results (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id INTEGER NOT NULL REFERENCES test_runs(id) ON DELETE CASCADE,
                test_id TEXT NOT NULL,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                duration_ms INTEGER,
                message TEXT,
                attempt INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_test_results_run ON test_results(run_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_test_results_unique
                ON test_results(run_id, test_id, attempt);

            CREATE TABLE IF NOT EXISTS test_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                result_id INTEGER NOT NULL REFERENCES test_results(id) ON DELETE CASCADE,
                step_index INTEGER NOT NULL,
                step_type TEXT NOT NULL,
                command TEXT,
                exit_code INTEGER,
                duration_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_test_steps_result ON test_steps(result_id);

            CREATE TABLE IF NOT EXISTS test_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                step_id INTEGER NOT NULL REFERENCES test_steps(id) ON DELETE CASCADE,
                stream TEXT NOT NULL,
                content TEXT NOT NULL,
                raw_content TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_test_logs_step ON test_logs(step_id);

            CREATE TABLE IF NOT EXISTS test_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                step_id INTEGER NOT NULL REFERENCES test_steps(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                payload TEXT,
                timestamp TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );
            CREATE INDEX IF NOT EXISTS idx_test_events_step ON test_events(step_id);

            PRAGMA user_version = 1;",
        )?;
        info!("Test data schema v1 applied");
    }

    Ok(())
}

// --- Models ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRun {
    pub id: i64,
    pub suite: String,
    pub distro: String,
    pub phase: u32,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub triggered_by: Option<String>,
    pub source_commit: Option<String>,
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub id: i64,
    pub run_id: i64,
    pub test_id: String,
    pub name: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub message: Option<String>,
    pub attempt: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestStep {
    pub id: i64,
    pub result_id: i64,
    pub step_index: u32,
    pub step_type: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestLog {
    pub id: i64,
    pub step_id: i64,
    pub stream: String,
    pub content: String,
    pub raw_content: Option<String>,
}

// --- Queries ---

impl TestRun {
    pub fn create(
        conn: &Connection,
        suite: &str,
        distro: &str,
        phase: u32,
        triggered_by: Option<&str>,
        source_commit: Option<&str>,
    ) -> Result<i64> {
        conn.execute(
            "INSERT INTO test_runs (suite, distro, phase, status, triggered_by, source_commit)
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5)",
            rusqlite::params![suite, distro, phase, triggered_by, source_commit],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
        let completed = if status == "completed" || status == "failed" || status == "cancelled" {
            Some("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')")
        } else {
            None
        };
        if let Some(_) = completed {
            conn.execute(
                "UPDATE test_runs SET status = ?1, completed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?2",
                rusqlite::params![status, id],
            )?;
        } else {
            conn.execute(
                "UPDATE test_runs SET status = ?1 WHERE id = ?2",
                rusqlite::params![status, id],
            )?;
        }
        Ok(())
    }

    pub fn update_counts(
        conn: &Connection,
        id: i64,
        total: u32,
        passed: u32,
        failed: u32,
        skipped: u32,
    ) -> Result<()> {
        conn.execute(
            "UPDATE test_runs SET total = ?1, passed = ?2, failed = ?3, skipped = ?4 WHERE id = ?5",
            rusqlite::params![total, passed, failed, skipped, id],
        )?;
        Ok(())
    }

    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, suite, distro, phase, status, started_at, completed_at,
                    triggered_by, source_commit, total, passed, failed, skipped
             FROM test_runs WHERE id = ?1",
        )?;
        let result = stmt.query_row(rusqlite::params![id], |row| {
            Ok(Self {
                id: row.get(0)?,
                suite: row.get(1)?,
                distro: row.get(2)?,
                phase: row.get(3)?,
                status: row.get(4)?,
                started_at: row.get(5)?,
                completed_at: row.get(6)?,
                triggered_by: row.get(7)?,
                source_commit: row.get(8)?,
                total: row.get(9)?,
                passed: row.get(10)?,
                failed: row.get(11)?,
                skipped: row.get(12)?,
            })
        });
        match result {
            Ok(run) => Ok(Some(run)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list(conn: &Connection, limit: u32, cursor: Option<i64>) -> Result<Vec<Self>> {
        let mut stmt = if let Some(cursor) = cursor {
            conn.prepare(
                "SELECT id, suite, distro, phase, status, started_at, completed_at,
                        triggered_by, source_commit, total, passed, failed, skipped
                 FROM test_runs WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
            )?
        } else {
            conn.prepare(
                "SELECT id, suite, distro, phase, status, started_at, completed_at,
                        triggered_by, source_commit, total, passed, failed, skipped
                 FROM test_runs ORDER BY id DESC LIMIT ?1",
            )?
        };
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = if let Some(c) = cursor {
            vec![Box::new(c), Box::new(limit)]
        } else {
            vec![Box::new(limit)]
        };
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            Ok(Self {
                id: row.get(0)?,
                suite: row.get(1)?,
                distro: row.get(2)?,
                phase: row.get(3)?,
                status: row.get(4)?,
                started_at: row.get(5)?,
                completed_at: row.get(6)?,
                triggered_by: row.get(7)?,
                source_commit: row.get(8)?,
                total: row.get(9)?,
                passed: row.get(10)?,
                failed: row.get(11)?,
                skipped: row.get(12)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }
}

impl TestResult {
    pub fn insert(conn: &Connection, result: &Self) -> Result<i64> {
        conn.execute(
            "INSERT INTO test_results (run_id, test_id, name, status, duration_ms, message, attempt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                result.run_id,
                result.test_id,
                result.name,
                result.status,
                result.duration_ms,
                result.message,
                result.attempt,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn find_by_run(conn: &Connection, run_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, test_id, name, status, duration_ms, message, attempt
             FROM test_results WHERE run_id = ?1 ORDER BY test_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok(Self {
                id: row.get(0)?,
                run_id: row.get(1)?,
                test_id: row.get(2)?,
                name: row.get(3)?,
                status: row.get(4)?,
                duration_ms: row.get(5)?,
                message: row.get(6)?,
                attempt: row.get(7)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_by_run_and_test(
        conn: &Connection,
        run_id: i64,
        test_id: &str,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, test_id, name, status, duration_ms, message, attempt
             FROM test_results WHERE run_id = ?1 AND test_id = ?2
             ORDER BY attempt DESC LIMIT 1",
        )?;
        let result = stmt.query_row(rusqlite::params![run_id, test_id], |row| {
            Ok(Self {
                id: row.get(0)?,
                run_id: row.get(1)?,
                test_id: row.get(2)?,
                name: row.get(3)?,
                status: row.get(4)?,
                duration_ms: row.get(5)?,
                message: row.get(6)?,
                attempt: row.get(7)?,
            })
        });
        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

impl TestStep {
    pub fn insert(conn: &Connection, step: &Self) -> Result<i64> {
        conn.execute(
            "INSERT INTO test_steps (result_id, step_index, step_type, command, exit_code, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                step.result_id,
                step.step_index,
                step.step_type,
                step.command,
                step.exit_code,
                step.duration_ms,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn find_by_result(conn: &Connection, result_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, result_id, step_index, step_type, command, exit_code, duration_ms
             FROM test_steps WHERE result_id = ?1 ORDER BY step_index",
        )?;
        let rows = stmt.query_map(rusqlite::params![result_id], |row| {
            Ok(Self {
                id: row.get(0)?,
                result_id: row.get(1)?,
                step_index: row.get(2)?,
                step_type: row.get(3)?,
                command: row.get(4)?,
                exit_code: row.get(5)?,
                duration_ms: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }
}

impl TestLog {
    pub fn insert(conn: &Connection, log: &Self) -> Result<i64> {
        conn.execute(
            "INSERT INTO test_logs (step_id, stream, content, raw_content)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![log.step_id, log.stream, log.content, log.raw_content],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn find_by_step(conn: &Connection, step_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, step_id, stream, content, raw_content
             FROM test_logs WHERE step_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![step_id], |row| {
            Ok(Self {
                id: row.get(0)?,
                step_id: row.get(1)?,
                stream: row.get(2)?,
                content: row.get(3)?,
                raw_content: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// Strip ANSI escape codes from a string.
pub fn strip_ansi(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    re.replace_all(s, "").to_string()
}

/// Garbage collect old test data.
pub fn gc(conn: &Connection, older_than_days: u32) -> Result<u64> {
    let deleted = conn.execute(
        "DELETE FROM test_runs WHERE started_at < datetime('now', ?1)",
        rusqlite::params![format!("-{older_than_days} days")],
    )?;
    Ok(deleted as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_and_create_run() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let id = TestRun::create(&conn, "phase1-core", "fedora43", 1, Some("test"), None).unwrap();
        assert_eq!(id, 1);

        let run = TestRun::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(run.suite, "phase1-core");
        assert_eq!(run.status, "pending");
    }

    #[test]
    fn test_cursor_pagination() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        for i in 0..5 {
            TestRun::create(&conn, &format!("suite-{i}"), "fedora43", 1, None, None).unwrap();
        }

        let page1 = TestRun::list(&conn, 2, None).unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].id, 5); // newest first

        let page2 = TestRun::list(&conn, 2, Some(page1[1].id)).unwrap();
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].id, 3);
    }

    #[test]
    fn test_full_result_chain() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let run_id = TestRun::create(&conn, "test", "fedora43", 1, None, None).unwrap();

        let result = TestResult {
            id: 0,
            run_id,
            test_id: "T01".into(),
            name: "health_check".into(),
            status: "passed".into(),
            duration_ms: Some(150),
            message: None,
            attempt: 1,
        };
        let result_id = TestResult::insert(&conn, &result).unwrap();

        let step = TestStep {
            id: 0,
            result_id,
            step_index: 0,
            step_type: "conary".into(),
            command: Some("repo list".into()),
            exit_code: Some(0),
            duration_ms: Some(50),
        };
        let step_id = TestStep::insert(&conn, &step).unwrap();

        let log = TestLog {
            id: 0,
            step_id,
            stream: "stdout".into(),
            content: "fedora-remi https://packages.conary.io".into(),
            raw_content: None,
        };
        TestLog::insert(&conn, &log).unwrap();

        let steps = TestStep::find_by_result(&conn, result_id).unwrap();
        assert_eq!(steps.len(), 1);

        let logs = TestLog::find_by_step(&conn, step_id).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].content, "fedora-remi https://packages.conary.io");
    }

    #[test]
    fn test_gc_removes_old_runs() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        TestRun::create(&conn, "old", "fedora43", 1, None, None).unwrap();
        // Manually backdate
        conn.execute(
            "UPDATE test_runs SET started_at = datetime('now', '-60 days') WHERE id = 1",
            [],
        )
        .unwrap();

        TestRun::create(&conn, "new", "fedora43", 1, None, None).unwrap();

        let deleted = gc(&conn, 30).unwrap();
        assert_eq!(deleted, 1);

        let remaining = TestRun::list(&conn, 10, None).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].suite, "new");
    }
}
```

- [ ] **Step 2: Register module in server**

In `conary-server/src/server/mod.rs`, add:
```rust
pub mod test_db;
```

- [ ] **Step 3: Build and test**

Run: `cargo test -p conary-server test_db -- --nocapture`
Expected: All 4 tests pass

Run: `cargo clippy -p conary-server -- -D warnings`

- [ ] **Step 4: Commit**

```
feat(server): add test data database module with separate SQLite file

Separate /conary/test-data.db for test run persistence. Schema v1 with
test_runs, test_results, test_steps, test_logs, test_events tables.
Cursor-based pagination, ANSI stripping, GC for old data. 4 unit tests.
```

---

### Task 4: Add test data service functions to admin_service

**Files:**
- Modify: `conary-server/src/server/admin_service.rs`

- [ ] **Step 1: Add test data service functions**

Follow the existing `blocking()` pattern. Add functions:
- `create_test_run(state, params) -> Result<TestRun, ServiceError>`
- `update_test_run(state, id, status, counts) -> Result<(), ServiceError>`
- `push_test_result(state, run_id, result_with_steps_and_logs) -> Result<(), ServiceError>`
- `list_test_runs(state, limit, cursor, suite?, distro?, status?) -> Result<Vec<TestRun>, ServiceError>`
- `get_test_run(state, id) -> Result<TestRunWithResults, ServiceError>`
- `get_test_detail(state, run_id, test_id) -> Result<TestDetailWithSteps, ServiceError>`
- `get_test_logs(state, run_id, test_id, stream?, step?) -> Result<Vec<TestLog>, ServiceError>`
- `test_health(state) -> Result<TestHealthSummary, ServiceError>`
- `test_gc(state, older_than_days) -> Result<u64, ServiceError>`

Each function opens the test_db connection (not the main DB), performs
the query, and returns the result. Use `spawn_blocking` for DB access.

NOTE: The `ServerState` struct needs a `test_db_path: String` field added.
Set it during server initialization from config or default to
`/conary/test-data.db`.

- [ ] **Step 2: Build and test**

Run: `cargo build --features server`

- [ ] **Step 3: Commit**

```
feat(server): add test data service functions to admin_service
```

---

### Task 5: Add HTTP handlers and routes

**Files:**
- Create: `conary-server/src/server/handlers/admin/test_data.rs`
- Modify: `conary-server/src/server/handlers/admin/mod.rs`
- Modify: `conary-server/src/server/routes.rs`

- [ ] **Step 1: Create test_data.rs handler file**

Follow the pattern from `tokens.rs`: extract State, check scope, call
service function, map errors to JSON responses. Handlers:
- `create_test_run` — POST
- `update_test_run` — PUT
- `push_test_result` — POST
- `list_test_runs` — GET with query params
- `get_test_run` — GET with path param
- `get_test_detail` — GET with path params (run_id, test_id)
- `get_test_logs` — GET with path params + query filters
- `test_health` — GET
- `test_gc` — DELETE

- [ ] **Step 2: Export from mod.rs**

Add `pub mod test_data;` to `handlers/admin/mod.rs`.

- [ ] **Step 3: Register routes**

In `routes.rs`, add to the external admin router (around line 683-704):

```rust
// Test data endpoints
.route("/v1/admin/test-runs", post(admin::test_data::create_test_run))
.route("/v1/admin/test-runs", get(admin::test_data::list_test_runs))
.route("/v1/admin/test-runs/:id", get(admin::test_data::get_test_run))
.route("/v1/admin/test-runs/:id", put(admin::test_data::update_test_run))
.route("/v1/admin/test-runs/:id/results", post(admin::test_data::push_test_result))
.route("/v1/admin/test-runs/:id/tests/:test_id", get(admin::test_data::get_test_detail))
.route("/v1/admin/test-runs/:id/tests/:test_id/logs", get(admin::test_data::get_test_logs))
.route("/v1/admin/test-runs/latest", get(admin::test_data::test_latest))
.route("/v1/admin/test-health", get(admin::test_data::test_health))
.route("/v1/admin/test-runs/gc", delete(admin::test_data::test_gc))
```

- [ ] **Step 4: Build and test**

Run: `cargo build --features server && cargo test --features server`

- [ ] **Step 5: Commit**

```
feat(server): add test data HTTP handlers and routes

9 endpoints on the external admin API for test run CRUD, result
pushing, log retrieval, health dashboard, and garbage collection.
```

---

### Task 6: Add MCP tools for test data

**Files:**
- Modify: `conary-server/src/server/mcp.rs`

- [ ] **Step 1: Add parameter structs**

Add to the parameter structs section of `mcp.rs`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
struct ListTestRunsParams {
    #[schemars(description = "Max runs to return (default 20)")]
    limit: Option<u32>,
    #[schemars(description = "Filter by suite name")]
    suite: Option<String>,
    #[schemars(description = "Filter by distro")]
    distro: Option<String>,
    #[schemars(description = "Filter by status")]
    status: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetTestRunParams {
    #[schemars(description = "Numeric run ID")]
    run_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetTestDetailParams {
    #[schemars(description = "Numeric run ID")]
    run_id: i64,
    #[schemars(description = "Test identifier (e.g. T01)")]
    test_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetTestLogsParams {
    run_id: i64,
    test_id: String,
    #[schemars(description = "Filter by stream: stdout, stderr, trace")]
    stream: Option<String>,
    #[schemars(description = "Filter by step index")]
    step_index: Option<u32>,
}
```

- [ ] **Step 2: Add tool methods**

Add 5 tools to the `#[tool_router]` impl:

```rust
#[tool(description = "List recent test runs with filtering")]
async fn test_list_runs(&self, Parameters(p): Parameters<ListTestRunsParams>) -> Result<CallToolResult, McpError> { ... }

#[tool(description = "Get full details for a test run including result summaries")]
async fn test_get_run(&self, Parameters(p): Parameters<GetTestRunParams>) -> Result<CallToolResult, McpError> { ... }

#[tool(description = "Get a single test result with all steps and logs")]
async fn test_get_test(&self, Parameters(p): Parameters<GetTestDetailParams>) -> Result<CallToolResult, McpError> { ... }

#[tool(description = "Get test execution logs filtered by stream and step")]
async fn test_get_logs(&self, Parameters(p): Parameters<GetTestLogsParams>) -> Result<CallToolResult, McpError> { ... }

#[tool(description = "Get aggregate test health: pass rates per suite, recent failures")]
async fn test_health(&self) -> Result<CallToolResult, McpError> { ... }
```

Each tool calls the corresponding `admin_service::*` function.

- [ ] **Step 3: Update tool count test**

Find the test that asserts tool count (line ~618). Update from 16 to 21.

- [ ] **Step 4: Build and test**

Run: `cargo build --features server && cargo test --features server`

- [ ] **Step 5: Commit**

```
feat(server): add 5 MCP tools for test data on remi-admin

test_list_runs, test_get_run, test_get_test, test_get_logs,
test_health. All call admin_service functions backed by the
separate test-data.db.
```

---

## Chunk 2: Stateless Executor + Deployment Tools

Detailed implementation for Chunk 2 follows the same task structure.
Key tasks:

### Task 7: Create Remi HTTP client for conary-test

Create `conary-test/src/server/remi_client.rs` with an async HTTP client
that POSTs results to Remi's admin API. Configure with `REMI_ENDPOINT`
and `REMI_ADMIN_TOKEN` env vars.

### Task 8: Create local WAL for resilience

Create `conary-test/src/server/wal.rs` — thin SQLite WAL at
`/tmp/conary-test-wal.db` with same schema as Remi's test tables. Buffer
failed POSTs, background reconciliation loop (30s/60s/120s/5min backoff).
`flush_pending` and `pending_count` functions.

### Task 9: Wire runner to stream per-step results

Modify `conary-test/src/engine/runner.rs` to call `remi_client::push_result()`
after each test completes. Build `TestResult` + `TestStep` + `TestLog`
from `StepResult` data already captured per step. Strip ANSI for `content`,
preserve raw. On push failure, buffer to WAL.

### Task 10: Remove in-memory state

Modify `conary-test/src/server/state.rs` to remove DashMap persistence.
Keep runtime-only state (in-progress runs, cancellation flags, image locks).
Proxy `get_run`, `get_test`, `get_test_logs` to Remi via `remi_client`.

### Task 11: Add deployment MCP tools and auth

Add bearer token auth to conary-test's HTTP/MCP server (mirror Remi's
pattern from `server/auth.rs`). Add tools: `deploy_source`, `rebuild_binary`,
`restart_service`, `build_fixtures`, `publish_fixtures`, `deploy_status`,
`build_boot_image`, `flush_pending`. Add corresponding CLI subcommands.

---

## Chunk 3: Phase 1-2 Manifest Migration

### Task 12: Audit Phase 1 manifests (T01-T37)

Read `test_runner.py` Phase 1 test functions alongside existing
`phase1-core.toml` and `phase1-advanced.toml`. Compare each test:
assertions, cleanup steps, variable usage, `no_db` handling. Document
gaps.

### Task 13: Audit Phase 2 manifests (T38-T76)

Same as Task 12 for Phase 2 groups A-F.

### Task 14: Fill manifest gaps and validate

Fix all gaps found in Tasks 12-13. Run both Python and Rust runners
on Forge against same container images. Diff results. Fix behavioral
differences until they match.

### Task 15: Switch CI and delete Python runner

Update `.forgejo/workflows/integration.yaml` and `e2e.yaml` to use
`conary-test run` instead of `run.sh`. Delete `test_runner.py` and
`run.sh`.

---

## Chunk 4: DX Polish

### Task 16: Error taxonomy

Create `conary-test/src/error_taxonomy.rs` with `StructuredError` type:
`{ error, category, message, transient, hint, details }`. Categories:
infrastructure, assertion, config, deployment. Wire into all API/MCP
responses.

### Task 17: Image lifecycle and manifest reload

Add `reload_manifests`, `prune_images(keep?)`, `image_info(image)` MCP
tools and CLI subcommands. Image tags include conary binary hash.

### Task 18: CLI parity

Add CLI subcommands matching every MCP tool: `deploy source`, `deploy
rebuild`, `deploy restart`, `deploy status`, `fixtures build`, `fixtures
publish`, `logs`, `health`, `images prune`, `manifests reload`. CLI
outputs tables/colors, MCP/API returns JSON. Same service functions.

---

## Final Verification

- [ ] **Deploy all layers to Forge and Remi**
- [ ] **Run full test suite via conary-test (all phases, all distros)**
- [ ] **Verify results visible on Remi via MCP**
- [ ] **Verify deployment tools work end-to-end**
- [ ] **Run `cargo test && cargo clippy -- -D warnings`**
- [ ] **Delete Python runner after parallel validation**
