// src/repository/mirror_selector.rs

//! Dynamic mirror selection based on health tracking data
//!
//! Provides strategies for choosing the best mirror for downloads:
//! - Static: use configured URL directly
//! - Dynamic: weighted random selection based on health scores
//! - Chain: try mirrors in fixed order (Nix-style)

use crate::error::{Error, Result};
use crate::repository::mirror_health::MirrorHealthTracker;
use rand::distributions::WeightedIndex;
use rand::prelude::*;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Strategy for selecting mirrors
#[derive(Debug, Clone)]
pub enum MirrorStrategy {
    /// Use the configured URL directly (current behavior)
    Static,
    /// Select based on health scores with weighted random
    Dynamic,
    /// Try sources in fixed order (Nix-style)
    Chain(Vec<String>),
}

/// Mirror selector that queries health data to rank mirrors
pub struct MirrorSelector {
    db_path: PathBuf,
}

impl MirrorSelector {
    /// Create a new mirror selector backed by the given database
    pub fn new(db_path: impl AsRef<Path>) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
        }
    }

    /// Return an ordered list of mirror URLs based on the strategy.
    ///
    /// - `Static`: returns an empty vec (caller should use the original URL)
    /// - `Dynamic`: queries mirror_health, returns mirrors weighted by health score
    /// - `Chain`: returns the configured list as-is
    pub fn select_mirrors(&self, repo_id: i64, strategy: &MirrorStrategy) -> Result<Vec<String>> {
        match strategy {
            MirrorStrategy::Static => Ok(Vec::new()),
            MirrorStrategy::Chain(urls) => Ok(urls.clone()),
            MirrorStrategy::Dynamic => {
                let conn = Connection::open(&self.db_path).map_err(|e| {
                    Error::InitError(format!(
                        "Failed to open database {}: {}",
                        self.db_path.display(),
                        e
                    ))
                })?;
                Self::select_dynamic(&conn, repo_id)
            }
        }
    }

    /// Select mirrors using weighted random based on health scores.
    ///
    /// Higher health scores get proportionally more selection probability.
    /// Returns all enabled mirrors in a weighted-shuffled order: repeatedly
    /// samples from the remaining mirrors without replacement, so healthier
    /// mirrors are more likely to appear earlier in the list.
    fn select_dynamic(conn: &Connection, repo_id: i64) -> Result<Vec<String>> {
        let mirrors = MirrorHealthTracker::get_ranked_mirrors(conn, repo_id)?;

        if mirrors.is_empty() {
            return Ok(Vec::new());
        }

        if mirrors.len() == 1 {
            return Ok(vec![mirrors[0].mirror_url.clone()]);
        }

        // Build weights from health scores (minimum weight 0.01 to avoid zero)
        let mut remaining: Vec<(String, f64)> = mirrors
            .iter()
            .map(|m| (m.mirror_url.clone(), m.health_score.max(0.01)))
            .collect();

        let mut rng = thread_rng();
        let mut result = Vec::with_capacity(remaining.len());

        // Weighted shuffle: repeatedly sample without replacement
        while !remaining.is_empty() {
            let weights: Vec<f64> = remaining.iter().map(|(_, w)| *w).collect();
            match WeightedIndex::new(&weights) {
                Ok(dist) => {
                    let idx = dist.sample(&mut rng);
                    let (url, _) = remaining.remove(idx);
                    result.push(url);
                }
                Err(_) => {
                    // All remaining weights are zero; append in current order
                    result.extend(remaining.drain(..).map(|(url, _)| url));
                }
            }
        }

        debug!(
            "Dynamic selection for repo {}: {} mirrors, primary={}",
            repo_id,
            result.len(),
            result[0]
        );

        Ok(result)
    }

    /// Try an operation across mirrors with automatic fallback and health recording.
    ///
    /// Attempts the closure on each mirror URL in order. Records success or
    /// failure to the health tracker for future selection decisions. Returns the
    /// result along with the mirror URL that succeeded.
    ///
    /// If all mirrors fail, returns the error from the last attempt.
    pub fn try_download_with_fallback<T, F>(
        mirrors: &[String],
        operation: F,
        conn: &Connection,
        repo_id: i64,
    ) -> Result<(T, String)>
    where
        F: Fn(&str) -> Result<T>,
    {
        if mirrors.is_empty() {
            return Err(Error::NotFound("No mirrors available".to_string()));
        }

        let mut last_error: Option<Error> = None;

        for mirror_url in mirrors {
            debug!("Trying mirror: {}", mirror_url);

            let start = std::time::Instant::now();
            match operation(mirror_url) {
                Ok(result) => {
                    let elapsed_ms = start.elapsed().as_millis() as i64;
                    if let Err(e) = MirrorHealthTracker::record_success(
                        conn, repo_id, mirror_url, elapsed_ms, 0,
                    ) {
                        warn!("Failed to record mirror success for {}: {}", mirror_url, e);
                    }
                    info!("Mirror {} succeeded in {}ms", mirror_url, elapsed_ms);
                    return Ok((result, mirror_url.clone()));
                }
                Err(e) => {
                    warn!("Mirror {} failed: {}", mirror_url, e);
                    if let Err(record_err) =
                        MirrorHealthTracker::record_failure(conn, repo_id, mirror_url)
                    {
                        warn!(
                            "Failed to record mirror failure for {}: {}",
                            mirror_url, record_err
                        );
                    }
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::DownloadError("All mirrors failed".to_string())))
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

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority) VALUES ('test-repo', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();

        (temp_file, conn)
    }

    #[test]
    fn test_static_returns_empty() {
        let tmp = NamedTempFile::new().unwrap();
        let selector = MirrorSelector::new(tmp.path());

        let result = selector.select_mirrors(1, &MirrorStrategy::Static).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_chain_returns_list() {
        let tmp = NamedTempFile::new().unwrap();
        let selector = MirrorSelector::new(tmp.path());

        let urls = vec![
            "https://mirror1.example.com".to_string(),
            "https://mirror2.example.com".to_string(),
            "https://mirror3.example.com".to_string(),
        ];

        let result = selector
            .select_mirrors(1, &MirrorStrategy::Chain(urls.clone()))
            .unwrap();
        assert_eq!(result, urls);
    }

    #[test]
    fn test_dynamic_selection() {
        let (tmp, conn) = create_test_db();

        // Insert mirror health data with varying scores
        MirrorHealthTracker::record_success(&conn, 1, "https://fast.example.com", 10, 10_000_000)
            .unwrap();
        MirrorHealthTracker::record_success(&conn, 1, "https://medium.example.com", 50, 5_000_000)
            .unwrap();
        MirrorHealthTracker::record_success(&conn, 1, "https://slow.example.com", 200, 1_000_000)
            .unwrap();
        drop(conn);

        let selector = MirrorSelector::new(tmp.path());
        let mirrors = selector
            .select_mirrors(1, &MirrorStrategy::Dynamic)
            .unwrap();

        // Should return all 3 mirrors
        assert_eq!(mirrors.len(), 3);
        // All URLs should be present (order varies due to weighted random)
        assert!(mirrors.contains(&"https://fast.example.com".to_string()));
        assert!(mirrors.contains(&"https://medium.example.com".to_string()));
        assert!(mirrors.contains(&"https://slow.example.com".to_string()));
    }

    #[test]
    fn test_dynamic_empty_when_no_mirrors() {
        let (tmp, conn) = create_test_db();
        drop(conn);

        let selector = MirrorSelector::new(tmp.path());
        let result = selector
            .select_mirrors(1, &MirrorStrategy::Dynamic)
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_dynamic_single_mirror() {
        let (tmp, conn) = create_test_db();

        MirrorHealthTracker::record_success(&conn, 1, "https://only.example.com", 50, 1_000_000)
            .unwrap();
        drop(conn);

        let selector = MirrorSelector::new(tmp.path());
        let result = selector
            .select_mirrors(1, &MirrorStrategy::Dynamic)
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "https://only.example.com");
    }

    #[test]
    fn test_dynamic_weighted_selection_favors_healthy() {
        let (tmp, conn) = create_test_db();

        // Give fast mirror many successes, slow mirror many failures
        for _ in 0..20 {
            MirrorHealthTracker::record_success(
                &conn,
                1,
                "https://fast.example.com",
                10,
                10_000_000,
            )
            .unwrap();
        }
        MirrorHealthTracker::record_success(&conn, 1, "https://slow.example.com", 500, 100_000)
            .unwrap();
        for _ in 0..4 {
            MirrorHealthTracker::record_failure(&conn, 1, "https://slow.example.com").unwrap();
        }
        drop(conn);

        let selector = MirrorSelector::new(tmp.path());

        // Run multiple selections and count how often fast is picked first
        let mut fast_first = 0;
        let iterations = 100;
        for _ in 0..iterations {
            let result = selector
                .select_mirrors(1, &MirrorStrategy::Dynamic)
                .unwrap();
            if result[0] == "https://fast.example.com" {
                fast_first += 1;
            }
        }

        // Fast mirror should be selected most of the time (at least 60%)
        assert!(
            fast_first > 60,
            "Expected fast mirror to be selected >60% of the time, got {}/{}",
            fast_first,
            iterations
        );
    }

    #[test]
    fn test_fallback_tries_all_mirrors() {
        let (_tmp, conn) = create_test_db();

        let mirrors = vec![
            "https://mirror1.example.com".to_string(),
            "https://mirror2.example.com".to_string(),
            "https://mirror3.example.com".to_string(),
        ];

        // Operation that only succeeds on the last mirror
        let result = MirrorSelector::try_download_with_fallback(
            &mirrors,
            |url| {
                if url == "https://mirror3.example.com" {
                    Ok("success")
                } else {
                    Err(Error::DownloadError(format!("{} failed", url)))
                }
            },
            &conn,
            1,
        )
        .unwrap();

        assert_eq!(result.0, "success");
        assert_eq!(result.1, "https://mirror3.example.com");
    }

    #[test]
    fn test_fallback_records_health() {
        let (_tmp, conn) = create_test_db();

        let mirrors = vec![
            "https://bad.example.com".to_string(),
            "https://good.example.com".to_string(),
        ];

        // First mirror fails, second succeeds
        let _result = MirrorSelector::try_download_with_fallback(
            &mirrors,
            |url| {
                if url == "https://good.example.com" {
                    Ok(42)
                } else {
                    Err(Error::DownloadError("simulated failure".to_string()))
                }
            },
            &conn,
            1,
        )
        .unwrap();

        // Check that failure was recorded
        let bad_health =
            MirrorHealthTracker::get_health(&conn, 1, "https://bad.example.com").unwrap();
        assert!(bad_health.is_some());
        let bad = bad_health.unwrap();
        assert_eq!(bad.failure_count, 1);

        // Check that success was recorded
        let good_health =
            MirrorHealthTracker::get_health(&conn, 1, "https://good.example.com").unwrap();
        assert!(good_health.is_some());
        let good = good_health.unwrap();
        assert_eq!(good.success_count, 1);
    }

    #[test]
    fn test_fallback_all_fail() {
        let (_tmp, conn) = create_test_db();

        let mirrors = vec![
            "https://mirror1.example.com".to_string(),
            "https://mirror2.example.com".to_string(),
        ];

        let result = MirrorSelector::try_download_with_fallback::<(), _>(
            &mirrors,
            |url| Err(Error::DownloadError(format!("{} unavailable", url))),
            &conn,
            1,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_fallback_empty_mirrors() {
        let (_tmp, conn) = create_test_db();

        let result =
            MirrorSelector::try_download_with_fallback::<(), _>(&[], |_url| Ok(()), &conn, 1);

        assert!(result.is_err());
    }
}
