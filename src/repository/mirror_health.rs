// src/repository/mirror_health.rs

//! Mirror health tracking and ranking
//!
//! Tracks per-mirror latency, throughput, and failure rates in the database.
//! Provides ranked mirror selection based on a composite health score.

use crate::error::Result;
use rusqlite::Connection;
use tracing::{debug, info, warn};

/// Exponential moving average smoothing factor for latency/throughput updates
const EMA_ALPHA: f64 = 0.3;

/// Number of consecutive failures before a mirror is automatically disabled
const AUTO_DISABLE_THRESHOLD: i64 = 5;

/// Health data for a single mirror endpoint
#[derive(Debug, Clone)]
pub struct MirrorHealth {
    pub id: i64,
    pub repository_id: i64,
    pub mirror_url: String,
    pub latency_avg_ms: i64,
    pub throughput_bps: i64,
    pub success_count: i64,
    pub failure_count: i64,
    pub consecutive_failures: i64,
    pub health_score: f64,
    pub disabled: bool,
    pub geo_hint: Option<String>,
    pub last_probed: Option<String>,
    pub last_success: Option<String>,
}

fn row_to_mirror_health(row: &rusqlite::Row) -> rusqlite::Result<MirrorHealth> {
    Ok(MirrorHealth {
        id: row.get(0)?,
        repository_id: row.get(1)?,
        mirror_url: row.get(2)?,
        latency_avg_ms: row.get(3)?,
        throughput_bps: row.get(4)?,
        success_count: row.get(5)?,
        failure_count: row.get(6)?,
        consecutive_failures: row.get(7)?,
        health_score: row.get(8)?,
        disabled: row.get::<_, i64>(9)? != 0,
        geo_hint: row.get(10)?,
        last_probed: row.get(11)?,
        last_success: row.get(12)?,
    })
}

/// Database-backed mirror health tracker
///
/// All operations go directly to SQLite. No in-memory caching.
pub struct MirrorHealthTracker;

impl MirrorHealthTracker {
    /// Record a successful request to a mirror.
    ///
    /// Updates latency and throughput using an exponential moving average (alpha=0.3).
    /// Resets consecutive failure count and recalculates health score.
    pub fn record_success(
        conn: &Connection,
        repo_id: i64,
        mirror_url: &str,
        latency_ms: i64,
        bytes_per_sec: i64,
    ) -> Result<()> {
        // Try to fetch existing record
        let existing = Self::get_health(conn, repo_id, mirror_url)?;

        match existing {
            Some(health) => {
                // EMA update for latency and throughput
                let new_latency =
                    (EMA_ALPHA * latency_ms as f64 + (1.0 - EMA_ALPHA) * health.latency_avg_ms as f64) as i64;
                let new_throughput =
                    (EMA_ALPHA * bytes_per_sec as f64 + (1.0 - EMA_ALPHA) * health.throughput_bps as f64) as i64;

                conn.execute(
                    "UPDATE mirror_health SET
                        latency_avg_ms = ?1,
                        throughput_bps = ?2,
                        success_count = success_count + 1,
                        consecutive_failures = 0,
                        last_success = CURRENT_TIMESTAMP
                     WHERE repository_id = ?3 AND mirror_url = ?4",
                    rusqlite::params![new_latency, new_throughput, repo_id, mirror_url],
                )?;
            }
            None => {
                conn.execute(
                    "INSERT INTO mirror_health (repository_id, mirror_url, latency_avg_ms, throughput_bps, success_count, consecutive_failures, last_success)
                     VALUES (?1, ?2, ?3, ?4, 1, 0, CURRENT_TIMESTAMP)",
                    rusqlite::params![repo_id, mirror_url, latency_ms, bytes_per_sec],
                )?;
            }
        }

        Self::update_health_score(conn, repo_id, mirror_url)?;
        debug!(
            "Recorded success for mirror {} (latency={}ms, throughput={} B/s)",
            mirror_url, latency_ms, bytes_per_sec
        );
        Ok(())
    }

    /// Record a failed request to a mirror.
    ///
    /// Increments failure and consecutive failure counters.
    /// Automatically disables the mirror after 5 consecutive failures.
    pub fn record_failure(conn: &Connection, repo_id: i64, mirror_url: &str) -> Result<()> {
        let existing = Self::get_health(conn, repo_id, mirror_url)?;

        match existing {
            Some(_) => {
                conn.execute(
                    "UPDATE mirror_health SET
                        failure_count = failure_count + 1,
                        consecutive_failures = consecutive_failures + 1
                     WHERE repository_id = ?1 AND mirror_url = ?2",
                    rusqlite::params![repo_id, mirror_url],
                )?;
            }
            None => {
                conn.execute(
                    "INSERT INTO mirror_health (repository_id, mirror_url, failure_count, consecutive_failures)
                     VALUES (?1, ?2, 1, 1)",
                    rusqlite::params![repo_id, mirror_url],
                )?;
            }
        }

        // Check if we should auto-disable
        let consecutive: i64 = conn.query_row(
            "SELECT consecutive_failures FROM mirror_health WHERE repository_id = ?1 AND mirror_url = ?2",
            rusqlite::params![repo_id, mirror_url],
            |row| row.get(0),
        )?;

        if consecutive >= AUTO_DISABLE_THRESHOLD {
            warn!(
                "Mirror {} has {} consecutive failures, auto-disabling",
                mirror_url, consecutive
            );
            Self::disable_mirror(conn, repo_id, mirror_url)?;
        }

        Self::update_health_score(conn, repo_id, mirror_url)?;
        debug!(
            "Recorded failure for mirror {} (consecutive={})",
            mirror_url, consecutive
        );
        Ok(())
    }

    /// Get health data for a specific mirror
    pub fn get_health(
        conn: &Connection,
        repo_id: i64,
        mirror_url: &str,
    ) -> Result<Option<MirrorHealth>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_id, mirror_url, latency_avg_ms, throughput_bps,
                    success_count, failure_count, consecutive_failures, health_score,
                    disabled, geo_hint, last_probed, last_success
             FROM mirror_health
             WHERE repository_id = ?1 AND mirror_url = ?2",
        )?;

        let result = stmt.query_row(rusqlite::params![repo_id, mirror_url], row_to_mirror_health);

        match result {
            Ok(health) => Ok(Some(health)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all enabled mirrors for a repository, ranked by health score (best first)
    pub fn get_ranked_mirrors(conn: &Connection, repo_id: i64) -> Result<Vec<MirrorHealth>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_id, mirror_url, latency_avg_ms, throughput_bps,
                    success_count, failure_count, consecutive_failures, health_score,
                    disabled, geo_hint, last_probed, last_success
             FROM mirror_health
             WHERE repository_id = ?1 AND disabled = 0
             ORDER BY health_score DESC",
        )?;

        let mirrors = stmt
            .query_map(rusqlite::params![repo_id], row_to_mirror_health)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(mirrors)
    }

    /// Disable a mirror (exclude from ranked selection)
    pub fn disable_mirror(conn: &Connection, repo_id: i64, mirror_url: &str) -> Result<()> {
        conn.execute(
            "UPDATE mirror_health SET disabled = 1 WHERE repository_id = ?1 AND mirror_url = ?2",
            rusqlite::params![repo_id, mirror_url],
        )?;
        info!("Disabled mirror {}", mirror_url);
        Ok(())
    }

    /// Re-enable a previously disabled mirror
    pub fn enable_mirror(conn: &Connection, repo_id: i64, mirror_url: &str) -> Result<()> {
        conn.execute(
            "UPDATE mirror_health SET disabled = 0, consecutive_failures = 0 WHERE repository_id = ?1 AND mirror_url = ?2",
            rusqlite::params![repo_id, mirror_url],
        )?;
        info!("Enabled mirror {}", mirror_url);
        Ok(())
    }

    /// Recalculate the composite health score for a mirror.
    ///
    /// Formula: `0.4 * success_rate + 0.3 * normalized_throughput + 0.2 * normalized_latency + 0.1 * recency_bonus`
    ///
    /// - success_rate: success_count / (success_count + failure_count)
    /// - normalized_throughput: throughput relative to best mirror in the repo (0.0-1.0)
    /// - normalized_latency: inverse of latency relative to best mirror (0.0-1.0, lower latency = higher score)
    /// - recency_bonus: 1.0 if last success within 1 hour, decays toward 0.0
    pub fn update_health_score(
        conn: &Connection,
        repo_id: i64,
        mirror_url: &str,
    ) -> Result<()> {
        // Get the target mirror's stats
        let health = match Self::get_health(conn, repo_id, mirror_url)? {
            Some(h) => h,
            None => return Ok(()),
        };

        let total = health.success_count + health.failure_count;
        let success_rate = if total > 0 {
            health.success_count as f64 / total as f64
        } else {
            1.0 // New mirror, give benefit of the doubt
        };

        // Get max throughput and min latency across all mirrors for this repo
        let max_throughput: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(throughput_bps), 1) FROM mirror_health WHERE repository_id = ?1",
                rusqlite::params![repo_id],
                |row| row.get(0),
            )
            .unwrap_or(1);

        let min_latency: i64 = conn
            .query_row(
                "SELECT COALESCE(MIN(CASE WHEN latency_avg_ms > 0 THEN latency_avg_ms ELSE NULL END), 1) FROM mirror_health WHERE repository_id = ?1",
                rusqlite::params![repo_id],
                |row| row.get(0),
            )
            .unwrap_or(1);

        let normalized_throughput = if max_throughput > 0 {
            health.throughput_bps as f64 / max_throughput as f64
        } else {
            0.0
        };

        let normalized_latency = if health.latency_avg_ms > 0 {
            min_latency as f64 / health.latency_avg_ms as f64
        } else {
            1.0 // No latency data, assume good
        };

        let recency_bonus = match &health.last_success {
            Some(ts) if !ts.is_empty() => 1.0,
            _ => 0.0,
        };

        let score = 0.4 * success_rate
            + 0.3 * normalized_throughput
            + 0.2 * normalized_latency
            + 0.1 * recency_bonus;

        conn.execute(
            "UPDATE mirror_health SET health_score = ?1 WHERE repository_id = ?2 AND mirror_url = ?3",
            rusqlite::params![score, repo_id, mirror_url],
        )?;

        debug!("Updated health score for {}: {:.3}", mirror_url, score);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        // Insert a test repository
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority) VALUES ('test-repo', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();

        (temp_file, conn)
    }

    #[test]
    fn test_record_success_new_mirror() {
        let (_tmp, conn) = create_test_db();

        MirrorHealthTracker::record_success(&conn, 1, "https://mirror1.example.com", 50, 1_000_000)
            .unwrap();

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://mirror1.example.com")
            .unwrap()
            .unwrap();

        assert_eq!(health.latency_avg_ms, 50);
        assert_eq!(health.throughput_bps, 1_000_000);
        assert_eq!(health.success_count, 1);
        assert_eq!(health.failure_count, 0);
        assert_eq!(health.consecutive_failures, 0);
        assert!(!health.disabled);
    }

    #[test]
    fn test_record_success_ema_update() {
        let (_tmp, conn) = create_test_db();

        // First success: latency=100, throughput=1000
        MirrorHealthTracker::record_success(&conn, 1, "https://mirror1.example.com", 100, 1000)
            .unwrap();

        // Second success: latency=50, throughput=2000
        // EMA: new_latency = 0.3*50 + 0.7*100 = 15 + 70 = 85
        // EMA: new_throughput = 0.3*2000 + 0.7*1000 = 600 + 700 = 1300
        MirrorHealthTracker::record_success(&conn, 1, "https://mirror1.example.com", 50, 2000)
            .unwrap();

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://mirror1.example.com")
            .unwrap()
            .unwrap();

        assert_eq!(health.latency_avg_ms, 85);
        assert_eq!(health.throughput_bps, 1300);
        assert_eq!(health.success_count, 2);
    }

    #[test]
    fn test_record_failure_increments() {
        let (_tmp, conn) = create_test_db();

        MirrorHealthTracker::record_failure(&conn, 1, "https://mirror1.example.com").unwrap();

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://mirror1.example.com")
            .unwrap()
            .unwrap();

        assert_eq!(health.failure_count, 1);
        assert_eq!(health.consecutive_failures, 1);
        assert!(!health.disabled);
    }

    #[test]
    fn test_auto_disable_after_consecutive_failures() {
        let (_tmp, conn) = create_test_db();

        for _ in 0..5 {
            MirrorHealthTracker::record_failure(&conn, 1, "https://bad-mirror.example.com")
                .unwrap();
        }

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://bad-mirror.example.com")
            .unwrap()
            .unwrap();

        assert_eq!(health.consecutive_failures, 5);
        assert!(health.disabled);
    }

    #[test]
    fn test_success_resets_consecutive_failures() {
        let (_tmp, conn) = create_test_db();

        // Record 3 failures
        for _ in 0..3 {
            MirrorHealthTracker::record_failure(&conn, 1, "https://mirror1.example.com").unwrap();
        }

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://mirror1.example.com")
            .unwrap()
            .unwrap();
        assert_eq!(health.consecutive_failures, 3);

        // One success resets consecutive count
        MirrorHealthTracker::record_success(&conn, 1, "https://mirror1.example.com", 50, 1000)
            .unwrap();

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://mirror1.example.com")
            .unwrap()
            .unwrap();
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.failure_count, 3);
        assert_eq!(health.success_count, 1);
    }

    #[test]
    fn test_get_ranked_mirrors() {
        let (_tmp, conn) = create_test_db();

        // Create mirrors with different performance
        MirrorHealthTracker::record_success(&conn, 1, "https://fast.example.com", 10, 10_000_000)
            .unwrap();
        MirrorHealthTracker::record_success(&conn, 1, "https://medium.example.com", 50, 5_000_000)
            .unwrap();
        MirrorHealthTracker::record_success(&conn, 1, "https://slow.example.com", 200, 1_000_000)
            .unwrap();

        let ranked = MirrorHealthTracker::get_ranked_mirrors(&conn, 1).unwrap();
        assert_eq!(ranked.len(), 3);
        // Best mirror should be first
        assert_eq!(ranked[0].mirror_url, "https://fast.example.com");
        // Scores should be descending
        assert!(ranked[0].health_score >= ranked[1].health_score);
        assert!(ranked[1].health_score >= ranked[2].health_score);
    }

    #[test]
    fn test_disabled_mirror_excluded_from_ranking() {
        let (_tmp, conn) = create_test_db();

        MirrorHealthTracker::record_success(&conn, 1, "https://good.example.com", 10, 10_000_000)
            .unwrap();
        MirrorHealthTracker::record_success(&conn, 1, "https://disabled.example.com", 10, 10_000_000)
            .unwrap();

        MirrorHealthTracker::disable_mirror(&conn, 1, "https://disabled.example.com").unwrap();

        let ranked = MirrorHealthTracker::get_ranked_mirrors(&conn, 1).unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].mirror_url, "https://good.example.com");
    }

    #[test]
    fn test_enable_mirror() {
        let (_tmp, conn) = create_test_db();

        MirrorHealthTracker::record_success(&conn, 1, "https://mirror1.example.com", 50, 1000)
            .unwrap();
        MirrorHealthTracker::disable_mirror(&conn, 1, "https://mirror1.example.com").unwrap();

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://mirror1.example.com")
            .unwrap()
            .unwrap();
        assert!(health.disabled);

        MirrorHealthTracker::enable_mirror(&conn, 1, "https://mirror1.example.com").unwrap();

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://mirror1.example.com")
            .unwrap()
            .unwrap();
        assert!(!health.disabled);
        assert_eq!(health.consecutive_failures, 0);
    }

    #[test]
    fn test_health_score_formula() {
        let (_tmp, conn) = create_test_db();

        // Mirror with all successes, good throughput, low latency
        for _ in 0..10 {
            MirrorHealthTracker::record_success(&conn, 1, "https://perfect.example.com", 10, 10_000_000)
                .unwrap();
        }

        let health = MirrorHealthTracker::get_health(&conn, 1, "https://perfect.example.com")
            .unwrap()
            .unwrap();

        // Single mirror: success_rate=1.0, normalized_throughput=1.0,
        // normalized_latency=1.0, recency=1.0
        // Score = 0.4*1.0 + 0.3*1.0 + 0.2*1.0 + 0.1*1.0 = 1.0
        assert!((health.health_score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_get_nonexistent_mirror() {
        let (_tmp, conn) = create_test_db();

        let result = MirrorHealthTracker::get_health(&conn, 1, "https://nonexistent.example.com")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_multiple_repositories() {
        let (_tmp, conn) = create_test_db();

        // Add a second repository
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority) VALUES ('other-repo', 'https://other.com', 1, 5)",
            [],
        )
        .unwrap();

        MirrorHealthTracker::record_success(&conn, 1, "https://mirror.example.com", 50, 1000)
            .unwrap();
        MirrorHealthTracker::record_success(&conn, 2, "https://mirror.example.com", 100, 500)
            .unwrap();

        let health1 = MirrorHealthTracker::get_health(&conn, 1, "https://mirror.example.com")
            .unwrap()
            .unwrap();
        let health2 = MirrorHealthTracker::get_health(&conn, 2, "https://mirror.example.com")
            .unwrap()
            .unwrap();

        // Same URL but different repositories should have separate records
        assert_eq!(health1.latency_avg_ms, 50);
        assert_eq!(health2.latency_avg_ms, 100);
    }
}
