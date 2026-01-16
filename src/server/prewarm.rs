// src/server/prewarm.rs
//! Pre-warming job for proactive package conversion
//!
//! Downloads and converts popular packages before they're requested,
//! reducing latency for first-time package fetches.

use crate::db::models::RepositoryPackage;
use crate::server::conversion::ConversionService;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
    let conn = crate::db::open(&config.db_path)?;

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

        // Run conversion synchronously (blocking)
        match tokio::runtime::Runtime::new()
            .context("Failed to create runtime")?
            .block_on(conversion_service.convert_package(
                &config.distro,
                &pkg.name,
                Some(&pkg.version),
            ))
        {
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
                result.converted.push(format!("{}-{}", pkg.name, pkg.version));
            }
            Err(e) => {
                warn!("Failed to convert {} {}: {}", pkg.name, pkg.version, e);
                result.packages_failed += 1;
                result.failed.push((format!("{}-{}", pkg.name, pkg.version), e.to_string()));
            }
        }
    }

    info!(
        "Pre-warm complete: {} converted, {} skipped, {} failed",
        result.packages_converted, result.packages_skipped, result.packages_failed
    );

    Ok(result)
}

/// Get packages to convert, ordered by popularity
fn get_packages_to_convert(
    conn: &rusqlite::Connection,
    config: &PrewarmConfig,
) -> Result<Vec<RepositoryPackage>> {
    // If popularity file provided, use it for ordering
    let popularity = if let Some(path) = &config.popularity_file {
        load_popularity_data(path)?
    } else {
        vec![]
    };

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
        })
    })?;

    let mut packages: Vec<RepositoryPackage> = rows.filter_map(|r| r.ok()).collect();

    // Filter by pattern if provided
    if let Some(pattern) = &config.pattern {
        let re = regex::Regex::new(pattern).context("Invalid pattern regex")?;
        packages.retain(|p| re.is_match(&p.name));
    }

    // Sort by popularity if we have data
    if !popularity.is_empty() {
        let pop_map: std::collections::HashMap<&str, u64> = popularity
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
fn is_already_converted(
    conn: &rusqlite::Connection,
    name: &str,
    version: &str,
) -> Result<bool> {
    // Check converted_packages table
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM converted_packages cp
         JOIN troves t ON cp.trove_id = t.id
         WHERE t.name = ?1 AND t.version = ?2",
        [name, version],
        |row| row.get(0),
    ).unwrap_or(0);

    Ok(count > 0)
}

/// Background pre-warming task
///
/// Runs periodically to convert popular packages that haven't been requested yet.
#[allow(dead_code)] // Will be used when background pre-warming is enabled
pub async fn run_prewarm_background(
    db_path: String,
    chunk_dir: String,
    cache_dir: String,
    distro: String,
    interval_hours: u64,
    max_packages_per_run: usize,
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
            popularity_file: None,
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
}
