// apps/remi/src/server/handlers/index.rs
//! Repository index endpoints - metadata serving

use crate::server::ServerState;
use crate::server::conversion::ScriptletPackageMetadata;
use anyhow::Context;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
use conary_core::repository::remi_metadata::{RemiProvide, RemiRequirementGroup};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Repository metadata response
#[derive(Serialize)]
pub struct RepositoryMetadata {
    /// Repository identifier
    pub id: String,
    /// Distribution name
    pub distro: String,
    /// Last sync timestamp (ISO 8601)
    pub last_sync: Option<String>,
    /// Number of packages available
    pub package_count: usize,
    /// Number of packages already converted to CCS
    pub converted_count: usize,
    /// List of available packages (names only for index)
    pub packages: Vec<PackageEntry>,
}

#[derive(Serialize)]
pub struct PackageEntry {
    pub name: String,
    pub version: String,
    /// Native package release identity, when published by Remi native CCS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// Native package architecture from upstream repository metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    /// Whether this package has been converted to CCS
    pub converted: bool,
    /// Exact normalized native provides used by dependency resolution.
    pub provides: Vec<RemiProvide>,
    /// Exact grouped native requirements and relations.
    pub requirement_groups: Vec<RemiRequirementGroup>,
    /// Additional non-authoritative package metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// GET /v1/:distro/metadata
///
/// Returns repository metadata index. Cached by Cloudflare.
pub async fn get_metadata(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
    }

    let db_path = state.read().await.config.db_path.clone();

    let result = tokio::task::spawn_blocking(move || build_metadata(&db_path, &distro)).await;

    match result {
        Ok(Ok(metadata)) => {
            let json = match super::serialize_json(&metadata, "repository metadata") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to build metadata: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build metadata",
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in get_metadata: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Build repository metadata from database
fn build_metadata(
    db_path: &std::path::Path,
    route_slug: &str,
) -> Result<RepositoryMetadata, anyhow::Error> {
    let conn = Connection::open(db_path)?;
    let profile = conary_core::repository::supported_profiles::profile_for_remi_route(route_slug)
        .ok_or_else(|| anyhow::anyhow!("unsupported public route '{route_slug}'"))?;

    // Public HTTP routes are stable family slugs; persisted repository
    // authority is always the exact release profile.
    let repositories = find_repositories_for_profile(&conn, profile.id())?;

    if repositories.is_empty() {
        return Ok(RepositoryMetadata {
            id: format!("conary-{route_slug}"),
            distro: route_slug.to_string(),
            last_sync: None,
            package_count: 0,
            converted_count: 0,
            packages: vec![],
        });
    }

    // Use the most recent last_sync across all matching repos
    let last_sync = repositories
        .iter()
        .filter_map(|r| r.last_sync.as_ref())
        .max()
        .cloned();

    // Aggregate packages from all matching repos
    let mut repo_packages = Vec::new();
    for repo in &repositories {
        let id = super::require_persisted_repository_id(repo)?;
        repo_packages.extend(RepositoryPackage::find_by_repository(&conn, id)?);
    }
    repo_packages.retain(|pkg| pkg.size > 0);

    // Query current conversions once to mark their exact repository-backed
    // entries as converted.
    let converted_packages = load_converted_metadata_rows(&conn, profile.id())?;
    let converted_set: HashSet<PackageKey> = converted_packages
        .iter()
        .map(|pkg| package_key(&pkg.name, &pkg.version, None, pkg.architecture.as_deref()))
        .collect();
    let converted_scriptlets_by_key: HashMap<PackageKey, ScriptletPackageMetadata> =
        converted_packages
            .iter()
            .map(|pkg| {
                (
                    package_key(&pkg.name, &pkg.version, None, pkg.architecture.as_deref()),
                    pkg.scriptlets.clone(),
                )
            })
            .collect();

    // Build package entries
    let mut packages: Vec<PackageEntry> = repo_packages
        .iter()
        .map(|pkg| {
            let release = non_empty_release(&pkg.package_release);
            let key = package_key(
                &pkg.name,
                &pkg.version,
                release.as_deref(),
                pkg.architecture.as_deref(),
            );
            let metadata = pkg
                .metadata
                .as_deref()
                .map(serde_json::from_str::<serde_json::Value>)
                .transpose()
                .with_context(|| {
                    format!(
                        "repository package '{}' version '{}' has malformed persisted metadata",
                        pkg.name, pkg.version
                    )
                })?;
            let scriptlets = converted_scriptlets_by_key.get(&key);
            let exact = crate::server::package_metadata::load_exact_package_metadata(
                &conn,
                pkg.id
                    .ok_or_else(|| anyhow::anyhow!("repository package has no persisted ID"))?,
            )?;
            Ok(PackageEntry {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                release,
                architecture: pkg.architecture.clone(),
                converted: converted_set.contains(&key),
                provides: exact.provides,
                requirement_groups: exact.requirement_groups,
                metadata: metadata_with_scriptlets(metadata, scriptlets)?,
            })
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    // Sort by name, then version
    packages.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.release.cmp(&b.release))
    });

    let converted_count = packages.iter().filter(|p| p.converted).count();

    Ok(RepositoryMetadata {
        id: format!("conary-{route_slug}"),
        distro: route_slug.to_string(),
        last_sync,
        package_count: packages.len(),
        converted_count,
        packages,
    })
}

/// Alias to shared implementation in handlers/mod.rs
use super::find_repositories_for_profile;

type PackageKey = (String, String, Option<String>, Option<String>);

fn package_key(
    name: &str,
    version: &str,
    release: Option<&str>,
    architecture: Option<&str>,
) -> PackageKey {
    (
        name.to_string(),
        version.to_string(),
        release.map(str::to_string),
        architecture.map(str::to_string),
    )
}

fn non_empty_release(release: &str) -> Option<String> {
    (!release.is_empty()).then(|| release.to_string())
}

#[derive(Debug, Clone)]
struct ConvertedMetadataRow {
    name: String,
    version: String,
    architecture: Option<String>,
    scriptlets: ScriptletPackageMetadata,
}

fn load_converted_metadata_rows(
    conn: &Connection,
    source_profile: &str,
) -> Result<Vec<ConvertedMetadataRow>, anyhow::Error> {
    let mut packages = Vec::new();
    for converted in ConvertedPackage::find_current_conversions(conn, source_profile, None)? {
        let artifact = converted.repository_artifact()?;
        let name = artifact.package_name.to_string();
        let version = artifact.package_version.to_string();
        let architecture = Some(artifact.package_architecture.to_string());

        let scriptlet_summary = converted.scriptlet_summary()?;

        packages.push(ConvertedMetadataRow {
            name,
            version,
            architecture,
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
        });
    }

    Ok(packages)
}

fn metadata_with_scriptlets(
    metadata: Option<serde_json::Value>,
    scriptlets: Option<&ScriptletPackageMetadata>,
) -> anyhow::Result<Option<serde_json::Value>> {
    let Some(scriptlets) = scriptlets else {
        return Ok(metadata);
    };
    let scriptlets =
        serde_json::to_value(scriptlets).context("serialize scriptlet package metadata")?;
    Ok(match metadata {
        Some(serde_json::Value::Object(mut object)) => {
            object.insert("scriptlets".to_string(), scriptlets);
            Some(serde_json::Value::Object(object))
        }
        Some(existing) => {
            let mut object = serde_json::Map::new();
            object.insert("native".to_string(), existing);
            object.insert("scriptlets".to_string(), scriptlets);
            Some(serde_json::Value::Object(object))
        }
        None => {
            let mut object = serde_json::Map::new();
            object.insert("scriptlets".to_string(), scriptlets);
            Some(serde_json::Value::Object(object))
        }
    })
}

#[cfg(test)]
#[path = "index/tests.rs"]
mod tests;
