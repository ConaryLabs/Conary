// conary-server/src/server/handlers/admin/audit.rs
//! Audit log query and purge handlers

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::auth::{Scope, TokenScopes, json_error};
use crate::server::ServerState;

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
    let db_path = { state.read().await.config.db_path.clone() };
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        conary_core::db::models::audit_log::query(
            &conn,
            query.limit,
            query.action.as_deref(),
            query.since.as_deref(),
            query.token_name.as_deref(),
        )
    })
    .await;
    match result {
        Ok(Ok(entries)) => Json(entries).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to query audit log: {e}");
            json_error(500, "Failed to query audit log", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error querying audit: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
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
    let db_path = { state.read().await.config.db_path.clone() };
    let before = query.before.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        conary_core::db::models::audit_log::purge(&conn, &before)
    })
    .await;
    match result {
        Ok(Ok(deleted)) => {
            Json(serde_json::json!({"deleted": deleted, "before": query.before})).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to purge audit log: {e}");
            json_error(500, "Failed to purge audit log", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error purging audit: {e}");
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}
