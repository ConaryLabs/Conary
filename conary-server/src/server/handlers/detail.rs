// conary-server/src/server/handlers/detail.rs
//! Package detail API for the Remi package index
//!
//! Provides rich per-package endpoints for the web frontend, including
//! package metadata, version history, dependency graphs, and statistics.
//! All database queries run via `spawn_blocking` for async compatibility.

use crate::server::ServerState;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::db::models::DownloadCount;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::run_blocking;

/// Full package detail response
#[derive(Debug, Serialize)]
pub struct PackageDetail {
    pub name: String,
    pub distro: String,
    pub latest_version: String,
    pub description: Option<String>,
    pub versions: Vec<VersionSummary>,
    pub dependencies: Vec<String>,
    pub download_count: i64,
    pub download_count_30d: i64,
    pub size_bytes: i64,
    pub license: Option<String>,
    pub homepage: Option<String>,
    pub converted: bool,
}

/// Version entry within a package detail
#[derive(Debug, Serialize)]
pub struct VersionSummary {
    pub version: String,
    pub architecture: Option<String>,
    pub size: i64,
    pub converted: bool,
}

/// Query parameters for stats endpoints
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    /// Optional distribution filter
    pub distro: Option<String>,
    /// Maximum results (default 50, max 200)
    pub limit: Option<usize>,
}

/// Popular/recent package entry
#[derive(Debug, Serialize)]
pub struct PackageSummary {
    pub name: String,
    pub distro: String,
    pub version: String,
    pub description: Option<String>,
    pub download_count: i64,
    pub size: i64,
}

/// Global overview statistics
#[derive(Debug, Serialize)]
pub struct OverviewStats {
    pub total_packages: i64,
    pub total_downloads: i64,
    pub downloads_30d: i64,
    pub total_distros: i64,
    pub total_converted: i64,
}

/// GET /v1/packages/:distro/:name
///
/// Full package detail including versions, download counts, and metadata.
pub async fn get_package_detail(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Result<Response, Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let db_path = state.read().await.config.db_path.clone();
    let detail = run_blocking("package detail", move || {
        query_package_detail(&db_path, &distro, &name)
    })
    .await?;

    match detail {
        Some(detail) => Ok((
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(detail),
        )
            .into_response()),
        None => Ok((StatusCode::NOT_FOUND, "Package not found").into_response()),
    }
}

/// GET /v1/packages/:distro/:name/versions
///
/// List all available versions for a package.
pub async fn get_versions(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Result<Response, Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let db_path = state.read().await.config.db_path.clone();
    let versions =
        run_blocking("versions", move || query_versions(&db_path, &distro, &name)).await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(versions),
    )
        .into_response())
}

/// GET /v1/packages/:distro/:name/dependencies
///
/// List dependencies for a package.
pub async fn get_dependencies(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Result<Response, Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let db_path = state.read().await.config.db_path.clone();
    let deps = run_blocking("dependencies", move || {
        query_dependencies(&db_path, &distro, &name)
    })
    .await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(deps),
    )
        .into_response())
}

/// GET /v1/packages/:distro/:name/rdepends
///
/// List packages that depend on this package (reverse dependencies).
pub async fn get_reverse_dependencies(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Result<Response, Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let db_path = state.read().await.config.db_path.clone();
    let rdeps = run_blocking("reverse dependencies", move || {
        query_reverse_dependencies(&db_path, &distro, &name)
    })
    .await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(rdeps),
    )
        .into_response())
}

/// GET /v1/stats/popular?distro=fedora&limit=50
///
/// Most popular packages by download count.
pub async fn get_popular(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<StatsQuery>,
) -> Result<Response, Response> {
    let db_path = state.read().await.config.db_path.clone();
    let limit = params.limit.unwrap_or(50).min(200);
    let distro = params.distro;

    let packages = run_blocking("popular", move || {
        query_popular(&db_path, distro.as_deref(), limit)
    })
    .await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(packages),
    )
        .into_response())
}

/// GET /v1/stats/recent?distro=fedora&limit=50
///
/// Recently updated packages (by sync time).
pub async fn get_recent(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<StatsQuery>,
) -> Result<Response, Response> {
    let db_path = state.read().await.config.db_path.clone();
    let limit = params.limit.unwrap_or(50).min(200);
    let distro = params.distro;

    let packages = run_blocking("recent", move || {
        query_recent(&db_path, distro.as_deref(), limit)
    })
    .await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(packages),
    )
        .into_response())
}

/// GET /v1/stats/overview
///
/// Global statistics: total packages, downloads, distros, conversions.
pub async fn get_overview(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Response, Response> {
    let db_path = state.read().await.config.db_path.clone();

    let stats = run_blocking("overview", move || query_overview(&db_path)).await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=60")],
        Json(stats),
    )
        .into_response())
}

// --- Database query functions (run on blocking threads) ---

fn query_package_detail(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Option<PackageDetail>> {
    let conn = conary_core::db::open(db_path)?;

    let repo_ids = resolve_all_repo_ids(&conn, distro)?;
    if repo_ids.is_empty() {
        return Ok(None);
    }

    // Get the latest version and basic info across all repos for this distro
    let placeholders = repo_ids_placeholders(repo_ids.len());
    let sql = format!(
        "SELECT rp.name, rp.version, rp.description, rp.size, rp.dependencies,
                rp.architecture
         FROM repository_packages rp
         WHERE rp.repository_id IN ({placeholders}) AND rp.name = ?{}
         ORDER BY rp.synced_at DESC
         LIMIT 1",
        repo_ids.len() + 1
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    params.push(Box::new(name.to_string()));

    let latest = conn.query_row(&sql, rusqlite::params_from_iter(&params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    });

    let (pkg_name, latest_version, description, size, deps_str, _arch) = match latest {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    // Parse dependencies (stored as JSON array in DB)
    let dependencies = parse_dependencies(deps_str.as_deref())?;

    // Get all versions
    let versions = query_versions_internal(&conn, distro, name)?;

    // Check if converted
    let converted = conn
        .query_row(
            "SELECT COUNT(*) FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2",
            rusqlite::params![distro, name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    // Get download counts
    let (download_count, download_count_30d) =
        match DownloadCount::find_by_package(&conn, distro, name)? {
            Some(dc) => (dc.total_count, dc.count_30d),
            None => (0, 0),
        };

    // Extract license and homepage from metadata JSON if available
    let (license, homepage) = extract_metadata(&conn, distro, name)?;

    Ok(Some(PackageDetail {
        name: pkg_name,
        distro: distro.to_string(),
        latest_version,
        description,
        versions,
        dependencies,
        download_count,
        download_count_30d,
        size_bytes: size,
        license,
        homepage,
        converted,
    }))
}

fn query_versions(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Vec<VersionSummary>> {
    let conn = conary_core::db::open(db_path)?;
    query_versions_internal(&conn, distro, name)
}

fn query_versions_internal(
    conn: &Connection,
    distro: &str,
    name: &str,
) -> anyhow::Result<Vec<VersionSummary>> {
    let repo_ids = resolve_all_repo_ids(conn, distro)?;
    if repo_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = repo_ids_placeholders(repo_ids.len());
    let distro_idx = repo_ids.len() + 1;
    let name_idx = repo_ids.len() + 2;
    let sql = format!(
        "SELECT rp.version, rp.architecture, rp.size,
                CASE WHEN cp.id IS NOT NULL THEN 1 ELSE 0 END as is_converted
         FROM repository_packages rp
         LEFT JOIN converted_packages cp
             ON cp.package_name = rp.name AND cp.distro = ?{distro_idx}
                AND cp.package_version = rp.version
         WHERE rp.repository_id IN ({placeholders}) AND rp.name = ?{name_idx}
         ORDER BY rp.synced_at DESC"
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    params.push(Box::new(distro.to_string()));
    params.push(Box::new(name.to_string()));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
        Ok(VersionSummary {
            version: row.get(0)?,
            architecture: row.get(1)?,
            size: row.get(2)?,
            converted: row.get::<_, i64>(3).map(|c| c != 0)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn query_dependencies(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Vec<String>> {
    let conn = conary_core::db::open(db_path)?;

    let repo_ids = resolve_all_repo_ids(&conn, distro)?;
    if repo_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = repo_ids_placeholders(repo_ids.len());
    let name_idx = repo_ids.len() + 1;
    let sql = format!(
        "SELECT rp.dependencies
         FROM repository_packages rp
         WHERE rp.repository_id IN ({placeholders}) AND rp.name = ?{name_idx}
         ORDER BY rp.synced_at DESC
         LIMIT 1"
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    params.push(Box::new(name.to_string()));

    let deps_str: Option<String> = conn
        .query_row(&sql, rusqlite::params_from_iter(&params), |row| row.get(0))
        .ok();

    parse_dependencies(deps_str.as_deref())
}

fn query_reverse_dependencies(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Vec<String>> {
    let conn = conary_core::db::open(db_path)?;

    let repo_ids = resolve_all_repo_ids(&conn, distro)?;
    if repo_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = repo_ids_placeholders(repo_ids.len());
    let cap_idx = repo_ids.len() + 1;
    let name_idx = repo_ids.len() + 2;
    let sql = format!(
        "SELECT DISTINCT rp.name
         FROM repository_packages rp
         JOIN repository_requirements rr ON rr.repository_package_id = rp.id
         WHERE rr.capability = ?{cap_idx}
           AND rp.repository_id IN ({placeholders})
           AND rp.name != ?{name_idx}
         ORDER BY rp.name"
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    params.push(Box::new(name.to_string()));
    params.push(Box::new(name.to_string()));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
        row.get::<_, String>(0)
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn query_popular(
    db_path: &std::path::Path,
    distro: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<PackageSummary>> {
    let conn = conary_core::db::open(db_path)?;

    if let Some(distro) = distro {
        let counts = DownloadCount::popular(&conn, distro, limit)?;
        let mut results = Vec::with_capacity(counts.len());
        for count in counts {
            let summary = enrich_package_summary(
                &conn,
                &count.distro,
                &count.package_name,
                count.total_count,
            )?;
            if let Some(s) = summary {
                results.push(s);
            }
        }
        Ok(results)
    } else {
        // All distros - query download_counts directly
        let mut stmt = conn.prepare(
            "SELECT distro, package_name, total_count
             FROM download_counts
             ORDER BY total_count DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows.flatten() {
            let (distro, name, count) = row;
            if let Some(s) = enrich_package_summary(&conn, &distro, &name, count)? {
                results.push(s);
            }
        }
        Ok(results)
    }
}

fn query_recent(
    db_path: &std::path::Path,
    distro: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<PackageSummary>> {
    let conn = conary_core::db::open(db_path)?;

    if let Some(distro) = distro {
        let repo_ids = resolve_all_repo_ids(&conn, distro)?;
        if repo_ids.is_empty() {
            return Ok(Vec::new());
        }

        let n = repo_ids.len();
        let ph = repo_ids_placeholders(n);
        let distro_idx = n + 1;
        let limit_idx = n + 2;

        // Pick one row per package name: the one with the newest synced_at
        // (highest id as tiebreaker for same-second syncs). The correlated
        // subquery reuses the same ?1..?n placeholders for repo filtering.
        let sql = format!(
            "SELECT rp.name, rp.version, rp.description, rp.size,
                    COALESCE(dc.total_count, 0) as downloads
             FROM repository_packages rp
             LEFT JOIN download_counts dc ON dc.distro = ?{distro_idx} AND dc.package_name = rp.name
             WHERE rp.repository_id IN ({ph})
               AND rp.id = (
                   SELECT rp2.id FROM repository_packages rp2
                   WHERE rp2.name = rp.name
                     AND rp2.repository_id IN ({ph})
                   ORDER BY rp2.synced_at DESC, rp2.id DESC
                   LIMIT 1
               )
             ORDER BY rp.synced_at DESC
             LIMIT ?{limit_idx}",
        );
        let mut stmt = conn.prepare(&sql)?;

        // Params: repo_ids + distro + limit
        // SQLite reuses the same positional params for both IN clauses
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for id in &repo_ids {
            params_vec.push(Box::new(*id));
        }
        params_vec.push(Box::new(distro.to_string()));
        params_vec.push(Box::new(limit as i64));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(PackageSummary {
                name: row.get(0)?,
                distro: distro.to_string(),
                version: row.get(1)?,
                description: row.get(2)?,
                download_count: row.get(4)?,
                size: row.get(3)?,
            })
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    } else {
        let mut stmt = conn.prepare(
            "SELECT rp.name, r.name, rp.version, rp.description, rp.size,
                    COALESCE(dc.total_count, 0) as downloads
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             LEFT JOIN download_counts dc ON dc.distro = r.name AND dc.package_name = rp.name
             WHERE r.enabled = 1
             ORDER BY rp.synced_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
            Ok(PackageSummary {
                name: row.get(0)?,
                distro: row.get(1)?,
                version: row.get(2)?,
                description: row.get(3)?,
                download_count: row.get(5)?,
                size: row.get(4)?,
            })
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

fn query_overview(db_path: &std::path::Path) -> anyhow::Result<OverviewStats> {
    let conn = conary_core::db::open(db_path)?;

    // Total packages across all repos
    let total_packages: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT rp.name)
         FROM repository_packages rp
         JOIN repositories r ON rp.repository_id = r.id
         WHERE r.enabled = 1",
        [],
        |row| row.get(0),
    )?;

    // Total converted packages
    let total_converted: i64 = conn.query_row(
        "SELECT COUNT(*) FROM converted_packages WHERE distro IS NOT NULL",
        [],
        |row| row.get(0),
    )?;

    // Count distinct distro families from enabled repositories.
    // Multiple repos can serve the same distro (e.g., arch-core, arch-extra,
    // arch-multilib are all "Arch Linux"). Normalize by grouping related
    // repos into families. The remi repo is Conary's native CCS format,
    // not a distro — exclude it from the distro count.
    let total_distros: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT
            CASE
                WHEN name = 'remi' THEN NULL
                WHEN name LIKE 'arch-%' THEN 'arch'
                WHEN name LIKE 'fedora-%' THEN 'fedora'
                WHEN name LIKE 'ubuntu-%' THEN 'ubuntu'
                ELSE name
            END
         ) FROM repositories WHERE enabled = 1 AND name != 'remi'",
        [],
        |row| row.get(0),
    )?;

    // Download stats from aggregated table
    let download_stats = DownloadCount::global_stats(&conn)?;

    Ok(OverviewStats {
        total_packages,
        total_downloads: download_stats.total_downloads,
        downloads_30d: download_stats.downloads_30d,
        total_distros,
        total_converted,
    })
}

/// Enrich a download count entry with package metadata
fn enrich_package_summary(
    conn: &Connection,
    distro: &str,
    name: &str,
    download_count: i64,
) -> anyhow::Result<Option<PackageSummary>> {
    let repo_ids = resolve_all_repo_ids(conn, distro)?;
    if repo_ids.is_empty() {
        return Ok(None);
    }

    let placeholders = repo_ids_placeholders(repo_ids.len());
    let sql = format!(
        "SELECT rp.version, rp.description, rp.size
         FROM repository_packages rp
         WHERE rp.repository_id IN ({placeholders}) AND rp.name = ?{}
         ORDER BY rp.synced_at DESC
         LIMIT 1",
        repo_ids.len() + 1,
    );
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = repo_ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    params_vec.push(Box::new(name.to_string()));
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();

    let result = conn.query_row(&sql, param_refs.as_slice(), |row| {
        Ok(PackageSummary {
            name: name.to_string(),
            distro: distro.to_string(),
            version: row.get(0)?,
            description: row.get(1)?,
            download_count,
            size: row.get(2)?,
        })
    });

    match result {
        Ok(summary) => Ok(Some(summary)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Extract license and homepage from the JSON metadata column
fn extract_metadata(
    conn: &Connection,
    distro: &str,
    name: &str,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let repo_ids = resolve_all_repo_ids(conn, distro)?;
    if repo_ids.is_empty() {
        return Ok((None, None));
    }

    let placeholders = repo_ids_placeholders(repo_ids.len());
    let name_idx = repo_ids.len() + 1;
    let sql = format!(
        "SELECT rp.metadata
         FROM repository_packages rp
         WHERE rp.repository_id IN ({placeholders}) AND rp.name = ?{name_idx}
         ORDER BY rp.synced_at DESC
         LIMIT 1"
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    params.push(Box::new(name.to_string()));

    let metadata_json: Option<String> = conn
        .query_row(&sql, rusqlite::params_from_iter(&params), |row| row.get(0))
        .ok();

    if let Some(json_str) = metadata_json
        && let Ok(meta) = serde_json::from_str::<serde_json::Value>(&json_str)
    {
        let license = meta
            .get("license")
            .and_then(|v| v.as_str())
            .map(String::from);
        let homepage = meta
            .get("homepage")
            .or_else(|| meta.get("url"))
            .and_then(|v| v.as_str())
            .map(String::from);
        return Ok((license, homepage));
    }

    Ok((None, None))
}

/// Parse dependencies from the DB storage format (JSON array string).
///
/// Falls back to comma-separated parsing for legacy data.
fn parse_dependencies(deps_str: Option<&str>) -> anyhow::Result<Vec<String>> {
    let Some(s) = deps_str else {
        return Ok(Vec::new());
    };

    if s.is_empty() {
        return Ok(Vec::new());
    }

    // Try JSON array first (current format)
    if let Ok(deps) = serde_json::from_str::<Vec<String>>(s) {
        return Ok(deps);
    }

    // Fallback: comma-separated (legacy)
    Ok(s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

/// Resolve all repository IDs for a distro (e.g. arch-core + arch-extra).
fn resolve_all_repo_ids(conn: &Connection, distro: &str) -> anyhow::Result<Vec<i64>> {
    let repos = super::find_repositories_for_distro(conn, distro)?;
    Ok(repos.into_iter().filter_map(|r| r.id).collect())
}

/// Build a comma-separated `?1, ?2, ...` placeholder string for N parameters.
fn repo_ids_placeholders(count: usize) -> String {
    (1..=count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}
