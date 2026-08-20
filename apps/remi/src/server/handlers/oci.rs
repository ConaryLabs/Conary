// apps/remi/src/server/handlers/oci.rs
//! OCI Distribution Spec v3 package registry surface
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
use anyhow::Context;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::db::models::ConvertedPackage;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::HandlerResult;

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
    let (status, json) = match serde_json::to_vec(&body) {
        Ok(json) => (status, json),
        Err(error) => {
            tracing::error!(%error, "failed to serialize OCI error response");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                br#"{"errors":[{"code":"INTERNAL_ERROR","message":"Failed to serialize OCI error response"}]}"#
                    .to_vec(),
            )
        }
    };

    let mut response = Response::new(Body::from(json));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

async fn blob_allowed_by_public_gate(
    db_path: std::path::PathBuf,
    hash: String,
) -> HandlerResult<bool> {
    match tokio::task::spawn_blocking(move || {
        crate::server::publication::local_chunk_servable(&db_path, &hash)
    })
    .await
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::error!("Failed to check OCI blob publication reachability: {error}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
                .into_response()
                .into())
        }
        Err(error) => {
            tracing::error!("OCI blob reachability task failed: {error}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
                .into_response()
                .into())
        }
    }
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
async fn get_blob_inner(state: Arc<RwLock<ServerState>>, _name: &str, digest: &str) -> Response {
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

    // Normalize hash to lowercase to prevent cache bypass with mixed-case digests
    let hash = super::chunks::normalize_hash(hash);

    let (db_path, chunk_path) = {
        let state_guard = state.read().await;
        (
            state_guard.config.db_path.clone(),
            state_guard.chunk_cache.chunk_path(&hash),
        )
    };
    match blob_allowed_by_public_gate(db_path, hash.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return oci_error_response(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "Blob not found");
        }
        Err(response) => return response.into_response(),
    }

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
async fn head_blob_inner(state: Arc<RwLock<ServerState>>, _name: &str, digest: &str) -> Response {
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

    let hash = super::chunks::normalize_hash(hash);
    let (db_path, chunk_path) = {
        let state_guard = state.read().await;
        (
            state_guard.config.db_path.clone(),
            state_guard.chunk_cache.chunk_path(&hash),
        )
    };
    match blob_allowed_by_public_gate(db_path, hash).await {
        Ok(true) => {}
        Ok(false) => {
            return oci_error_response(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "Blob not found");
        }
        Err(response) => return response.into_response(),
    }

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
    // Resolve reference: if it starts with "sha256:", treat as digest lookup;
    // otherwise treat as a version tag
    let version = if reference.starts_with("sha256:") {
        // Digest reference -- find by content hash
        None
    } else {
        Some(reference)
    };

    let conn = Connection::open(db_path)?;
    let source_profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(distro)
            .ok_or_else(|| anyhow::anyhow!("unsupported public route '{distro}'"))?;

    let converted = if let Some(ver) = version {
        ConvertedPackage::find_by_package_identity(&conn, source_profile.id(), package, Some(ver))?
    } else {
        ConvertedPackage::find_by_content_hash_identity(
            &conn,
            source_profile.id(),
            package,
            reference,
        )?
    };

    let Some(converted) = converted else {
        return Ok(None);
    };
    if !converted.repository_metadata_is_current(&conn)? {
        return Ok(None);
    }
    converted.scriptlet_summary()?;

    let artifact = converted.repository_artifact()?;
    let chunk_hashes = artifact
        .transport
        .objects
        .iter()
        .map(|object| object.sha256.as_str())
        .collect::<Vec<_>>();

    if chunk_hashes.is_empty() {
        return Ok(None);
    }

    // Build layer descriptors from chunk hashes
    let mut layers = Vec::with_capacity(chunk_hashes.len());
    for hash in chunk_hashes {
        // Try to get actual size from disk
        let chunk_path = chunk_cache.chunk_path(hash);
        let size = i64::try_from(
            std::fs::metadata(&chunk_path)
                .with_context(|| format!("converted chunk {hash} is missing from local CAS"))?
                .len(),
        )
        .context("converted chunk size exceeds OCI descriptor range")?;
        let digest = if hash.starts_with("sha256:") {
            hash.to_string()
        } else {
            format!("sha256:{hash}")
        };

        layers.push(OciDescriptor {
            media_type: CONARY_CHUNK_MEDIA_TYPE.to_string(),
            digest,
            size,
            annotations: None,
        });
    }

    // Build config blob (synthetic JSON with package metadata)
    let config_json = serde_json::json!({
        "name": artifact.package_name,
        "version": artifact.package_version,
        "distro": distro,
        "format": converted.original_format,
        "total_size": artifact.total_size,
        "content_hash": artifact.content_hash,
    });
    let config_bytes = serde_json::to_vec(&config_json)?;
    let config_digest = format!("sha256:{}", conary_core::hash::sha256(&config_bytes));
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
    let manifest_digest = format!(
        "sha256:{}",
        conary_core::hash::sha256(manifest_json.as_bytes())
    );

    Ok(Some((manifest_json, manifest_digest)))
}

/// Build tags list for a package (available versions)
fn build_tags_list(
    db_path: &std::path::Path,
    distro: &str,
    package: &str,
) -> Result<Vec<String>, anyhow::Error> {
    let conn = Connection::open(db_path)?;
    let source_profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(distro)
            .ok_or_else(|| anyhow::anyhow!("unsupported public route '{distro}'"))?;
    let mut tags = Vec::new();
    for converted in
        ConvertedPackage::find_current_conversions(&conn, source_profile.id(), Some(package))?
    {
        converted.scriptlet_summary()?;
        tags.push(converted.repository_artifact()?.package_version.to_string());
    }
    tags.sort();
    tags.dedup();

    Ok(tags)
}

/// Build the OCI catalog (list of all repositories)
fn build_catalog(db_path: &std::path::Path) -> Result<OciCatalog, anyhow::Error> {
    let conn = Connection::open(db_path)?;
    let mut repositories = Vec::new();
    for converted in ConvertedPackage::list_repository_conversions(&conn)? {
        if !converted.repository_metadata_is_current(&conn)? {
            continue;
        }
        converted.scriptlet_summary()?;
        let artifact = converted.repository_artifact()?;
        let profile = conary_core::repository::supported_profiles::profile_by_public_id(
            artifact.source_profile,
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "converted artifact carries unsupported source profile '{}'",
                artifact.source_profile
            )
        })?;
        repositories.push(format!(
            "conary/{}/{}",
            profile.remi_route_slug(),
            artifact.package_name
        ));
    }
    repositories.sort();
    repositories.dedup();

    Ok(OciCatalog { repositories })
}

/// Strip the "sha256:" prefix from an OCI digest, returning the bare hex hash.
///
/// Validates that the remaining string is exactly 64 lowercase hex characters
/// to prevent path traversal via crafted digest strings.
/// Strip the `sha256:` prefix and validate the hash.
///
/// OCI digests may contain uppercase hex, so this function validates against
/// the case-insensitive `is_valid_hex_hash` (unlike chunk endpoints which
/// require lowercase). Callers must normalize to lowercase before CAS lookup.
fn strip_digest_prefix(digest: &str) -> Option<&str> {
    let hash = digest.strip_prefix("sha256:")?;
    if super::is_valid_hex_hash(hash) {
        Some(hash)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "oci/tests.rs"]
mod tests;
