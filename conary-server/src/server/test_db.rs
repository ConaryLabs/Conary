// conary-server/src/server/test_db.rs
//! Persistent storage for test run data in a separate SQLite database.
//!
//! This module manages `/conary/test-data.db`, which is completely independent
//! from the main conary package database. It stores test runs, results, steps,
//! logs, and events with cursor-based pagination and automatic garbage collection.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tracing::info;

// ---------------------------------------------------------------------------
// Connection setup
// ---------------------------------------------------------------------------

/// Standard PRAGMAs for the test data database.
const CONNECTION_PRAGMAS: &str = "\
    PRAGMA journal_mode = WAL;\
    PRAGMA synchronous = NORMAL;\
    PRAGMA foreign_keys = ON;\
    PRAGMA busy_timeout = 5000;\
";

/// Open (or create) the test data database and run migrations.
///
/// This is the primary entry point. The database is entirely separate from
/// conary-core's package database.
pub fn init(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open test data database at {path}"))?;
    conn.execute_batch(CONNECTION_PRAGMAS)
        .context("failed to set connection pragmas")?;
    migrate(&conn)?;
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Schema migration
// ---------------------------------------------------------------------------

fn migrate(conn: &Connection) -> Result<()> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);

    if version < 1 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS test_runs (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                suite        TEXT    NOT NULL,
                distro       TEXT    NOT NULL,
                phase        INTEGER NOT NULL,
                status       TEXT    NOT NULL DEFAULT 'pending',
                started_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                completed_at TEXT,
                triggered_by TEXT,
                source_commit TEXT,
                total        INTEGER NOT NULL DEFAULT 0,
                passed       INTEGER NOT NULL DEFAULT 0,
                failed       INTEGER NOT NULL DEFAULT 0,
                skipped      INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS test_results (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id      INTEGER NOT NULL REFERENCES test_runs(id) ON DELETE CASCADE,
                test_id     TEXT    NOT NULL,
                name        TEXT    NOT NULL,
                status      TEXT    NOT NULL,
                duration_ms INTEGER,
                message     TEXT,
                attempt     INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS test_steps (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                result_id   INTEGER NOT NULL REFERENCES test_results(id) ON DELETE CASCADE,
                step_index  INTEGER NOT NULL,
                step_type   TEXT    NOT NULL,
                command     TEXT,
                exit_code   INTEGER,
                duration_ms INTEGER
            );

            CREATE TABLE IF NOT EXISTS test_logs (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                step_id     INTEGER NOT NULL REFERENCES test_steps(id) ON DELETE CASCADE,
                stream      TEXT    NOT NULL,
                content     TEXT    NOT NULL,
                raw_content TEXT
            );

            CREATE TABLE IF NOT EXISTS test_events (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                step_id    INTEGER NOT NULL REFERENCES test_steps(id) ON DELETE CASCADE,
                event_type TEXT    NOT NULL,
                payload    TEXT,
                timestamp  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );

            -- Foreign key indexes for efficient CASCADE deletes and joins
            CREATE INDEX IF NOT EXISTS idx_test_results_run    ON test_results(run_id);
            CREATE INDEX IF NOT EXISTS idx_test_steps_result   ON test_steps(result_id);
            CREATE INDEX IF NOT EXISTS idx_test_logs_step      ON test_logs(step_id);
            CREATE INDEX IF NOT EXISTS idx_test_events_step    ON test_events(step_id);

            -- Query indexes
            CREATE INDEX IF NOT EXISTS idx_test_runs_status    ON test_runs(status);
            CREATE INDEX IF NOT EXISTS idx_test_runs_distro    ON test_runs(distro);
            CREATE INDEX IF NOT EXISTS idx_test_results_test   ON test_results(run_id, test_id);

            PRAGMA user_version = 1;
            ",
        )
        .context("failed to apply test data schema v1")?;
        info!("Test data schema v1 applied");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRun {
    pub id: i64,
    pub suite: String,
    pub distro: String,
    pub phase: i32,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub triggered_by: Option<String>,
    pub source_commit: Option<String>,
    pub total: i32,
    pub passed: i32,
    pub failed: i32,
    pub skipped: i32,
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
    pub attempt: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestStep {
    pub id: i64,
    pub result_id: i64,
    pub step_index: i32,
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

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<TestRun> {
    Ok(TestRun {
        id: row.get("id")?,
        suite: row.get("suite")?,
        distro: row.get("distro")?,
        phase: row.get("phase")?,
        status: row.get("status")?,
        started_at: row.get("started_at")?,
        completed_at: row.get("completed_at")?,
        triggered_by: row.get("triggered_by")?,
        source_commit: row.get("source_commit")?,
        total: row.get("total")?,
        passed: row.get("passed")?,
        failed: row.get("failed")?,
        skipped: row.get("skipped")?,
    })
}

fn row_to_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<TestResult> {
    Ok(TestResult {
        id: row.get("id")?,
        run_id: row.get("run_id")?,
        test_id: row.get("test_id")?,
        name: row.get("name")?,
        status: row.get("status")?,
        duration_ms: row.get("duration_ms")?,
        message: row.get("message")?,
        attempt: row.get("attempt")?,
    })
}

fn row_to_step(row: &rusqlite::Row<'_>) -> rusqlite::Result<TestStep> {
    Ok(TestStep {
        id: row.get("id")?,
        result_id: row.get("result_id")?,
        step_index: row.get("step_index")?,
        step_type: row.get("step_type")?,
        command: row.get("command")?,
        exit_code: row.get("exit_code")?,
        duration_ms: row.get("duration_ms")?,
    })
}

fn row_to_log(row: &rusqlite::Row<'_>) -> rusqlite::Result<TestLog> {
    Ok(TestLog {
        id: row.get("id")?,
        step_id: row.get("step_id")?,
        stream: row.get("stream")?,
        content: row.get("content")?,
        raw_content: row.get("raw_content")?,
    })
}

// ---------------------------------------------------------------------------
// TestRun CRUD
// ---------------------------------------------------------------------------

impl TestRun {
    /// Create a new test run. Returns the inserted row.
    pub fn create(
        conn: &Connection,
        suite: &str,
        distro: &str,
        phase: i32,
        triggered_by: Option<&str>,
        source_commit: Option<&str>,
    ) -> Result<Self> {
        conn.execute(
            "INSERT INTO test_runs (suite, distro, phase, triggered_by, source_commit)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![suite, distro, phase, triggered_by, source_commit],
        )
        .context("insert test_run")?;

        let id = conn.last_insert_rowid();
        Self::find_by_id(conn, id)?
            .ok_or_else(|| anyhow::anyhow!("test_run {id} not found after insert"))
    }

    /// Find a test run by primary key.
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn
            .prepare("SELECT * FROM test_runs WHERE id = ?1")
            .context("prepare find_by_id")?;
        let mut rows = stmt.query_map(params![id], row_to_run)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// List test runs with cursor-based pagination (newest first).
    ///
    /// If `cursor` is `Some(id)`, only runs with `id < cursor` are returned.
    /// Returns at most `limit` rows.
    pub fn list(conn: &Connection, cursor: Option<i64>, limit: u32) -> Result<Vec<Self>> {
        let rows = if let Some(cursor_id) = cursor {
            let mut stmt = conn.prepare(
                "SELECT * FROM test_runs WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
            )?;
            stmt.query_map(params![cursor_id, limit], row_to_run)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT * FROM test_runs ORDER BY id DESC LIMIT ?1",
            )?;
            stmt.query_map(params![limit], row_to_run)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    }

    /// Update the status of a test run, optionally setting `completed_at`.
    pub fn update_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
        let completed_at = match status {
            "passed" | "failed" | "cancelled" | "error" => {
                Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string())
            }
            _ => None,
        };
        conn.execute(
            "UPDATE test_runs SET status = ?1, completed_at = ?2 WHERE id = ?3",
            params![status, completed_at, id],
        )
        .context("update test_run status")?;
        Ok(())
    }

    /// Update the aggregate counts on a test run.
    pub fn update_counts(
        conn: &Connection,
        id: i64,
        total: i32,
        passed: i32,
        failed: i32,
        skipped: i32,
    ) -> Result<()> {
        conn.execute(
            "UPDATE test_runs SET total = ?1, passed = ?2, failed = ?3, skipped = ?4 WHERE id = ?5",
            params![total, passed, failed, skipped, id],
        )
        .context("update test_run counts")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TestResult CRUD
// ---------------------------------------------------------------------------

/// Input parameters for inserting a new test result.
///
/// Bundled into a struct to keep `TestResult::insert` under the clippy
/// `too_many_arguments` threshold.
pub struct NewTestResult<'a> {
    pub run_id: i64,
    pub test_id: &'a str,
    pub name: &'a str,
    pub status: &'a str,
    pub duration_ms: Option<i64>,
    pub message: Option<&'a str>,
    pub attempt: i32,
}

impl TestResult {
    /// Insert a test result for a run.
    pub fn insert(conn: &Connection, new: &NewTestResult<'_>) -> Result<Self> {
        conn.execute(
            "INSERT INTO test_results (run_id, test_id, name, status, duration_ms, message, attempt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                new.run_id,
                new.test_id,
                new.name,
                new.status,
                new.duration_ms,
                new.message,
                new.attempt
            ],
        )
        .context("insert test_result")?;

        let id = conn.last_insert_rowid();
        Ok(Self {
            id,
            run_id: new.run_id,
            test_id: new.test_id.to_string(),
            name: new.name.to_string(),
            status: new.status.to_string(),
            duration_ms: new.duration_ms,
            message: new.message.map(String::from),
            attempt: new.attempt,
        })
    }

    /// Find all results for a given run, ordered by id.
    pub fn find_by_run(conn: &Connection, run_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM test_results WHERE run_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![run_id], row_to_result)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find a specific result by run and test id.
    pub fn find_by_run_and_test(
        conn: &Connection,
        run_id: i64,
        test_id: &str,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM test_results WHERE run_id = ?1 AND test_id = ?2 ORDER BY attempt DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![run_id, test_id], row_to_result)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// TestStep CRUD
// ---------------------------------------------------------------------------

impl TestStep {
    /// Insert a test step for a result.
    pub fn insert(
        conn: &Connection,
        result_id: i64,
        step_index: i32,
        step_type: &str,
        command: Option<&str>,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
    ) -> Result<Self> {
        conn.execute(
            "INSERT INTO test_steps (result_id, step_index, step_type, command, exit_code, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![result_id, step_index, step_type, command, exit_code, duration_ms],
        )
        .context("insert test_step")?;

        let id = conn.last_insert_rowid();
        Ok(Self {
            id,
            result_id,
            step_index,
            step_type: step_type.to_string(),
            command: command.map(String::from),
            exit_code,
            duration_ms,
        })
    }

    /// Find all steps for a given result, ordered by step index.
    pub fn find_by_result(conn: &Connection, result_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM test_steps WHERE result_id = ?1 ORDER BY step_index",
        )?;
        let rows = stmt
            .query_map(params![result_id], row_to_step)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

// ---------------------------------------------------------------------------
// TestLog CRUD
// ---------------------------------------------------------------------------

impl TestLog {
    /// Insert a log entry for a step. ANSI escape codes are stripped from
    /// `content`; the original is preserved in `raw_content`.
    pub fn insert(
        conn: &Connection,
        step_id: i64,
        stream: &str,
        raw: &str,
    ) -> Result<Self> {
        let content = strip_ansi(raw);
        let raw_content = if content == raw { None } else { Some(raw) };
        conn.execute(
            "INSERT INTO test_logs (step_id, stream, content, raw_content)
             VALUES (?1, ?2, ?3, ?4)",
            params![step_id, stream, content, raw_content],
        )
        .context("insert test_log")?;

        let id = conn.last_insert_rowid();
        Ok(Self {
            id,
            step_id,
            stream: stream.to_string(),
            content,
            raw_content: raw_content.map(String::from),
        })
    }

    /// Find all log entries for a step, ordered by id.
    pub fn find_by_step(conn: &Connection, step_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM test_logs WHERE step_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![step_id], row_to_log)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

// ---------------------------------------------------------------------------
// ANSI stripping
// ---------------------------------------------------------------------------

/// Strip ANSI escape sequences from a string.
///
/// Handles CSI sequences (`ESC[...X`), OSC sequences (`ESC]...ST`), and
/// simple two-byte escapes (`ESC X`).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek() {
                // CSI: ESC [ ... <final byte 0x40-0x7E>
                Some('[') => {
                    chars.next(); // consume '['
                    for c in chars.by_ref() {
                        if c.is_ascii() && (0x40..=0x7E).contains(&(c as u32)) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... (terminated by ST = ESC\ or BEL)
                Some(']') => {
                    chars.next(); // consume ']'
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break; // BEL terminator
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next(); // consume '\'
                            break;
                        }
                    }
                }
                // Simple two-byte escape (e.g., ESC c, ESC D)
                Some(&c) if c.is_ascii() => {
                    chars.next();
                }
                _ => {}
            }
        } else {
            out.push(ch);
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Garbage collection
// ---------------------------------------------------------------------------

/// Delete test runs (and all children via CASCADE) older than `days` days.
///
/// Returns the number of runs deleted.
pub fn gc(conn: &Connection, older_than_days: u32) -> Result<u64> {
    let deleted = conn
        .execute(
            "DELETE FROM test_runs WHERE started_at < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)",
            params![format!("-{older_than_days} days")],
        )
        .context("gc test_runs")?;
    if deleted > 0 {
        info!("GC: removed {deleted} test runs older than {older_than_days} days");
    }
    Ok(deleted as u64)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create an in-memory test data database.
    fn mem_db() -> Connection {
        init(":memory:").expect("init in-memory test db")
    }

    #[test]
    fn test_init_and_create_run() {
        let conn = mem_db();

        let run = TestRun::create(&conn, "phase1-core", "fedora43", 1, Some("ci"), Some("abc123"))
            .expect("create run");

        assert_eq!(run.suite, "phase1-core");
        assert_eq!(run.distro, "fedora43");
        assert_eq!(run.phase, 1);
        assert_eq!(run.status, "pending");
        assert!(run.completed_at.is_none());
        assert_eq!(run.triggered_by.as_deref(), Some("ci"));
        assert_eq!(run.source_commit.as_deref(), Some("abc123"));

        // Verify round-trip through find_by_id
        let found = TestRun::find_by_id(&conn, run.id)
            .expect("find_by_id")
            .expect("should exist");
        assert_eq!(found.id, run.id);
        assert_eq!(found.suite, run.suite);

        // Update status to a terminal state and verify completed_at is set
        TestRun::update_status(&conn, run.id, "passed").expect("update status");
        let updated = TestRun::find_by_id(&conn, run.id)
            .expect("find")
            .expect("exists");
        assert_eq!(updated.status, "passed");
        assert!(updated.completed_at.is_some());

        // Update counts
        TestRun::update_counts(&conn, run.id, 10, 8, 1, 1).expect("update counts");
        let counted = TestRun::find_by_id(&conn, run.id)
            .expect("find")
            .expect("exists");
        assert_eq!(counted.total, 10);
        assert_eq!(counted.passed, 8);
        assert_eq!(counted.failed, 1);
        assert_eq!(counted.skipped, 1);
    }

    #[test]
    fn test_cursor_pagination() {
        let conn = mem_db();

        // Insert 5 runs
        let mut ids = Vec::new();
        for i in 0..5 {
            let run = TestRun::create(
                &conn,
                &format!("suite-{i}"),
                "fedora43",
                1,
                None,
                None,
            )
            .expect("create run");
            ids.push(run.id);
        }

        // First page: no cursor, limit 2 (newest first)
        let page1 = TestRun::list(&conn, None, 2).expect("list page 1");
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].id, ids[4]); // newest
        assert_eq!(page1[1].id, ids[3]);

        // Second page: cursor = last id from page 1
        let cursor = page1.last().map(|r| r.id);
        let page2 = TestRun::list(&conn, cursor, 2).expect("list page 2");
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].id, ids[2]);
        assert_eq!(page2[1].id, ids[1]);

        // Third page: only 1 remaining
        let cursor2 = page2.last().map(|r| r.id);
        let page3 = TestRun::list(&conn, cursor2, 2).expect("list page 3");
        assert_eq!(page3.len(), 1);
        assert_eq!(page3[0].id, ids[0]);

        // Fourth page: empty
        let cursor3 = page3.last().map(|r| r.id);
        let page4 = TestRun::list(&conn, cursor3, 2).expect("list page 4");
        assert!(page4.is_empty());
    }

    #[test]
    fn test_full_result_chain() {
        let conn = mem_db();

        // Create run -> result -> step -> log
        let run = TestRun::create(&conn, "phase1-core", "fedora43", 1, None, None)
            .expect("create run");

        let result = TestResult::insert(
            &conn,
            &NewTestResult {
                run_id: run.id,
                test_id: "T01",
                name: "health_check",
                status: "passed",
                duration_ms: Some(150),
                message: None,
                attempt: 1,
            },
        )
        .expect("insert result");
        assert_eq!(result.run_id, run.id);
        assert_eq!(result.test_id, "T01");

        let step = TestStep::insert(
            &conn,
            result.id,
            0,
            "exec",
            Some("conary --version"),
            Some(0),
            Some(42),
        )
        .expect("insert step");
        assert_eq!(step.result_id, result.id);

        // Log with ANSI codes -- should be stripped in `content`, preserved in `raw_content`
        let raw_output = "\x1b[32mconary 0.5.0\x1b[0m";
        let log = TestLog::insert(&conn, step.id, "stdout", raw_output)
            .expect("insert log");
        assert_eq!(log.content, "conary 0.5.0");
        assert_eq!(log.raw_content.as_deref(), Some(raw_output));

        // Verify find methods
        let results = TestResult::find_by_run(&conn, run.id).expect("find results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].test_id, "T01");

        let found = TestResult::find_by_run_and_test(&conn, run.id, "T01")
            .expect("find by run+test")
            .expect("should exist");
        assert_eq!(found.id, result.id);

        let steps = TestStep::find_by_result(&conn, result.id).expect("find steps");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].command.as_deref(), Some("conary --version"));

        let logs = TestLog::find_by_step(&conn, step.id).expect("find logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].stream, "stdout");

        // Verify CASCADE delete: deleting the run should remove everything
        conn.execute("DELETE FROM test_runs WHERE id = ?1", params![run.id])
            .expect("delete run");
        assert!(TestResult::find_by_run(&conn, run.id).expect("find").is_empty());
        assert!(TestStep::find_by_result(&conn, result.id).expect("find").is_empty());
        assert!(TestLog::find_by_step(&conn, step.id).expect("find").is_empty());
    }

    #[test]
    fn test_gc_removes_old_runs() {
        let conn = mem_db();

        // Create a run with a timestamp well in the past
        conn.execute(
            "INSERT INTO test_runs (suite, distro, phase, status, started_at)
             VALUES ('old-suite', 'fedora43', 1, 'passed', '2020-01-01T00:00:00Z')",
            [],
        )
        .expect("insert old run");

        // Create a recent run (default timestamp is now)
        let _recent = TestRun::create(&conn, "new-suite", "fedora43", 1, None, None)
            .expect("create recent run");

        // GC with 30-day window should remove the old run
        let deleted = gc(&conn, 30).expect("gc");
        assert_eq!(deleted, 1);

        // Only the recent run should remain
        let remaining = TestRun::list(&conn, None, 100).expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].suite, "new-suite");
    }

    #[test]
    fn test_strip_ansi() {
        // CSI sequences (color codes)
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[1;32mbold green\x1b[0m"), "bold green");

        // No ANSI -- passthrough
        assert_eq!(strip_ansi("plain text"), "plain text");

        // Empty string
        assert_eq!(strip_ansi(""), "");

        // OSC sequence (e.g., terminal title)
        assert_eq!(strip_ansi("\x1b]0;title\x07rest"), "rest");

        // Mixed content
        assert_eq!(
            strip_ansi("before\x1b[33myellow\x1b[0m after"),
            "beforeyellow after"
        );
    }

    #[test]
    fn test_log_no_raw_when_clean() {
        let conn = mem_db();
        let run = TestRun::create(&conn, "s", "d", 1, None, None).expect("create run");
        let result = TestResult::insert(
            &conn,
            &NewTestResult {
                run_id: run.id,
                test_id: "T01",
                name: "t",
                status: "passed",
                duration_ms: None,
                message: None,
                attempt: 1,
            },
        )
        .expect("insert result");
        let step = TestStep::insert(&conn, result.id, 0, "exec", None, Some(0), None)
            .expect("insert step");

        // Clean content (no ANSI) should NOT store raw_content
        let log = TestLog::insert(&conn, step.id, "stdout", "clean output")
            .expect("insert log");
        assert_eq!(log.content, "clean output");
        assert!(log.raw_content.is_none());
    }
}
