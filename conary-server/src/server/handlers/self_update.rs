// conary-server/src/server/handlers/self_update.rs
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

/// Cached scan_versions result with expiry timestamp.
static VERSIONS_CACHE: std::sync::LazyLock<Mutex<Option<(Instant, Vec<String>)>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Response for GET /v1/ccs/conary/latest
#[derive(Serialize)]
pub struct LatestResponse {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
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

/// Scan the self-update directory and return sorted (ascending) version strings.
#[allow(clippy::result_large_err)]
fn scan_versions(dir: &PathBuf) -> Result<Vec<String>, Response> {
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
    Ok(versions)
}

/// Return cached versions or re-scan if the cache has expired.
#[allow(clippy::result_large_err)]
async fn scan_versions_cached(dir: &PathBuf) -> Result<Vec<String>, Response> {
    let ttl = std::time::Duration::from_secs(VERSIONS_CACHE_TTL_SECS);

    {
        let cache = VERSIONS_CACHE.lock().await;
        if let Some((instant, ref versions)) = *cache {
            if instant.elapsed() < ttl {
                return Ok(versions.clone());
            }
        }
    }

    // Cache miss or expired -- rescan
    let versions = scan_versions(dir)?;

    {
        let mut cache = VERSIONS_CACHE.lock().await;
        *cache = Some((Instant::now(), versions.clone()));
    }

    Ok(versions)
}

/// GET /v1/ccs/conary/latest
///
/// Returns metadata about the latest available Conary self-update package.
pub async fn get_latest(State(state): State<Arc<RwLock<ServerState>>>) -> Response {
    let state_guard = state.read().await;
    let dir = self_update_dir(&state_guard);
    drop(state_guard);

    let versions = match scan_versions_cached(&dir).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    let latest = versions.last().expect("scan_versions guarantees non-empty");
    let ccs_path = dir.join(format!("conary-{latest}.ccs"));

    // Read file to compute sha256 and size (use tokio::fs to avoid blocking async runtime)
    let data = match tokio::fs::read(&ccs_path).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to read CCS package {}: {}", ccs_path.display(), e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read package").into_response();
        }
    };

    let sha256 = conary_core::hash::sha256(&data);
    let size = data.len() as u64;

    let response = LatestResponse {
        version: latest.clone(),
        download_url: format!("/v1/ccs/conary/{latest}/download"),
        sha256,
        size,
    };

    let json = match super::serialize_json(&response, "self-update latest") {
        Ok(j) => j,
        Err(e) => return e,
    };

    // Cache for 5 minutes -- clients should not hammer this endpoint
    super::json_response(json, 300)
}

/// GET /v1/ccs/conary/versions
///
/// Returns a list of all available self-update versions.
pub async fn get_versions(State(state): State<Arc<RwLock<ServerState>>>) -> Response {
    let state_guard = state.read().await;
    let dir = self_update_dir(&state_guard);
    drop(state_guard);

    let versions = match scan_versions_cached(&dir).await {
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
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 || !parts.iter().all(|p| p.parse::<u64>().is_ok()) {
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
    use std::fs;

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
        let result = scan_versions(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_versions_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = scan_versions(&dir.path().to_path_buf());
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_versions_with_packages() {
        let dir = tempfile::tempdir().unwrap();
        // Create some fake CCS packages
        fs::write(dir.path().join("conary-0.1.0.ccs"), b"fake").unwrap();
        fs::write(dir.path().join("conary-0.2.0.ccs"), b"fake").unwrap();
        fs::write(dir.path().join("conary-0.1.5.ccs"), b"fake").unwrap();
        // Non-matching files should be ignored
        fs::write(dir.path().join("readme.txt"), b"ignored").unwrap();
        fs::write(dir.path().join("conary-beta.ccs"), b"ignored").unwrap();

        let versions = scan_versions(&dir.path().to_path_buf()).unwrap();
        assert_eq!(versions, vec!["0.1.0", "0.1.5", "0.2.0"]);
    }
}
