// conary-server/src/server/handlers/artifacts.rs
//! Public handlers for static test fixtures and test artifacts.

use crate::server::ServerState;
use crate::server::artifact_paths::{ArtifactRoot, artifact_root, sanitize_relative_path};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::path::Path as FsPath;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

fn content_type(path: &FsPath) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("sha256") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn serve_artifact(
    state: Arc<RwLock<ServerState>>,
    path: String,
    root: ArtifactRoot,
    method: Method,
) -> Response {
    let relative = match sanitize_relative_path(&path) {
        Ok(path) => path,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };

    let full_path = {
        let guard = state.read().await;
        artifact_root(&guard, root).join(relative)
    };

    let metadata = match tokio::fs::metadata(&full_path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(err) => {
            tracing::error!("Failed to stat artifact {}: {}", full_path.display(), err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read artifact metadata",
            )
                .into_response();
        }
    };

    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(&full_path))
        .header(header::CONTENT_LENGTH, metadata.len().to_string())
        .header(header::CACHE_CONTROL, "public, max-age=300");

    if method == Method::HEAD {
        return builder
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let file = match tokio::fs::File::open(&full_path).await {
        Ok(file) => file,
        Err(err) => {
            tracing::error!("Failed to open artifact {}: {}", full_path.display(), err);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open artifact").into_response();
        }
    };

    builder
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn get_fixture(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(path): Path<String>,
) -> Response {
    serve_artifact(state, path, ArtifactRoot::Fixtures, Method::GET).await
}

pub async fn head_fixture(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(path): Path<String>,
) -> Response {
    serve_artifact(state, path, ArtifactRoot::Fixtures, Method::HEAD).await
}

pub async fn get_test_artifact(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(path): Path<String>,
) -> Response {
    serve_artifact(state, path, ArtifactRoot::Artifacts, Method::GET).await
}

pub async fn head_test_artifact(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(path): Path<String>,
) -> Response {
    serve_artifact(state, path, ArtifactRoot::Artifacts, Method::HEAD).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{ServerConfig, ServerState};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::get;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_public_fixture_get_and_head() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage_root = temp_dir.path().join("storage");
        let chunk_dir = storage_root.join("chunks");
        let cache_dir = storage_root.join("cache");
        let fixture_path = storage_root.join("test-fixtures/adversarial/demo/sample.ccs");
        std::fs::create_dir_all(&chunk_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::create_dir_all(fixture_path.parent().unwrap()).unwrap();
        std::fs::write(&fixture_path, b"fixture-bytes").unwrap();

        let state = Arc::new(RwLock::new(
            ServerState::new(ServerConfig {
                db_path: storage_root.join("metadata/conary.db"),
                chunk_dir,
                cache_dir,
                ..Default::default()
            })
            .expect("test server state"),
        ));
        let app = Router::new()
            .route(
                "/test-fixtures/{*path}",
                get(get_fixture).head(head_fixture),
            )
            .with_state(state);

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/test-fixtures/adversarial/demo/sample.ccs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"fixture-bytes");

        let head_response = app
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/test-fixtures/adversarial/demo/sample.ccs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head_response.status(), StatusCode::OK);
        assert_eq!(
            head_response.headers().get(header::CONTENT_LENGTH).unwrap(),
            "13"
        );
    }

    #[tokio::test]
    async fn test_public_artifact_rejects_traversal() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage_root = temp_dir.path().join("storage");
        let chunk_dir = storage_root.join("chunks");
        let cache_dir = storage_root.join("cache");
        std::fs::create_dir_all(&chunk_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();

        let state = Arc::new(RwLock::new(
            ServerState::new(ServerConfig {
                db_path: storage_root.join("metadata/conary.db"),
                chunk_dir,
                cache_dir,
                ..Default::default()
            })
            .expect("test server state"),
        ));
        let app = Router::new()
            .route(
                "/test-artifacts/{*path}",
                get(get_test_artifact).head(head_test_artifact),
            )
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/test-artifacts/../secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
