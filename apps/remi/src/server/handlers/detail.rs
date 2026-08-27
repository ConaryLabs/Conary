// apps/remi/src/server/handlers/detail.rs
//! Package detail API for the Remi package index
//!
//! Provides rich per-package endpoints for the web frontend, including
//! package metadata, version history, dependency graphs, and statistics.
//! All database queries run via `spawn_blocking` for async compatibility.

use crate::server::ServerState;
use crate::server::catalog_authority::{CatalogAuthority, ProfileRevisionSelection};
use crate::server::profile_catalog::ProfileCatalog;
use crate::server::public_universe::PublicUniverseSnapshot;
use anyhow::Context;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::db::models::{ConvertedPackage, DownloadCount};
use conary_core::repository::remi_metadata::RemiRequirementGroup;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::public_read;
use super::{HandlerResult, open_handler_db};

mod catalog;

use catalog::{
    extract_catalog_metadata, latest_catalog_package, pin_response_catalog,
    route_for_source_profile, source_profile_for_route,
};

/// Full package detail response
#[derive(Debug, Serialize)]
pub struct PackageDetail {
    pub name: String,
    pub distro: String,
    pub latest_version: String,
    pub description: Option<String>,
    pub versions: Vec<VersionSummary>,
    pub requirement_groups: Vec<RemiRequirementGroup>,
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
) -> HandlerResult<Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let context = public_read::context(&state).await?;
    let selection = context.profile_for_route(&distro)?;
    let db_path = context.db_path.clone();
    let catalog_authority = context.catalog_authority.clone();
    let query_distro = distro.clone();
    let query_name = name.clone();
    let query_selection = selection.clone();
    let detail = public_read::run("package detail", move || {
        query_package_detail(
            &catalog_authority,
            &db_path,
            &query_distro,
            &query_name,
            &query_selection,
        )
    })
    .await?;

    match detail {
        Some(detail) => Ok(public_read::stamp(
            (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "public, max-age=300")],
                Json(detail),
            )
                .into_response(),
            &context.universe,
            Some(&selection),
        )),
        None => Ok(public_read::stamp(
            (StatusCode::NOT_FOUND, "Package not found").into_response(),
            &context.universe,
            Some(&selection),
        )),
    }
}

/// GET /v1/packages/:distro/:name/versions
///
/// List all available versions for a package.
pub async fn get_versions(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> HandlerResult<Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let context = public_read::context(&state).await?;
    let selection = context.profile_for_route(&distro)?;
    let db_path = context.db_path.clone();
    let catalog_authority = context.catalog_authority.clone();
    let query_distro = distro.clone();
    let query_name = name.clone();
    let query_selection = selection.clone();
    let versions = public_read::run("versions", move || {
        query_versions(
            &catalog_authority,
            &db_path,
            &query_distro,
            &query_name,
            &query_selection,
        )
    })
    .await?;

    Ok(public_read::stamp(
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(versions),
        )
            .into_response(),
        &context.universe,
        Some(&selection),
    ))
}

/// GET /v1/packages/:distro/:name/dependencies
///
/// List dependencies for a package.
pub async fn get_dependencies(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> HandlerResult<Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let context = public_read::context(&state).await?;
    let selection = context.profile_for_route(&distro)?;
    let catalog_authority = context.catalog_authority.clone();
    let query_distro = distro.clone();
    let query_name = name.clone();
    let query_selection = selection.clone();
    let deps = public_read::run("dependencies", move || {
        query_dependencies(
            &catalog_authority,
            &query_distro,
            &query_name,
            &query_selection,
        )
    })
    .await?;

    Ok(public_read::stamp(
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(deps),
        )
            .into_response(),
        &context.universe,
        Some(&selection),
    ))
}

/// GET /v1/packages/:distro/:name/rdepends
///
/// List packages that depend on this package (reverse dependencies).
pub async fn get_reverse_dependencies(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> HandlerResult<Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let context = public_read::context(&state).await?;
    let selection = context.profile_for_route(&distro)?;
    let catalog_authority = context.catalog_authority.clone();
    let query_distro = distro.clone();
    let query_name = name.clone();
    let query_selection = selection.clone();
    let rdeps = public_read::run("reverse dependencies", move || {
        query_reverse_dependencies(
            &catalog_authority,
            &query_distro,
            &query_name,
            &query_selection,
        )
    })
    .await?;

    Ok(public_read::stamp(
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(rdeps),
        )
            .into_response(),
        &context.universe,
        Some(&selection),
    ))
}

/// GET /v1/stats/popular?distro=fedora&limit=50
///
/// Most popular packages by download count.
pub async fn get_popular(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<StatsQuery>,
) -> HandlerResult<Response> {
    let context = public_read::context(&state).await?;
    let limit = params.limit.unwrap_or(50).min(200);
    let distro = params.distro;
    if let Some(distro) = distro.as_deref() {
        super::validate_supported_distro_route(distro)?;
    }
    let profile = distro
        .as_deref()
        .map(|distro| context.profile_for_route(distro))
        .transpose()?;

    let db_path = context.db_path.clone();
    let catalog_authority = context.catalog_authority.clone();
    let universe = context.universe.clone();
    let packages = public_read::run("popular", move || {
        query_popular(
            &catalog_authority,
            &db_path,
            &universe,
            distro.as_deref(),
            limit,
        )
    })
    .await?;

    Ok(public_read::stamp(
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(packages),
        )
            .into_response(),
        &context.universe,
        profile.as_ref(),
    ))
}

/// GET /v1/stats/recent?distro=fedora&limit=50
///
/// Recently downloaded packages, enriched from the active immutable catalog.
pub async fn get_recent(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<StatsQuery>,
) -> HandlerResult<Response> {
    let context = public_read::context(&state).await?;
    let limit = params.limit.unwrap_or(50).min(200);
    let distro = params.distro;
    if let Some(distro) = distro.as_deref() {
        super::validate_supported_distro_route(distro)?;
    }
    let profile = distro
        .as_deref()
        .map(|distro| context.profile_for_route(distro))
        .transpose()?;

    let db_path = context.db_path.clone();
    let catalog_authority = context.catalog_authority.clone();
    let universe = context.universe.clone();
    let packages = public_read::run("recent", move || {
        query_recent(
            &catalog_authority,
            &db_path,
            &universe,
            distro.as_deref(),
            limit,
        )
    })
    .await?;

    Ok(public_read::stamp(
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(packages),
        )
            .into_response(),
        &context.universe,
        profile.as_ref(),
    ))
}

/// GET /v1/stats/overview
///
/// Global statistics: total packages, downloads, distros, conversions.
pub async fn get_overview(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> HandlerResult<Response> {
    let context = public_read::context(&state).await?;

    let db_path = context.db_path.clone();
    let catalog_authority = context.catalog_authority.clone();
    let universe = context.universe.clone();
    let stats = public_read::run("overview", move || {
        query_overview(&catalog_authority, &db_path, &universe)
    })
    .await?;

    Ok(public_read::stamp(
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=60")],
            Json(stats),
        )
            .into_response(),
        &context.universe,
        None,
    ))
}

// --- Database query functions (run on blocking threads) ---

fn query_package_detail(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    selection: &ProfileRevisionSelection,
) -> anyhow::Result<Option<PackageDetail>> {
    let conn = open_handler_db(db_path)?;
    let source_profile = source_profile_for_route(distro)?;
    anyhow::ensure!(selection.source_profile == source_profile);

    // Package identity, version ordering, payload metadata, and dependency
    // groups all come from this one immutable reader. Operational SQLite is
    // used below only for analytics and conversion rows.
    let pinned = catalog_authority.open_selected_profile(selection)?;
    let catalog = ProfileCatalog::new(&pinned);
    let packages = catalog.find_package_records_by_name(name)?;
    let latest = latest_catalog_package(&packages)?;
    let Some(latest) = latest else {
        return Ok(None);
    };

    let projected = catalog.project_package(latest)?;
    let versions = version_summaries(&conn, &catalog, name)?;

    let converted = versions.iter().any(|version| version.converted);

    // Get download counts
    let (download_count, download_count_30d) =
        match DownloadCount::find_by_package(&conn, source_profile, name)? {
            Some(dc) => (dc.total_count, dc.count_30d),
            None => (0, 0),
        };

    // Extract license and homepage from metadata JSON if available
    let (license, homepage) = extract_catalog_metadata(latest)?;
    let size = i64::try_from(latest.size).with_context(|| {
        format!(
            "immutable catalog package '{}' version '{}' size {} exceeds Remi wire range",
            latest.name, latest.version, latest.size
        )
    })?;

    Ok(Some(PackageDetail {
        name: latest.name.clone(),
        distro: distro.to_string(),
        latest_version: latest.version.clone(),
        description: latest.description.clone(),
        versions,
        requirement_groups: projected.requirement_groups,
        download_count,
        download_count_30d,
        size_bytes: size,
        license,
        homepage,
        converted,
    }))
}

fn query_versions(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    selection: &ProfileRevisionSelection,
) -> anyhow::Result<Vec<VersionSummary>> {
    let conn = open_handler_db(db_path)?;
    let source_profile = source_profile_for_route(distro)?;
    anyhow::ensure!(selection.source_profile == source_profile);
    let pinned = catalog_authority.open_selected_profile(selection)?;
    let catalog = ProfileCatalog::new(&pinned);
    version_summaries(&conn, &catalog, name)
}

fn version_summaries(
    conn: &Connection,
    catalog: &ProfileCatalog<'_>,
    name: &str,
) -> anyhow::Result<Vec<VersionSummary>> {
    let packages = catalog.find_package_records_by_name(name)?;
    let converted_keys = current_converted_keys(
        conn,
        catalog.source_profile(),
        catalog.profile_revision_sha256(),
        name,
    )?;
    packages
        .into_iter()
        .map(|package| {
            let size = i64::try_from(package.size).with_context(|| {
                format!(
                    "immutable catalog package '{}' version '{}' size {} exceeds Remi wire range",
                    package.name, package.version, package.size
                )
            })?;
            Ok(VersionSummary {
                converted: converted_keys
                    .contains(&(package.version.clone(), package.architecture.clone())),
                version: package.version,
                architecture: package.architecture,
                size,
            })
        })
        .collect()
}

fn query_dependencies(
    catalog_authority: &CatalogAuthority,
    distro: &str,
    name: &str,
    selection: &ProfileRevisionSelection,
) -> anyhow::Result<Vec<RemiRequirementGroup>> {
    let source_profile = source_profile_for_route(distro)?;
    anyhow::ensure!(selection.source_profile == source_profile);
    let pinned = catalog_authority.open_selected_profile(selection)?;
    let catalog = ProfileCatalog::new(&pinned);
    let packages = catalog.find_package_records_by_name(name)?;
    let Some(latest) = latest_catalog_package(&packages)? else {
        return Ok(Vec::new());
    };
    Ok(catalog.project_package(latest)?.requirement_groups)
}

fn query_reverse_dependencies(
    catalog_authority: &CatalogAuthority,
    distro: &str,
    name: &str,
    selection: &ProfileRevisionSelection,
) -> anyhow::Result<Vec<String>> {
    let source_profile = source_profile_for_route(distro)?;
    anyhow::ensure!(selection.source_profile == source_profile);
    let pinned = catalog_authority.open_selected_profile(selection)?;
    let packages = ProfileCatalog::new(&pinned).package_records()?;
    let mut names = HashSet::new();
    for package in packages {
        if package.name == name {
            continue;
        }
        if package
            .requirement_groups
            .iter()
            .flat_map(|group| group.atoms.iter())
            .any(|atom| atom.capability == name)
        {
            names.insert(package.name);
        }
    }
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn query_popular(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    universe: &PublicUniverseSnapshot,
    distro: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<PackageSummary>> {
    let conn = open_handler_db(db_path)?;

    if let Some(distro) = distro {
        let source_profile = source_profile_for_route(distro)?;
        let selection = universe.profile(source_profile).with_context(|| {
            format!("profile '{source_profile}' is absent from the selected public universe")
        })?;
        let counts = DownloadCount::popular(&conn, source_profile, limit)?;
        let mut pinned_profiles = HashMap::new();
        let mut results = Vec::with_capacity(counts.len());
        for count in counts {
            let summary = {
                let pinned =
                    pin_response_catalog(catalog_authority, &mut pinned_profiles, selection)?;
                let catalog = ProfileCatalog::new(pinned);
                enrich_package_summary(&catalog, &count.package_name, count.total_count)?
            };
            if let Some(s) = summary {
                results.push(s);
            }
        }
        Ok(results)
    } else {
        let source_profiles = universe
            .profiles()
            .map(|selection| selection.source_profile.clone())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            !source_profiles.is_empty(),
            "public universe has no profiles"
        );
        let placeholders = std::iter::repeat_n("?", source_profiles.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT source_profile, package_name, total_count
             FROM download_counts
             WHERE source_profile IN ({placeholders})
             ORDER BY total_count DESC
             LIMIT ?"
        );
        let mut parameters = source_profiles
            .into_iter()
            .map(rusqlite::types::Value::Text)
            .collect::<Vec<_>>();
        parameters.push(rusqlite::types::Value::Integer(i64::try_from(limit)?));
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt
            .query_map(rusqlite::params_from_iter(parameters), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut results = Vec::new();
        let mut pinned_profiles = HashMap::new();
        for (source_profile, name, count) in rows {
            let Some(selection) = universe.profile(&source_profile) else {
                continue;
            };
            let summary = {
                let pinned =
                    pin_response_catalog(catalog_authority, &mut pinned_profiles, selection)?;
                let catalog = ProfileCatalog::new(pinned);
                enrich_package_summary(&catalog, &name, count)?
            };
            if let Some(s) = summary {
                results.push(s);
            }
        }
        Ok(results)
    }
}

fn query_recent(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    universe: &PublicUniverseSnapshot,
    distro: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<PackageSummary>> {
    let conn = open_handler_db(db_path)?;

    if let Some(distro) = distro {
        let source_profile = source_profile_for_route(distro)?;
        let selection = universe.profile(source_profile).with_context(|| {
            format!("profile '{source_profile}' is absent from the selected public universe")
        })?;
        let mut stmt = conn.prepare(
            "SELECT package_name
             FROM download_stats
             WHERE source_profile = ?1
             GROUP BY package_name
             ORDER BY MAX(downloaded_at) DESC, package_name
             LIMIT ?2",
        )?;
        let names = stmt
            .query_map(rusqlite::params![source_profile, limit as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut summaries = Vec::with_capacity(names.len());
        let mut pinned_profiles = HashMap::new();
        for name in names {
            let download_count = DownloadCount::find_by_package(&conn, source_profile, &name)?
                .map_or(0, |count| count.total_count);
            let summary = {
                let pinned =
                    pin_response_catalog(catalog_authority, &mut pinned_profiles, selection)?;
                let catalog = ProfileCatalog::new(pinned);
                enrich_package_summary(&catalog, &name, download_count)?
            };
            if let Some(summary) = summary {
                summaries.push(summary);
            }
        }
        Ok(summaries)
    } else {
        let source_profiles = universe
            .profiles()
            .map(|selection| selection.source_profile.clone())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            !source_profiles.is_empty(),
            "public universe has no profiles"
        );
        let placeholders = std::iter::repeat_n("?", source_profiles.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT package_name, source_profile
             FROM download_stats
             WHERE source_profile IN ({placeholders})
             GROUP BY package_name, source_profile
             ORDER BY MAX(downloaded_at) DESC, source_profile, package_name
             LIMIT ?"
        );
        let mut parameters = source_profiles
            .into_iter()
            .map(rusqlite::types::Value::Text)
            .collect::<Vec<_>>();
        parameters.push(rusqlite::types::Value::Integer(i64::try_from(limit)?));
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt
            .query_map(rusqlite::params_from_iter(parameters), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut summaries = Vec::new();
        let mut pinned_profiles = HashMap::new();
        for (name, source_profile) in rows {
            let Some(selection) = universe.profile(&source_profile) else {
                continue;
            };
            let download_count = DownloadCount::find_by_package(&conn, &source_profile, &name)?
                .map_or(0, |count| count.total_count);
            let summary = {
                let pinned =
                    pin_response_catalog(catalog_authority, &mut pinned_profiles, selection)?;
                let catalog = ProfileCatalog::new(pinned);
                enrich_package_summary(&catalog, &name, download_count)?
            };
            if let Some(summary) = summary {
                summaries.push(summary);
            }
        }
        Ok(summaries)
    }
}

fn query_overview(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    universe: &PublicUniverseSnapshot,
) -> anyhow::Result<OverviewStats> {
    let conn = open_handler_db(db_path)?;

    // Package counts and conversion identities come only from the exact
    // revisions in this one signed universe snapshot.
    let mut total_packages = 0_i64;
    let mut total_converted = 0_i64;
    for selection in universe.profiles() {
        let pinned = catalog_authority.open_selected_profile(selection)?;
        let packages = ProfileCatalog::new(&pinned).package_records()?;
        let package_keys = packages
            .iter()
            .map(|package| {
                (
                    package.name.clone(),
                    package.version.clone(),
                    package.architecture.clone(),
                )
            })
            .collect::<HashSet<_>>();
        total_packages += i64::try_from(
            packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<HashSet<_>>()
                .len(),
        )?;
        for converted in ConvertedPackage::find_current_conversions(
            &conn,
            pinned.profile_revision_sha256(),
            None,
        )? {
            let converted_id = converted.id.ok_or_else(|| {
                anyhow::anyhow!(
                    "current repository conversion for profile '{}' has no database id",
                    selection.source_profile
                )
            })?;
            ConvertedPackage::require_conversion_pin(&conn, converted_id)?;
            converted.scriptlet_summary()?;
            let artifact = converted.repository_artifact()?;
            if artifact.source_profile == selection.source_profile
                && package_keys.contains(&(
                    artifact.package_name.to_string(),
                    artifact.package_version.to_string(),
                    Some(artifact.package_architecture.to_string()),
                ))
            {
                total_converted += 1;
            }
        }
    }
    let total_distros = i64::try_from(universe.profiles().len())?;

    let mut total_downloads = 0_i64;
    let mut downloads_30d = 0_i64;
    for selection in universe.profiles() {
        let (profile_total, profile_30d) = conn.query_row(
            "SELECT COALESCE(SUM(total_count), 0), COALESCE(SUM(count_30d), 0)
             FROM download_counts
             WHERE source_profile = ?1",
            [&selection.source_profile],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        total_downloads = total_downloads
            .checked_add(profile_total)
            .context("public universe total download count overflow")?;
        downloads_30d = downloads_30d
            .checked_add(profile_30d)
            .context("public universe 30-day download count overflow")?;
    }

    Ok(OverviewStats {
        total_packages,
        total_downloads,
        downloads_30d,
        total_distros,
        total_converted,
    })
}

/// Enrich a download count entry with package metadata
fn enrich_package_summary(
    catalog: &ProfileCatalog<'_>,
    name: &str,
    download_count: i64,
) -> anyhow::Result<Option<PackageSummary>> {
    let route = route_for_source_profile(catalog.source_profile())?.to_string();
    let packages = catalog.find_package_records_by_name(name)?;
    let Some(package) = latest_catalog_package(&packages)? else {
        return Ok(None);
    };
    let size = i64::try_from(package.size).with_context(|| {
        format!(
            "immutable catalog package '{}' version '{}' size {} exceeds Remi wire range",
            package.name, package.version, package.size
        )
    })?;
    Ok(Some(PackageSummary {
        name: package.name.clone(),
        distro: route,
        version: package.version.clone(),
        description: package.description.clone(),
        download_count,
        size,
    }))
}

type ConvertedVersionKey = (String, Option<String>);

fn current_converted_keys(
    conn: &Connection,
    source_profile: &str,
    profile_revision_sha256: &str,
    name: &str,
) -> anyhow::Result<HashSet<ConvertedVersionKey>> {
    let mut keys = HashSet::new();
    for converted in
        ConvertedPackage::find_current_conversions(conn, profile_revision_sha256, Some(name))?
    {
        let converted_id = converted.id.ok_or_else(|| {
            anyhow::anyhow!(
                "current repository conversion for profile '{source_profile}' has no database id"
            )
        })?;
        ConvertedPackage::require_conversion_pin(conn, converted_id)?;
        converted.scriptlet_summary()?;
        let artifact = converted.repository_artifact()?;
        if artifact.source_profile != source_profile {
            continue;
        }
        keys.insert((
            artifact.package_version.to_string(),
            Some(artifact.package_architecture.to_string()),
        ));
    }
    Ok(keys)
}

#[cfg(test)]
mod tests;
