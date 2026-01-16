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
            let job_id_clone = job_id.clone();
            tokio::spawn(async move {
                run_conversion(state_clone, job_id_clone).await;
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
fn check_converted(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    version: Option<&str>,
) -> Result<Option<PackageManifest>, anyhow::Error> {
    // TODO: Query converted_packages table
    // For now, return None (not converted)
    let _ = (db_path, distro, name, version);
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
