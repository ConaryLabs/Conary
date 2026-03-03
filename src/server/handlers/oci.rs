// src/server/handlers/oci.rs
//! OCI Distribution Spec v2 compatibility layer
//!
//! Exposes CCS packages as OCI artifacts so any OCI-compatible tool
//! (Harbor, Zot, ORAS, crane) can interact with Conary's package store.
//!
//! Endpoints:
//! - GET /v2/ - Version check
//! - GET /v2/_catalog - List repositories
//! - GET /v2/{name}/manifests/{reference} - Get manifest
//! - HEAD /v2/{name}/manifests/{reference} - Check manifest existence
//! - GET /v2/{name}/blobs/{digest} - Get blob (chunk) data
//! - HEAD /v2/{name}/blobs/{digest} - Check blob existence
//! - GET /v2/{name}/tags/list - List tags (versions)

use crate::server::ServerState;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// OCI media type constants
const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const CONARY_CONFIG_MEDIA_TYPE: &str = "application/vnd.conary.package.config.v1+json";
const CONARY_CHUNK_MEDIA_TYPE: &str = "application/vnd.conary.chunk.v1";

/// OCI Image Manifest v2
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciManifest {
    schema_version: u32,
    media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_type: Option<String>,
    config: OciDescriptor,
    layers: Vec<OciDescriptor>,
}

/// OCI Content Descriptor
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OciDescriptor {
    media_type: String,
    digest: String,
    size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<HashMap<String, String>>,
}

/// OCI Tags List
#[derive(Debug, Serialize)]
struct OciTagsList {
    name: String,
    tags: Vec<String>,
}

/// OCI Catalog
#[derive(Debug, Serialize)]
struct OciCatalog {
    repositories: Vec<String>,
}

/// OCI error response body
#[derive(Debug, Serialize)]
struct OciErrors {
    errors: Vec<OciError>,
}

#[derive(Debug, Serialize)]
struct OciError {
    code: String,
    message: String,
}

fn oci_error_response(status: StatusCode, code: &str, message: &str) -> Response {
    let body = OciErrors {
        errors: vec![OciError {
            code: code.to_string(),
            message: message.to_string(),
        }],
    };
    let json = serde_json::to_string(&body).unwrap_or_default();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// GET /v2/ - OCI version check
///
/// Required by OCI spec. Returns empty JSON with 200 status.
pub async fn version_check() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header("Docker-Distribution-API-Version", "registry/2.0")
        .body(Body::from("{}"))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// GET /v2/_catalog - List repositories
pub async fn catalog(State(state): State<Arc<RwLock<ServerState>>>) -> Response {
    let state_guard = state.read().await;
    let db_path = state_guard.config.db_path.clone();
    drop(state_guard);

    let result = tokio::task::spawn_blocking(move || build_catalog(&db_path)).await;

    match result {
        Ok(Ok(catalog)) => {
            let json = match super::serialize_json(&catalog, "OCI catalog") {
                Ok(j) => j,
                Err(e) => return e,
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to build OCI catalog: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Catch-all handler for GET requests under /v2/*path
///
/// OCI names can contain slashes (e.g., conary/fedora/nginx), so we use
/// a wildcard route and parse the path to determine which endpoint to call.
pub async fn oci_catchall(
    State(state): State<Arc<RwLock<ServerState>>>,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    dispatch_oci_path(state, &path, headers.get(header::ACCEPT), false).await
}

/// Catch-all handler for HEAD requests under /v2/*path
pub async fn oci_catchall_head(
    State(state): State<Arc<RwLock<ServerState>>>,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    dispatch_oci_path(state, &path, headers.get(header::ACCEPT), true).await
}

/// Parse and dispatch OCI paths to the appropriate handler
///
/// Supported patterns:
/// - {name}/tags/list -> list_tags
/// - {name}/manifests/{reference} -> get_manifest / head_manifest
/// - {name}/blobs/{digest} -> get_blob / head_blob
async fn dispatch_oci_path(
    state: Arc<RwLock<ServerState>>,
    path: &str,
    _accept: Option<&axum::http::HeaderValue>,
    head_only: bool,
) -> Response {
    // Strip leading slash if present
    let path = path.strip_prefix('/').unwrap_or(path);

    // Try to match /tags/list at the end
    if let Some(name) = path.strip_suffix("/tags/list") {
        return list_tags_inner(state, name).await;
    }

    // Try to match /manifests/{reference}
    if let Some((name, reference)) = split_oci_segment(path, "/manifests/") {
        return if head_only {
            head_manifest_inner(state, name, reference).await
        } else {
            get_manifest_inner(state, name, reference).await
        };
    }

    // Try to match /blobs/{digest}
    if let Some((name, digest)) = split_oci_segment(path, "/blobs/") {
        return if head_only {
            head_blob_inner(state, name, digest).await
        } else {
            get_blob_inner(state, name, digest).await
        };
    }

    oci_error_response(
        StatusCode::NOT_FOUND,
        "NAME_UNKNOWN",
        "Unknown OCI endpoint",
    )
}

/// Split an OCI path at the last occurrence of a segment marker.
///
/// For example, splitting "conary/fedora/nginx/manifests/1.24.0" at "/manifests/"
/// yields ("conary/fedora/nginx", "1.24.0").
fn split_oci_segment<'a>(path: &'a str, segment: &str) -> Option<(&'a str, &'a str)> {
    // Use rfind to handle names that might contain the segment text (unlikely but safe)
    let idx = path.rfind(segment)?;
    let name = &path[..idx];
    let reference = &path[idx + segment.len()..];
    if name.is_empty() || reference.is_empty() {
        return None;
    }
    Some((name, reference))
}

/// Parse an OCI repository name into (distro, package_name).
///
/// Accepts formats:
/// - "conary/{distro}/{package}" (namespaced)
/// - "{distro}/{package}" (bare)
fn parse_oci_name(name: &str) -> Option<(&str, &str)> {
    let name = name.strip_prefix("conary/").unwrap_or(name);

    // Split into distro/package at the first slash
    let slash_pos = name.find('/')?;
    let distro = &name[..slash_pos];
    let package = &name[slash_pos + 1..];

    if distro.is_empty() || package.is_empty() || package.contains('/') {
        return None;
    }

    Some((distro, package))
}

/// GET /v2/{name}/manifests/{reference}
async fn get_manifest_inner(
    state: Arc<RwLock<ServerState>>,
    name: &str,
    reference: &str,
) -> Response {
    manifest_inner(state, name, reference, false).await
}

/// HEAD /v2/{name}/manifests/{reference}
async fn head_manifest_inner(
    state: Arc<RwLock<ServerState>>,
    name: &str,
    reference: &str,
) -> Response {
    manifest_inner(state, name, reference, true).await
}

async fn manifest_inner(
    state: Arc<RwLock<ServerState>>,
    name: &str,
    reference: &str,
    head_only: bool,
) -> Response {
    let (distro, package) = match parse_oci_name(name) {
        Some(p) => p,
        None => {
            return oci_error_response(
                StatusCode::NOT_FOUND,
                "NAME_UNKNOWN",
                "Invalid repository name format. Expected: conary/{distro}/{package}",
            );
        }
    };

    let state_guard = state.read().await;
    let db_path = state_guard.config.db_path.clone();
    let chunk_cache = state_guard.chunk_cache.clone();
    drop(state_guard);

    let distro = distro.to_string();
    let package = package.to_string();
    let reference = reference.to_string();

    let result = tokio::task::spawn_blocking(move || {
        build_manifest(&db_path, &distro, &package, &reference, &chunk_cache)
    })
    .await;

    match result {
        Ok(Ok(Some((manifest_json, manifest_digest)))) => {
            let body = if head_only {
                Body::empty()
            } else {
                Body::from(manifest_json.clone())
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, OCI_MANIFEST_MEDIA_TYPE)
                .header(header::CONTENT_LENGTH, manifest_json.len().to_string())
                .header("Docker-Content-Digest", &manifest_digest)
                .body(body)
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(Ok(None)) => oci_error_response(
            StatusCode::NOT_FOUND,
            "MANIFEST_UNKNOWN",
            "Manifest not found",
        ),
        Ok(Err(e)) => {
            tracing::error!("Failed to build OCI manifest: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// GET /v2/{name}/blobs/{digest}
async fn get_blob_inner(
    state: Arc<RwLock<ServerState>>,
    _name: &str,
    digest: &str,
) -> Response {
    let hash = match strip_digest_prefix(digest) {
        Some(h) => h,
        None => {
            return oci_error_response(
                StatusCode::BAD_REQUEST,
                "DIGEST_INVALID",
                "Invalid digest format. Expected: sha256:{hex}",
            );
        }
    };

    let state_guard = state.read().await;
    let chunk_path = state_guard.chunk_cache.chunk_path(hash);

    // Check if it exists on disk
    match tokio::fs::File::open(&chunk_path).await {
        Ok(file) => {
            let metadata = match file.metadata().await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!("Failed to get blob metadata: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read blob")
                        .into_response();
                }
            };

            let stream = tokio_util::io::ReaderStream::new(file);
            let body = Body::from_stream(stream);

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, metadata.len())
                .header("Docker-Content-Digest", digest)
                .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                .body(body)
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => {
            // The digest might be a config blob (synthetic JSON).
            // Config blobs are computed on the fly, so we check if this is
            // a known config by looking it up in converted_packages.
            // For simplicity, return 404 -- config blobs are embedded in
            // the manifest response and clients rarely fetch them separately.
            oci_error_response(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "Blob not found")
        }
    }
}

/// HEAD /v2/{name}/blobs/{digest}
async fn head_blob_inner(
    state: Arc<RwLock<ServerState>>,
    _name: &str,
    digest: &str,
) -> Response {
    let hash = match strip_digest_prefix(digest) {
        Some(h) => h,
        None => {
            return oci_error_response(
                StatusCode::BAD_REQUEST,
                "DIGEST_INVALID",
                "Invalid digest format",
            );
        }
    };

    let state_guard = state.read().await;
    let chunk_path = state_guard.chunk_cache.chunk_path(hash);

    match tokio::fs::metadata(&chunk_path).await {
        Ok(metadata) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, metadata.len())
            .header("Docker-Content-Digest", digest)
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => oci_error_response(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "Blob not found"),
    }
}

/// GET /v2/{name}/tags/list
async fn list_tags_inner(state: Arc<RwLock<ServerState>>, name: &str) -> Response {
    let (distro, package) = match parse_oci_name(name) {
        Some(p) => p,
        None => {
            return oci_error_response(
                StatusCode::NOT_FOUND,
                "NAME_UNKNOWN",
                "Invalid repository name",
            );
        }
    };

    let state_guard = state.read().await;
    let db_path = state_guard.config.db_path.clone();
    drop(state_guard);

    let oci_name = format!("conary/{}/{}", distro, package);
    let distro = distro.to_string();
    let package = package.to_string();

    let result =
        tokio::task::spawn_blocking(move || build_tags_list(&db_path, &distro, &package)).await;

    match result {
        Ok(Ok(tags)) => {
            let tags_list = OciTagsList {
                name: oci_name,
                tags,
            };
            let json = match super::serialize_json(&tags_list, "OCI tags list") {
                Ok(j) => j,
                Err(e) => return e,
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to build tags list: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// === Database query helpers ===

/// Build OCI manifest for a specific package version
fn build_manifest(
    db_path: &std::path::Path,
    distro: &str,
    package: &str,
    reference: &str,
    chunk_cache: &crate::server::ChunkCache,
) -> Result<Option<(String, String)>, anyhow::Error> {
    use crate::db::models::ConvertedPackage;

    // Resolve reference: if it starts with "sha256:", treat as digest lookup;
    // otherwise treat as a version tag
    let version = if reference.starts_with("sha256:") {
        // Digest reference -- find by content hash
        None
    } else {
        Some(reference)
    };

    let conn = Connection::open(db_path)?;

    let converted = if let Some(ver) = version {
        ConvertedPackage::find_by_package_identity(&conn, distro, package, Some(ver))?
    } else {
        // Find by content hash digest
        let hash = reference;
        conn.query_row(
            "SELECT id, trove_id, original_format, original_checksum, conversion_version,
                    conversion_fidelity, detected_hooks, converted_at,
                    enhancement_version, inferred_caps_json, extracted_provenance_json,
                    enhancement_status, enhancement_error, enhancement_attempted_at,
                    package_name, package_version, distro, chunk_hashes_json,
                    total_size, content_hash, ccs_path
             FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2 AND content_hash = ?3",
            rusqlite::params![distro, package, hash],
            |row| {
                // Re-use the same field order as ConvertedPackage::from_row
                Ok(ConvertedPackage {
                    id: row.get(0)?,
                    trove_id: row.get(1)?,
                    original_format: row.get(2)?,
                    original_checksum: row.get(3)?,
                    conversion_version: row.get(4)?,
                    conversion_fidelity: row.get(5)?,
                    detected_hooks: row.get(6)?,
                    converted_at: row.get(7)?,
                    enhancement_version: row.get(8).unwrap_or(0),
                    inferred_caps_json: row.get(9).ok(),
                    extracted_provenance_json: row.get(10).ok(),
                    enhancement_status: row.get(11).unwrap_or_else(|_| "pending".to_string()),
                    enhancement_error: row.get(12).ok(),
                    enhancement_attempted_at: row.get(13).ok(),
                    package_name: row.get(14).ok(),
                    package_version: row.get(15).ok(),
                    distro: row.get(16).ok(),
                    chunk_hashes_json: row.get(17).ok(),
                    total_size: row.get(18).ok(),
                    content_hash: row.get(19).ok(),
                    ccs_path: row.get(20).ok(),
                })
            },
        )
        .optional()?
    };

    let converted = match converted {
        Some(c) => c,
        None => return Ok(None),
    };

    // Parse chunk hashes from JSON
    let chunk_hashes: Vec<String> = converted
        .chunk_hashes_json
        .as_ref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();

    if chunk_hashes.is_empty() {
        return Ok(None);
    }

    // Build layer descriptors from chunk hashes
    let mut layers = Vec::with_capacity(chunk_hashes.len());
    for hash in &chunk_hashes {
        // Try to get actual size from disk
        let chunk_path = chunk_cache.chunk_path(hash);
        let size = std::fs::metadata(&chunk_path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        layers.push(OciDescriptor {
            media_type: CONARY_CHUNK_MEDIA_TYPE.to_string(),
            digest: format!("sha256:{}", hash),
            size,
            annotations: None,
        });
    }

    // Build config blob (synthetic JSON with package metadata)
    let pkg_version = converted
        .package_version
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let pkg_name = converted
        .package_name
        .clone()
        .unwrap_or_else(|| package.to_string());
    let pkg_distro = converted
        .distro
        .clone()
        .unwrap_or_else(|| distro.to_string());

    let config_json = serde_json::json!({
        "name": pkg_name,
        "version": pkg_version,
        "distro": pkg_distro,
        "format": converted.original_format,
        "total_size": converted.total_size.unwrap_or(0),
        "content_hash": converted.content_hash.clone().unwrap_or_default(),
    });
    let config_bytes = serde_json::to_vec(&config_json)?;
    let config_digest = format!("sha256:{}", crate::hash::sha256(&config_bytes));
    let config_size = config_bytes.len() as i64;

    let manifest = OciManifest {
        schema_version: 2,
        media_type: OCI_MANIFEST_MEDIA_TYPE.to_string(),
        artifact_type: Some("application/vnd.conary.package.v1".to_string()),
        config: OciDescriptor {
            media_type: CONARY_CONFIG_MEDIA_TYPE.to_string(),
            digest: config_digest,
            size: config_size,
            annotations: None,
        },
        layers,
    };

    let manifest_json = serde_json::to_string(&manifest)?;
    let manifest_digest = format!("sha256:{}", crate::hash::sha256(manifest_json.as_bytes()));

    Ok(Some((manifest_json, manifest_digest)))
}

/// Build tags list for a package (available versions)
fn build_tags_list(
    db_path: &std::path::Path,
    distro: &str,
    package: &str,
) -> Result<Vec<String>, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT DISTINCT package_version FROM converted_packages
         WHERE distro = ?1 AND package_name = ?2 AND package_version IS NOT NULL
         ORDER BY package_version",
    )?;

    let tags: Vec<String> = stmt
        .query_map(rusqlite::params![distro, package], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(tags)
}

/// Build the OCI catalog (list of all repositories)
fn build_catalog(db_path: &std::path::Path) -> Result<OciCatalog, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT DISTINCT distro, package_name FROM converted_packages
         WHERE distro IS NOT NULL AND package_name IS NOT NULL
         ORDER BY distro, package_name",
    )?;

    let repositories: Vec<String> = stmt
        .query_map([], |row| {
            let distro: String = row.get(0)?;
            let name: String = row.get(1)?;
            Ok(format!("conary/{}/{}", distro, name))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(OciCatalog { repositories })
}

/// Strip the "sha256:" prefix from an OCI digest, returning the bare hex hash
fn strip_digest_prefix(digest: &str) -> Option<&str> {
    digest.strip_prefix("sha256:")
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::ConvertedPackage;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn insert_converted_package(
        conn: &Connection,
        distro: &str,
        name: &str,
        version: &str,
        chunks: &[String],
    ) {
        let mut pkg = ConvertedPackage::new_server(
            distro.to_string(),
            name.to_string(),
            version.to_string(),
            "rpm".to_string(),
            format!("sha256:test-{}-{}", name, version),
            "high".to_string(),
            chunks,
            4096,
            format!("sha256:content-{}-{}", name, version),
            format!("/data/{}-{}.ccs", name, version),
        );
        pkg.insert(conn).unwrap();
    }

    #[test]
    fn test_parse_oci_name_namespaced() {
        let (distro, pkg) = parse_oci_name("conary/fedora/nginx").unwrap();
        assert_eq!(distro, "fedora");
        assert_eq!(pkg, "nginx");
    }

    #[test]
    fn test_parse_oci_name_bare() {
        let (distro, pkg) = parse_oci_name("fedora/nginx").unwrap();
        assert_eq!(distro, "fedora");
        assert_eq!(pkg, "nginx");
    }

    #[test]
    fn test_parse_oci_name_invalid() {
        assert!(parse_oci_name("nginx").is_none());
        assert!(parse_oci_name("").is_none());
        assert!(parse_oci_name("/nginx").is_none());
        assert!(parse_oci_name("fedora/").is_none());
        // Nested packages not supported
        assert!(parse_oci_name("fedora/nginx/extra").is_none());
    }

    #[test]
    fn test_split_oci_segment() {
        let (name, reference) =
            split_oci_segment("conary/fedora/nginx/manifests/1.24.0", "/manifests/").unwrap();
        assert_eq!(name, "conary/fedora/nginx");
        assert_eq!(reference, "1.24.0");

        let (name, digest) = split_oci_segment(
            "fedora/curl/blobs/sha256:abc123",
            "/blobs/",
        )
        .unwrap();
        assert_eq!(name, "fedora/curl");
        assert_eq!(digest, "sha256:abc123");
    }

    #[test]
    fn test_split_oci_segment_missing() {
        assert!(split_oci_segment("foo/bar", "/manifests/").is_none());
        assert!(split_oci_segment("/manifests/ref", "/manifests/").is_none());
        assert!(split_oci_segment("foo/manifests/", "/manifests/").is_none());
    }

    #[test]
    fn test_strip_digest_prefix() {
        assert_eq!(strip_digest_prefix("sha256:abc123"), Some("abc123"));
        assert_eq!(strip_digest_prefix("abc123"), None);
        assert_eq!(strip_digest_prefix("sha512:abc"), None);
    }

    #[test]
    fn test_build_tags_list() {
        let (temp_file, conn) = create_test_db();

        insert_converted_package(
            &conn,
            "fedora",
            "nginx",
            "1.24.0-1.fc43",
            &["chunk1".to_string()],
        );
        insert_converted_package(
            &conn,
            "fedora",
            "nginx",
            "1.25.0-1.fc43",
            &["chunk2".to_string()],
        );
        insert_converted_package(
            &conn,
            "arch",
            "nginx",
            "1.25.0-1",
            &["chunk3".to_string()],
        );

        let tags = build_tags_list(temp_file.path(), "fedora", "nginx").unwrap();
        assert_eq!(tags, vec!["1.24.0-1.fc43", "1.25.0-1.fc43"]);

        let tags = build_tags_list(temp_file.path(), "arch", "nginx").unwrap();
        assert_eq!(tags, vec!["1.25.0-1"]);

        let tags = build_tags_list(temp_file.path(), "fedora", "nonexistent").unwrap();
        assert!(tags.is_empty());
    }

    #[test]
    fn test_build_catalog() {
        let (temp_file, conn) = create_test_db();

        insert_converted_package(
            &conn,
            "fedora",
            "nginx",
            "1.24.0",
            &["chunk1".to_string()],
        );
        insert_converted_package(
            &conn,
            "fedora",
            "curl",
            "8.5.0",
            &["chunk2".to_string()],
        );
        insert_converted_package(
            &conn,
            "arch",
            "nginx",
            "1.25.0",
            &["chunk3".to_string()],
        );

        let catalog = build_catalog(temp_file.path()).unwrap();
        assert_eq!(
            catalog.repositories,
            vec![
                "conary/arch/nginx",
                "conary/fedora/curl",
                "conary/fedora/nginx",
            ]
        );
    }

    #[test]
    fn test_build_catalog_empty() {
        let (temp_file, _conn) = create_test_db();
        let catalog = build_catalog(temp_file.path()).unwrap();
        assert!(catalog.repositories.is_empty());
    }

    #[test]
    fn test_build_manifest() {
        let (temp_file, conn) = create_test_db();

        let chunks = vec!["aabbccdd".to_string(), "eeff0011".to_string()];
        insert_converted_package(&conn, "fedora", "nginx", "1.24.0", &chunks);

        // Create a temporary chunk cache directory
        let chunk_dir = tempfile::tempdir().unwrap();
        let chunk_cache = crate::server::ChunkCache::new(
            chunk_dir.path().to_path_buf(),
            1024 * 1024 * 1024,
            30,
            temp_file.path().to_path_buf(),
        );

        let result =
            build_manifest(temp_file.path(), "fedora", "nginx", "1.24.0", &chunk_cache).unwrap();

        assert!(result.is_some());
        let (json, digest) = result.unwrap();

        // Verify it parses as valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["schemaVersion"], 2);
        assert_eq!(parsed["mediaType"], OCI_MANIFEST_MEDIA_TYPE);

        // Should have 2 layers (one per chunk)
        let layers = parsed["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0]["digest"], "sha256:aabbccdd");
        assert_eq!(layers[1]["digest"], "sha256:eeff0011");

        // Config should be present
        assert!(parsed["config"]["digest"].as_str().unwrap().starts_with("sha256:"));

        // Digest should be sha256
        assert!(digest.starts_with("sha256:"));
    }

    #[test]
    fn test_build_manifest_not_found() {
        let (temp_file, _conn) = create_test_db();
        let chunk_dir = tempfile::tempdir().unwrap();
        let chunk_cache = crate::server::ChunkCache::new(
            chunk_dir.path().to_path_buf(),
            1024 * 1024 * 1024,
            30,
            temp_file.path().to_path_buf(),
        );

        let result = build_manifest(
            temp_file.path(),
            "fedora",
            "nonexistent",
            "1.0.0",
            &chunk_cache,
        )
        .unwrap();
        assert!(result.is_none());
    }
}
