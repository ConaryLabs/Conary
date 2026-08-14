// conary-test/src/wal.rs
//! SQLite-backed write-ahead log for buffering test results when Remi is
//! unreachable.
//!
//! Failed per-test result POST payloads are stored locally and replayed only
//! by an invocation whose `create_run` call was acknowledged by Remi. A flush
//! removes successful and unparseable rows, records failed attempts, and
//! purges rows after [`MAX_FLUSH_ATTEMPTS`] failed flush attempts, logging each
//! purged row with its retry context. Run-status updates are deliberately not
//! WAL-buffered; they are best-effort updates made after the aggregate result
//! flush.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;
use tracing::warn;

use crate::remi_client::{PushResultData, RemiResultPusher};

/// Number of failed flush attempts tolerated before a result is discarded.
pub const MAX_FLUSH_ATTEMPTS: u32 = 5;

/// How long a writer waits for a concurrent `conary-test` process to release
/// the shared WAL file before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Counts the work performed by one WAL replay pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlushOutcome {
    pub flushed: u64,
    pub failed: u64,
    pub discarded: u64,
    pub purged: u64,
}

/// SQLite-backed buffer for test result payloads that failed to reach Remi.
pub struct Wal {
    conn: Connection,
}

/// A single pending result waiting to be flushed to Remi.
#[derive(Debug, Clone)]
pub struct PendingItem {
    pub id: i64,
    pub run_id: i64,
    pub payload: String,
    pub retry_count: u32,
    pub created_at: String,
    pub last_error: Option<String>,
}

impl Wal {
    /// Open or create the WAL database at `path`.
    ///
    /// Pass `":memory:"` for an ephemeral in-memory database (useful in
    /// tests).
    ///
    /// The WAL path is shared by every `conary-test` process on the host, so a
    /// second concurrent run would otherwise take an immediate `SQLITE_BUSY`
    /// and drop the result it was trying to buffer. Waiting is always better
    /// than losing the row this file exists to keep.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).context("failed to open WAL database")?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .context("failed to set WAL busy timeout")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pending_results (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id      INTEGER NOT NULL,
                payload     TEXT    NOT NULL,
                created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error  TEXT
            );",
        )
        .context("failed to initialize WAL schema")?;

        Ok(Self { conn })
    }

    /// Buffer a failed result payload for later retry.
    pub fn buffer(&self, run_id: i64, payload: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO pending_results (run_id, payload) VALUES (?1, ?2)",
                rusqlite::params![run_id, payload],
            )
            .context("failed to buffer result in WAL")?;
        Ok(())
    }

    /// Get the number of pending (unflushed) items.
    pub fn pending_count(&self) -> Result<u64> {
        let count: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM pending_results", [], |row| {
                row.get::<_, i64>(0).map(|v| v as u64)
            })
            .context("failed to count pending WAL items")?;
        Ok(count)
    }

    /// Retrieve all pending items ordered oldest-first for sequential replay.
    pub fn pending_items(&self) -> Result<Vec<PendingItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, run_id, payload, retry_count, created_at, last_error \
                 FROM pending_results ORDER BY id ASC",
            )
            .context("failed to prepare pending_items query")?;

        let items = stmt
            .query_map([], |row| {
                Ok(PendingItem {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    payload: row.get(2)?,
                    retry_count: row.get(3)?,
                    created_at: row.get(4)?,
                    last_error: row.get(5)?,
                })
            })
            .context("failed to query pending WAL items")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to read pending WAL rows")?;

        Ok(items)
    }

    /// Remove an item after it has been successfully flushed.
    pub fn remove(&self, id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM pending_results WHERE id = ?1",
                rusqlite::params![id],
            )
            .context("failed to remove WAL item")?;
        Ok(())
    }

    /// Increment the retry count and record the last error message for a
    /// failed flush attempt.
    pub fn mark_retry(&self, id: i64, error: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE pending_results \
                 SET retry_count = retry_count + 1, last_error = ?2 \
                 WHERE id = ?1",
                rusqlite::params![id, error],
            )
            .context("failed to update WAL retry state")?;
        Ok(())
    }

    /// Delete items whose retry count exceeds `max_retries`.
    ///
    /// The doomed rows are read and logged before deletion so result loss is
    /// observable rather than a silent retry cap.
    pub fn purge_dead(&self, max_retries: u32) -> Result<u64> {
        let doomed = self
            .pending_items()?
            .into_iter()
            .filter(|item| item.retry_count > max_retries)
            .collect::<Vec<_>>();

        for item in &doomed {
            warn!(
                id = item.id,
                run_id = item.run_id,
                retry_count = item.retry_count,
                last_error = ?item.last_error,
                "purging WAL result after retry limit"
            );
        }

        let deleted = self
            .conn
            .execute(
                "DELETE FROM pending_results WHERE retry_count > ?1",
                rusqlite::params![max_retries],
            )
            .context("failed to purge dead WAL items")?;
        Ok(deleted as u64)
    }
}

/// Attempt to flush all pending WAL items to Remi.
///
/// Items with unparseable payloads are removed and counted as discarded. After
/// the replay pass, failed rows beyond the retry threshold are purged.
pub async fn flush<C>(wal: &Wal, client: &C) -> FlushOutcome
where
    C: RemiResultPusher + ?Sized,
{
    let items = match wal.pending_items() {
        Ok(items) => items,
        Err(error) => {
            warn!(error = %error, "failed to read WAL items for flush");
            return FlushOutcome::default();
        }
    };
    let mut outcome = FlushOutcome::default();

    for item in items {
        let data: PushResultData = match serde_json::from_str(&item.payload) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("WAL item {} has invalid payload: {}", item.id, e);
                match wal.remove(item.id) {
                    Ok(()) => outcome.discarded += 1,
                    Err(remove_error) => warn!(
                        id = item.id,
                        error = %remove_error,
                        "failed to discard invalid WAL item"
                    ),
                }
                continue;
            }
        };

        match client.push_result(item.run_id, &data).await {
            Ok(()) => match wal.remove(item.id) {
                Ok(()) => outcome.flushed += 1,
                Err(remove_error) => warn!(
                    id = item.id,
                    error = %remove_error,
                    "failed to remove flushed WAL item"
                ),
            },
            Err(e) => {
                outcome.failed += 1;
                if let Err(mark_error) = wal.mark_retry(item.id, &e.to_string()) {
                    warn!(
                        id = item.id,
                        error = %mark_error,
                        "failed to record WAL retry"
                    );
                }
            }
        }
    }

    match wal.purge_dead(MAX_FLUSH_ATTEMPTS - 1) {
        Ok(purged) => outcome.purged = purged,
        Err(error) => warn!(error = %error, "failed to purge dead WAL items"),
    }

    outcome
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remi_client::{PushStepData, RemiResultPusher};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakePusher {
        calls: AtomicU64,
        fail: bool,
    }

    impl FakePusher {
        fn working() -> Self {
            Self {
                calls: AtomicU64::new(0),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                calls: AtomicU64::new(0),
                fail: true,
            }
        }

        fn calls(&self) -> u64 {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl RemiResultPusher for FakePusher {
        async fn push_result(&self, _run_id: i64, _data: &PushResultData) -> Result<()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                anyhow::bail!("fake Remi unavailable");
            }
            Ok(())
        }
    }

    fn push_data() -> PushResultData {
        PushResultData {
            test_id: "T-WAL".to_string(),
            name: "wal_replay".to_string(),
            status: "failed".to_string(),
            duration_ms: Some(12),
            message: Some("failed once".to_string()),
            attempt: Some(1),
            steps: vec![PushStepData {
                step_type: "exec".to_string(),
                command: Some("true".to_string()),
                exit_code: Some(1),
                duration_ms: Some(12),
                stdout: Some(String::new()),
                stderr: Some("failure".to_string()),
            }],
        }
    }

    /// Every `conary-test` process on the host shares one WAL file, so a
    /// writer must wait for a concurrent one rather than take an immediate
    /// `SQLITE_BUSY` and drop the result it was buffering.
    #[test]
    fn open_configures_a_busy_timeout_for_concurrent_writers() {
        let wal = Wal::open(":memory:").unwrap();

        let configured: i64 = wal
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();

        assert_eq!(configured, BUSY_TIMEOUT.as_millis() as i64);
        assert!(configured > 0);
    }

    #[test]
    fn test_buffer_and_retrieve() {
        let wal = Wal::open(":memory:").unwrap();
        wal.buffer(1, r#"{"test_id":"T01","name":"test"}"#).unwrap();
        assert_eq!(wal.pending_count().unwrap(), 1);

        let items = wal.pending_items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].run_id, 1);
    }

    #[test]
    fn test_remove_after_flush() {
        let wal = Wal::open(":memory:").unwrap();
        wal.buffer(1, "{}").unwrap();

        let items = wal.pending_items().unwrap();
        wal.remove(items[0].id).unwrap();
        assert_eq!(wal.pending_count().unwrap(), 0);
    }

    #[test]
    fn test_mark_retry_increments_count() {
        let wal = Wal::open(":memory:").unwrap();
        wal.buffer(1, "{}").unwrap();

        let items = wal.pending_items().unwrap();
        wal.mark_retry(items[0].id, "connection refused").unwrap();

        let items = wal.pending_items().unwrap();
        assert_eq!(items[0].retry_count, 1);
    }

    #[tokio::test]
    async fn flush_replays_buffered_result_and_removes_row() {
        let wal = Wal::open(":memory:").unwrap();
        let data = push_data();
        wal.buffer(7, &serde_json::to_string(&data).unwrap())
            .unwrap();
        let client = FakePusher::working();

        let outcome = flush(&wal, &client).await;

        assert_eq!(outcome.flushed, 1);
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.discarded, 0);
        assert_eq!(outcome.purged, 0);
        assert_eq!(wal.pending_count().unwrap(), 0);
        assert_eq!(client.calls(), 1);
    }

    #[tokio::test]
    async fn flush_purges_after_five_failed_attempts() {
        let wal = Wal::open(":memory:").unwrap();
        let data = push_data();
        wal.buffer(7, &serde_json::to_string(&data).unwrap())
            .unwrap();
        let client = FakePusher::failing();

        for attempt in 1..=MAX_FLUSH_ATTEMPTS {
            let outcome = flush(&wal, &client).await;
            assert_eq!(outcome.failed, 1);
            assert_eq!(outcome.flushed, 0);
            assert_eq!(outcome.discarded, 0);
            if attempt < MAX_FLUSH_ATTEMPTS {
                assert_eq!(outcome.purged, 0);
                assert_eq!(wal.pending_count().unwrap(), 1);
            } else {
                assert_eq!(outcome.purged, 1);
                assert_eq!(wal.pending_count().unwrap(), 0);
            }
        }

        assert_eq!(client.calls(), u64::from(MAX_FLUSH_ATTEMPTS));
    }

    #[tokio::test]
    async fn flush_counts_and_removes_unparseable_payloads() {
        let wal = Wal::open(":memory:").unwrap();
        wal.buffer(7, "not-json").unwrap();
        let client = FakePusher::working();

        let outcome = flush(&wal, &client).await;

        assert_eq!(outcome.discarded, 1);
        assert_eq!(outcome.flushed, 0);
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.purged, 0);
        assert_eq!(wal.pending_count().unwrap(), 0);
        assert_eq!(client.calls(), 0);
    }

    #[test]
    fn test_purge_dead_removes_exceeded() {
        let wal = Wal::open(":memory:").unwrap();
        wal.buffer(1, "{}").unwrap();

        let items = wal.pending_items().unwrap();
        for _ in 0..5 {
            wal.mark_retry(items[0].id, "err").unwrap();
        }

        let purged = wal.purge_dead(3).unwrap();
        assert_eq!(purged, 1);
        assert_eq!(wal.pending_count().unwrap(), 0);
    }

    #[test]
    fn test_ordering_is_fifo() {
        let wal = Wal::open(":memory:").unwrap();
        wal.buffer(10, r#"{"first": true}"#).unwrap();
        wal.buffer(20, r#"{"second": true}"#).unwrap();
        wal.buffer(30, r#"{"third": true}"#).unwrap();

        let items = wal.pending_items().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].run_id, 10);
        assert_eq!(items[1].run_id, 20);
        assert_eq!(items[2].run_id, 30);
    }

    #[test]
    fn test_purge_dead_keeps_items_under_threshold() {
        let wal = Wal::open(":memory:").unwrap();
        wal.buffer(1, "{}").unwrap();
        wal.buffer(2, "{}").unwrap();

        let items = wal.pending_items().unwrap();
        // Push first item over the limit.
        for _ in 0..4 {
            wal.mark_retry(items[0].id, "err").unwrap();
        }
        // Second item stays at retry_count = 1.
        wal.mark_retry(items[1].id, "err").unwrap();

        let purged = wal.purge_dead(3).unwrap();
        assert_eq!(purged, 1);
        assert_eq!(wal.pending_count().unwrap(), 1);

        let remaining = wal.pending_items().unwrap();
        assert_eq!(remaining[0].run_id, 2);
    }
}
