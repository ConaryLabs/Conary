// apps/remi/src/server/handlers/packages.rs
//! Package metadata endpoint - triggers on-demand conversion
//!
//! When a client requests package metadata:
//! - If already converted: return manifest immediately
//! - If not converted: return 202 Accepted with job ID for polling

use crate::server::ServerState;
use crate::server::conversion::ScriptletPackageMetadata;
use crate::server::jobs::{ConversionJob, JobId, JobStatus};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

mod serving;
#[cfg(test)]
use serving::check_converted;
use serving::{
    ConvertedDownloadLookup, ConvertedManifestLookup, active_profile_revision_for_request,
    converted_ccs_path_for_download, converted_manifest_for_request,
};

/// Query parameters for package requests
#[derive(Debug, Deserialize)]
pub struct PackageQuery {
    /// Specific version to fetch (optional)
    pub version: Option<String>,
    /// Specific native package release to fetch (optional)
    pub release: Option<String>,
    /// Specific native package architecture to fetch (optional)
    #[serde(alias = "architecture")]
    pub arch: Option<String>,
}

/// Response when package is ready
#[derive(Debug, Serialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub distro: String,
    /// Exact signed CCS control documents and canonical object set.
    pub transport: conary_core::ccs::CcsTransportEnvelopeV1,
    /// Total size when reassembled
    pub total_size: u64,
    /// SHA-256 of the complete reassembled content
    pub content_hash: String,
    pub native: bool,
    pub converted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    /// Passive native lifecycle metadata summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scriptlets: Option<ScriptletPackageMetadata>,
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

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RepositoryNotReadyReason {
    PublicationPending,
    ProfileEmpty,
    PopulationProbeUnavailable,
}

#[derive(Serialize)]
struct RepositoryNotReady {
    code: &'static str,
    profile: String,
    reason: RepositoryNotReadyReason,
    publication: crate::server::readiness::PublicationReadiness,
    retry_after_seconds: u32,
}

fn repository_not_ready_response(
    profile: &str,
    reason: RepositoryNotReadyReason,
    publication: crate::server::readiness::PublicationReadiness,
) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(RepositoryNotReady {
            code: "REPOSITORY_NOT_READY",
            profile: profile.to_string(),
            reason,
            publication,
            retry_after_seconds: 30,
        }),
    )
        .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("30"),
    );
    response
}

async fn repository_not_ready_after_cache_miss(
    state: &Arc<RwLock<ServerState>>,
    route: &str,
) -> Option<Response> {
    let profile = conary_core::repository::supported_profiles::profile_for_remi_route(route)?;
    let (publication, catalog_authority) = {
        let state = state.read().await;
        (
            state.publication_readiness.clone(),
            state.catalog_authority.clone(),
        )
    };
    if !publication.is_ready() {
        return Some(repository_not_ready_response(
            profile.id(),
            RepositoryNotReadyReason::PublicationPending,
            publication,
        ));
    }

    let profile_id = profile.id().to_string();
    match tokio::task::spawn_blocking(move || {
        crate::server::readiness::source_profile_is_populated(&catalog_authority, &profile_id)
    })
    .await
    {
        Ok(Ok(true)) => None,
        Ok(Ok(false)) => Some(repository_not_ready_response(
            profile.id(),
            RepositoryNotReadyReason::ProfileEmpty,
            publication,
        )),
        Ok(Err(error)) => {
            tracing::warn!(
                profile = profile.id(),
                "Repository readiness probe failed: {error}"
            );
            Some(repository_not_ready_response(
                profile.id(),
                RepositoryNotReadyReason::PopulationProbeUnavailable,
                publication,
            ))
        }
        Err(error) => {
            tracing::warn!(
                profile = profile.id(),
                "Repository readiness task failed: {error}"
            );
            Some(repository_not_ready_response(
                profile.id(),
                RepositoryNotReadyReason::PopulationProbeUnavailable,
                publication,
            ))
        }
    }
}

async fn start_conversion_when_repository_ready(
    state: Arc<RwLock<ServerState>>,
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    query: &PackageQuery,
) -> Response {
    let coordinator = state.read().await.publication_coordinator.clone();
    let _publication_guard = coordinator.lock_owned().await;
    if let Some(response) = repository_not_ready_after_cache_miss(&state, distro).await {
        return response;
    }

    start_conversion_after_cache_miss(state, db_path, distro, name, query).await
}

fn active_conversion_response(job: &ConversionJob, eta_seconds: Option<u32>) -> Response {
    let status = match job.status {
        JobStatus::Pending => "pending",
        JobStatus::Converting => "converting",
        JobStatus::Ready | JobStatus::Failed(_) | JobStatus::Cancelled(_) => {
            unreachable!("terminal conversion jobs cannot own package-key deduplication")
        }
    };
    (
        StatusCode::ACCEPTED,
        Json(ConversionAccepted {
            status,
            job_id: job.id.to_string(),
            poll_url: format!("/v1/jobs/{}", job.id),
            eta_seconds,
        }),
    )
        .into_response()
}

fn conversion_job_key(distro: &str, name: &str, query: &PackageQuery) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        distro,
        name,
        query.version.as_deref().unwrap_or("latest"),
        query.release.as_deref().unwrap_or("default"),
        query.arch.as_deref().unwrap_or("default")
    )
}

async fn start_conversion_after_cache_miss(
    state: Arc<RwLock<ServerState>>,
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    query: &PackageQuery,
) -> Response {
    let job_key = conversion_job_key(distro, name, query);
    let mut state_guard = state.write().await;
    if let Some(existing_job) = state_guard.job_manager.get_active_job_by_key(&job_key) {
        return active_conversion_response(existing_job, None);
    }

    // Keep job-key coordination stable across the final persisted lookup. A
    // conversion can commit after the caller's first cache miss, and its ready
    // polling record can then be evicted. Holding this lock makes the lookup
    // and any replacement reservation one atomic decision relative to job
    // creation, completion, and cleanup.
    let catalog_authority = state_guard.catalog_authority.clone();
    match converted_manifest_for_request(db_path, &catalog_authority, distro, name, query).await {
        Ok(ConvertedManifestLookup::Ready(manifest)) => {
            return Json(manifest).into_response();
        }
        Ok(ConvertedManifestLookup::Missing) => {}
        Err(error) => return error.into_response(),
    }

    match state_guard.job_manager.create_job(
        job_key,
        distro.to_string(),
        name.to_string(),
        query.version.clone(),
        query.arch.clone(),
    ) {
        Ok(job_id) => {
            let state_clone = Arc::clone(&state);
            tokio::spawn(async move {
                run_conversion(state_clone, job_id).await;
            });

            (
                StatusCode::ACCEPTED,
                Json(ConversionAccepted {
                    status: "pending",
                    job_id: job_id.to_string(),
                    poll_url: format!("/v1/jobs/{}", job_id),
                    eta_seconds: Some(30),
                }),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!("Failed to create conversion job: {}", error);
            (StatusCode::SERVICE_UNAVAILABLE, "Conversion queue full").into_response()
        }
    }
}

fn native_ambiguity_response(releases: Vec<String>) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "code": "NATIVE_RELEASE_AMBIGUOUS",
            "error": "multiple native releases match package request",
            "releases": releases,
        })),
    )
        .into_response()
}

fn manifest_from_native_publication(
    native: conary_core::db::models::NativePackagePublication,
    route_slug: &str,
) -> anyhow::Result<PackageManifest> {
    let transport = serde_json::from_str(&native.transport_json)?;
    Ok(PackageManifest {
        name: native.name,
        version: native.version,
        release: Some(native.package_release),
        distro: route_slug.to_string(),
        transport,
        total_size: native.total_size as u64,
        content_hash: native.content_hash,
        native: true,
        converted: false,
        source_kind: Some("native-ccs".to_string()),
        scriptlets: None,
    })
}

fn native_manifest_lookup(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    version: Option<&str>,
    release: Option<&str>,
    architecture: Option<&str>,
) -> anyhow::Result<crate::server::native_publish::public_lookup::NativeLookup<PackageManifest>> {
    use crate::server::native_publish::public_lookup::{
        NativeLookup, resolve_active_native_publication,
    };

    match resolve_active_native_publication(db_path, distro, name, version, release, architecture)?
    {
        NativeLookup::Ready(native) => Ok(NativeLookup::Ready(manifest_from_native_publication(
            native, distro,
        )?)),
        NativeLookup::Ambiguous(releases) => Ok(NativeLookup::Ambiguous(releases)),
        NativeLookup::Missing => Ok(NativeLookup::Missing),
    }
}

#[cfg(test)]
fn native_manifest_for_package(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    version: Option<&str>,
    release: Option<&str>,
    architecture: Option<&str>,
) -> anyhow::Result<Option<PackageManifest>> {
    use crate::server::native_publish::public_lookup::NativeLookup;

    match native_manifest_lookup(db_path, distro, name, version, release, architecture)? {
        NativeLookup::Ready(manifest) => Ok(Some(manifest)),
        NativeLookup::Ambiguous(releases) => anyhow::bail!(
            "multiple native releases match package request: {}",
            releases.join(", ")
        ),
        NativeLookup::Missing => Ok(None),
    }
}

/// GET /v1/:distro/packages/:name
///
/// Returns package metadata and its authenticated CCS transport descriptor.
/// If package needs conversion, returns 202 Accepted with job ID.
pub async fn get_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
    Query(query): Query<PackageQuery>,
) -> Response {
    // Validate path parameters
    if let Err(e) = super::validate_distro_and_name(&distro, &name) {
        return e;
    }

    let (db_path, catalog_authority) = {
        let state_guard = state.read().await;
        (
            state_guard.config.db_path.clone(),
            state_guard.catalog_authority.clone(),
        )
    };

    // Check if package is already converted (use spawn_blocking to avoid blocking
    // the async runtime with synchronous SQLite I/O)
    let native_db = db_path.clone();
    let native_route = distro.clone();
    let native_name = name.clone();
    let native_version = query.version.clone();
    let native_release = query.release.clone();
    let native_arch = query.arch.clone();
    match tokio::task::spawn_blocking(move || {
        native_manifest_lookup(
            &native_db,
            &native_route,
            &native_name,
            native_version.as_deref(),
            native_release.as_deref(),
            native_arch.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(crate::server::native_publish::public_lookup::NativeLookup::Ready(manifest))) => {
            return Json(manifest).into_response();
        }
        Ok(Ok(crate::server::native_publish::public_lookup::NativeLookup::Ambiguous(releases))) => {
            return native_ambiguity_response(releases);
        }
        Ok(Ok(crate::server::native_publish::public_lookup::NativeLookup::Missing)) => {}
        Ok(Err(e)) => {
            tracing::error!("Database error checking native publication: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    }

    // Check if package is already converted (use spawn_blocking to avoid blocking
    // the async runtime with synchronous SQLite I/O)
    match converted_manifest_for_request(&db_path, &catalog_authority, &distro, &name, &query).await
    {
        Ok(ConvertedManifestLookup::Ready(manifest)) => return Json(manifest).into_response(),
        Ok(ConvertedManifestLookup::Missing) => {}
        Err(error) => return error.into_response(),
    }

    let job_key = conversion_job_key(&distro, &name, &query);
    if let Some(job) = state
        .read()
        .await
        .job_manager
        .get_active_job_by_key(&job_key)
        .cloned()
    {
        return active_conversion_response(&job, None);
    }

    start_conversion_when_repository_ready(state, &db_path, &distro, &name, &query).await
}

/// Run the actual conversion in a background task
async fn run_conversion(state: Arc<RwLock<ServerState>>, job_id: JobId) {
    // Acquire a semaphore permit to limit concurrent conversions.
    // This prevents unbounded parallelism when many conversion requests
    // arrive simultaneously. Clone the Arc<Semaphore> so the permit can
    // outlive the RwLock guard.
    let semaphore = {
        let state_guard = state.read().await;
        state_guard.job_manager.semaphore()
    };
    let _permit = match semaphore.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            tracing::error!("Conversion semaphore closed (job {})", job_id);
            let mut state_guard = state.write().await;
            state_guard.job_manager.update_status(
                &job_id,
                JobStatus::Cancelled("Conversion worker shut down".to_string()),
            );
            return;
        }
    };

    let (job, conversion_service) = {
        let state_guard = state.read().await;
        let job = match state_guard.job_manager.get_job(&job_id) {
            Some(j) => j.clone(),
            None => {
                tracing::error!("Job {} not found", job_id);
                return;
            }
        };
        // Clone the state-owned service so requests and prewarm work retain
        // the same conversion database writer.
        let svc = state_guard.conversion_service.clone();
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
        state_guard
            .job_manager
            .update_status(&job_id, JobStatus::Converting);
    }

    let distro = job.distro.clone();
    let package_name = job.package_name.clone();
    let version = job.version.clone();
    let architecture = job.architecture.clone();
    let result = conversion_service
        .convert_package_async(
            &distro,
            &package_name,
            version.as_deref(),
            architecture.as_deref(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("conversion failed: {e}"));

    // Update job status based on result
    {
        let mut state_guard = state.write().await;
        match result {
            Ok(outcome) => {
                let conversion_result = outcome;
                tracing::info!(
                    "Conversion complete: {}:{} -> {} signed objects (job {})",
                    job.distro,
                    job.package_name,
                    conversion_result.transport.objects.len(),
                    job_id
                );
                // Store result with job for later retrieval
                let job_result = crate::server::jobs::ConversionResult {
                    transport: conversion_result.transport,
                    total_size: conversion_result.total_size,
                    content_hash: conversion_result.content_hash,
                    ccs_path: conversion_result.ccs_path,
                    actual_version: conversion_result.version,
                    scriptlets: conversion_result.scriptlets,
                };
                state_guard
                    .job_manager
                    .complete_with_result(&job_id, job_result);
            }
            Err(e) => {
                tracing::error!(
                    "Conversion failed: {}:{} - {} (job {})",
                    job.distro,
                    job.package_name,
                    e,
                    job_id
                );
                state_guard
                    .job_manager
                    .update_status(&job_id, JobStatus::Failed(e.to_string()));
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
    headers: axum::http::HeaderMap,
) -> Response {
    // Validate path parameters
    if let Err(e) = super::validate_distro_and_name(&distro, &name) {
        return e;
    }

    let (native_db_path, native_analytics) = {
        let state_guard = state.read().await;
        (
            state_guard.config.db_path.clone(),
            state_guard.analytics.clone(),
        )
    };
    let native_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let native_lookup_db = native_db_path.clone();
    let native_lookup_distro = distro.clone();
    let native_lookup_name = name.clone();
    let native_lookup_version = query.version.clone();
    let native_lookup_release = query.release.clone();
    let native_lookup_arch = query.arch.clone();
    match tokio::task::spawn_blocking(move || {
        crate::server::native_publish::public_lookup::resolve_active_native_publication(
            &native_lookup_db,
            &native_lookup_distro,
            &native_lookup_name,
            native_lookup_version.as_deref(),
            native_lookup_release.as_deref(),
            native_lookup_arch.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(crate::server::native_publish::public_lookup::NativeLookup::Ready(native))) => {
            return stream_ccs_file(
                std::path::PathBuf::from(native.package_path),
                native_analytics,
                &distro,
                &name,
                query.version.as_deref(),
                query.arch.as_deref(),
                native_ua.as_deref(),
            )
            .await;
        }
        Ok(Ok(crate::server::native_publish::public_lookup::NativeLookup::Ambiguous(releases))) => {
            return native_ambiguity_response(releases);
        }
        Ok(Ok(crate::server::native_publish::public_lookup::NativeLookup::Missing)) => {}
        Ok(Err(e)) => {
            tracing::error!("Database error checking native publication download: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    }

    let state_guard = state.read().await;

    // Only active jobs own request deduplication. Completed artifacts are
    // validated against persisted repository authority below; failed work may
    // be retried through `get_package`.
    let job_key = conversion_job_key(&distro, &name, &query);
    let job_info = state_guard
        .job_manager
        .get_active_job_by_key(&job_key)
        .cloned();

    if let Some(job) = &job_info {
        return active_conversion_response(job, None);
    }

    // No job found: look up the converted package through the DB instead of
    // trusting a cache filename. Conversion-version bumps make old CCS files
    // stale even when the bytes are still on disk.
    let db_path = state_guard.config.db_path.clone();
    let catalog_authority = state_guard.catalog_authority.clone();
    let analytics = state_guard.analytics.clone();
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    drop(state_guard);

    let lookup_db = db_path.clone();
    let lookup_name = name.clone();
    let lookup_version = query.version.clone();
    let lookup_arch = query.arch.clone();
    let Some(profile_revision_sha256) =
        active_profile_revision_for_request(catalog_authority, &distro).await
    else {
        return get_package(State(state), Path((distro, name)), Query(query)).await;
    };
    let ccs_path = match tokio::task::spawn_blocking(move || {
        converted_ccs_path_for_download(
            &lookup_db,
            &profile_revision_sha256,
            &lookup_name,
            lookup_version.as_deref(),
            lookup_arch.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(ConvertedDownloadLookup::Ready(path))) => path,
        Ok(Ok(ConvertedDownloadLookup::Missing)) => {
            return get_package(State(state), Path((distro, name)), Query(query)).await;
        }
        Ok(Err(e)) => {
            tracing::error!("Database error checking downloadable conversion: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    // Stream the file (analytics recorded inside after confirming file is readable)
    stream_ccs_file(
        ccs_path,
        analytics,
        &distro,
        &name,
        query.version.as_deref(),
        query.arch.as_deref(),
        ua.as_deref(),
    )
    .await
}

/// Stream a CCS file as a response, recording analytics only on success.
///
/// Analytics are recorded after the file is confirmed readable (open + metadata),
/// so failed streams don't inflate download counts.
async fn stream_ccs_file(
    ccs_path: std::path::PathBuf,
    analytics: Option<Arc<crate::server::AnalyticsRecorder>>,
    distro: &str,
    name: &str,
    version: Option<&str>,
    architecture: Option<&str>,
    ua: Option<&str>,
) -> Response {
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
            tracing::error!(
                "Failed to get CCS package metadata {}: {}",
                ccs_path.display(),
                e
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read package").into_response();
        }
    };

    let raw_filename = ccs_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("package.ccs");
    // Sanitize filename for Content-Disposition header: allow only safe characters
    let safe_filename: String = raw_filename
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    let filename = if safe_filename.is_empty() {
        "package.ccs".to_string()
    } else {
        safe_filename
    };

    tracing::info!(
        "Serving CCS package: {} ({} bytes)",
        filename,
        metadata.len()
    );

    // Record analytics only after confirming the file is readable
    if let Some(recorder) = analytics {
        recorder
            .record(distro, name, version, architecture, ua)
            .await;
    }

    // Stream the file
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        // CCS packages are versioned but can be re-converted, so moderate caching
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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
    // Publish admin event before starting conversion
    {
        let state_guard = state.read().await;
        state_guard.publish_event(
            "conversion",
            serde_json::json!({
                "action": "started",
                "distro": req.distro,
                "package": req.package,
            }),
        );
    }

    // Reuse the get_package logic
    let query = PackageQuery {
        version: req.version,
        release: None,
        arch: None,
    };
    get_package(State(state), Path((req.distro, req.package)), Query(query)).await
}

mod delta;
pub(crate) use delta::get_delta;

#[cfg(test)]
#[path = "packages/tests.rs"]
mod tests;
