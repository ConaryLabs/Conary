// src/server/handlers/packages.rs
//! Package metadata endpoint - triggers on-demand conversion
//!
//! When a client requests package metadata:
//! - If already converted: return manifest immediately
//! - If not converted: return 202 Accepted with job ID for polling

use crate::server::jobs::{JobId, JobStatus};
use crate::server::ServerState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Query parameters for package requests
#[derive(Debug, Deserialize)]
pub struct PackageQuery {
    /// Specific version to fetch (optional)
    pub version: Option<String>,
}

/// Response when package is ready
#[derive(Serialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub distro: String,
    /// List of chunk hashes that make up this package
    pub chunks: Vec<ChunkRef>,
    /// Total size when reassembled
    pub total_size: u64,
    /// SHA-256 of the complete reassembled content
    pub content_hash: String,
}

#[derive(Serialize)]
pub struct ChunkRef {
    pub hash: String,
    pub size: u64,
    pub offset: u64,
}

/// Response when conversion is in progress (202 Accepted)
#[derive(Serialize)]
pub struct ConversionAccepted {
    pub status: &'static str,
    pub job_id: String,
    pub poll_url: String,
    /// Estimated seconds until ready (if known)
    pub eta_seconds: Option<u32>,
}

/// GET /v1/:distro/packages/:name
///
/// Returns package metadata and chunk list.
/// If package needs conversion, returns 202 Accepted with job ID.
pub async fn get_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
    Query(query): Query<PackageQuery>,
) -> Response {
    let state_guard = state.read().await;

    // Validate distro
    if !["arch", "fedora", "ubuntu", "debian"].contains(&distro.as_str()) {
        return (StatusCode::BAD_REQUEST, "Unknown distribution").into_response();
    }

    // Check if package is already converted
    let db_path = &state_guard.config.db_path;
    let converted = match check_converted(db_path, &distro, &name, query.version.as_deref()) {
        Ok(Some(manifest)) => return Json(manifest).into_response(),
        Ok(None) => false,
        Err(e) => {
            tracing::error!("Database error checking conversion: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    if converted {
        unreachable!("Already returned above");
    }

    // Package not converted - check if conversion is already in progress
    let job_key = format!("{}:{}:{}", distro, name, query.version.as_deref().unwrap_or("latest"));

    if let Some(existing_job) = state_guard.job_manager.get_job_by_key(&job_key) {
        // Return existing job ID
        return (
            StatusCode::ACCEPTED,
            Json(ConversionAccepted {
                status: "converting",
                job_id: existing_job.to_string(),
                poll_url: format!("/v1/jobs/{}", existing_job),
                eta_seconds: None, // TODO: estimate based on package size
            }),
        )
            .into_response();
    }

    // Start new conversion job
    drop(state_guard); // Release read lock before acquiring write
    let mut state_guard = state.write().await;

    match state_guard.job_manager.create_job(
        job_key.clone(),
        distro.clone(),
        name.clone(),
        query.version.clone(),
    ) {
        Ok(job_id) => {
            // Spawn conversion task
            let state_clone = state.clone();
            tokio::spawn(async move {
                run_conversion(state_clone, job_id).await;
            });

            (
                StatusCode::ACCEPTED,
                Json(ConversionAccepted {
                    status: "queued",
                    job_id: job_id.to_string(),
                    poll_url: format!("/v1/jobs/{}", job_id),
                    eta_seconds: Some(30), // Default estimate
                }),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to create conversion job: {}", e);
            (StatusCode::SERVICE_UNAVAILABLE, "Conversion queue full").into_response()
        }
    }
}

/// Check if a package has already been converted
///
/// Queries the converted_packages table for a matching distro/name/version.
/// Returns the manifest if found and the CCS file still exists.
fn check_converted(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    version: Option<&str>,
) -> Result<Option<PackageManifest>, anyhow::Error> {
    use crate::db::models::ConvertedPackage;

    // Open database connection
    let conn = crate::db::open(db_path)?;

    // Query for existing conversion
    let existing = ConvertedPackage::find_by_package_identity(&conn, distro, name, version)?;

    if let Some(converted) = existing {
        // Check if the CCS file still exists
        if let Some(ccs_path_str) = &converted.ccs_path {
            let ccs_path = std::path::Path::new(ccs_path_str);
            if ccs_path.exists() {
                // Build manifest from stored data
                let chunk_hashes: Vec<String> = converted
                    .chunk_hashes_json
                    .as_ref()
                    .and_then(|json| serde_json::from_str(json).ok())
                    .unwrap_or_default();

                let chunks: Vec<ChunkRef> = chunk_hashes
                    .iter()
                    .enumerate()
                    .map(|(i, hash)| ChunkRef {
                        hash: hash.clone(),
                        size: 0, // Size per chunk not stored, use 0
                        offset: i as u64,
                    })
                    .collect();

                return Ok(Some(PackageManifest {
                    name: converted.package_name.unwrap_or_else(|| name.to_string()),
                    version: converted.package_version.unwrap_or_else(|| "unknown".to_string()),
                    distro: converted.distro.unwrap_or_else(|| distro.to_string()),
                    chunks,
                    total_size: converted.total_size.unwrap_or(0) as u64,
                    content_hash: converted.content_hash.unwrap_or_default(),
                }));
            }
        }
    }

    Ok(None)
}

/// Run the actual conversion in a background task
async fn run_conversion(state: Arc<RwLock<ServerState>>, job_id: JobId) {
    let (job, conversion_service) = {
        let state_guard = state.read().await;
        let job = match state_guard.job_manager.get_job(&job_id) {
            Some(j) => j.clone(),
            None => {
                tracing::error!("Job {} not found", job_id);
                return;
            }
        };
        // Clone the conversion service config for use outside the lock
        let svc = crate::server::ConversionService::new(
            state_guard.config.chunk_dir.clone(),
            state_guard.config.cache_dir.clone(),
            state_guard.config.db_path.clone(),
        );
        (job, svc)
    };

    tracing::info!(
        "Starting conversion: {}:{} (job {})",
        job.distro,
        job.package_name,
        job_id
    );

    // Update status to converting
    {
        let mut state_guard = state.write().await;
        state_guard.job_manager.update_status(&job_id, JobStatus::Converting);
    }

    // Run the actual conversion
    let result = conversion_service
        .convert_package(&job.distro, &job.package_name, job.version.as_deref())
        .await;

    // Update job status based on result
    {
        let mut state_guard = state.write().await;
        match result {
            Ok(conversion_result) => {
                tracing::info!(
                    "Conversion complete: {}:{} -> {} chunks (job {})",
                    job.distro,
                    job.package_name,
                    conversion_result.chunk_hashes.len(),
                    job_id
                );
                // Store result with job for later retrieval
                let job_result = crate::server::jobs::ConversionResult {
                    chunk_hashes: conversion_result.chunk_hashes,
                    total_size: conversion_result.total_size,
                    content_hash: conversion_result.content_hash,
                    ccs_path: conversion_result.ccs_path,
                    actual_version: conversion_result.version,
                };
                state_guard.job_manager.complete_with_result(&job_id, job_result);
            }
            Err(e) => {
                tracing::error!(
                    "Conversion failed: {}:{} - {} (job {})",
                    job.distro,
                    job.package_name,
                    e,
                    job_id
                );
                state_guard.job_manager.update_status(
                    &job_id,
                    JobStatus::Failed(e.to_string()),
                );
            }
        }
    }
}

/// GET /v1/:distro/packages/:name/download
///
/// Download the complete CCS package file. Returns:
/// - 200 OK with CCS package data
/// - 202 Accepted if conversion still in progress
/// - 404 Not Found if package doesn't exist or hasn't been converted
pub async fn download_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
    Query(query): Query<PackageQuery>,
) -> Response {
    let state_guard = state.read().await;

    // Validate distro
    if !["arch", "fedora", "ubuntu", "debian"].contains(&distro.as_str()) {
        return (StatusCode::BAD_REQUEST, "Unknown distribution").into_response();
    }

    // Check for conversion job (in-progress or completed)
    let job_key = format!("{}:{}:{}", distro, name, query.version.as_deref().unwrap_or("latest"));
    let job_info = state_guard.job_manager.get_job_by_key(&job_key)
        .and_then(|id| state_guard.job_manager.get_job(&id).map(|j| (id, j.clone())));

    // If job exists, check its status
    if let Some((job_id, job)) = &job_info {
        match &job.status {
            crate::server::jobs::JobStatus::Ready => {
                // Job completed - use the CCS path from the result
                if let Some(result) = &job.result {
                    if result.ccs_path.exists() {
                        // Use the path from the job result (guaranteed to be correct)
                        let ccs_path = result.ccs_path.clone();
                        drop(state_guard);
                        return stream_ccs_file(ccs_path).await;
                    }
                }
                // Result missing or file deleted - fall through to filesystem lookup
            }
            crate::server::jobs::JobStatus::Failed(error) => {
                // Conversion failed - return error
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Conversion failed: {}", error),
                ).into_response();
            }
            _ => {
                // Still converting/pending - return 202
                return (
                    StatusCode::ACCEPTED,
                    Json(ConversionAccepted {
                        status: "converting",
                        job_id: job_id.to_string(),
                        poll_url: format!("/v1/jobs/{}", job_id),
                        eta_seconds: None,
                    }),
                ).into_response();
            }
        }
    }

    // No job found - look for the CCS package file on disk
    // The conversion service stores it at: {cache_dir}/packages/{name}-{version}.ccs
    let packages_dir = state_guard.config.cache_dir.join("packages");

    // If version specified, look for exact match
    // Otherwise, find the latest version
    let ccs_path = if let Some(version) = &query.version {
        packages_dir.join(format!("{}-{}.ccs", name, version))
    } else {
        // Find any matching package (glob for {name}-*.ccs)
        match find_latest_package(&packages_dir, &name) {
            Some(path) => path,
            None => {
                // No converted package found - trigger conversion
                drop(state_guard);
                return get_package(
                    State(state),
                    Path((distro, name)),
                    Query(query),
                )
                .await;
            }
        }
    };

    if !ccs_path.exists() {
        // No converted package found - trigger conversion
        drop(state_guard);
        return get_package(
            State(state),
            Path((distro, name)),
            Query(query),
        )
        .await;
    }

    drop(state_guard);

    // Use helper function to stream the file
    stream_ccs_file(ccs_path).await
}

/// Find the latest version of a package in the packages directory
fn find_latest_package(packages_dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let prefix = format!("{}-", name);

    std::fs::read_dir(packages_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_name().to_str()
                .map(|n| n.starts_with(&prefix) && n.ends_with(".ccs"))
                .unwrap_or(false)
        })
        .max_by_key(|entry| entry.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|entry| entry.path())
}

/// Stream a CCS file as a response
async fn stream_ccs_file(ccs_path: std::path::PathBuf) -> Response {
    use axum::body::Body;
    use axum::http::header;
    use tokio::fs::File;
    use tokio_util::io::ReaderStream;

    // Open file for streaming
    let file = match File::open(&ccs_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Failed to open CCS package {}: {}", ccs_path.display(), e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read package").into_response();
        }
    };

    // Get file size for Content-Length
    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to get CCS package metadata {}: {}", ccs_path.display(), e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read package").into_response();
        }
    };

    let filename = ccs_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("package.ccs");

    tracing::info!("Serving CCS package: {} ({} bytes)", filename, metadata.len());

    // Stream the file
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename))
        // CCS packages are versioned but can be re-converted, so moderate caching
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(body)
        .unwrap()
}

/// POST /v1/admin/convert
///
/// Manually trigger conversion of a package (admin endpoint)
#[derive(Deserialize)]
pub struct ConvertRequest {
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
}

pub async fn trigger_conversion(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<ConvertRequest>,
) -> Response {
    // Reuse the get_package logic
    let query = PackageQuery { version: req.version };
    get_package(
        State(state),
        Path((req.distro, req.package)),
        Query(query),
    )
    .await
}
