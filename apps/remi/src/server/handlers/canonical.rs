// apps/remi/src/server/handlers/canonical.rs

//! Canonical package identity endpoints for the Remi server
//!
//! Provides lookup, search, and group listing endpoints for cross-distro
//! canonical package mappings.

use crate::server::ServerState;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use conary_core::db::models::CanonicalMappingAuthority;

use super::{HandlerResult, open_handler_db, run_blocking};

type CanonicalMapAccumulator = (String, Option<String>, BTreeMap<String, String>);

/// Canonical package lookup response
#[derive(Debug, Serialize)]
pub struct CanonicalLookupResponse {
    pub canonical_name: String,
    pub appstream_id: Option<String>,
    pub kind: String,
    pub description: Option<String>,
    pub implementations: Vec<ImplementationInfo>,
}

/// A single distro implementation
#[derive(Debug, Serialize)]
pub struct ImplementationInfo {
    pub distro: String,
    pub distro_name: String,
    pub source: CanonicalMappingAuthority,
}

/// Search query parameters
#[derive(Debug, Deserialize)]
pub struct CanonicalSearchQuery {
    pub q: Option<String>,
}

/// GET /v1/canonical/:name -- lookup canonical package by name, AppStream ID,
/// or distro-specific name. Returns the resolved canonical entry plus all
/// known distro implementations.
pub async fn canonical_lookup(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> HandlerResult<Response> {
    super::validate_name(&name)?;

    let db_path = state.read().await.config.db_path.clone();

    let result = run_blocking("canonical lookup", move || {
        let conn = open_handler_db(&db_path)?;

        use conary_core::db::models::{CanonicalPackage, PackageImplementation};

        let pkg = match CanonicalPackage::resolve_name(&conn, &name)? {
            Some(pkg) => pkg,
            None => return Ok(None),
        };

        let impls = PackageImplementation::find_by_canonical(&conn, pkg.id.unwrap())?;

        Ok(Some(CanonicalLookupResponse {
            canonical_name: pkg.name,
            appstream_id: pkg.appstream_id,
            kind: pkg.kind,
            description: pkg.description,
            implementations: impls
                .into_iter()
                .map(|i| ImplementationInfo {
                    distro: i.distro,
                    distro_name: i.distro_name,
                    source: i.source,
                })
                .collect(),
        }))
    })
    .await?;

    match result {
        Some(response) => Ok(Json(response).into_response()),
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        )
            .into_response()),
    }
}

/// GET /v1/canonical/search?q=query -- search canonical package registry by
/// name or description substring.
pub async fn canonical_search(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<CanonicalSearchQuery>,
) -> HandlerResult<Response> {
    let query = params.q.unwrap_or_default();
    if query.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "missing q parameter"})),
        )
            .into_response());
    }

    let db_path = state.read().await.config.db_path.clone();

    let items = run_blocking("canonical search", move || {
        let conn = open_handler_db(&db_path)?;

        use conary_core::db::models::CanonicalPackage;

        let results = CanonicalPackage::search(&conn, &query)?;
        Ok(results
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "kind": p.kind,
                    "appstream_id": p.appstream_id,
                    "description": p.description,
                })
            })
            .collect::<Vec<_>>())
    })
    .await?;

    Ok(Json(items).into_response())
}

/// GET /v1/groups -- list all canonical groups (kind = "group").
pub async fn groups_list(State(state): State<Arc<RwLock<ServerState>>>) -> HandlerResult<Response> {
    let db_path = state.read().await.config.db_path.clone();

    let items = run_blocking("groups list", move || {
        let conn = open_handler_db(&db_path)?;

        use conary_core::db::models::CanonicalPackage;

        let groups = CanonicalPackage::list_by_kind(&conn, "group")?;
        Ok(groups
            .into_iter()
            .map(|g| {
                serde_json::json!({
                    "name": g.name,
                    "description": g.description,
                })
            })
            .collect::<Vec<_>>())
    })
    .await?;

    Ok(Json(items).into_response())
}

/// A single entry in the canonical map response.
#[derive(Debug, Serialize)]
pub struct CanonicalMapEntry {
    pub canonical: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub implementations: std::collections::BTreeMap<String, String>,
}

/// Full canonical map response returned by `GET /v1/canonical/map`.
#[derive(Debug, Serialize)]
pub struct CanonicalMapResponse {
    pub schema_version: u32,
    pub revision: u64,
    pub generated_at: Option<String>,
    pub entries: Vec<CanonicalMapEntry>,
}

/// GET /v1/canonical/map -- returns the full canonical package map as JSON.
///
/// Groups all canonical packages with their distro implementations into a
/// single document suitable for client-side caching during repo sync.
/// Response is cached for 5 minutes via `Cache-Control`. Supports ETag
/// conditional requests via `If-None-Match` for efficient polling.
pub async fn canonical_map(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    let db_path = state.read().await.config.db_path.clone();

    let response = run_blocking("canonical_map", move || {
        let conn = open_handler_db(&db_path)?;

        let revision_value = conary_core::db::models::get_metadata(
            &conn,
            conary_core::db::models::MetadataTable::Server,
            "canonical_map_revision",
        )
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| anyhow::anyhow!("canonical map revision metadata is missing"))?;
        let revision = revision_value.parse::<u64>().map_err(|error| {
            anyhow::anyhow!("canonical map revision '{revision_value}' is invalid: {error}")
        })?;
        let generated_at = conary_core::db::models::get_metadata(
            &conn,
            conary_core::db::models::MetadataTable::Server,
            "last_canonical_rebuild",
        )
        .map_err(anyhow::Error::from)?;
        match (revision, generated_at.as_deref()) {
            (0, None) => {}
            (0, Some(_)) => {
                anyhow::bail!("canonical map revision is zero but a rebuild timestamp is present")
            }
            (_, None) => {
                anyhow::bail!("canonical map revision {revision} has no rebuild timestamp")
            }
            (_, Some(timestamp)) => {
                chrono::DateTime::parse_from_rfc3339(timestamp).map_err(|error| {
                    anyhow::anyhow!("canonical map rebuild timestamp is invalid: {error}")
                })?;
            }
        }

        let mut stmt = conn.prepare(
            "SELECT cp.name, cp.kind, cp.category, pi.distro, pi.distro_name
             FROM canonical_packages cp
             JOIN package_implementations pi ON pi.canonical_id = cp.id
             ORDER BY cp.name, pi.distro",
        )?;

        let mut entries_map: BTreeMap<String, CanonicalMapAccumulator> = BTreeMap::new();

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        for row in rows {
            let (canonical, kind, category, distro, distro_name) = row?;
            let (_, _, implementations) = entries_map
                .entry(canonical)
                .or_insert_with(|| (kind, category, BTreeMap::new()));
            implementations.insert(distro, distro_name);
        }

        let entries: Vec<CanonicalMapEntry> = entries_map
            .into_iter()
            .map(
                |(canonical, (kind, category, implementations))| CanonicalMapEntry {
                    canonical,
                    kind,
                    category,
                    implementations,
                },
            )
            .collect();
        if revision == 0 && !entries.is_empty() {
            anyhow::bail!("canonical map has entries but its persisted revision is zero");
        }

        Ok((
            revision,
            CanonicalMapResponse {
                schema_version: conary_core::canonical::CANONICAL_MAP_SCHEMA_VERSION,
                revision,
                generated_at,
                entries,
            },
        ))
    })
    .await?;

    let (revision, map_response) = response;
    let json = super::serialize_json(&map_response, "canonical map")?;
    let checksum = conary_core::hash::sha256(json.as_bytes());
    let etag = format!("\"sha256:{checksum}\"");

    if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return Ok(axum::http::Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header("ETag", &etag)
            .body(axum::body::Body::empty())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()));
    }

    let mut response = super::json_response(json, 300);
    response.headers_mut().insert(
        "ETag",
        etag.parse()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?,
    );
    response.headers_mut().insert(
        "X-Conary-Canonical-Sha256",
        checksum
            .parse()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?,
    );
    response.headers_mut().insert(
        "X-Conary-Canonical-Revision",
        revision
            .to_string()
            .parse()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?,
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_lookup_response_serialization() {
        let response = CanonicalLookupResponse {
            canonical_name: "apache-httpd".to_string(),
            appstream_id: None,
            kind: "package".to_string(),
            description: Some("Apache HTTP Server".to_string()),
            implementations: vec![
                ImplementationInfo {
                    distro: "fedora-44".to_string(),
                    distro_name: "httpd".to_string(),
                    source: CanonicalMappingAuthority::Contract,
                },
                ImplementationInfo {
                    distro: "ubuntu-26.04".to_string(),
                    distro_name: "apache2".to_string(),
                    source: CanonicalMappingAuthority::Contract,
                },
            ],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("apache-httpd"));
        assert!(json.contains("httpd"));
        assert!(json.contains("apache2"));
        assert!(json.contains("Apache HTTP Server"));
    }

    #[test]
    fn test_implementation_info_serialization() {
        let info = ImplementationInfo {
            distro: "arch".to_string(),
            distro_name: "nginx".to_string(),
            source: CanonicalMappingAuthority::Remi,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("arch"));
        assert!(json.contains("nginx"));
        assert!(json.contains("remi"));
    }

    #[test]
    fn test_empty_implementations() {
        let response = CanonicalLookupResponse {
            canonical_name: "orphan-pkg".to_string(),
            appstream_id: None,
            kind: "package".to_string(),
            description: None,
            implementations: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("orphan-pkg"));
        assert!(json.contains("\"implementations\":[]"));
    }

    #[test]
    fn test_canonical_map_response_serialization() {
        let mut impls = std::collections::BTreeMap::new();
        impls.insert("fedora-44".to_string(), "openssl".to_string());
        impls.insert("ubuntu-26.04".to_string(), "libssl3".to_string());

        let response = CanonicalMapResponse {
            schema_version: 1,
            revision: 7,
            generated_at: Some("2026-03-16T00:00:00Z".to_string()),
            entries: vec![CanonicalMapEntry {
                canonical: "openssl".to_string(),
                kind: "package".to_string(),
                category: Some("security".to_string()),
                implementations: impls,
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"schema_version\":1"));
        assert!(json.contains("\"revision\":7"));
        assert!(json.contains("\"canonical\":\"openssl\""));
        assert!(json.contains("\"kind\":\"package\""));
        assert!(json.contains("\"category\":\"security\""));
        assert!(json.contains("\"fedora-44\":\"openssl\""));
        assert!(json.contains("\"ubuntu-26.04\":\"libssl3\""));
    }

    #[test]
    fn test_canonical_map_empty_entries() {
        let response = CanonicalMapResponse {
            schema_version: 1,
            revision: 7,
            generated_at: Some("2026-03-16T00:00:00Z".to_string()),
            entries: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"entries\":[]"));
    }
}
