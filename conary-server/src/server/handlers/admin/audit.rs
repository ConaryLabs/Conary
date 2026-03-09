// conary-server/src/server/handlers/admin/audit.rs
//! Audit log query and purge handlers

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service;
use crate::server::auth::{Scope, TokenScopes, json_error};

use super::check_scope;

/// Query parameters for the audit log endpoint.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub action: Option<String>,
    pub since: Option<String>,
    pub token_name: Option<String>,
}

/// Query parameters for purging audit entries.
#[derive(Debug, Deserialize)]
pub struct PurgeQuery {
    pub before: String,
}

/// GET /v1/admin/audit
pub async fn query_audit(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<AuditQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::query_audit(
        &state,
        query.limit,
        query.action,
        query.since,
        query.token_name,
    )
    .await
    {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => {
            tracing::error!("Failed to query audit log: {e}");
            json_error(500, "Failed to query audit log", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/audit
pub async fn purge_audit(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<PurgeQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    match admin_service::purge_audit(&state, &query.before).await {
        Ok(deleted) => {
            Json(serde_json::json!({"deleted": deleted, "before": query.before})).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to purge audit log: {e}");
            json_error(500, "Failed to purge audit log", "INTERNAL_ERROR")
        }
    }
}
