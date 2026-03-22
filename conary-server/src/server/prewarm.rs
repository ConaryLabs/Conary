// conary-server/src/server/prewarm.rs
//! Pre-warming job for proactive package conversion
//!
//! Downloads and converts popular packages before they're requested,
//! reducing latency for first-time package fetches.

use crate::server::conversion::ConversionService;
use anyhow::{Context, Result};
use conary_core::db::models::RepositoryPackage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Pre-warming configuration
#[derive(Debug, Clone)]
pub struct PrewarmConfig {
    /// Path to the database
    pub db_path: String,
    /// Path to chunk storage
    pub chunk_dir: String,
    /// Path to cache directory
    pub cache_dir: String,
    /// Distribution to pre-warm
    pub distro: String,
    /// Maximum number of packages to convert
    pub max_packages: usize,
    /// Path to popularity data file (JSON)
    pub popularity_file: Option<String>,
    /// Only convert packages matching this pattern
    pub pattern: Option<String>,
    /// Dry run - don't actually convert
    pub dry_run: bool,
}

/// Result of a pre-warming run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrewarmResult {
    /// Number of packages processed
    pub packages_processed: usize,
    /// Number of packages successfully converted
    pub packages_converted: usize,
    /// Number of packages skipped (already converted)
    pub packages_skipped: usize,
    /// Number of packages that failed
    pub packages_failed: usize,
    /// Total bytes of chunks created
    pub total_bytes: u64,
    /// List of converted package names
    pub converted: Vec<String>,
    /// List of failed package names with errors
    pub failed: Vec<(String, String)>,
}

/// Popularity data for a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagePopularity {
    /// Package name
    pub name: String,
    /// Popularity score (downloads, installs, etc.)
    pub score: u64,
}

/// Run pre-warming job
pub fn run_prewarm(config: &PrewarmConfig) -> Result<PrewarmResult> {
    info!(
        "Starting pre-warm for {} (max {} packages)",
        config.distro, config.max_packages
    );

    // Open database
    let conn = conary_core::db::open(&config.db_path)?;

    // Get packages to convert
    let packages = get_packages_to_convert(&conn, config)?;
    info!("Found {} packages to potentially convert", packages.len());

    if config.dry_run {
        info!("Dry run - not converting packages");
        return Ok(PrewarmResult {
            packages_processed: packages.len(),
            packages_converted: 0,
            packages_skipped: 0,
            packages_failed: 0,
            total_bytes: 0,
            converted: packages.iter().map(|p| p.name.clone()).collect(),
            failed: vec![],
        });
    }

    // Create conversion service
    let conversion_service = ConversionService::new(
        config.chunk_dir.clone().into(),
        config.cache_dir.clone().into(),
        config.db_path.clone().into(),
        None, // R2 not available in prewarm context
    );

    let mut result = PrewarmResult {
        packages_processed: 0,
        packages_converted: 0,
        packages_skipped: 0,
        packages_failed: 0,
        total_bytes: 0,
        converted: vec![],
        failed: vec![],
    };

    // Convert packages
    for pkg in packages.iter().take(config.max_packages) {
        result.packages_processed += 1;

        // Check if already converted
        if is_already_converted(&conn, &pkg.name, &pkg.version)? {
            debug!("Skipping {} {} - already converted", pkg.name, pkg.version);
            result.packages_skipped += 1;
            continue;
        }

        info!("Converting {} {}...", pkg.name, pkg.version);

        // Run conversion (blocking -- convert_package uses Handle::block_on internally)
        match conversion_service.convert_package(&config.distro, &pkg.name, Some(&pkg.version)) {
            Ok(conv_result) => {
                info!(
                    "Converted {} {}: {} chunks, {} bytes",
                    pkg.name,
                    pkg.version,
                    conv_result.chunk_hashes.len(),
                    conv_result.total_size
                );
                result.packages_converted += 1;
                result.total_bytes += conv_result.total_size;
                result
                    .converted
                    .push(format!("{}-{}", pkg.name, pkg.version));
            }
            Err(e) => {
                warn!("Failed to convert {} {}: {}", pkg.name, pkg.version, e);
                result.packages_failed += 1;
                result
                    .failed
                    .push((format!("{}-{}", pkg.name, pkg.version), e.to_string()));
            }
        }
    }

    info!(
        "Pre-warm complete: {} converted, {} skipped, {} failed",
        result.packages_converted, result.packages_skipped, result.packages_failed
    );

    Ok(result)
}

/// Merge upstream popularity data (from JSON file) with local download statistics.
///
/// Packages that appear in both sources receive a boosted combined score.
/// The final list is sorted by combined score descending.
pub fn merge_popularity(
    conn: &rusqlite::Connection,
    popularity_file: Option<&str>,
) -> Vec<PackagePopularity> {
    // Load upstream popularity from file
    let upstream = popularity_file
        .map(|path| {
            load_popularity_data(path).unwrap_or_else(|e| {
                warn!("Failed to load popularity file: {}", e);
                vec![]
            })
        })
        .unwrap_or_default();

    // Build a map from upstream data
    let mut combined: HashMap<String, u64> = upstream
        .into_iter()
        .map(|entry| (entry.name, entry.score))
        .collect();

    // Query local download statistics (use 30-day counts for recency)
    let local_counts = conn
        .prepare("SELECT package_name, count_30d FROM download_counts ORDER BY count_30d DESC")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_else(|e| {
            debug!("No local download stats available: {}", e);
            vec![]
        });

    // Merge: packages popular both upstream AND locally get highest scores.
    // Local score is weighted 10x to boost packages actually requested on this instance.
    for (name, local_count) in &local_counts {
        let local_score = (*local_count as u64) * 10;
        let entry = combined.entry(name.clone()).or_insert(0);
        *entry += local_score;
    }

    let mut result: Vec<PackagePopularity> = combined
        .into_iter()
        .map(|(name, score)| PackagePopularity { name, score })
        .collect();

    result.sort_by(|a, b| b.score.cmp(&a.score));
    result
}

/// Get packages to convert, ordered by merged popularity (upstream + local)
fn get_packages_to_convert(
    conn: &rusqlite::Connection,
    config: &PrewarmConfig,
) -> Result<Vec<RepositoryPackage>> {
    // Merge upstream + local popularity
    let popularity = merge_popularity(conn, config.popularity_file.as_deref());

    // Query repository packages for this distro
    let distro_pattern = format!("%{}%", config.distro);
    let mut stmt = conn.prepare(
        "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                rp.description, rp.size, rp.checksum, rp.download_url, rp.dependencies
         FROM repository_packages rp
         JOIN repositories r ON rp.repository_id = r.id
         WHERE r.name LIKE ?1 OR r.url LIKE ?2
         ORDER BY rp.name, rp.version DESC",
    )?;

    let rows = stmt.query_map([&distro_pattern, &distro_pattern], |row| {
        Ok(RepositoryPackage {
            id: row.get(0)?,
            repository_id: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            architecture: row.get(4)?,
            description: row.get(5)?,
            size: row.get(6)?,
            checksum: row.get(7)?,
            download_url: row.get(8)?,
            dependencies: row.get(9)?,
            metadata: None,
            synced_at: None,
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            distro: None,
            version_scheme: None,
        })
    })?;

    let mut packages: Vec<RepositoryPackage> = rows.filter_map(|r| r.ok()).collect();

    // Filter by pattern if provided
    if let Some(pattern) = &config.pattern {
        let re = regex::Regex::new(pattern).context("Invalid pattern regex")?;
        packages.retain(|p| re.is_match(&p.name));
    }

    // Sort by merged popularity
    if !popularity.is_empty() {
        let pop_map: HashMap<&str, u64> = popularity
            .iter()
            .map(|p| (p.name.as_str(), p.score))
            .collect();

        packages.sort_by(|a, b| {
            let score_a = pop_map.get(a.name.as_str()).unwrap_or(&0);
            let score_b = pop_map.get(b.name.as_str()).unwrap_or(&0);
            score_b.cmp(score_a) // Descending
        });
    }

    Ok(packages)
}

/// Load popularity data from JSON file
fn load_popularity_data(path: &str) -> Result<Vec<PackagePopularity>> {
    let content = std::fs::read_to_string(path).context("Failed to read popularity file")?;
    let data: Vec<PackagePopularity> =
        serde_json::from_str(&content).context("Failed to parse popularity file")?;
    Ok(data)
}

/// Check if a package is already converted
fn is_already_converted(conn: &rusqlite::Connection, name: &str, version: &str) -> Result<bool> {
    // Check converted_packages table
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM converted_packages cp
         JOIN troves t ON cp.trove_id = t.id
         WHERE t.name = ?1 AND t.version = ?2",
            [name, version],
            |row| row.get(0),
        )
        .unwrap_or(0);

    Ok(count > 0)
}

/// Background pre-warming task
///
/// Runs periodically to convert popular packages that haven't been requested yet.
/// Uses merged popularity from both upstream data and local download statistics.
pub async fn run_prewarm_background(
    db_path: String,
    chunk_dir: String,
    cache_dir: String,
    distro: String,
    interval_hours: u64,
    max_packages_per_run: usize,
    popularity_file: Option<String>,
) {
    use std::time::Duration;

    let interval = Duration::from_secs(interval_hours * 3600);

    loop {
        tokio::time::sleep(interval).await;

        let config = PrewarmConfig {
            db_path: db_path.clone(),
            chunk_dir: chunk_dir.clone(),
            cache_dir: cache_dir.clone(),
            distro: distro.clone(),
            max_packages: max_packages_per_run,
            popularity_file: popularity_file.clone(),
            pattern: None,
            dry_run: false,
        };

        // Run in blocking task
        match tokio::task::spawn_blocking(move || run_prewarm(&config)).await {
            Ok(Ok(result)) => {
                info!(
                    "Background pre-warm complete: {} converted, {} failed",
                    result.packages_converted, result.packages_failed
                );
            }
            Ok(Err(e)) => {
                warn!("Background pre-warm failed: {}", e);
            }
            Err(e) => {
                warn!("Background pre-warm task panicked: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prewarm_result_serialization() {
        let result = PrewarmResult {
            packages_processed: 10,
            packages_converted: 8,
            packages_skipped: 1,
            packages_failed: 1,
            total_bytes: 1024 * 1024,
            converted: vec!["nginx-1.24.0".to_string(), "curl-8.0.0".to_string()],
            failed: vec![("broken-1.0.0".to_string(), "Download failed".to_string())],
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("nginx-1.24.0"));
        assert!(json.contains("packages_converted"));

        let parsed: PrewarmResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.packages_converted, 8);
    }

    #[test]
    fn test_popularity_data_parsing() {
        let json = r#"[
            {"name": "nginx", "score": 1000},
            {"name": "curl", "score": 800},
            {"name": "vim", "score": 500}
        ]"#;

        let data: Vec<PackagePopularity> = serde_json::from_str(json).unwrap();
        assert_eq!(data.len(), 3);
        assert_eq!(data[0].name, "nginx");
        assert_eq!(data[0].score, 1000);
    }

    #[test]
    fn test_merge_popularity_upstream_only() {
        use conary_core::db::schema;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        // Write a temporary popularity file
        let pop_file = NamedTempFile::new().unwrap();
        let pop_data = r#"[
            {"name": "nginx", "score": 1000},
            {"name": "curl", "score": 800}
        ]"#;
        std::fs::write(pop_file.path(), pop_data).unwrap();

        let result = merge_popularity(&conn, Some(pop_file.path().to_str().unwrap()));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "nginx");
        assert_eq!(result[0].score, 1000);
        assert_eq!(result[1].name, "curl");
        assert_eq!(result[1].score, 800);
    }

    #[test]
    fn test_merge_popularity_local_only() {
        use conary_core::db::models::{DownloadCount, DownloadStat};
        use conary_core::db::schema;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        // Insert some download stats
        let events = vec![
            DownloadStat::new("fedora".into(), "vim".into()),
            DownloadStat::new("fedora".into(), "vim".into()),
            DownloadStat::new("fedora".into(), "vim".into()),
            DownloadStat::new("fedora".into(), "git".into()),
        ];
        DownloadStat::insert_batch(&conn, &events).unwrap();
        DownloadCount::refresh_aggregates(&conn).unwrap();

        let result = merge_popularity(&conn, None);
        assert_eq!(result.len(), 2);
        // vim has 3 downloads * 10 = 30 score, git has 1 * 10 = 10
        assert_eq!(result[0].name, "vim");
        assert_eq!(result[0].score, 30);
        assert_eq!(result[1].name, "git");
        assert_eq!(result[1].score, 10);
    }

    #[test]
    fn test_merge_popularity_combined() {
        use conary_core::db::models::{DownloadCount, DownloadStat};
        use conary_core::db::schema;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        // Upstream: nginx=1000, curl=800
        let pop_file = NamedTempFile::new().unwrap();
        let pop_data = r#"[
            {"name": "nginx", "score": 1000},
            {"name": "curl", "score": 800}
        ]"#;
        std::fs::write(pop_file.path(), pop_data).unwrap();

        // Local: curl downloaded 5 times (5*10=50 boost), vim 2 times (2*10=20)
        let events = vec![
            DownloadStat::new("fedora".into(), "curl".into()),
            DownloadStat::new("fedora".into(), "curl".into()),
            DownloadStat::new("fedora".into(), "curl".into()),
            DownloadStat::new("fedora".into(), "curl".into()),
            DownloadStat::new("fedora".into(), "curl".into()),
            DownloadStat::new("fedora".into(), "vim".into()),
            DownloadStat::new("fedora".into(), "vim".into()),
        ];
        DownloadStat::insert_batch(&conn, &events).unwrap();
        DownloadCount::refresh_aggregates(&conn).unwrap();

        let result = merge_popularity(&conn, Some(pop_file.path().to_str().unwrap()));

        // Expected: nginx=1000, curl=800+50=850, vim=20
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "nginx");
        assert_eq!(result[0].score, 1000);
        assert_eq!(result[1].name, "curl");
        assert_eq!(result[1].score, 850);
        assert_eq!(result[2].name, "vim");
        assert_eq!(result[2].score, 20);
    }

    #[test]
    fn test_merge_popularity_no_data() {
        use conary_core::db::schema;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        let result = merge_popularity(&conn, None);
        assert!(result.is_empty());
    }
}
