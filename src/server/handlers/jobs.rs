// src/server/handlers/jobs.rs
//! Job status endpoint for 202 Accepted polling

use crate::server::jobs::JobStatus;
use crate::server::ServerState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Response for job status queries
#[derive(Serialize)]
pub struct JobStatusResponse {
    pub job_id: String,
    pub status: String,
    /// Package info
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    /// Progress percentage (0-100) if available
    pub progress: Option<u8>,
    /// Error message if failed
    pub error: Option<String>,
    /// Manifest data if ready
    pub manifest: Option<serde_json::Value>,
}

/// GET /v1/jobs/:job_id
///
/// Poll conversion job status. Returns:
/// - 200 OK with status (pending, converting, ready, failed)
/// - 404 Not Found if job doesn't exist or expired
pub async fn get_job_status(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(job_id): Path<String>,
) -> Response {
    let state = state.read().await;

    // Parse job ID
    let job_id = match job_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Invalid job ID format").into_response();
        }
    };

    // Look up job
    let job = match state.job_manager.get_job(&job_id) {
        Some(j) => j.clone(),
        None => {
            return (StatusCode::NOT_FOUND, "Job not found or expired").into_response();
        }
    };

    let status_str = match job.status {
        JobStatus::Pending => "pending",
        JobStatus::Converting => "converting",
        JobStatus::Ready => "ready",
        JobStatus::Failed(_) => "failed",
    };

    let error = match &job.status {
        JobStatus::Failed(msg) => Some(msg.clone()),
        _ => None,
    };

    // Include manifest if ready and result is available
    let manifest = if matches!(job.status, JobStatus::Ready) {
        job.result.as_ref().map(|r| {
            serde_json::json!({
                "name": job.package_name,
                "version": job.version.as_deref().unwrap_or("latest"),
                "distro": job.distro,
                "chunks": r.chunk_hashes.iter().enumerate().map(|(i, hash)| {
                    serde_json::json!({
                        "hash": hash,
                        "size": 0,  // Size not tracked per-chunk yet
                        "offset": i
                    })
                }).collect::<Vec<_>>(),
                "total_size": r.total_size,
                "content_hash": r.content_hash
            })
        })
    } else {
        None
    };

    Json(JobStatusResponse {
        job_id: job_id.to_string(),
        status: status_str.to_string(),
        distro: job.distro,
        package: job.package_name,
        version: job.version,
        progress: job.progress,
        error,
        manifest,
    })
    .into_response()
}
