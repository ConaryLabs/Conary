// apps/remi/src/server/handlers/admin/refresh.rs

//! Typed all-profile and profile-scoped repository refresh HTTP surface.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service::{self, RepoRefreshBatchState};
use crate::server::auth::{Scope, TokenScopes, json_error};
use crate::server::publication_coordinator::RepositoryRefreshExecution;

use super::check_scope;

/// Query parameters for the complete repository refresh endpoint.
#[derive(Debug, Deserialize)]
pub struct RefreshQuery {
    #[serde(default)]
    pub force: bool,
    /// Restrict a retry to one exact configured native source profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Reuse an exact complete all-profile result from this process only when
    /// it finished strictly after this Unix timestamp.
    #[serde(default)]
    pub accept_completed_after: Option<i64>,
}

pub(crate) fn refresh_query_error(query: &RefreshQuery) -> Option<Response> {
    let floor = query.accept_completed_after?;
    if !query.force {
        return Some(json_error(
            400,
            "accept_completed_after requires force=true",
            "INVALID_REFRESH_FLOOR",
        ));
    }
    if query.profile.is_some() {
        return Some(json_error(
            400,
            "accept_completed_after is valid only for an all-profile refresh",
            "INVALID_REFRESH_FLOOR",
        ));
    }
    if floor <= 0 {
        return Some(json_error(
            400,
            "accept_completed_after must be a positive Unix timestamp",
            "INVALID_REFRESH_FLOOR",
        ));
    }
    None
}

/// POST /v1/admin/refresh
///
/// Synchronize enabled repositories, optionally restricting work to one exact
/// native source profile. Fresh repositories are skipped unless `force=true`.
/// Requires the `repos:write` scope.
pub async fn refresh_repos(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<RefreshQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposWrite) {
        return err;
    }
    refresh_repos_inner(&state, query).await
}

pub(crate) async fn refresh_repos_inner(
    state: &Arc<RwLock<ServerState>>,
    query: RefreshQuery,
) -> Response {
    if let Some(error) = refresh_query_error(&query) {
        return error;
    }

    let result = match query.profile.as_deref() {
        Some(profile) => {
            admin_service::refresh_profile_repositories(state, profile, query.force).await
        }
        None => {
            admin_service::refresh_repositories(state, query.force, query.accept_completed_after)
                .await
        }
    };
    match result {
        Ok(execution) => {
            refresh_batch_response(state, query.force, query.profile.as_deref(), execution).await
        }
        Err(error) => {
            tracing::error!("Failed to refresh repositories: {error}");
            json_error(500, "Failed to refresh repositories", "INTERNAL_ERROR")
        }
    }
}

/// Publish and render the one canonical multi-source refresh execution.
pub(crate) async fn refresh_batch_response(
    state: &Arc<RwLock<ServerState>>,
    requested_force: bool,
    requested_profile: Option<&str>,
    execution: RepositoryRefreshExecution,
) -> Response {
    let batch_state = execution.batch.state();
    let synced = execution.batch.synced_count();
    let skipped = execution.batch.skipped_count();
    let failed = execution.batch.failures.len();
    let status_code = refresh_status_code(batch_state);

    {
        let guard = state.read().await;
        guard.publish_event(
            "repos.refreshed",
            serde_json::json!({
                "force": requested_force,
                "profile": requested_profile,
                "state": batch_state,
                "synced": synced,
                "skipped": skipped,
                "failed": failed,
                "refresh_generation": execution.generation,
                "refresh_scope": &execution.scope,
                "refresh_force": execution.force,
                "refresh_started_at": execution.started_at,
                "refresh_finished_at": execution.finished_at,
                "coalesced": execution.coalesced,
            }),
        );
    }

    (
        status_code,
        Json(serde_json::json!({
            "status": batch_state.as_str(),
            "force": requested_force,
            "profile": requested_profile,
            "refresh_generation": execution.generation,
            "refresh_scope": execution.scope,
            "refresh_force": execution.force,
            "refresh_started_at": execution.started_at,
            "refresh_finished_at": execution.finished_at,
            "coalesced": execution.coalesced,
            "synced": synced,
            "skipped": skipped,
            "failed": failed,
            "results": execution.batch.results,
            "failures": execution.batch.failures,
        })),
    )
        .into_response()
}

fn refresh_status_code(batch_state: RepoRefreshBatchState) -> StatusCode {
    match batch_state {
        RepoRefreshBatchState::Complete => StatusCode::OK,
        RepoRefreshBatchState::Partial => StatusCode::MULTI_STATUS,
        RepoRefreshBatchState::Failed => StatusCode::BAD_GATEWAY,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use tower::ServiceExt;

    use super::super::test_helpers::test_app;
    use super::{RepoRefreshBatchState, refresh_status_code};

    #[tokio::test]
    async fn empty_refresh_returns_typed_generation() {
        let (app, _db_path) = test_app().await;

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/refresh?force=true&accept_completed_after=1")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["status"], "complete");
        assert_eq!(body["force"], true);
        assert_eq!(body["refresh_generation"], 1);
        assert_eq!(body["refresh_scope"]["kind"], "all");
        assert_eq!(body["refresh_force"], true);
        assert_eq!(body["coalesced"], false);
        assert!(body["refresh_started_at"].as_i64().unwrap() > 1);
        assert!(
            body["refresh_finished_at"].as_i64().unwrap()
                >= body["refresh_started_at"].as_i64().unwrap()
        );
        assert_eq!(body["synced"], 0);
        assert_eq!(body["skipped"], 0);
        assert_eq!(body["failed"], 0);
    }

    #[tokio::test]
    async fn causal_floor_requires_forced_all_profile_request() {
        for uri in [
            "/v1/admin/refresh?accept_completed_after=1",
            "/v1/admin/refresh?force=true&profile=fedora-44&accept_completed_after=1",
            "/v1/admin/refresh?force=true&accept_completed_after=0",
        ] {
            let (app, _db_path) = test_app().await;
            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("Authorization", "Bearer test-admin-token-12345")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "URI: {uri}");
        }
    }

    #[test]
    fn refresh_batch_state_has_distinct_http_outcomes() {
        assert_eq!(
            refresh_status_code(RepoRefreshBatchState::Complete),
            StatusCode::OK
        );
        assert_eq!(
            refresh_status_code(RepoRefreshBatchState::Partial),
            StatusCode::MULTI_STATUS
        );
        assert_eq!(
            refresh_status_code(RepoRefreshBatchState::Failed),
            StatusCode::BAD_GATEWAY
        );
    }
}
