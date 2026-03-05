// conary-server/src/server/analytics.rs
//! Download analytics recorder for the Remi package index
//!
//! Buffers download events in memory and periodically flushes them to SQLite.
//! This avoids write contention on the database from individual download requests.
//! A background loop also refreshes the aggregated `download_counts` table.

use conary_core::db::models::{DownloadCount, DownloadStat};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A single download event to be recorded
struct DownloadEvent {
    distro: String,
    package_name: String,
    package_version: Option<String>,
    client_ip_hash: Option<String>,
    user_agent: Option<String>,
}

/// Buffered download analytics recorder
///
/// Collects download events in memory and flushes them to the database
/// when the buffer reaches a threshold or on periodic timer.
pub struct AnalyticsRecorder {
    buffer: Mutex<Vec<DownloadEvent>>,
    db_path: PathBuf,
    flush_threshold: usize,
}

impl AnalyticsRecorder {
    /// Create a new analytics recorder with default 100-event flush threshold
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            buffer: Mutex::new(Vec::new()),
            db_path,
            flush_threshold: 100,
        }
    }

    /// Record a download event
    ///
    /// Buffers the event in memory. If the buffer reaches the flush threshold,
    /// automatically flushes to the database.
    pub async fn record(
        &self,
        distro: &str,
        package: &str,
        version: Option<&str>,
        ip_hash: Option<&str>,
        ua: Option<&str>,
    ) {
        let should_flush = {
            let mut buffer = self.buffer.lock().await;
            buffer.push(DownloadEvent {
                distro: distro.to_string(),
                package_name: package.to_string(),
                package_version: version.map(String::from),
                client_ip_hash: ip_hash.map(String::from),
                user_agent: ua.map(String::from),
            });
            buffer.len() >= self.flush_threshold
        };

        if should_flush && let Err(e) = self.flush().await {
            tracing::error!("Failed to auto-flush analytics: {}", e);
        }
    }

    /// Flush buffered events to the database
    ///
    /// Returns the number of events flushed.
    pub async fn flush(&self) -> Result<usize> {
        let events = {
            let mut buffer = self.buffer.lock().await;
            std::mem::take(&mut *buffer)
        };

        if events.is_empty() {
            return Ok(0);
        }

        let count = events.len();
        let db_path = self.db_path.clone();

        // Convert to DownloadStat models
        let stats: Vec<DownloadStat> = events
            .into_iter()
            .map(|e| {
                let mut stat = DownloadStat::new(e.distro, e.package_name);
                stat.package_version = e.package_version;
                stat.client_ip_hash = e.client_ip_hash;
                stat.user_agent = e.user_agent;
                stat
            })
            .collect();

        // Write to DB on blocking thread
        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)?;
            DownloadStat::insert_batch(&conn, &stats)?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        tracing::debug!("Flushed {} download events to database", count);
        Ok(count)
    }

    /// Refresh the aggregated download_counts table from raw download_stats
    pub async fn refresh_aggregates(&self) -> Result<()> {
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)?;
            let updated = DownloadCount::refresh_aggregates(&conn)?;
            tracing::debug!("Refreshed {} download count aggregates", updated);
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }
}

/// Background loop that periodically flushes analytics and refreshes aggregates
///
/// Runs every 5 minutes:
/// 1. Flushes any buffered download events to the database
/// 2. Refreshes the aggregated download_counts table
pub async fn run_analytics_loop(analytics: Arc<AnalyticsRecorder>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));

    loop {
        interval.tick().await;

        // Flush buffered events
        match analytics.flush().await {
            Ok(count) => {
                if count > 0 {
                    tracing::info!("Analytics flush: {} events written", count);
                }
            }
            Err(e) => {
                tracing::error!("Analytics flush failed: {}", e);
            }
        }

        // Refresh aggregates
        if let Err(e) = analytics.refresh_aggregates().await {
            tracing::error!("Analytics aggregate refresh failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, PathBuf) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        let path = temp_file.path().to_path_buf();
        (temp_file, path)
    }

    #[tokio::test]
    async fn test_record_and_flush() {
        let (_temp, db_path) = create_test_db();
        let recorder = AnalyticsRecorder::new(db_path.clone());

        // Record some events
        recorder
            .record("fedora", "nginx", Some("1.24"), None, None)
            .await;
        recorder
            .record("fedora", "nginx", Some("1.24"), None, None)
            .await;
        recorder.record("arch", "curl", None, None, None).await;

        // Flush
        let count = recorder.flush().await.unwrap();
        assert_eq!(count, 3);

        // Verify in DB
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM download_stats", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 3);

        // Flush again should return 0 (buffer empty)
        let count = recorder.flush().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_auto_flush_at_threshold() {
        let (_temp, db_path) = create_test_db();
        let mut recorder = AnalyticsRecorder::new(db_path.clone());
        recorder.flush_threshold = 5; // Low threshold for testing

        // Record events up to threshold
        for i in 0..5 {
            recorder
                .record("fedora", &format!("pkg-{}", i), None, None, None)
                .await;
        }

        // Buffer should have been auto-flushed
        let buffer = recorder.buffer.lock().await;
        assert!(buffer.is_empty(), "Buffer should be empty after auto-flush");
        drop(buffer);

        // Verify in DB
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM download_stats", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 5);
    }

    #[tokio::test]
    async fn test_refresh_aggregates() {
        let (_temp, db_path) = create_test_db();
        let recorder = AnalyticsRecorder::new(db_path.clone());

        // Record and flush events
        recorder.record("fedora", "nginx", None, None, None).await;
        recorder.record("fedora", "nginx", None, None, None).await;
        recorder.record("fedora", "curl", None, None, None).await;
        recorder.flush().await.unwrap();

        // Refresh aggregates
        recorder.refresh_aggregates().await.unwrap();

        // Verify aggregated counts
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count = DownloadCount::find_by_package(&conn, "fedora", "nginx")
            .unwrap()
            .unwrap();
        assert_eq!(count.total_count, 2);

        let count = DownloadCount::find_by_package(&conn, "fedora", "curl")
            .unwrap()
            .unwrap();
        assert_eq!(count.total_count, 1);
    }

    #[tokio::test]
    async fn test_record_with_metadata() {
        let (_temp, db_path) = create_test_db();
        let recorder = AnalyticsRecorder::new(db_path.clone());

        recorder
            .record(
                "fedora",
                "nginx",
                Some("1.24.0"),
                Some("abcdef12"),
                Some("conary/0.1"),
            )
            .await;
        recorder.flush().await.unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let (version, ip_hash, ua): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT package_version, client_ip_hash, user_agent FROM download_stats LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(version.as_deref(), Some("1.24.0"));
        assert_eq!(ip_hash.as_deref(), Some("abcdef12"));
        assert_eq!(ua.as_deref(), Some("conary/0.1"));
    }
}
