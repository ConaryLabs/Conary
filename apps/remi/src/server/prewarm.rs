// apps/remi/src/server/prewarm.rs
//! Pre-warming job for proactive package conversion
//!
//! Downloads and converts popular packages before they're requested,
//! reducing latency for first-time package fetches.

use crate::server::catalog_authority::{CatalogAuthority, PinnedProfileCatalog};
use crate::server::conversion::ConversionService;
use crate::server::profile_catalog::ProfileCatalog;
use anyhow::{Context, Result};
use conary_core::corpus::ConversionFailure;
use conary_core::db::models::ConvertedPackage;
use conary_core::repository::catalog::CatalogPackageRecordV1;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

mod scheduler;

pub(crate) use scheduler::run_prewarm_jobs;

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
    /// Failed packages with their typed failure category.
    ///
    /// The category comes from the concrete error type, so prewarm results can
    /// be aggregated by authority rather than by message wording. See
    /// `conary_core::corpus` for the taxonomy.
    pub failed: Vec<PrewarmFailure>,
}

/// One package that failed prewarm, with its typed reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrewarmFailure {
    /// `name-version` of the package that failed.
    pub package: String,
    /// Typed category plus its human-readable diagnostic.
    pub failure: ConversionFailure,
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
    run_prewarm_with_permits(config, None, None).await
}

async fn run_prewarm_with_permits(
    config: &PrewarmConfig,
    conversion_permits: Option<&Semaphore>,
    shared_conversion_service: Option<ConversionService>,
) -> Result<PrewarmResult> {
    let source_profile =
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

    // Standalone prewarm has no ServerState to supply the authority. The
    // configured chunk root and server storage layout share the same parent,
    // so derive the immutable catalog root from that stable product path.
    let conversion_service = shared_conversion_service.unwrap_or_else(|| {
        let catalog_dir = PathBuf::from(&config.chunk_dir)
            .parent()
            .map(|parent| parent.join("catalogs"))
            .unwrap_or_else(|| PathBuf::from("catalogs"));
        let authority = CatalogAuthority::from_paths(
            config.db_path.clone(),
            catalog_dir,
            crate::server::database_writer::DatabaseWriter::default(),
        );
        ConversionService::new(
            config.chunk_dir.clone().into(),
            config.cache_dir.clone().into(),
            config.db_path.clone().into(),
            None,
        )
        .with_catalog_authority(authority)
        .with_repository_keys_dir(config.repository_keys_dir.clone())
    });
    let catalog_authority = conversion_service
        .catalog_authority
        .clone()
        .ok_or_else(|| {
            anyhow::anyhow!("prewarm requires an immutable profile catalog authority")
        })?;

    let db_path = config.db_path.clone();
    let config_for_lookup = config.clone();
    let (selection_pin, packages) = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        let pinned = catalog_authority.open_active_profile(source_profile.id())?;
        let packages = get_packages_to_convert(&pinned, &conn, &config_for_lookup)?;
        Ok::<_, anyhow::Error>((pinned, packages))
    })
    .await
    .map_err(|e| anyhow::anyhow!("prewarm package lookup task panicked: {e}"))??;
    let selection = selection_pin.selection().clone();
    let profile_revision_sha256 = selection.profile_revision_sha256.clone();
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
            pkg.architecture.as_deref(),
            &profile_revision_sha256,
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

        let _permit = match conversion_permits {
            Some(permits) => Some(
                permits
                    .acquire()
                    .await
                    .map_err(|_| anyhow::anyhow!("conversion semaphore closed"))?,
            ),
            None => None,
        };
        match conversion_service
            .convert_package_from_selection_async(
                &config.distro,
                &pkg.name,
                Some(&pkg.version),
                pkg.architecture.as_deref(),
                selection.clone(),
            )
            .await
        {
            Ok(outcome) => {
                let conv_result = outcome;
                info!(
                    "Converted {} {}: {} chunks, {} bytes",
                    pkg.name,
                    pkg.version,
                    conv_result.transport.objects.len(),
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
                result.failed.push(PrewarmFailure {
                    package: format!("{}-{}", pkg.name, pkg.version),
                    failure: ConversionFailure::classify(&e),
                });
            }
        }
    }

    info!(
        "Pre-warm complete: {} converted, {} skipped, {} failed",
        result.packages_converted, result.packages_skipped, result.packages_failed
    );

    drop(selection_pin);
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
    pinned: &PinnedProfileCatalog,
    conn: &rusqlite::Connection,
    config: &PrewarmConfig,
) -> Result<Vec<CatalogPackageRecordV1>> {
    let profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(&config.distro)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "prewarm route '{}' does not map to exactly one public profile",
                    config.distro
                )
            })?;
    if pinned.source_profile() != profile.id() {
        anyhow::bail!(
            "prewarm route '{}' resolved profile '{}' but active catalog is for '{}'",
            config.distro,
            profile.id(),
            pinned.source_profile()
        );
    }
    // Merge upstream + local popularity
    let popularity = merge_popularity(conn, config.popularity_file.as_deref());

    // Package identity and download metadata are owned by the pinned catalog;
    // operational repository rows are not a fallback authority.
    let mut packages = ProfileCatalog::new(pinned).downloadable_package_records(1)?;
    packages.sort_by(catalog_package_order);

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
            score_b
                .cmp(score_a)
                .then_with(|| catalog_package_order(a, b))
        });
    }

    Ok(packages)
}

fn catalog_package_order(
    left: &CatalogPackageRecordV1,
    right: &CatalogPackageRecordV1,
) -> Ordering {
    left.name
        .cmp(&right.name)
        .then_with(|| right.version.cmp(&left.version))
        .then_with(|| left.package_release.cmp(&right.package_release))
        .then_with(|| left.architecture.cmp(&right.architecture))
        .then_with(|| left.package_key_sha256.cmp(&right.package_key_sha256))
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
/// The active immutable profile revision is the cache authority. Installed
/// conversion records keyed by `trove_id` are a separate artifact class, and
/// repository rows without a valid conversion pin never count as a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingConversionState {
    MissingOrStale,
    Current,
}

fn existing_conversion_state(
    conn: &rusqlite::Connection,
    name: &str,
    version: &str,
    architecture: Option<&str>,
    profile_revision_sha256: &str,
) -> Result<ExistingConversionState> {
    for converted in
        ConvertedPackage::find_current_conversions(conn, profile_revision_sha256, Some(name))?
    {
        let converted_id = converted.id.ok_or_else(|| {
            anyhow::anyhow!(
                "current repository conversion for '{}' has no durable database identity",
                name
            )
        })?;
        // A status/cache hit is valid only when its exact durable conversion
        // pin still names the active profile revision.
        ConvertedPackage::require_conversion_pin(conn, converted_id)?;
        let artifact = converted.repository_artifact()?;
        if artifact.package_version != version
            || architecture.is_some_and(|expected| artifact.package_architecture != expected)
        {
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
    architecture: Option<&str>,
    profile_revision_sha256: &str,
) -> Result<ExistingConversionState> {
    let db_path = db_path.to_string();
    let name = name.to_string();
    let version = version.to_string();
    let architecture = architecture.map(str::to_string);
    let profile_revision_sha256 = profile_revision_sha256.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        existing_conversion_state(
            &conn,
            &name,
            &version,
            architecture.as_deref(),
            &profile_revision_sha256,
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!("prewarm cache lookup task panicked: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::{
        ActiveCatalogFixture, package as catalog_package,
    };
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
            failed: vec![PrewarmFailure {
                package: "broken-1.0.0".to_string(),
                failure: ConversionFailure::Publication {
                    detail: "Download failed".to_string(),
                },
            }],
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
    fn prewarm_selection_reads_the_active_catalog_without_operational_packages() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![catalog_package(
                "fedora-44",
                "catalog-only",
                "1.0",
                "",
                Some("x86_64"),
                3,
                "catalog-selection",
            )],
        );
        let conn = fixture.connection();
        let pinned = fixture
            .authority()
            .open_active_profile("fedora-44")
            .unwrap();
        assert_eq!(pinned.profile_revision_sha256(), revision);

        let config = PrewarmConfig {
            db_path: fixture.db_path().display().to_string(),
            chunk_dir: fixture
                .db_path()
                .with_extension("chunks")
                .display()
                .to_string(),
            cache_dir: fixture
                .db_path()
                .with_extension("cache")
                .display()
                .to_string(),
            repository_keys_dir: None,
            distro: "fedora".to_string(),
            max_packages: 10,
            popularity_file: None,
            pattern: None,
            dry_run: false,
        };
        let packages = get_packages_to_convert(&pinned, &conn, &config).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "catalog-only");
    }

    #[test]
    fn prewarm_rebuilds_stale_rows_and_skips_current_rows() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![
                catalog_package("fedora-44", "pkg", "1.0", "", Some("x86_64"), 3, "one"),
                catalog_package("fedora-44", "pkg", "2.0", "", Some("x86_64"), 3, "two"),
            ],
        );
        let conn = fixture.connection();

        let transport = crate::server::conversion::test_support::test_transport(&[]);
        let mut stale = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            revision.clone(),
            "pkg".to_string(),
            "1.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            "sha256:pkg-1.0-source".to_string(),
            &transport,
            3,
            "sha256:pkg-1.0-content".to_string(),
            "/tmp/pkg-1.0.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        stale.conversion_version = CONVERSION_VERSION - 1;
        stale.insert_with_conversion_pin(&conn, 1).unwrap();

        let transport = crate::server::conversion::test_support::test_transport(&[]);
        let mut current = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            revision.clone(),
            "pkg".to_string(),
            "2.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            "sha256:pkg-2.0-source".to_string(),
            &transport,
            3,
            "sha256:pkg-2.0-content".to_string(),
            "/tmp/pkg-2.0.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        current.insert_with_conversion_pin(&conn, 1).unwrap();

        assert_eq!(
            existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap(),
            ExistingConversionState::MissingOrStale
        );
        assert_eq!(
            existing_conversion_state(&conn, "pkg", "2.0", Some("x86_64"), &revision).unwrap(),
            ExistingConversionState::Current
        );
    }

    #[test]
    fn prewarm_cache_hits_require_the_exact_durable_conversion_pin() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![catalog_package(
                "fedora-44",
                "pkg",
                "1.0",
                "",
                Some("x86_64"),
                3,
                "pin",
            )],
        );
        let conn = fixture.connection();
        let transport = crate::server::conversion::test_support::test_transport(&[]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            revision.clone(),
            "pkg".to_string(),
            "1.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            "sha256:pkg-source".to_string(),
            &transport,
            3,
            "sha256:pkg-content".to_string(),
            "/tmp/pkg.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        let converted_id = converted.insert_with_conversion_pin(&conn, 1).unwrap();
        conn.execute(
            "DELETE FROM remi_profile_revision_pins WHERE owner_kind = 'conversion' AND owner_identity = ?1",
            [converted_id.to_string()],
        )
        .unwrap();

        let error =
            existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap_err();
        assert!(
            error.to_string().contains("no exact profile-revision pin"),
            "{error}"
        );
    }

    #[test]
    fn installed_conversion_identity_is_not_a_prewarm_cache_key() {
        use conary_core::db::models::{ConvertedPackage, Trove, TroveType};

        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![catalog_package(
                "fedora-44",
                "pkg",
                "1.0",
                "",
                Some("x86_64"),
                3,
                "installed",
            )],
        );
        let conn = fixture.connection();

        let mut trove = Trove::new(
            "pkg".to_string(),
            "1.0.0".to_string(),
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
            existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap(),
            ExistingConversionState::MissingOrStale
        );
    }

    #[test]
    fn prewarm_conversion_lookup_propagates_database_errors() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![catalog_package(
                "fedora-44",
                "pkg",
                "1.0",
                "",
                Some("x86_64"),
                3,
                "database-error",
            )],
        );
        let conn = fixture.connection();
        conn.execute("DROP TABLE converted_packages", []).unwrap();

        let error =
            existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap_err();
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
            DownloadStat::new("fedora-44".into(), "vim".into()),
            DownloadStat::new("fedora-44".into(), "vim".into()),
            DownloadStat::new("fedora-44".into(), "vim".into()),
            DownloadStat::new("fedora-44".into(), "git".into()),
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
            DownloadStat::new("fedora-44".into(), "curl".into()),
            DownloadStat::new("fedora-44".into(), "curl".into()),
            DownloadStat::new("fedora-44".into(), "curl".into()),
            DownloadStat::new("fedora-44".into(), "curl".into()),
            DownloadStat::new("fedora-44".into(), "curl".into()),
            DownloadStat::new("fedora-44".into(), "vim".into()),
            DownloadStat::new("fedora-44".into(), "vim".into()),
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
