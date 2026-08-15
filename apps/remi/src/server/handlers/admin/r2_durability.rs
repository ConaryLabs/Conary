// apps/remi/src/server/handlers/admin/r2_durability.rs

//! R2 durability inventory and backfill handler for the external admin API.

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service::{self, ServiceError};
use crate::server::auth::{Scope, TokenScopes, json_error};
use crate::server::r2_durability::{DEFAULT_BACKFILL_CONCURRENCY, R2DurabilityMode};

use super::check_scope;

#[derive(Debug, Default, Deserialize)]
pub struct R2DurabilityRequest {
    pub mode: Option<R2DurabilityMode>,
    pub concurrency: Option<usize>,
}

impl R2DurabilityRequest {
    fn mode(&self) -> R2DurabilityMode {
        self.mode.unwrap_or(R2DurabilityMode::Plan)
    }

    fn concurrency(&self) -> usize {
        self.concurrency.unwrap_or(DEFAULT_BACKFILL_CONCURRENCY)
    }
}

/// POST /v1/admin/r2-durability
///
/// An omitted body or mode plans the exact local-versus-R2 backfill. Applying
/// requires the explicit `{"mode":"apply"}` request.
pub async fn r2_durability(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    body: Option<Json<R2DurabilityRequest>>,
) -> Response {
    if let Some(error) = check_scope(&scopes, Scope::Admin) {
        return error;
    }

    let request = body.map_or_else(R2DurabilityRequest::default, |Json(body)| body);
    match admin_service::run_r2_durability_op(&state, request.mode(), request.concurrency()).await {
        Ok(report) => Json(report).into_response(),
        Err(ServiceError::BadRequest(message)) => json_error(400, &message, "BAD_REQUEST"),
        Err(error) => {
            tracing::error!("R2 durability operation failed: {error}");
            json_error(500, "Failed to inventory R2 durability", "INTERNAL_ERROR")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::super::test_helpers::test_app;

    #[test]
    fn omitted_body_plans_with_bounded_default_concurrency() {
        let request = R2DurabilityRequest::default();
        assert_eq!(request.mode(), R2DurabilityMode::Plan);
        assert_eq!(request.concurrency(), DEFAULT_BACKFILL_CONCURRENCY);
    }

    #[test]
    fn apply_requires_explicit_typed_mode() {
        let request: R2DurabilityRequest =
            serde_json::from_str(r#"{"mode":"apply","concurrency":8}"#).unwrap();
        assert_eq!(request.mode(), R2DurabilityMode::Apply);
        assert_eq!(request.concurrency(), 8);
    }

    #[tokio::test]
    async fn route_requires_authentication() {
        let (app, _db_path) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/r2-durability")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_plan_fails_typed_when_r2_is_unconfigured() {
        let (app, _db_path) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/r2-durability")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["code"], "BAD_REQUEST");
    }

    #[tokio::test]
    async fn route_rejects_out_of_bounds_backfill_concurrency() {
        let (app, _db_path) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/r2-durability")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"mode":"apply","concurrency":65}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["code"], "BAD_REQUEST");
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("between 1 and 64")
        );
    }
}
