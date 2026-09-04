// apps/remi/src/server/release_publish.rs
//! Remi release artifact upload, gate enforcement, and public metadata commit.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::native_publish::{self, NativePublishError};

const MAX_RELEASE_UPLOAD_SIZE: u64 = 512 * 1024 * 1024;
const RELEASE_PUBLISH_POLICY_DIGEST: &str = "m2-static-publish-policy-v1";

#[derive(Debug, Serialize)]
pub struct ReleaseUploadResponse {
    status: &'static str,
    distro: String,
    package: String,
    version: String,
    release: String,
    architecture: String,
    path: String,
    size: u64,
    content_hash: String,
}

#[derive(Debug)]
struct ReleaseUploadError {
    status: StatusCode,
    code: String,
    message: String,
}

impl ReleaseUploadError {
    fn bad_request(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.into(),
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<NativePublishError> for ReleaseUploadError {
    fn from(error: NativePublishError) -> Self {
        Self {
            status: error.status,
            code: error.code.as_str().to_string(),
            message: error.message,
        }
    }
}

struct StagedRelease {
    path: PathBuf,
}

pub async fn handle_release_upload(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: Request,
) -> Response {
    match release_upload_inner(state, distro, request).await {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(error) => release_upload_error_response(error),
    }
}

async fn release_upload_inner(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: Request,
) -> Result<ReleaseUploadResponse, ReleaseUploadError> {
    native_publish::verify::validate_supported_release_distro(&distro)
        .map_err(ReleaseUploadError::from)?;
    let staged = stage_release_body(&state, request).await?;
    let result = release_upload_after_stage(&state, &distro, &staged).await;
    let _ = tokio::fs::remove_file(&staged.path).await;
    result
}

async fn release_upload_after_stage(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
    staged: &StagedRelease,
) -> Result<ReleaseUploadResponse, ReleaseUploadError> {
    let (cache_dir, chunk_dir, release_publish) = {
        let guard = state.read().await;
        (
            guard.config.cache_dir.clone(),
            guard.config.chunk_dir.clone(),
            guard.config.release_publish.clone(),
        )
    };
    let accepted = native_publish::verify::accepted_release_signers(&release_publish)
        .map_err(ReleaseUploadError::from)?;
    let artifact_path = staged.path.clone();
    let route_slug = distro.to_string();
    let artifact = tokio::task::spawn_blocking(move || {
        native_publish::verify::verify_native_artifact(
            &artifact_path,
            &route_slug,
            &accepted,
            RELEASE_PUBLISH_POLICY_DIGEST,
        )
    })
    .await
    .map_err(|error| {
        ReleaseUploadError::internal(
            format!("join native release verification task: {error}"),
            "INTERNAL_ERROR",
        )
    })?
    .map_err(ReleaseUploadError::from)?;

    let response_package = artifact.name.clone();
    let response_version = artifact.version.clone();
    let response_release = artifact.package_release.clone();
    let response_architecture = artifact.architecture.clone();
    let response_size = artifact.total_size;
    let response_content_hash = artifact.content_hash.clone();
    let promoted = native_publish::storage::promote_native_artifact(
        &cache_dir,
        &chunk_dir,
        distro,
        &staged.path,
        &artifact,
    )
    .await
    .map_err(ReleaseUploadError::from)?;
    let response_path = promoted.package_path.to_string_lossy().to_string();
    let promoted_for_cleanup = promoted.clone();
    let commit =
        native_publish::persistence::commit_native_publication(state, distro, artifact, promoted)
            .await;
    if let Err(error) = commit {
        promoted_for_cleanup.cleanup_public_objects().await;
        return Err(ReleaseUploadError::from(error));
    }

    Ok(ReleaseUploadResponse {
        status: "created",
        distro: distro.to_string(),
        package: response_package,
        version: response_version,
        release: response_release,
        architecture: response_architecture,
        path: response_path,
        size: response_size,
        content_hash: response_content_hash,
    })
}

async fn stage_release_body(
    state: &Arc<RwLock<ServerState>>,
    request: Request,
) -> Result<StagedRelease, ReleaseUploadError> {
    let cache_dir = state.read().await.config.cache_dir.clone();
    let staging_dir = cache_dir.join("releases").join("staging");
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .map_err(|error| {
            ReleaseUploadError::internal(
                format!("create release staging directory: {error}"),
                "IO_ERROR",
            )
        })?;

    let path = staging_dir.join(format!("release-{}.ccs", uuid::Uuid::new_v4().simple()));
    let mut file = tokio::fs::File::create(&path).await.map_err(|error| {
        ReleaseUploadError::internal(format!("create staged release body: {error}"), "IO_ERROR")
    })?;

    let mut size = 0u64;
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|error| {
            ReleaseUploadError::bad_request(format!("invalid upload body: {error}"), "INVALID_BODY")
        })?;
        size += bytes.len() as u64;
        if size > MAX_RELEASE_UPLOAD_SIZE {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ReleaseUploadError {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "PAYLOAD_TOO_LARGE".to_string(),
                message: "Upload exceeds maximum size (512 MB)".to_string(),
            });
        }
        if let Err(error) = file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ReleaseUploadError::internal(
                format!("write staged release body: {error}"),
                "IO_ERROR",
            ));
        }
    }

    if size == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ReleaseUploadError::bad_request(
            "Package body must not be empty",
            "INVALID_BODY",
        ));
    }

    if let Err(error) = file.flush().await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ReleaseUploadError::internal(
            format!("flush staged release body: {error}"),
            "IO_ERROR",
        ));
    }

    Ok(StagedRelease { path })
}

fn release_upload_error_response(error: ReleaseUploadError) -> Response {
    (
        error.status,
        Json(serde_json::json!({
            "error": error.message,
            "code": error.code,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
