// apps/remi/src/server/handlers/detail.rs
//! Package detail API for the Remi package index
//!
//! Provides rich per-package endpoints for the web frontend, including
//! package metadata, version history, dependency graphs, and statistics.
//! All database queries run via `spawn_blocking` for async compatibility.

use crate::server::ServerState;
use crate::server::catalog_authority::CatalogAuthority;
use crate::server::profile_catalog::ProfileCatalog;
use anyhow::Context;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::db::models::{ConvertedPackage, DownloadCount, RemiActiveProfileRevision};
use conary_core::repository::remi_metadata::RemiRequirementGroup;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{HandlerResult, open_handler_db, run_blocking};

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

    let state = state.read().await;
    let db_path = state.config.db_path.clone();
    let catalog_authority = state.catalog_authority.clone();
    drop(state);
    let detail = run_blocking("package detail", move || {
        query_package_detail(&catalog_authority, &db_path, &distro, &name)
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
) -> HandlerResult<Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let state = state.read().await;
    let db_path = state.config.db_path.clone();
    let catalog_authority = state.catalog_authority.clone();
    drop(state);
    let versions = run_blocking("versions", move || {
        query_versions(&catalog_authority, &db_path, &distro, &name)
    })
    .await?;

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
) -> HandlerResult<Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let state = state.read().await;
    let db_path = state.config.db_path.clone();
    let catalog_authority = state.catalog_authority.clone();
    drop(state);
    let deps = run_blocking("dependencies", move || {
        query_dependencies(&catalog_authority, &db_path, &distro, &name)
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
) -> HandlerResult<Response> {
    super::validate_distro_and_name(&distro, &name)?;

    let state = state.read().await;
    let db_path = state.config.db_path.clone();
    let catalog_authority = state.catalog_authority.clone();
    drop(state);
    let rdeps = run_blocking("reverse dependencies", move || {
        query_reverse_dependencies(&catalog_authority, &db_path, &distro, &name)
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
) -> HandlerResult<Response> {
    let state = state.read().await;
    let db_path = state.config.db_path.clone();
    let catalog_authority = state.catalog_authority.clone();
    drop(state);
    let limit = params.limit.unwrap_or(50).min(200);
    let distro = params.distro;

    let packages = run_blocking("popular", move || {
        query_popular(&catalog_authority, &db_path, distro.as_deref(), limit)
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
/// Recently downloaded packages, enriched from the active immutable catalog.
pub async fn get_recent(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<StatsQuery>,
) -> HandlerResult<Response> {
    let state = state.read().await;
    let db_path = state.config.db_path.clone();
    let catalog_authority = state.catalog_authority.clone();
    drop(state);
    let limit = params.limit.unwrap_or(50).min(200);
    let distro = params.distro;

    let packages = run_blocking("recent", move || {
        query_recent(&catalog_authority, &db_path, distro.as_deref(), limit)
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
) -> HandlerResult<Response> {
    let state = state.read().await;
    let db_path = state.config.db_path.clone();
    let catalog_authority = state.catalog_authority.clone();
    drop(state);

    let stats = run_blocking("overview", move || {
        query_overview(&catalog_authority, &db_path)
    })
    .await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=60")],
        Json(stats),
    )
        .into_response())
}

// --- Database query functions (run on blocking threads) ---

fn query_package_detail(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Option<PackageDetail>> {
    let conn = open_handler_db(db_path)?;
    let source_profile = source_profile_for_route(distro)?;

    // Package identity, version ordering, payload metadata, and dependency
    // groups all come from this one immutable reader. Operational SQLite is
    // used below only for analytics and conversion rows.
    let pinned = catalog_authority.open_active_profile(source_profile)?;
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
) -> anyhow::Result<Vec<VersionSummary>> {
    let conn = open_handler_db(db_path)?;
    let source_profile = source_profile_for_route(distro)?;
    let pinned = catalog_authority.open_active_profile(source_profile)?;
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
    _db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Vec<RemiRequirementGroup>> {
    let source_profile = source_profile_for_route(distro)?;
    let pinned = catalog_authority.open_active_profile(source_profile)?;
    let catalog = ProfileCatalog::new(&pinned);
    let packages = catalog.find_package_records_by_name(name)?;
    let Some(latest) = latest_catalog_package(&packages)? else {
        return Ok(Vec::new());
    };
    Ok(catalog.project_package(latest)?.requirement_groups)
}

fn query_reverse_dependencies(
    catalog_authority: &CatalogAuthority,
    _db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Vec<String>> {
    let source_profile = source_profile_for_route(distro)?;
    let pinned = catalog_authority.open_active_profile(source_profile)?;
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
    distro: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<PackageSummary>> {
    let conn = open_handler_db(db_path)?;

    if let Some(distro) = distro {
        let source_profile = source_profile_for_route(distro)?;
        let counts = DownloadCount::popular(&conn, source_profile, limit)?;
        let mut pinned_profiles = HashMap::new();
        let mut results = Vec::with_capacity(counts.len());
        for count in counts {
            let summary = {
                let pinned = pin_response_catalog(
                    catalog_authority,
                    &mut pinned_profiles,
                    &count.source_profile,
                )?;
                let catalog = ProfileCatalog::new(pinned);
                enrich_package_summary(&catalog, &count.package_name, count.total_count)?
            };
            if let Some(s) = summary {
                results.push(s);
            }
        }
        Ok(results)
    } else {
        // All distros - query download_counts directly
        let mut stmt = conn.prepare(
            "SELECT source_profile, package_name, total_count
             FROM download_counts
             ORDER BY total_count DESC
             LIMIT ?1",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
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
            let summary = {
                let pinned =
                    pin_response_catalog(catalog_authority, &mut pinned_profiles, &source_profile)?;
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
    distro: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<PackageSummary>> {
    let conn = open_handler_db(db_path)?;

    if let Some(distro) = distro {
        let source_profile = source_profile_for_route(distro)?;
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
                    pin_response_catalog(catalog_authority, &mut pinned_profiles, source_profile)?;
                let catalog = ProfileCatalog::new(pinned);
                enrich_package_summary(&catalog, &name, download_count)?
            };
            if let Some(summary) = summary {
                summaries.push(summary);
            }
        }
        Ok(summaries)
    } else {
        let mut stmt = conn.prepare(
            "SELECT package_name, source_profile
             FROM download_stats
             GROUP BY package_name, source_profile
             ORDER BY MAX(downloaded_at) DESC, source_profile, package_name
             LIMIT ?1",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut summaries = Vec::new();
        let mut pinned_profiles = HashMap::new();
        for (name, source_profile) in rows {
            let download_count = DownloadCount::find_by_package(&conn, &source_profile, &name)?
                .map_or(0, |count| count.total_count);
            let summary = {
                let pinned =
                    pin_response_catalog(catalog_authority, &mut pinned_profiles, &source_profile)?;
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
) -> anyhow::Result<OverviewStats> {
    let conn = open_handler_db(db_path)?;

    // The active pointer set is operational control-plane state; all package
    // counts and conversion identities below come from each pointer's exact
    // immutable catalog revision.
    let active_profiles = RemiActiveProfileRevision::list(&conn)?;
    let mut total_packages = 0_i64;
    let mut total_converted = 0_i64;
    for pointer in &active_profiles {
        let pinned = catalog_authority.open_active_profile(&pointer.source_profile)?;
        if pinned.profile_revision_sha256() != pointer.profile_revision_sha256 {
            anyhow::bail!(
                "active profile '{}' changed while building overview",
                pointer.source_profile
            );
        }
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
                    pointer.source_profile
                )
            })?;
            ConvertedPackage::require_conversion_pin(&conn, converted_id)?;
            converted.scriptlet_summary()?;
            let artifact = converted.repository_artifact()?;
            if artifact.source_profile == pointer.source_profile
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
    let total_distros = i64::try_from(active_profiles.len())?;

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
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::{
        ActiveCatalogFixture, package as catalog_package,
    };
    use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};
    use conary_core::repository::catalog::CatalogPackageRecordV1;

    fn package(
        name: &str,
        version: &str,
        architecture: &str,
        marker: &str,
    ) -> CatalogPackageRecordV1 {
        let mut package = catalog_package(
            "fedora-44",
            name,
            version,
            "",
            Some(architecture),
            3,
            marker,
        );
        package.description = Some(format!("catalog description {marker}"));
        package.metadata = Some(
            serde_json::json!({
                "license": "MIT",
                "homepage": format!("https://example.invalid/{marker}")
            })
            .to_string(),
        );
        package
    }

    fn insert_converted(
        conn: &Connection,
        profile_revision_sha256: &str,
        name: &str,
        version: &str,
        architecture: &str,
        conversion_version: i32,
    ) {
        let transport = crate::server::conversion::test_support::test_transport(&[]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            profile_revision_sha256.to_string(),
            name.to_string(),
            version.to_string(),
            architecture.to_string(),
            "rpm".to_string(),
            format!("sha256:source-{name}-{version}"),
            &transport,
            3,
            format!("sha256:content-{name}-{version}"),
            format!("/tmp/{name}-{version}.ccs"),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        converted.conversion_version = conversion_version;
        converted.insert_with_conversion_pin(conn, 1).unwrap();
    }

    fn insert_stale_conversion(
        conn: &Connection,
        profile_revision_sha256: &str,
        name: &str,
        version: &str,
        architecture: &str,
    ) {
        let transport = crate::server::conversion::test_support::test_transport(&[]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            profile_revision_sha256.to_string(),
            name.to_string(),
            version.to_string(),
            architecture.to_string(),
            "rpm".to_string(),
            format!("sha256:source-{name}-{version}"),
            &transport,
            3,
            format!("sha256:content-{name}-{version}"),
            format!("/tmp/{name}-{version}.ccs"),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        converted.conversion_version = CONVERSION_VERSION - 1;
        converted.insert_with_conversion_pin(conn, 1).unwrap();
    }

    #[test]
    fn package_detail_ignores_stale_converted_rows() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![package("pkg", "1.0", "x86_64", "stale")],
        );
        let conn = fixture.connection();
        insert_converted(
            &conn,
            &revision,
            "pkg",
            "1.0",
            "x86_64",
            CONVERSION_VERSION - 1,
        );

        let detail = query_package_detail(fixture.authority(), fixture.db_path(), "fedora", "pkg")
            .unwrap()
            .unwrap();

        assert!(!detail.converted);
        assert!(detail.versions.iter().all(|version| !version.converted));
    }

    #[test]
    fn package_versions_require_matching_architecture_for_converted_status() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![package("pkg", "1.0", "aarch64", "catalog")],
        );
        let conn = fixture.connection();
        insert_converted(&conn, &revision, "pkg", "1.0", "x86_64", CONVERSION_VERSION);

        let versions =
            query_versions(fixture.authority(), fixture.db_path(), "fedora", "pkg").unwrap();

        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].architecture.as_deref(), Some("aarch64"));
        assert!(!versions[0].converted);
    }

    #[test]
    fn overview_ignores_stale_converted_rows() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![
                package("stale", "1.0", "x86_64", "stale"),
                package("current", "1.0", "x86_64", "current"),
            ],
        );
        let conn = fixture.connection();
        insert_converted(
            &conn,
            &revision,
            "stale",
            "1.0",
            "x86_64",
            CONVERSION_VERSION - 1,
        );
        insert_converted(
            &conn,
            &revision,
            "current",
            "1.0",
            "x86_64",
            CONVERSION_VERSION,
        );
        let overview = query_overview(fixture.authority(), fixture.db_path()).unwrap();

        assert_eq!(overview.total_converted, 1);
    }

    #[test]
    fn recent_packages_use_analytics_order_and_catalog_payload_without_package_rows() {
        let fixture = ActiveCatalogFixture::new();
        fixture.activate(
            "fedora-44",
            1,
            vec![
                package("recent-new", "1.0", "x86_64", "new-catalog"),
                package("recent-old", "1.0", "x86_64", "old-catalog"),
            ],
        );
        let conn = fixture.connection();
        conn.execute(
            "INSERT INTO download_stats (
                 source_profile, package_name, package_version, downloaded_at
             ) VALUES ('fedora-44', 'recent-new', '1.0', '2026-08-22 02:00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO download_stats (
                 source_profile, package_name, package_version, downloaded_at
             ) VALUES ('fedora-44', 'recent-old', '1.0', '2026-08-22 01:00:00')",
            [],
        )
        .unwrap();

        let recent =
            query_recent(fixture.authority(), fixture.db_path(), Some("fedora"), 10).unwrap();

        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].name, "recent-new");
        assert_eq!(
            recent[0].description.as_deref(),
            Some("catalog description new-catalog")
        );
        assert_eq!(recent[1].name, "recent-old");
        assert_eq!(
            recent[1].description.as_deref(),
            Some("catalog description old-catalog")
        );
    }

    #[test]
    fn popular_packages_use_analytics_order_and_catalog_payload_without_package_rows() {
        let fixture = ActiveCatalogFixture::new();
        fixture.activate(
            "fedora-44",
            1,
            vec![
                package("popular-high", "1.0", "x86_64", "high-catalog"),
                package("popular-low", "1.0", "x86_64", "low-catalog"),
            ],
        );
        let conn = fixture.connection();
        conn.execute(
            "INSERT INTO download_counts (source_profile, package_name, total_count)
             VALUES ('fedora-44', 'popular-high', 20)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO download_counts (source_profile, package_name, total_count)
             VALUES ('fedora-44', 'popular-low', 5)",
            [],
        )
        .unwrap();

        let popular =
            query_popular(fixture.authority(), fixture.db_path(), Some("fedora"), 10).unwrap();

        assert_eq!(popular.len(), 2);
        assert_eq!(popular[0].name, "popular-high");
        assert_eq!(popular[0].download_count, 20);
        assert_eq!(
            popular[0].description.as_deref(),
            Some("catalog description high-catalog")
        );
        assert_eq!(popular[1].name, "popular-low");
        assert_eq!(popular[1].download_count, 5);
        assert_eq!(
            popular[1].description.as_deref(),
            Some("catalog description low-catalog")
        );
    }

    #[test]
    fn package_detail_counts_only_current_conversions() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![
                package("pkg", "1.0", "x86_64", "one"),
                package("pkg", "2.0", "x86_64", "two"),
            ],
        );
        let conn = fixture.connection();

        insert_converted(&conn, &revision, "pkg", "1.0", "x86_64", CONVERSION_VERSION);
        insert_stale_conversion(&conn, &revision, "pkg", "2.0", "x86_64");

        let detail = query_package_detail(fixture.authority(), fixture.db_path(), "fedora", "pkg")
            .unwrap()
            .unwrap();
        let versions =
            query_versions(fixture.authority(), fixture.db_path(), "fedora", "pkg").unwrap();
        let overview = query_overview(fixture.authority(), fixture.db_path()).unwrap();

        assert!(detail.converted);
        assert_eq!(overview.total_converted, 1);
        assert!(
            versions
                .iter()
                .any(|version| version.version == "1.0" && version.converted)
        );
        assert!(
            versions
                .iter()
                .any(|version| version.version == "2.0" && !version.converted)
        );
    }
}
