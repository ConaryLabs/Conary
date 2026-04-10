// apps/remi/src/server/handlers/self_update.rs
//! Self-update endpoints for Conary binary distribution
//!
//! Serves CCS packages from a `self-update/` directory under the storage root.
//! Packages are named `conary-{version}.ccs` (e.g., `conary-0.2.0.ccs`).
//!
//! Endpoints:
//! - GET /v1/ccs/conary/latest     -> latest version metadata
//! - GET /v1/ccs/conary/versions   -> list all available versions
//! - GET /v1/ccs/conary/:version/download -> stream the CCS package

use crate::server::ServerState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tokio_util::io::ReaderStream;

/// TTL for the cached scan_versions result (60 seconds).
const VERSIONS_CACHE_TTL_SECS: u64 = 60;

/// Cached scan_versions result with expiry timestamp, plus precomputed
/// latest-version hash and size so `get_latest` avoids re-reading the file.
struct VersionsCacheEntry {
    fetched_at: Instant,
    versions: Vec<String>,
    /// SHA-256 and size of the latest CCS package, computed once during scan.
    latest_hash: LatestHash,
}

static VERSIONS_CACHE: std::sync::LazyLock<Mutex<Option<VersionsCacheEntry>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Response for GET /v1/ccs/conary/latest
#[derive(Serialize)]
pub struct LatestResponse {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Response for GET /v1/ccs/conary/versions
#[derive(Serialize)]
pub struct VersionsResponse {
    pub versions: Vec<String>,
    pub latest: String,
}

/// Parse a semver string into (major, minor, patch) for comparison
fn parse_semver(v: &str) -> (u64, u64, u64) {
    let parts: Vec<&str> = v.split('.').collect();
    let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn is_valid_semver_triple(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| p.parse::<u64>().is_ok())
}

/// Derive the self-update directory from server config.
///
/// Layout: `{storage_root}/self-update/conary-{version}.ccs`
/// Since `chunk_dir` is `{storage_root}/chunks`, we go one level up.
fn self_update_dir(state: &ServerState) -> PathBuf {
    state
        .config
        .chunk_dir
        .parent()
        .unwrap_or(&state.config.chunk_dir)
        .join("self-update")
}

/// Precomputed hash, size, and optional signature for the latest CCS package.
type LatestHash = Option<(String, u64, Option<String>)>;

#[allow(clippy::result_large_err)]
fn read_version_payload(
    dir: &std::path::Path,
    version: &str,
) -> Result<(String, u64, Option<String>), Response> {
    let ccs_path = dir.join(format!("conary-{version}.ccs"));
    let data = match std::fs::read(&ccs_path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((StatusCode::NOT_FOUND, "Version not found").into_response());
        }
        Err(e) => {
            tracing::error!("Failed to read CCS package {}: {}", ccs_path.display(), e);
            return Err(
                (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read package").into_response(),
            );
        }
    };

    let sig_path = dir.join(format!("conary-{version}.ccs.sig"));
    let signature = std::fs::read_to_string(&sig_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Ok((
        conary_core::hash::sha256(&data),
        data.len() as u64,
        signature,
    ))
}

#[allow(clippy::result_large_err)]
fn build_version_response(
    dir: &std::path::Path,
    version: &str,
    cached_payload: LatestHash,
) -> Result<LatestResponse, Response> {
    let (sha256, size, signature) = match cached_payload {
        Some(cached) => cached,
        None => read_version_payload(dir, version)?,
    };

    Ok(LatestResponse {
        version: version.to_string(),
        download_url: format!("/v1/ccs/conary/{version}/download"),
        sha256,
        size,
        signature,
    })
}

/// Scan the self-update directory and return sorted (ascending) version strings,
/// plus the SHA-256 hash and size of the latest CCS package.
#[allow(clippy::result_large_err)]
fn scan_versions_and_hash(dir: &std::path::Path) -> Result<(Vec<String>, LatestHash), Response> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                (StatusCode::NOT_FOUND, "No self-update packages available").into_response()
            );
        }
        Err(e) => {
            tracing::error!(
                "Failed to read self-update directory {}: {}",
                dir.display(),
                e
            );
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read package directory",
            )
                .into_response());
        }
    };

    let mut versions: Vec<String> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_str()?.to_string();
            // Match pattern: conary-{version}.ccs
            let version = name.strip_prefix("conary-")?.strip_suffix(".ccs")?;
            // Basic validation: must look like a semver triple
            let parts: Vec<&str> = version.split('.').collect();
            if parts.len() == 3 && parts.iter().all(|p| p.parse::<u64>().is_ok()) {
                Some(version.to_string())
            } else {
                None
            }
        })
        .collect();

    if versions.is_empty() {
        return Err((StatusCode::NOT_FOUND, "No self-update packages available").into_response());
    }

    versions.sort_by_key(|v| parse_semver(v));

    // Compute SHA-256, size, and optional signature for the latest version during the scan
    let latest_hash = if let Some(latest) = versions.last() {
        let ccs_path = dir.join(format!("conary-{latest}.ccs"));
        match std::fs::read(&ccs_path) {
            Ok(data) => {
                let sha256 = conary_core::hash::sha256(&data);
                let size = data.len() as u64;
                let sig_path = dir.join(format!("conary-{latest}.ccs.sig"));
                let signature = std::fs::read_to_string(&sig_path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                Some((sha256, size, signature))
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to hash latest CCS package {}: {}",
                    ccs_path.display(),
                    e
                );
                None
            }
        }
    } else {
        None
    };

    Ok((versions, latest_hash))
}

/// Return cached versions (with latest hash) or re-scan if the cache has expired.
#[allow(clippy::result_large_err)]
async fn scan_versions_cached(
    dir: &std::path::Path,
) -> Result<(Vec<String>, LatestHash), Response> {
    let ttl = std::time::Duration::from_secs(VERSIONS_CACHE_TTL_SECS);

    {
        let cache = VERSIONS_CACHE.lock().await;
        if let Some(ref entry) = *cache
            && entry.fetched_at.elapsed() < ttl
        {
            return Ok((entry.versions.clone(), entry.latest_hash.clone()));
        }
    }

    // Cache miss or expired -- rescan (sync I/O, use spawn_blocking)
    let dir_owned = dir.to_path_buf();
    let (versions, latest_hash) =
        tokio::task::spawn_blocking(move || scan_versions_and_hash(&dir_owned))
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("spawn_blocking failed: {e}"),
                )
                    .into_response()
            })??;

    {
        let mut cache = VERSIONS_CACHE.lock().await;
        *cache = Some(VersionsCacheEntry {
            fetched_at: Instant::now(),
            versions: versions.clone(),
            latest_hash: latest_hash.clone(),
        });
    }

    Ok((versions, latest_hash))
}

/// GET /v1/ccs/conary/latest
///
/// Returns metadata about the latest available Conary self-update package.
/// The SHA-256 hash and size are computed once during the version scan and
/// cached, avoiding a full file read on every request.
pub async fn get_latest(State(state): State<Arc<RwLock<ServerState>>>) -> Response {
    let state_guard = state.read().await;
    let dir = self_update_dir(&state_guard);
    drop(state_guard);

    let (versions, latest_hash) = match scan_versions_cached(&dir).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    let latest = versions.last().expect("scan_versions guarantees non-empty");

    let response = match build_version_response(&dir, latest, latest_hash) {
        Ok(response) => response,
        Err(e) => return e,
    };

    let json = match super::serialize_json(&response, "self-update latest") {
        Ok(j) => j,
        Err(e) => return e,
    };

    // Cache for 5 minutes -- clients should not hammer this endpoint
    super::json_response(json, 300)
}

/// GET /v1/ccs/conary/:version
///
/// Returns metadata about a specific self-update package version.
pub async fn get_version_info(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(version): Path<String>,
) -> Response {
    if let Err(e) = super::validate_name(&version) {
        return e;
    }

    if !is_valid_semver_triple(&version) {
        return (StatusCode::BAD_REQUEST, "Invalid version format").into_response();
    }

    let state_guard = state.read().await;
    let dir = self_update_dir(&state_guard);
    drop(state_guard);

    let (versions, latest_hash) = match scan_versions_cached(&dir).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    if !versions.iter().any(|candidate| candidate == &version) {
        return (StatusCode::NOT_FOUND, "Version not found").into_response();
    }

    let cached_payload = if versions.last().is_some_and(|latest| latest == &version) {
        latest_hash
    } else {
        None
    };
    let response = match build_version_response(&dir, &version, cached_payload) {
        Ok(response) => response,
        Err(e) => return e,
    };

    let json = match super::serialize_json(&response, "self-update version") {
        Ok(j) => j,
        Err(e) => return e,
    };

    super::json_response(json, 300)
}

/// GET /v1/ccs/conary/versions
///
/// Returns a list of all available self-update versions.
pub async fn get_versions(State(state): State<Arc<RwLock<ServerState>>>) -> Response {
    let state_guard = state.read().await;
    let dir = self_update_dir(&state_guard);
    drop(state_guard);

    let (versions, _) = match scan_versions_cached(&dir).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    let latest = versions
        .last()
        .expect("scan_versions guarantees non-empty")
        .clone();

    let response = VersionsResponse { versions, latest };

    let json = match super::serialize_json(&response, "self-update versions") {
        Ok(j) => j,
        Err(e) => return e,
    };

    super::json_response(json, 300)
}

/// GET /v1/ccs/conary/:version/download
///
/// Streams the CCS package file for the requested version.
pub async fn download(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(version): Path<String>,
) -> Response {
    // Validate version string (prevent path traversal)
    if let Err(e) = super::validate_name(&version) {
        return e;
    }

    // Additional validation: must be a valid semver triple
    if !is_valid_semver_triple(&version) {
        return (StatusCode::BAD_REQUEST, "Invalid version format").into_response();
    }

    let state_guard = state.read().await;
    let dir = self_update_dir(&state_guard);
    drop(state_guard);

    let ccs_path = dir.join(format!("conary-{version}.ccs"));

    if !ccs_path.exists() {
        return (StatusCode::NOT_FOUND, "Version not found").into_response();
    }

    // Open file for streaming
    let file = match tokio::fs::File::open(&ccs_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Failed to open CCS package {}: {}", ccs_path.display(), e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read package").into_response();
        }
    };

    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to get metadata for {}: {}", ccs_path.display(), e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read package").into_response();
        }
    };

    let filename = format!("conary-{version}.ccs");
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CACHE_CONTROL, "public, max-age=86400, immutable")
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{ServerConfig, ServerState};
    use axum::body::to_bytes;
    use axum::extract::{Path as AxumPath, State as AxumState};
    use axum::http::StatusCode;
    use std::fs;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    async fn test_state(root: &tempfile::TempDir) -> Arc<RwLock<ServerState>> {
        let config = ServerConfig {
            db_path: root.path().join("remi.db"),
            chunk_dir: root.path().join("chunks"),
            cache_dir: root.path().join("cache"),
            ..Default::default()
        };

        fs::create_dir_all(&config.chunk_dir).unwrap();
        fs::create_dir_all(&config.cache_dir).unwrap();
        conary_core::db::init(&config.db_path).unwrap();

        Arc::new(RwLock::new(ServerState::new(config).unwrap()))
    }

    async fn clear_versions_cache() {
        *VERSIONS_CACHE.lock().await = None;
    }

    #[test]
    fn test_parse_semver() {
        assert_eq!(parse_semver("0.1.0"), (0, 1, 0));
        assert_eq!(parse_semver("1.2.3"), (1, 2, 3));
        assert_eq!(parse_semver("10.20.30"), (10, 20, 30));
        assert_eq!(parse_semver("invalid"), (0, 0, 0));
    }

    #[test]
    fn test_parse_semver_ordering() {
        assert!(parse_semver("0.2.0") > parse_semver("0.1.9"));
        assert!(parse_semver("1.0.0") > parse_semver("0.99.99"));
        assert!(parse_semver("0.1.1") > parse_semver("0.1.0"));
    }

    #[test]
    fn test_scan_versions_missing_dir() {
        let dir = PathBuf::from("/tmp/conary-test-nonexistent-self-update-dir");
        let result = scan_versions_and_hash(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_versions_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = scan_versions_and_hash(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_versions_with_packages() {
        let dir = tempfile::tempdir().unwrap();
        // Create some fake CCS packages
        fs::write(dir.path().join("conary-0.1.0.ccs"), b"fake").unwrap();
        fs::write(dir.path().join("conary-0.2.0.ccs"), b"latest-pkg").unwrap();
        fs::write(dir.path().join("conary-0.1.5.ccs"), b"fake").unwrap();
        // Non-matching files should be ignored
        fs::write(dir.path().join("readme.txt"), b"ignored").unwrap();
        fs::write(dir.path().join("conary-beta.ccs"), b"ignored").unwrap();

        let (versions, latest_hash) = scan_versions_and_hash(dir.path()).unwrap();
        assert_eq!(versions, vec!["0.1.0", "0.1.5", "0.2.0"]);

        // Hash should be computed for the latest version (0.2.0)
        let (sha256, size, signature) = latest_hash.expect("latest hash should be present");
        assert_eq!(size, b"latest-pkg".len() as u64);
        assert_eq!(sha256, conary_core::hash::sha256(b"latest-pkg"));
        assert!(signature.is_none(), "no .sig file was created");
    }

    #[tokio::test]
    async fn test_get_version_info_returns_requested_version_metadata() {
        clear_versions_cache().await;
        let temp = tempfile::tempdir().unwrap();
        let self_update_dir = temp.path().join("self-update");
        fs::create_dir_all(&self_update_dir).unwrap();
        fs::write(
            self_update_dir.join("conary-1.2.3.ccs"),
            b"requested-version",
        )
        .unwrap();

        let state = test_state(&temp).await;
        let response = get_version_info(AxumState(state), AxumPath("1.2.3".to_string())).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["version"], "1.2.3");
        assert_eq!(json["download_url"], "/v1/ccs/conary/1.2.3/download");
        assert_eq!(
            json["sha256"],
            conary_core::hash::sha256(b"requested-version")
        );
    }

    #[tokio::test]
    async fn test_get_version_info_returns_404_for_missing_version() {
        clear_versions_cache().await;
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("self-update")).unwrap();

        let state = test_state(&temp).await;
        let response = get_version_info(AxumState(state), AxumPath("9.9.9".to_string())).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
