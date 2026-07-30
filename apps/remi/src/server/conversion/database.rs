// apps/remi/src/server/conversion/database.rs
//! Process-local ownership for Remi conversion database mutations.

use anyhow::{Result, anyhow};
use std::sync::{Arc, Mutex};

/// Serializes the short SQLite mutation phases shared by conversion work.
///
/// SQLite admits one writer at a time. Conversion download, parsing, CCS
/// emission, CAS storage, and R2 upload remain concurrent; only the database
/// transactions that publish their results pass through this owner.
#[derive(Clone, Default)]
pub(super) struct ConversionDatabaseWriter {
    gate: Arc<Mutex<()>>,
}

impl ConversionDatabaseWriter {
    pub(super) fn execute<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| anyhow!("conversion database writer lock is poisoned"))?;
        operation()
    }

    #[cfg(test)]
    pub(super) fn shares_owner_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.gate, &other.gate)
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
            "CREATE TABLE conversion_writer_probe (
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
        let writer = ConversionDatabaseWriter::default();

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
                        "INSERT INTO conversion_writer_probe (value) VALUES (?1)",
                        [value],
                    )?;
                    std::thread::sleep(Duration::from_millis(2));
                    tx.commit()?;
                    Ok(())
                })
            }));
        }

        for task in tasks {
            task.await.unwrap().unwrap();
        }

        let conn = conary_core::db::open_fast(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM conversion_writer_probe", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 32);
    }
}
