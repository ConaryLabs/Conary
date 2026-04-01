// conary-core/src/db/models/download_stats.rs
//! Download statistics models for the Remi package index
//!
//! Two tables work together:
//! - `download_stats`: Individual download events (write-heavy, buffered)
//! - `download_counts`: Aggregated counts per package (read-heavy, periodically refreshed)

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

/// Individual download event
#[derive(Debug, Clone)]
pub struct DownloadStat {
    pub id: Option<i64>,
    pub distro: String,
    pub package_name: String,
    pub package_version: Option<String>,
    pub downloaded_at: Option<String>,
    pub client_ip_hash: Option<String>,
    pub user_agent: Option<String>,
}

impl DownloadStat {
    pub fn new(distro: String, package_name: String) -> Self {
        Self {
            id: None,
            distro,
            package_name,
            package_version: None,
            downloaded_at: None,
            client_ip_hash: None,
            user_agent: None,
        }
    }

    /// Insert a batch of download events (for buffered writes)
    ///
    /// Wrapped in a transaction so that either all events are inserted or none
    /// are, preventing partial inserts on failure.
    pub fn insert_batch(conn: &Connection, events: &[DownloadStat]) -> Result<usize> {
        let tx = conn.unchecked_transaction()?;

        let mut stmt = tx.prepare(
            "INSERT INTO download_stats (distro, package_name, package_version, client_ip_hash, user_agent)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;

        let mut count = 0;
        for event in events {
            stmt.execute(rusqlite::params![
                event.distro,
                event.package_name,
                event.package_version,
                event.client_ip_hash,
                event.user_agent,
            ])?;
            count += 1;
        }

        drop(stmt);
        tx.commit()?;
        Ok(count)
    }

    /// Prune old download events (keep last N days)
    pub fn prune_older_than(conn: &Connection, days: u32) -> Result<usize> {
        let deleted = conn.execute(
            "DELETE FROM download_stats WHERE downloaded_at < datetime('now', ?1)",
            [format!("-{days} days")],
        )?;
        Ok(deleted)
    }
}

/// Aggregated download counts per package
#[derive(Debug, Clone, Serialize)]
pub struct DownloadCount {
    pub distro: String,
    pub package_name: String,
    pub total_count: i64,
    pub count_30d: i64,
    pub count_7d: i64,
    pub last_updated: Option<String>,
}

impl DownloadCount {
    /// Convert a database row to a DownloadCount
    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Self {
            distro: row.get(0)?,
            package_name: row.get(1)?,
            total_count: row.get(2)?,
            count_30d: row.get(3)?,
            count_7d: row.get(4)?,
            last_updated: row.get(5)?,
        })
    }

    /// Refresh aggregated counts from raw download_stats
    ///
    /// This should be called periodically (e.g., every 15 minutes) to update
    /// the read-optimized download_counts table from the write-optimized download_stats.
    pub fn refresh_aggregates(conn: &Connection) -> Result<usize> {
        // Use INSERT OR REPLACE to upsert aggregated counts
        let updated = conn.execute(
            "INSERT OR REPLACE INTO download_counts (distro, package_name, total_count, count_30d, count_7d, last_updated)
             SELECT
                 distro,
                 package_name,
                 COUNT(*) as total_count,
                 SUM(CASE WHEN downloaded_at >= datetime('now', '-30 days') THEN 1 ELSE 0 END) as count_30d,
                 SUM(CASE WHEN downloaded_at >= datetime('now', '-7 days') THEN 1 ELSE 0 END) as count_7d,
                 datetime('now') as last_updated
             FROM download_stats
             GROUP BY distro, package_name",
            [],
        )?;
        Ok(updated)
    }

    /// Get download counts for a specific package
    pub fn find_by_package(conn: &Connection, distro: &str, name: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT distro, package_name, total_count, count_30d, count_7d, last_updated
                 FROM download_counts
                 WHERE distro = ?1 AND package_name = ?2",
                [distro, name],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Get most popular packages for a distro
    pub fn popular(conn: &Connection, distro: &str, limit: usize) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT distro, package_name, total_count, count_30d, count_7d, last_updated
             FROM download_counts
             WHERE distro = ?1
             ORDER BY total_count DESC
             LIMIT ?2",
        )?;

        let counts = stmt
            .query_map(rusqlite::params![distro, limit as i64], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(counts)
    }

    /// Get overall stats across all distros
    pub fn global_stats(conn: &Connection) -> Result<GlobalDownloadStats> {
        let total_downloads: i64 = conn.query_row(
            "SELECT COALESCE(SUM(total_count), 0) FROM download_counts",
            [],
            |row| row.get(0),
        )?;

        let total_packages: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT package_name) FROM download_counts",
            [],
            |row| row.get(0),
        )?;

        let total_distros: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT distro) FROM download_counts",
            [],
            |row| row.get(0),
        )?;

        let downloads_30d: i64 = conn.query_row(
            "SELECT COALESCE(SUM(count_30d), 0) FROM download_counts",
            [],
            |row| row.get(0),
        )?;

        Ok(GlobalDownloadStats {
            total_downloads,
            total_packages,
            total_distros,
            downloads_30d,
        })
    }
}

/// Global download statistics
#[derive(Debug, Clone, Serialize)]
pub struct GlobalDownloadStats {
    pub total_downloads: i64,
    pub total_packages: i64,
    pub total_distros: i64,
    pub downloads_30d: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn test_download_stat_insert_batch() {
        let (_temp, conn) = create_test_db();

        let events = vec![
            DownloadStat::new("fedora".into(), "nginx".into()),
            DownloadStat::new("fedora".into(), "nginx".into()),
            DownloadStat::new("fedora".into(), "curl".into()),
        ];

        let count = DownloadStat::insert_batch(&conn, &events).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_download_count_refresh() {
        let (_temp, conn) = create_test_db();

        // Insert some events
        let events = vec![
            DownloadStat::new("fedora".into(), "nginx".into()),
            DownloadStat::new("fedora".into(), "nginx".into()),
            DownloadStat::new("arch".into(), "nginx".into()),
        ];
        DownloadStat::insert_batch(&conn, &events).unwrap();

        // Refresh aggregates
        DownloadCount::refresh_aggregates(&conn).unwrap();

        // Check fedora nginx
        let count = DownloadCount::find_by_package(&conn, "fedora", "nginx")
            .unwrap()
            .unwrap();
        assert_eq!(count.total_count, 2);
        assert_eq!(count.count_30d, 2);

        // Check arch nginx
        let count = DownloadCount::find_by_package(&conn, "arch", "nginx")
            .unwrap()
            .unwrap();
        assert_eq!(count.total_count, 1);
    }

    #[test]
    fn test_download_count_popular() {
        let (_temp, conn) = create_test_db();

        let mut events = Vec::new();
        for _ in 0..10 {
            events.push(DownloadStat::new("fedora".into(), "nginx".into()));
        }
        for _ in 0..5 {
            events.push(DownloadStat::new("fedora".into(), "curl".into()));
        }
        events.push(DownloadStat::new("fedora".into(), "vim".into()));

        DownloadStat::insert_batch(&conn, &events).unwrap();
        DownloadCount::refresh_aggregates(&conn).unwrap();

        let popular = DownloadCount::popular(&conn, "fedora", 10).unwrap();
        assert_eq!(popular.len(), 3);
        assert_eq!(popular[0].package_name, "nginx");
        assert_eq!(popular[0].total_count, 10);
        assert_eq!(popular[1].package_name, "curl");
        assert_eq!(popular[2].package_name, "vim");
    }

    #[test]
    fn test_global_stats() {
        let (_temp, conn) = create_test_db();

        let events = vec![
            DownloadStat::new("fedora".into(), "nginx".into()),
            DownloadStat::new("fedora".into(), "curl".into()),
            DownloadStat::new("arch".into(), "nginx".into()),
        ];
        DownloadStat::insert_batch(&conn, &events).unwrap();
        DownloadCount::refresh_aggregates(&conn).unwrap();

        let stats = DownloadCount::global_stats(&conn).unwrap();
        assert_eq!(stats.total_downloads, 3);
        assert_eq!(stats.total_packages, 2); // nginx + curl (distinct)
        assert_eq!(stats.total_distros, 2); // fedora + arch
    }
}
