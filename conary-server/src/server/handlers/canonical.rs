// conary-server/src/server/handlers/canonical.rs

//! Canonical package identity endpoints for the Remi server
//!
//! Provides lookup, search, and group listing endpoints for cross-distro
//! canonical package mappings.

use crate::server::ServerState;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

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
    pub source: String,
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
) -> Response {
    if let Err(e) = super::validate_name(&name) {
        return e;
    }

    let db_path = state.read().await.config.db_path.clone();

    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<Option<CanonicalLookupResponse>> {
            let conn = conary_core::db::open(&db_path)?;

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
        .await;

    match result {
        Ok(Ok(Some(response))) => Json(response).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            tracing::error!("Canonical lookup error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Canonical lookup task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// GET /v1/canonical/search?q=query -- search canonical package registry by
/// name or description substring.
pub async fn canonical_search(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<CanonicalSearchQuery>,
) -> Response {
    let query = params.q.unwrap_or_default();
    if query.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "missing q parameter"})),
        )
            .into_response();
    }

    let db_path = state.read().await.config.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = conary_core::db::open(&db_path)?;

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
            .collect())
    })
    .await;

    match result {
        Ok(Ok(items)) => Json(items).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Canonical search error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Canonical search task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// GET /v1/groups -- list all canonical groups (kind = "group").
pub async fn groups_list(State(state): State<Arc<RwLock<ServerState>>>) -> Response {
    let db_path = state.read().await.config.db_path.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = conary_core::db::open(&db_path)?;

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
            .collect())
    })
    .await;

    match result {
        Ok(Ok(items)) => Json(items).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Groups list error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Groups list task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// A single entry in the canonical map response.
#[derive(Debug, Serialize)]
pub struct CanonicalMapEntry {
    pub canonical: String,
    pub implementations: std::collections::BTreeMap<String, String>,
}

/// Full canonical map response returned by `GET /v1/canonical/map`.
#[derive(Debug, Serialize)]
pub struct CanonicalMapResponse {
    pub version: u32,
    pub generated_at: String,
    pub entries: Vec<CanonicalMapEntry>,
}

/// GET /v1/canonical/map -- returns the full canonical package map as JSON.
///
/// Groups all canonical packages with their distro implementations into a
/// single document suitable for client-side caching during repo sync.
/// Response is cached for 5 minutes via `Cache-Control`.
pub async fn canonical_map(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Response, Response> {
    let db_path = state.read().await.config.db_path.clone();

    let response = super::run_blocking("canonical_map", move || {
        let conn = conary_core::db::open(&db_path)?;

        let mut stmt = conn.prepare(
            "SELECT cp.name, pi.distro, pi.distro_name
             FROM canonical_packages cp
             JOIN package_implementations pi ON pi.canonical_id = cp.id
             ORDER BY cp.name, pi.distro",
        )?;

        let mut entries_map: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
            std::collections::BTreeMap::new();

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        for row in rows {
            let (canonical, distro, distro_name) = row?;
            entries_map
                .entry(canonical)
                .or_default()
                .insert(distro, distro_name);
        }

        let entries: Vec<CanonicalMapEntry> = entries_map
            .into_iter()
            .map(|(canonical, implementations)| CanonicalMapEntry {
                canonical,
                implementations,
            })
            .collect();

        Ok(CanonicalMapResponse {
            version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            entries,
        })
    })
    .await?;

    let json = super::serialize_json(&response, "canonical map")?;
    Ok(super::json_response(json, 300))
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
                    distro: "fedora-41".to_string(),
                    distro_name: "httpd".to_string(),
                    source: "curated".to_string(),
                },
                ImplementationInfo {
                    distro: "ubuntu-noble".to_string(),
                    distro_name: "apache2".to_string(),
                    source: "curated".to_string(),
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
            source: "auto".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("arch"));
        assert!(json.contains("nginx"));
        assert!(json.contains("auto"));
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
        impls.insert("fedora".to_string(), "openssl".to_string());
        impls.insert("ubuntu".to_string(), "libssl3".to_string());

        let response = CanonicalMapResponse {
            version: 1,
            generated_at: "2026-03-16T00:00:00Z".to_string(),
            entries: vec![CanonicalMapEntry {
                canonical: "openssl".to_string(),
                implementations: impls,
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"canonical\":\"openssl\""));
        assert!(json.contains("\"fedora\":\"openssl\""));
        assert!(json.contains("\"ubuntu\":\"libssl3\""));
    }

    #[test]
    fn test_canonical_map_empty_entries() {
        let response = CanonicalMapResponse {
            version: 1,
            generated_at: "2026-03-16T00:00:00Z".to_string(),
            entries: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"entries\":[]"));
    }
}
