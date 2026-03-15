// conary-test/src/server/wal.rs
//! SQLite-backed write-ahead log for buffering test results when Remi is
//! unreachable.
//!
//! Failed POST payloads are stored locally and replayed on the next flush
//! cycle. Items that exceed a configurable retry limit are purged.

use anyhow::{Context, Result};
use rusqlite::Connection;

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
}

impl Wal {
    /// Open or create the WAL database at `path`.
    ///
    /// Pass `":memory:"` for an ephemeral in-memory database (useful in
    /// tests).
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path).context("failed to open WAL database")?;

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
            .query_row("SELECT COUNT(*) FROM pending_results", [], |row| row.get(0))
            .context("failed to count pending WAL items")?;
        Ok(count)
    }

    /// Retrieve all pending items ordered oldest-first for sequential replay.
    pub fn pending_items(&self) -> Result<Vec<PendingItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, run_id, payload, retry_count, created_at \
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
    /// Returns the number of purged rows.
    pub fn purge_dead(&self, max_retries: u32) -> Result<u64> {
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
/// Returns `(flushed_count, failed_count)`. Items with unparseable payloads
/// are silently removed rather than retried indefinitely.
pub async fn flush(wal: &Wal, client: &crate::server::remi_client::RemiClient) -> (u64, u64) {
    let items = wal.pending_items().unwrap_or_default();
    let mut flushed = 0u64;
    let mut failed = 0u64;

    for item in items {
        let data: crate::server::remi_client::PushResultData =
            match serde_json::from_str(&item.payload) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("WAL item {} has invalid payload: {}", item.id, e);
                    let _ = wal.remove(item.id);
                    continue;
                }
            };

        match client.push_result(item.run_id, &data).await {
            Ok(()) => {
                let _ = wal.remove(item.id);
                flushed += 1;
            }
            Err(e) => {
                let _ = wal.mark_retry(item.id, &e.to_string());
                failed += 1;
            }
        }
    }

    (flushed, failed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
