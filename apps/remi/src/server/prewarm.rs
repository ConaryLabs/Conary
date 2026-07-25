// apps/remi/src/server/prewarm.rs
//! Pre-warming job for proactive package conversion
//!
//! Downloads and converts popular packages before they're requested,
//! reducing latency for first-time package fetches.

use crate::server::conversion::ConversionService;
use anyhow::{Context, Result};
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// Remi-owned per-distro TUF keys used to sign converted CCS packages.
    pub repository_keys_dir: Option<PathBuf>,
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
pub async fn run_prewarm(config: &PrewarmConfig) -> Result<PrewarmResult> {
    conary_core::repository::supported_profiles::profile_for_remi_route(&config.distro)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "prewarm route '{}' does not map to exactly one public profile",
                config.distro
            )
        })?;
    info!(
        "Starting pre-warm for {} (max {} packages)",
        config.distro, config.max_packages
    );

    let db_path = config.db_path.clone();
    let config_for_lookup = config.clone();
    let packages = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        get_packages_to_convert(&conn, &config_for_lookup)
    })
    .await
    .map_err(|e| anyhow::anyhow!("prewarm package lookup task panicked: {e}"))??;
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
    )
    .with_repository_keys_dir(config.repository_keys_dir.clone());

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

        // Skip current conversions; stale records are rebuilt.
        match existing_conversion_state_async(
            &config.db_path,
            &pkg.name,
            &pkg.version,
            &config.distro,
        )
        .await?
        {
            ExistingConversionState::Current => {
                debug!("Skipping {} {} - already converted", pkg.name, pkg.version);
                result.packages_skipped += 1;
                continue;
            }
            ExistingConversionState::MissingOrStale => {}
        }

        info!("Converting {} {}...", pkg.name, pkg.version);

        match conversion_service
            .convert_package_async(
                &config.distro,
                &pkg.name,
                Some(&pkg.version),
                pkg.architecture.as_deref(),
            )
            .await
        {
            Ok(outcome) => {
                let conv_result = outcome;
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

    result.sort_by_key(|package| std::cmp::Reverse(package.score));
    result
}

/// Get packages to convert, ordered by merged popularity (upstream + local)
fn get_packages_to_convert(
    conn: &rusqlite::Connection,
    config: &PrewarmConfig,
) -> Result<Vec<RepositoryPackage>> {
    let profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(&config.distro)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "prewarm route '{}' does not map to exactly one public profile",
                    config.distro
                )
            })?;
    // Merge upstream + local popularity
    let popularity = merge_popularity(conn, config.popularity_file.as_deref());

    // Query repository packages for this distro
    let sql = format!(
        "SELECT {}
         FROM repository_packages rp
         JOIN repositories r ON rp.repository_id = r.id
         WHERE r.default_strategy_distro = ?1
         AND rp.size > 0
         ORDER BY rp.name, rp.version DESC",
        RepositoryPackage::COLUMNS_PREFIXED
    );
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([profile.id()], RepositoryPackage::from_row)?;

    let mut packages: Vec<RepositoryPackage> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

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

/// Check if a package is already converted.
///
/// Only repository conversion identity fields (`distro`, `package_name`, and
/// `package_version`) are authoritative for Remi prewarming. Installed
/// conversion records keyed by `trove_id` are a separate artifact class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingConversionState {
    MissingOrStale,
    Current,
}

fn existing_conversion_state(
    conn: &rusqlite::Connection,
    name: &str,
    version: &str,
    distro: &str,
) -> Result<ExistingConversionState> {
    for converted in ConvertedPackage::find_current_conversions(conn, distro, Some(name))? {
        if converted.repository_artifact()?.package_version != version {
            continue;
        }
        converted.scriptlet_summary()?;
        return Ok(ExistingConversionState::Current);
    }

    Ok(ExistingConversionState::MissingOrStale)
}

async fn existing_conversion_state_async(
    db_path: &str,
    name: &str,
    version: &str,
    distro: &str,
) -> Result<ExistingConversionState> {
    let db_path = db_path.to_string();
    let name = name.to_string();
    let version = version.to_string();
    let distro = distro.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        existing_conversion_state(&conn, &name, &version, &distro)
    })
    .await
    .map_err(|e| anyhow::anyhow!("prewarm cache lookup task panicked: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};

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
        schema::ensure_current(&conn).unwrap();

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
    fn prewarm_rebuilds_stale_rows_and_skips_current_rows() {
        use conary_core::db::schema;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();

        let mut stale = ConvertedPackage::new_repository(
            "fedora".to_string(),
            "pkg".to_string(),
            "1.0".to_string(),
            "rpm".to_string(),
            "sha256:pkg-1.0-source".to_string(),
            &[],
            3,
            "sha256:pkg-1.0-content".to_string(),
            "/tmp/pkg-1.0.ccs".to_string(),
        );
        stale.conversion_version = CONVERSION_VERSION - 1;
        stale.insert(&conn).unwrap();

        let mut current = ConvertedPackage::new_repository(
            "fedora".to_string(),
            "pkg".to_string(),
            "2.0".to_string(),
            "rpm".to_string(),
            "sha256:pkg-2.0-source".to_string(),
            &[],
            3,
            "sha256:pkg-2.0-content".to_string(),
            "/tmp/pkg-2.0.ccs".to_string(),
        );
        current.insert(&conn).unwrap();

        assert_eq!(
            existing_conversion_state(&conn, "pkg", "1.0", "fedora").unwrap(),
            ExistingConversionState::MissingOrStale
        );
        assert_eq!(
            existing_conversion_state(&conn, "pkg", "2.0", "fedora").unwrap(),
            ExistingConversionState::Current
        );
    }

    #[test]
    fn installed_conversion_identity_is_not_a_prewarm_cache_key() {
        use conary_core::db::models::{ConvertedPackage, Trove, TroveType};
        use conary_core::db::schema;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();

        let mut trove = Trove::new(
            "pkg".to_string(),
            "1.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let mut installed = ConvertedPackage::new_installed(
            trove_id,
            "rpm".to_string(),
            "sha256:installed".to_string(),
        );
        installed.insert(&conn).unwrap();

        assert_eq!(
            existing_conversion_state(&conn, "pkg", "1.0", "fedora").unwrap(),
            ExistingConversionState::MissingOrStale
        );
    }

    #[test]
    fn prewarm_conversion_lookup_propagates_database_errors() {
        use conary_core::db::schema;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();
        conn.execute("DROP TABLE converted_packages", []).unwrap();

        let error = existing_conversion_state(&conn, "pkg", "1.0", "fedora").unwrap_err();
        assert!(error.to_string().contains("converted_packages"), "{error}");
    }

    #[test]
    fn test_merge_popularity_local_only() {
        use conary_core::db::models::{DownloadCount, DownloadStat};
        use conary_core::db::schema;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();

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
        schema::ensure_current(&conn).unwrap();

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
        schema::ensure_current(&conn).unwrap();

        let result = merge_popularity(&conn, None);
        assert!(result.is_empty());
    }
}
