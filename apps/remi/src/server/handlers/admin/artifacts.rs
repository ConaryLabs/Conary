// apps/remi/src/server/handlers/admin/artifacts.rs
//! Admin upload handlers for test fixtures and test artifacts.

use super::check_scope;
use crate::server::ServerState;
use crate::server::artifact_paths::{ArtifactRoot, artifact_root, sanitize_relative_path};
use crate::server::auth::{Scope, TokenScopes, json_error};
use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde::Serialize;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

/// Maximum allowed upload size (512 MB).
const MAX_UPLOAD_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Serialize)]
struct UploadResponse {
    path: String,
    size: u64,
}

async fn upload_artifact(
    state: Arc<RwLock<ServerState>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    path: String,
    request: Request,
    root: ArtifactRoot,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    let relative = match sanitize_relative_path(&path) {
        Ok(path) => path,
        Err(message) => return json_error(400, message, "INVALID_PATH"),
    };

    let full_path = {
        let guard = state.read().await;
        artifact_root(&guard, root).join(&relative)
    };

    let parent = match full_path.parent() {
        Some(parent) => parent.to_path_buf(),
        None => {
            return json_error(
                400,
                "Artifact path must include a file name",
                "INVALID_PATH",
            );
        }
    };

    if let Err(err) = tokio::fs::create_dir_all(&parent).await {
        tracing::error!(
            "Failed to create artifact dir {}: {}",
            parent.display(),
            err
        );
        return json_error(500, "Failed to create artifact directory", "IO_ERROR");
    }

    let temp_path =
        full_path.with_extension(format!("{}.uploading", uuid::Uuid::new_v4().simple()));
    let mut file = match tokio::fs::File::create(&temp_path).await {
        Ok(file) => file,
        Err(err) => {
            tracing::error!(
                "Failed to create temp artifact {}: {}",
                temp_path.display(),
                err
            );
            return json_error(500, "Failed to store artifact", "IO_ERROR");
        }
    };

    let mut size = 0u64;
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                size += bytes.len() as u64;
                if size > MAX_UPLOAD_SIZE {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return json_error(
                        413,
                        "Upload exceeds maximum size (512 MB)",
                        "PAYLOAD_TOO_LARGE",
                    );
                }
                if let Err(err) = file.write_all(&bytes).await {
                    tracing::error!(
                        "Failed writing artifact chunk for {}: {}",
                        full_path.display(),
                        err
                    );
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return json_error(500, "Failed to store artifact", "IO_ERROR");
                }
            }
            Err(err) => {
                tracing::warn!(
                    "Failed reading upload body for {}: {}",
                    full_path.display(),
                    err
                );
                let _ = tokio::fs::remove_file(&temp_path).await;
                return json_error(400, "Invalid upload body", "INVALID_BODY");
            }
        }
    }

    if let Err(err) = file.flush().await {
        tracing::error!("Failed to flush {}: {}", temp_path.display(), err);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return json_error(500, "Failed to finalize artifact", "IO_ERROR");
    }
    drop(file);

    if let Err(err) = tokio::fs::rename(&temp_path, &full_path).await {
        tracing::error!(
            "Failed to move artifact {} into place: {}",
            full_path.display(),
            err
        );
        let _ = tokio::fs::remove_file(&temp_path).await;
        return json_error(500, "Failed to publish artifact", "IO_ERROR");
    }

    (
        StatusCode::CREATED,
        axum::Json(UploadResponse { path, size }),
    )
        .into_response()
}

pub async fn upload_fixture(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(path): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
    request: Request,
) -> Response {
    upload_artifact(state, scopes, path, request, ArtifactRoot::Fixtures).await
}

pub async fn upload_test_artifact(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(path): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
    request: Request,
) -> Response {
    upload_artifact(state, scopes, path, request, ArtifactRoot::Artifacts).await
}

#[cfg(test)]
mod tests {
    use crate::server::handlers::admin::test_helpers::test_app;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_upload_fixture_writes_file() {
        let (app, db_path) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/v1/admin/test-fixtures/adversarial/demo/sample.ccs")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::from("fixture-bytes"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let stored = db_path
            .parent()
            .unwrap()
            .join("test-fixtures/adversarial/demo/sample.ccs");
        assert_eq!(std::fs::read(stored).unwrap(), b"fixture-bytes");
    }

    #[tokio::test]
    async fn test_upload_fixture_rejects_invalid_path() {
        let (app, _) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/v1/admin/test-fixtures/../secret")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::from("fixture-bytes"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
