// apps/remi/src/server/analytics.rs
//! Download analytics recorder for the Remi package index
//!
//! Buffers download events in memory and periodically flushes them to SQLite.
//! This avoids write contention on the database from individual download requests.
//! A background loop also refreshes the aggregated `download_counts` table.

use anyhow::Result;
use conary_core::db::models::{DownloadCount, DownloadStat};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::server::database_writer::DatabaseWriter;

/// A single download event to be recorded
struct DownloadEvent {
    source_profile: String,
    package_name: String,
    package_version: Option<String>,
    client_ip_hash: Option<String>,
    user_agent: Option<String>,
}

/// Buffered download analytics recorder
///
/// Collects download events in memory and flushes them to the database
/// when the buffer reaches a threshold or on periodic timer.
pub(crate) struct AnalyticsRecorder {
    buffer: Mutex<Vec<DownloadEvent>>,
    db_path: PathBuf,
    flush_threshold: usize,
    database_writer: DatabaseWriter,
}

impl AnalyticsRecorder {
    /// Create a new analytics recorder with default 100-event flush threshold
    pub(crate) fn new(db_path: PathBuf, database_writer: DatabaseWriter) -> Self {
        Self {
            buffer: Mutex::new(Vec::new()),
            db_path,
            flush_threshold: 100,
            database_writer,
        }
    }

    /// Record a download event
    ///
    /// Buffers the event in memory. If the buffer reaches the flush threshold,
    /// automatically flushes to the database.
    pub(crate) async fn record(
        &self,
        route_slug: &str,
        package: &str,
        version: Option<&str>,
        ip_hash: Option<&str>,
        ua: Option<&str>,
    ) {
        let Some(source_profile) =
            conary_core::repository::supported_profiles::profile_for_remi_route(route_slug)
        else {
            tracing::error!("refusing analytics event for unsupported public route '{route_slug}'");
            return;
        };
        let should_flush = {
            let mut buffer = self.buffer.lock().await;
            buffer.push(DownloadEvent {
                source_profile: source_profile.id().to_string(),
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
    pub(crate) async fn flush(&self) -> Result<usize> {
        let events = {
            let mut buffer = self.buffer.lock().await;
            std::mem::take(&mut *buffer)
        };

        if events.is_empty() {
            return Ok(0);
        }

        let count = events.len();
        let db_path = self.db_path.clone();
        let database_writer = self.database_writer.clone();

        // Convert to DownloadStat models
        let stats: Vec<DownloadStat> = events
            .into_iter()
            .map(|e| {
                let mut stat = DownloadStat::new(e.source_profile, e.package_name);
                stat.package_version = e.package_version;
                stat.client_ip_hash = e.client_ip_hash;
                stat.user_agent = e.user_agent;
                stat
            })
            .collect();

        // Write to DB on blocking thread
        tokio::task::spawn_blocking(move || {
            database_writer.execute(|| {
                let conn = crate::server::open_runtime_db(&db_path)?;
                DownloadStat::insert_batch(&conn, &stats)?;
                Ok::<_, anyhow::Error>(())
            })
        })
        .await??;

        tracing::debug!("Flushed {} download events to database", count);
        Ok(count)
    }

    /// Refresh the aggregated download_counts table from raw download_stats
    pub(crate) async fn refresh_aggregates(&self) -> Result<()> {
        let db_path = self.db_path.clone();
        let database_writer = self.database_writer.clone();

        tokio::task::spawn_blocking(move || {
            database_writer.execute(|| {
                let conn = crate::server::open_runtime_db(&db_path)?;
                let updated = DownloadCount::refresh_aggregates(&conn)?;
                tracing::debug!("Refreshed {} download count aggregates", updated);
                Ok::<_, anyhow::Error>(())
            })
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
pub(crate) async fn run_analytics_loop(analytics: Arc<AnalyticsRecorder>) {
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
    use conary_core::db::models::{Repository, RepositoryPackage};
    use conary_core::db::schema;
    use conary_core::repository::{RepositoryParserConfig, add_repository};
    use rusqlite::{ErrorCode, TransactionBehavior};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    use crate::server::database_writer::DatabaseWriteEvent;

    fn create_test_db() -> (NamedTempFile, PathBuf) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        let path = temp_file.path().to_path_buf();
        (temp_file, path)
    }

    #[tokio::test]
    async fn test_record_and_flush() {
        let (_temp, db_path) = create_test_db();
        let recorder = AnalyticsRecorder::new(db_path.clone(), DatabaseWriter::default());

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
        let mut recorder = AnalyticsRecorder::new(db_path.clone(), DatabaseWriter::default());
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
        let recorder = AnalyticsRecorder::new(db_path.clone(), DatabaseWriter::default());

        // Record and flush events
        recorder.record("fedora", "nginx", None, None, None).await;
        recorder.record("fedora", "nginx", None, None, None).await;
        recorder.record("fedora", "curl", None, None, None).await;
        recorder.flush().await.unwrap();

        // Refresh aggregates
        recorder.refresh_aggregates().await.unwrap();

        // Verify aggregated counts
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count = DownloadCount::find_by_package(&conn, "fedora-44", "nginx")
            .unwrap()
            .unwrap();
        assert_eq!(count.total_count, 2);

        let count = DownloadCount::find_by_package(&conn, "fedora-44", "curl")
            .unwrap()
            .unwrap();
        assert_eq!(count.total_count, 1);
    }

    #[tokio::test]
    async fn test_record_with_metadata() {
        let (_temp, db_path) = create_test_db();
        let recorder = AnalyticsRecorder::new(db_path.clone(), DatabaseWriter::default());

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn repository_sync_and_aggregate_refresh_share_one_writer() {
        let (_temp, db_path) = create_test_db();
        let writer = DatabaseWriter::default();
        let recorder = Arc::new(AnalyticsRecorder::new(db_path.clone(), writer.clone()));
        recorder.record("fedora", "widget", None, None, None).await;
        recorder.record("fedora", "widget", None, None, None).await;
        recorder.flush().await.unwrap();

        let metadata = serde_json::json!({
            "name": "writer-regression",
            "version": "1",
            "security_advisory_source": null,
            "packages": [{
                "name": "widget",
                "version": "1.0-1",
                "version_scheme": "rpm",
                "architecture": "x86_64",
                "description": null,
                "checksum": "sha256:test",
                "size": 42,
                "download_url": "https://packages.example/widget.rpm",
                "delta_from": null,
                "security_advisory": null
            }]
        })
        .to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let repo_url = format!("http://{}/index.json", listener.local_addr().unwrap());
        let (served_tx, served_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                metadata.len(),
                metadata
            )
            .unwrap();
            stream.flush().unwrap();
            served_tx.send(()).unwrap();
        });

        let conn = crate::server::open_runtime_db(&db_path).unwrap();
        let repo = add_repository(
            &conn,
            "writer-regression".to_string(),
            repo_url,
            RepositoryParserConfig::Json,
            None,
            true,
            0,
        )
        .unwrap();
        let repo_id = repo.id.unwrap();
        drop(conn);

        let mut locking_conn = crate::server::open_runtime_db(&db_path).unwrap();
        let locking_tx = locking_conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        locking_tx
            .execute(
                "UPDATE repositories SET priority = priority WHERE id = ?1",
                [repo_id],
            )
            .unwrap();

        let direct_conn = crate::server::open_runtime_db(&db_path).unwrap();
        direct_conn.busy_timeout(Duration::from_millis(1)).unwrap();
        let old_ordering_error = DownloadCount::refresh_aggregates(&direct_conn).unwrap_err();
        assert!(matches!(
            old_ordering_error,
            conary_core::Error::Database(rusqlite::Error::SqliteFailure(code, _))
                if matches!(code.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
        ));
        drop(direct_conn);
        locking_tx.commit().unwrap();

        let write_events = writer.observe_for_test();
        let release_sync = writer.pause_next_for_test();
        let sync_writer = writer.clone();
        let sync_db = db_path.clone();
        let sync_task = tokio::spawn(async move {
            conary_core::repository::sync_repository_from_db_path(sync_db, repo, sync_writer).await
        });
        served_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            write_events.recv_timeout(Duration::from_secs(2)).unwrap(),
            DatabaseWriteEvent::Attempted
        );
        assert_eq!(
            write_events.recv_timeout(Duration::from_secs(2)).unwrap(),
            DatabaseWriteEvent::Acquired
        );

        let aggregate_recorder = Arc::clone(&recorder);
        let aggregate_task =
            tokio::spawn(async move { aggregate_recorder.refresh_aggregates().await });
        assert_eq!(
            write_events.recv_timeout(Duration::from_secs(2)).unwrap(),
            DatabaseWriteEvent::Attempted
        );
        assert!(
            write_events
                .recv_timeout(Duration::from_millis(25))
                .is_err()
        );

        release_sync.send(()).unwrap();
        assert_eq!(
            write_events.recv_timeout(Duration::from_secs(2)).unwrap(),
            DatabaseWriteEvent::Acquired
        );

        assert_eq!(sync_task.await.unwrap().unwrap(), 1);
        aggregate_task.await.unwrap().unwrap();
        server.join().unwrap();

        let conn = crate::server::open_runtime_db(&db_path).unwrap();
        let packages = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "widget");
        let persisted_repo = Repository::find_by_id(&conn, repo_id).unwrap().unwrap();
        assert!(persisted_repo.last_checked_at.is_some());
        let count = DownloadCount::find_by_package(&conn, "fedora-44", "widget")
            .unwrap()
            .unwrap();
        assert_eq!(count.total_count, 2);
    }
}
