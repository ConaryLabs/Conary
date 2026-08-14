// apps/remi/src/server/handlers/admin/chunk_gc.rs
//! Chunk garbage-collection handler for the external admin API.

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service;
use crate::server::auth::{Scope, TokenScopes, json_error};

use super::check_scope;

/// Request body for `POST /v1/admin/chunk-gc`.
#[derive(Debug, Default, Deserialize)]
pub struct ChunkGcRequest {
    pub dry_run: Option<bool>,
}

impl ChunkGcRequest {
    /// Deletion requires an explicit `dry_run: false`; anything else previews.
    fn dry_run(&self) -> bool {
        self.dry_run.unwrap_or(true)
    }
}

/// POST /v1/admin/chunk-gc
///
/// Delete chunks that no converted package references. An omitted body, or a
/// body without `dry_run`, previews the run instead of deleting.
pub async fn chunk_gc(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    body: Option<Json<ChunkGcRequest>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    let dry_run = body
        .map_or_else(ChunkGcRequest::default, |Json(body)| body)
        .dry_run();

    match admin_service::run_chunk_gc_op(&state, dry_run).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => {
            tracing::error!("Chunk garbage collection failed: {e}");
            json_error(500, "Failed to garbage-collect chunks", "INTERNAL_ERROR")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::super::test_helpers::{rebuild_app, test_app};

    #[test]
    fn omitted_body_previews() {
        assert!(ChunkGcRequest::default().dry_run());
    }

    #[test]
    fn omitted_dry_run_field_previews() {
        let request: ChunkGcRequest = serde_json::from_str("{}").unwrap();
        assert!(request.dry_run());
    }

    #[test]
    fn explicit_dry_run_false_deletes() {
        let request: ChunkGcRequest = serde_json::from_str(r#"{"dry_run": false}"#).unwrap();
        assert!(!request.dry_run());
    }

    #[tokio::test]
    async fn chunk_gc_route_rejects_token_without_admin_scope() {
        let (_app, db_path) = test_app().await;

        let reader_token = "chunk-gc-reader-token-67890";
        let hash = crate::server::auth::hash_token(reader_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(
                &conn,
                "chunk-gc-reader",
                &hash,
                "repos:read",
            )
            .unwrap();
        }

        let response = rebuild_app(&db_path)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/chunk-gc")
                    .header("Authorization", format!("Bearer {reader_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn chunk_gc_route_requires_authentication() {
        let (app, _db_path) = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/chunk-gc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn chunk_gc_route_defaults_to_dry_run_without_a_body() {
        let (app, _db_path) = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/chunk-gc")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(report["dry_run"], true);
        assert_eq!(report["local_deleted"], 0);
    }

    #[tokio::test]
    async fn chunk_gc_route_honors_explicit_dry_run_false() {
        let (app, _db_path) = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/chunk-gc")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"dry_run": false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(report["dry_run"], false);
        assert_eq!(report["local_scanned"], 0);
    }
}
