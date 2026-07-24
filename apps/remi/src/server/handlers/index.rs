// apps/remi/src/server/handlers/index.rs
//! Repository index endpoints - metadata serving

use crate::server::ServerState;
use crate::server::conversion::ScriptletPackageMetadata;
use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
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
    /// Dependency names (from native repo metadata)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    /// Additional native metadata, including provides for capability resolution.
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
    distro: &str,
) -> Result<RepositoryMetadata, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Find all repositories for this distro (e.g. arch-core + arch-extra)
    let repositories = find_repositories_for_distro(&conn, distro)?;

    if repositories.is_empty() {
        return Ok(RepositoryMetadata {
            id: format!("conary-{}", distro),
            distro: distro.to_string(),
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
        if let Some(id) = repo.id {
            repo_packages.extend(RepositoryPackage::find_by_repository(&conn, id)?);
        }
    }
    repo_packages.retain(|pkg| pkg.size > 0);

    // Query converted packages once so we can both mark repo-backed entries as
    // converted and surface packages that exist only in Remi's CCS store.
    let converted_packages = load_converted_metadata_rows(&conn, distro)?;
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
            let dependencies = pkg
                .dependencies
                .as_ref()
                .and_then(|deps_json| serde_json::from_str::<Vec<String>>(deps_json).ok());
            let metadata = pkg.metadata.as_ref().and_then(|metadata_json| {
                serde_json::from_str::<serde_json::Value>(metadata_json).ok()
            });
            let scriptlets = converted_scriptlets_by_key.get(&key);
            PackageEntry {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                release,
                architecture: pkg.architecture.clone(),
                converted: converted_set.contains(&key),
                dependencies,
                metadata: metadata_with_scriptlets(metadata, scriptlets),
            }
        })
        .collect();

    let existing_keys: HashSet<PackageKey> = packages
        .iter()
        .map(|pkg| {
            package_key(
                &pkg.name,
                &pkg.version,
                pkg.release.as_deref(),
                pkg.architecture.as_deref(),
            )
        })
        .collect();
    for converted in converted_packages {
        let key = package_key(
            &converted.name,
            &converted.version,
            None,
            converted.architecture.as_deref(),
        );
        if !existing_keys.contains(&key) {
            packages.push(PackageEntry {
                name: converted.name,
                version: converted.version,
                release: None,
                architecture: converted.architecture,
                converted: true,
                dependencies: None,
                metadata: metadata_with_scriptlets(None, Some(&converted.scriptlets)),
            });
        }
    }

    // Sort by name, then version
    packages.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.release.cmp(&b.release))
    });

    let converted_count = packages.iter().filter(|p| p.converted).count();

    Ok(RepositoryMetadata {
        id: format!("conary-{}", distro),
        distro: distro.to_string(),
        last_sync,
        package_count: packages.len(),
        converted_count,
        packages,
    })
}

/// Alias to shared implementation in handlers/mod.rs
use super::find_repositories_for_distro;

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

/// Build converted package entries for this distro.
#[cfg(test)]
fn build_converted_packages(
    conn: &Connection,
    distro: &str,
) -> Result<Vec<PackageEntry>, anyhow::Error> {
    Ok(load_converted_metadata_rows(conn, distro)?
        .into_iter()
        .map(|row| PackageEntry {
            name: row.name,
            version: row.version,
            release: None,
            architecture: row.architecture,
            converted: true,
            dependencies: None,
            metadata: metadata_with_scriptlets(None, Some(&row.scriptlets)),
        })
        .collect())
}

fn load_converted_metadata_rows(
    conn: &Connection,
    distro: &str,
) -> Result<Vec<ConvertedMetadataRow>, anyhow::Error> {
    let mut packages = Vec::new();
    for converted in ConvertedPackage::find_publication_candidates(conn, distro, None)? {
        let Some(name) = converted.package_name.clone() else {
            continue;
        };
        let Some(version) = converted.package_version.clone() else {
            continue;
        };
        let architecture = converted.package_architecture.clone();

        // Pre-architecture Remi conversion records cannot be addressed safely
        // once native metadata has multilib packages and epoch-aware versions.
        // Keep uploaded CCS fixtures visible, but do not advertise ambiguous
        // repo-derived conversions as installable repository metadata.
        if architecture.is_none() && converted.original_format != "ccs" {
            continue;
        }

        if !converted.is_scriptlet_public_ready() {
            continue;
        }

        packages.push(ConvertedMetadataRow {
            name,
            version,
            architecture,
            scriptlets: ScriptletPackageMetadata::from(&converted.scriptlet_summary()),
        });
    }

    Ok(packages)
}

fn metadata_with_scriptlets(
    metadata: Option<serde_json::Value>,
    scriptlets: Option<&ScriptletPackageMetadata>,
) -> Option<serde_json::Value> {
    let Some(scriptlets) = scriptlets else {
        return metadata;
    };
    let scriptlets = serde_json::to_value(scriptlets).ok()?;
    match metadata {
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
    }
}

/// GET /v1/:distro/metadata.sig
///
/// Returns GPG signature for repository metadata.
pub async fn get_metadata_sig(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
    }

    let state = state.read().await;
    let sig_path = state
        .config
        .chunk_dir
        .parent()
        .unwrap_or(&state.config.chunk_dir)
        .join("repo")
        .join(&distro)
        .join("metadata.json.sig");

    if !sig_path.exists() {
        return (StatusCode::NOT_FOUND, "Signature not found").into_response();
    }

    match tokio::fs::read(&sig_path).await {
        Ok(sig) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/pgp-signature")
            .header(header::CACHE_CONTROL, "public, max-age=300")
            .body(axum::body::Body::from(sig))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) => {
            tracing::error!("Failed to read signature: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read signature",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
#[path = "index/tests.rs"]
mod tests;
