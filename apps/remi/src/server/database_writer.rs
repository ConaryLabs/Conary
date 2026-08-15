// apps/remi/src/server/database_writer.rs
//! Process-local ownership for Remi database mutations.

use parking_lot::Mutex;
use std::sync::Arc;

/// Serializes short SQLite mutation phases within one Remi server.
///
/// Work that does not mutate SQLite stays outside this owner. Callers acquire
/// it immediately before opening their write transaction and release it after
/// commit so independent request and background paths cannot race SQLite's
/// single-writer boundary.
#[derive(Clone, Default)]
pub(crate) struct DatabaseWriter {
    gate: Arc<Mutex<()>>,
}

impl DatabaseWriter {
    pub(crate) fn execute<T, E>(&self, operation: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        let _guard = self.gate.lock();
        operation()
    }

    #[cfg(test)]
    pub(crate) fn shares_owner_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.gate, &other.gate)
    }

    #[cfg(test)]
    pub(crate) fn hold_for_test(&self) -> impl Drop + '_ {
        self.gate.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::TransactionBehavior;
    use std::path::PathBuf;
    use std::time::Duration;

    fn initialized_db(temp_dir: &tempfile::TempDir) -> PathBuf {
        let db_path = temp_dir.path().join("remi.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open_fast(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE database_writer_probe (
                value INTEGER PRIMARY KEY
            );",
        )
        .unwrap();
        db_path
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shared_writer_serializes_tiny_timeout_sqlite_transactions() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = initialized_db(&temp_dir);
        let writer = DatabaseWriter::default();

        let mut tasks = Vec::new();
        for value in 0..32_i64 {
            let db_path = db_path.clone();
            let writer = writer.clone();
            tasks.push(tokio::task::spawn_blocking(move || {
                writer.execute(|| {
                    let mut conn = conary_core::db::open_fast(&db_path)?;
                    conn.busy_timeout(Duration::from_millis(1))?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    tx.execute(
                        "INSERT INTO database_writer_probe (value) VALUES (?1)",
                        [value],
                    )?;
                    std::thread::sleep(Duration::from_millis(2));
                    tx.commit()?;
                    Ok::<_, conary_core::Error>(())
                })
            }));
        }

        for task in tasks {
            task.await.unwrap().unwrap();
        }

        let conn = conary_core::db::open_fast(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM database_writer_probe", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 32);
    }
}
