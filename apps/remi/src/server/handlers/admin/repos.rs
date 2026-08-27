// apps/remi/src/server/handlers/admin/repos.rs
//! Repository management handlers

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service::{
    self, CreateRepoInput, NativeSourcePolicyInput, RepoRefreshBatch, RepoRefreshBatchState,
    UpdateRepoInput,
};
use crate::server::auth::{Scope, TokenScopes, json_error};
use conary_core::db::models::RepositoryOwnership;
use conary_core::repository::{RepositoryFormat, RepositoryParserConfig, RepositoryTrustPolicy};

use super::{check_scope, validate_path_param};

/// Request body for creating a repository.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRepoRequest {
    pub name: String,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub metadata_expire: Option<i32>,
    pub parser: RepositoryParserConfig,
    pub trust: Option<RepositoryTrustPolicy>,
    pub native_source: Option<NativeSourcePolicyRequest>,
}

/// Request body for replacing a repository's mutable configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRepoRequest {
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub metadata_expire: Option<i32>,
    pub parser: RepositoryParserConfig,
    pub trust: Option<RepositoryTrustPolicy>,
    pub native_source: Option<NativeSourcePolicyRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSourcePolicyRequest {
    pub source_identity: String,
    pub repository_identity: String,
    pub stream_kind: String,
    pub stream_identity: String,
    pub policy_group: Option<String>,
    pub update_mode: String,
    pub pinned_snapshot_sha256: Option<String>,
}

impl From<NativeSourcePolicyRequest> for NativeSourcePolicyInput {
    fn from(value: NativeSourcePolicyRequest) -> Self {
        Self {
            source_identity: value.source_identity,
            repository_identity: value.repository_identity,
            stream_kind: value.stream_kind,
            stream_identity: value.stream_identity,
            policy_group: value.policy_group,
            update_mode: value.update_mode,
            pinned_snapshot_sha256: value.pinned_snapshot_sha256,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct NativeSourcePolicyResponse {
    pub source_identity: String,
    pub repository_identity: String,
    pub scope_kind: &'static str,
    pub scope_identity: String,
    pub ecosystem: &'static str,
    pub version_scheme: &'static str,
    pub stream_kind: &'static str,
    pub stream_identity: String,
    pub update_mode: &'static str,
    pub stream_binding_sha256: String,
    pub pinned_snapshot_sha256: Option<String>,
}

/// Response body for repository endpoints.
#[derive(Debug, Serialize)]
pub struct RepoResponse {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub trust: Option<RepositoryTrustPolicy>,
    pub metadata_expire: i32,
    pub last_checked_at: Option<String>,
    pub last_changed_at: Option<String>,
    pub last_validated_at: Option<String>,
    pub last_published_at: Option<String>,
    pub created_at: Option<String>,
    pub package_format: RepositoryFormat,
    pub parser: Option<RepositoryParserConfig>,
    pub managed_by: &'static str,
    pub native_source: Option<NativeSourcePolicyResponse>,
}

/// Query parameters for refresh endpoints.
#[derive(Debug, Deserialize)]
pub struct RefreshQuery {
    #[serde(default)]
    pub force: bool,
    /// Restrict a retry to one exact configured native source profile.
    #[serde(default)]
    pub profile: Option<String>,
}

impl TryFrom<conary_core::db::models::Repository> for RepoResponse {
    type Error = anyhow::Error;

    fn try_from(r: conary_core::db::models::Repository) -> Result<Self, Self::Error> {
        let native_source = match (
            r.source_policy.as_ref(),
            r.repository_identity.as_ref(),
            r.stream_binding_sha256.as_ref(),
        ) {
            (Some(policy), Some(repository_identity), Some(stream_binding_sha256)) => {
                Some(NativeSourcePolicyResponse {
                    source_identity: policy.source_identity.clone(),
                    repository_identity: repository_identity.clone(),
                    scope_kind: policy.scope.kind(),
                    scope_identity: policy.scope.identity().to_string(),
                    ecosystem: policy.ecosystem.as_str(),
                    version_scheme: policy.version_scheme.as_str(),
                    stream_kind: policy.stream.kind(),
                    stream_identity: policy.stream.identity().to_string(),
                    update_mode: policy.update_mode.as_str(),
                    stream_binding_sha256: stream_binding_sha256.clone(),
                    pinned_snapshot_sha256: r
                        .pinned_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.sha256().to_string()),
                })
            }
            (None, None, None) => None,
            _ => anyhow::bail!("repository '{}' has incomplete native source state", r.name),
        };
        Ok(Self {
            id: r.id.ok_or_else(|| {
                anyhow::anyhow!(
                    "repository '{}' was loaded for admin serving without a persisted ID",
                    r.name
                )
            })?,
            name: r.name,
            url: r.url,
            content_url: r.content_url,
            enabled: r.enabled,
            priority: r.priority,
            trust: r.trust_policy,
            metadata_expire: r.metadata_expire,
            last_checked_at: r.last_checked_at,
            last_changed_at: r.last_changed_at,
            last_validated_at: r.last_validated_at,
            last_published_at: r.last_published_at,
            created_at: r.created_at,
            package_format: r.package_format,
            parser: r.parser_config,
            managed_by: match r.managed_by {
                RepositoryOwnership::Operator => "operator",
                RepositoryOwnership::RemiConfig => "remi-config",
                RepositoryOwnership::NativeProjection => "native-projection",
                RepositoryOwnership::PackageProjection => "package-projection",
            },
            native_source,
        })
    }
}

/// GET /v1/admin/repos
///
/// List all configured repositories. Requires the "repos:read" scope.
pub async fn list_repos(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposRead) {
        return err;
    }

    match admin_service::list_repos(&state).await {
        Ok(repos) => {
            match repos
                .into_iter()
                .map(RepoResponse::try_from)
                .collect::<anyhow::Result<Vec<_>>>()
            {
                Ok(response) => Json(response).into_response(),
                Err(error) => {
                    tracing::error!("Failed to build repository response: {error}");
                    json_error(500, "Failed to list repositories", "INTERNAL_ERROR")
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to list repos: {e}");
            json_error(500, "Failed to list repositories", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/repos
///
/// Add a new repository. Requires the "repos:write" scope.
pub async fn create_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<CreateRepoRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposWrite) {
        return err;
    }

    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 {
        return json_error(
            400,
            "Repository name must be 1-128 characters",
            "INVALID_NAME",
        );
    }
    let name = name.to_string();
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    let url = body.url.trim().to_string();
    if url.is_empty() {
        return json_error(400, "Repository URL must not be empty", "INVALID_URL");
    }
    if url::Url::parse(&url).is_err() {
        return json_error(400, "Invalid URL format", "INVALID_INPUT");
    }

    if let Some(ref cu) = body.content_url {
        let cu_trimmed = cu.trim();
        if !cu_trimmed.is_empty() && url::Url::parse(cu_trimmed).is_err() {
            return json_error(400, "Invalid content_url format", "INVALID_INPUT");
        }
    }

    let input = CreateRepoInput {
        name: name.clone(),
        url,
        content_url: body.content_url,
        enabled: body.enabled.unwrap_or(true),
        priority: body.priority.unwrap_or(0),
        metadata_expire: body.metadata_expire.unwrap_or(3600),
        parser: body.parser,
        trust: body.trust,
        native_source: body.native_source.map(Into::into),
    };

    match admin_service::create_repo(&state, input).await {
        Ok(repo) => {
            let guard = state.read().await;
            guard.publish_event("repo.created", serde_json::json!({"name": &name}));
            drop(guard);
            match RepoResponse::try_from(repo) {
                Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
                Err(error) => {
                    tracing::error!("Failed to build repository response: {error}");
                    json_error(500, "Failed to create repository", "INTERNAL_ERROR")
                }
            }
        }
        Err(admin_service::ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(admin_service::ServiceError::Conflict(msg)) => json_error(409, &msg, "CONFLICT"),
        Err(e) => {
            tracing::error!("Failed to create repo: {e}");
            json_error(500, "Failed to create repository", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/repos/:name
///
/// Get details for a specific repository. Requires the "repos:read" scope.
pub async fn get_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposRead) {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    match admin_service::get_repo(&state, &name).await {
        Ok(Some(repo)) => match RepoResponse::try_from(repo) {
            Ok(response) => Json(response).into_response(),
            Err(error) => {
                tracing::error!("Failed to build repository response: {error}");
                json_error(500, "Failed to get repository", "INTERNAL_ERROR")
            }
        },
        Ok(None) => json_error(404, "Repository not found", "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to get repo: {e}");
            json_error(500, "Failed to get repository", "INTERNAL_ERROR")
        }
    }
}

/// PUT /v1/admin/repos/:name
///
/// Update an existing repository configuration. Requires the "repos:write" scope.
pub async fn update_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<UpdateRepoRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposWrite) {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    let url = body.url.trim().to_string();
    if url.is_empty() {
        return json_error(400, "URL is required", "INVALID_INPUT");
    }
    if url::Url::parse(&url).is_err() {
        return json_error(400, "Invalid URL format", "INVALID_INPUT");
    }

    if let Some(ref cu) = body.content_url {
        let cu_trimmed = cu.trim();
        if !cu_trimmed.is_empty() && url::Url::parse(cu_trimmed).is_err() {
            return json_error(400, "Invalid content_url format", "INVALID_INPUT");
        }
    }

    let input = UpdateRepoInput {
        url,
        content_url: body.content_url,
        enabled: body.enabled,
        priority: body.priority,
        metadata_expire: body.metadata_expire,
        parser: body.parser,
        trust: body.trust,
        native_source: body.native_source.map(Into::into),
    };

    match admin_service::update_repo(&state, &name, input).await {
        Ok(Some(repo)) => {
            let guard = state.read().await;
            guard.publish_event("repo.updated", serde_json::json!({"name": &repo.name}));
            drop(guard);
            match RepoResponse::try_from(repo) {
                Ok(response) => Json(response).into_response(),
                Err(error) => {
                    tracing::error!("Failed to build repository response: {error}");
                    json_error(500, "Failed to update repository", "INTERNAL_ERROR")
                }
            }
        }
        Ok(None) => json_error(404, "Repository not found", "NOT_FOUND"),
        Err(admin_service::ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(admin_service::ServiceError::Conflict(msg)) => json_error(409, &msg, "CONFLICT"),
        Err(e) => {
            tracing::error!("Failed to update repo: {e}");
            json_error(500, "Failed to update repository", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/repos/:name
///
/// Remove a repository. Requires the "repos:write" scope.
pub async fn delete_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposWrite) {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }

    match admin_service::delete_repo(&state, &name).await {
        Ok(true) => {
            let guard = state.read().await;
            guard.publish_event("repo.deleted", serde_json::json!({"name": &name}));
            drop(guard);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => json_error(404, "Repository not found", "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to delete repo {name}: {e}");
            json_error(500, "Failed to delete repository", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/repos/:name/sync
///
/// Trigger a manual sync for a repository.
/// Requires the "repos:write" scope.
pub async fn sync_repo(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    Query(query): Query<RefreshQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposWrite) {
        return err;
    }
    if let Some(err) = validate_path_param(&name, "repo name") {
        return err;
    }
    match admin_service::sync_repo(&state, &name, query.force).await {
        Ok(Some(result)) => {
            let guard = state.read().await;
            guard.publish_event(
                "repo.synced",
                serde_json::json!({
                    "name": &result.name,
                    "source_profile": &result.source_profile,
                    "packages_synced": result.packages_synced,
                    "skipped": result.skipped,
                    "force": query.force,
                }),
            );
            drop(guard);
            Json(serde_json::json!({
                "status": if result.skipped { "up_to_date" } else { "synced" },
                "name": result.name,
                "source_profile": result.source_profile,
                "packages_synced": result.packages_synced,
                "skipped": result.skipped,
                "force": query.force,
            }))
            .into_response()
        }
        Ok(None) => json_error(404, "Repository not found", "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to sync repo {name}: {e}");
            json_error(500, "Failed to sync repository", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/refresh
///
/// Synchronize enabled repositories, optionally restricting work to one exact
/// native source profile. Fresh repositories are skipped unless `force=true`.
/// Requires the "repos:write" scope.
pub async fn refresh_repos(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<RefreshQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposWrite) {
        return err;
    }

    let result = match query.profile.as_deref() {
        Some(profile) => {
            admin_service::refresh_profile_repositories(&state, profile, query.force).await
        }
        None => admin_service::refresh_repositories(&state, query.force).await,
    };
    match result {
        Ok(batch) => {
            refresh_batch_response(&state, query.force, query.profile.as_deref(), batch).await
        }
        Err(e) => {
            tracing::error!("Failed to refresh repositories: {e}");
            json_error(500, "Failed to refresh repositories", "INTERNAL_ERROR")
        }
    }
}

/// Publish and render the one canonical multi-source refresh result.
pub(crate) async fn refresh_batch_response(
    state: &Arc<RwLock<ServerState>>,
    force: bool,
    profile: Option<&str>,
    batch: RepoRefreshBatch,
) -> Response {
    let batch_state = batch.state();
    let synced = batch.synced_count();
    let skipped = batch.skipped_count();
    let failed = batch.failures.len();
    let status_code = refresh_status_code(batch_state);

    {
        let guard = state.read().await;
        guard.publish_event(
            "repos.refreshed",
            serde_json::json!({
                "force": force,
                "profile": profile,
                "state": batch_state,
                "synced": synced,
                "skipped": skipped,
                "failed": failed,
            }),
        );
    }

    (
        status_code,
        Json(serde_json::json!({
            "status": batch_state.as_str(),
            "force": force,
            "profile": profile,
            "synced": synced,
            "skipped": skipped,
            "failed": failed,
            "results": batch.results,
            "failures": batch.failures,
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

    use super::super::test_helpers::{rebuild_app, test_app, test_app_with_database_writer};
    use super::{RepoRefreshBatchState, refresh_status_code};

    #[tokio::test]
    async fn test_repo_crud_lifecycle() {
        let (app, db_path) = test_app().await;

        // Create a repo
        let create_body = serde_json::json!({
            "name": "fedora",
            "url": "https://93.184.216.34/fedora",
            "enabled": true,
            "priority": 10,
            "parser": {"package_format": "json"}
        });
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["name"], "fedora");
        assert_eq!(body["priority"], 10);

        // List repos and verify it appears
        let app2 = rebuild_app(&db_path);
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let repos = body.as_array().expect("should be an array");
        assert!(repos.iter().any(|r| r["name"] == "fedora"));

        // Get single repo
        let app3 = rebuild_app(&db_path);
        let resp = app3
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Update repo
        let app4 = rebuild_app(&db_path);
        let update_body = serde_json::json!({
            "url": "https://example.org/fedora",
            "priority": 20,
            "parser": {"package_format": "json"}
        });
        let resp = app4
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "repository update response: {}",
            String::from_utf8_lossy(&body_bytes)
        );
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["priority"], 20);

        // Delete repo
        let app5 = rebuild_app(&db_path);
        let resp = app5
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify it is gone
        let app6 = rebuild_app(&db_path);
        let resp = app6
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn repository_update_waits_for_the_shared_database_writer() {
        let (app, db_path, database_writer) = test_app_with_database_writer().await;
        let create_body = serde_json::json!({
            "name": "fedora",
            "url": "https://93.184.216.34/fedora",
            "parser": {"package_format": "json"}
        });
        let create_resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_resp.status(), StatusCode::CREATED);

        // Hold both the declared writer authority and SQLite's real write lock.
        // The old repository path bypassed the former, waited out the five-second
        // SQLite busy timeout on the latter, and returned HTTP 500. The corrected
        // path must remain queued on the shared owner without touching SQLite.
        let mut blocking_connection = conary_core::db::open_fast(&db_path).unwrap();
        let blocking_transaction = blocking_connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let writer_guard = database_writer.hold_for_test();
        let update_body = serde_json::json!({
            "url": "https://93.184.216.35/fedora",
            "priority": 20,
            "parser": {"package_format": "json"}
        });
        let mut update = tokio::spawn(
            app.oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(update_body.to_string()))
                    .unwrap(),
            ),
        );

        if let Ok(completed) =
            tokio::time::timeout(std::time::Duration::from_millis(5_500), &mut update).await
        {
            let response = completed.unwrap().unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            panic!(
                "repository update bypassed the shared database writer: {status} {}",
                String::from_utf8_lossy(&body)
            );
        }
        drop(blocking_transaction);
        drop(writer_guard);

        let response = update.await.unwrap().unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "repository update response: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[tokio::test]
    async fn native_repo_create_requires_and_returns_exact_source_policy() {
        let (app, _db_path) = test_app().await;
        let root = serde_json::json!({
            "url": "https://93.184.216.34/keys/repository.gpg",
            "fingerprint": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        });
        let create_body = serde_json::json!({
            "name": "third-party-rpm",
            "url": "https://93.184.216.34/rpm",
            "parser": {"package_format": "rpm", "architecture": "x86_64"},
            "trust": {
                "ecosystem": "rpm",
                "metadata": {"kind": "open-pgp", "keys": [root.clone()]},
                "package_keys": [root]
            },
            "native_source": {
                "source_identity": "third-party:widgets",
                "repository_identity": "widgets:x86_64",
                "stream_kind": "channel",
                "stream_identity": "stable",
                "update_mode": "follow"
            }
        });
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["native_source"]["source_identity"],
            "third-party:widgets"
        );
        assert_eq!(body["native_source"]["update_mode"], "follow");
        assert_eq!(
            body["native_source"]["stream_binding_sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
    }

    #[tokio::test]
    async fn test_repo_scope_enforcement() {
        let (app, db_path) = test_app().await;

        // Create a token with only repos:read scope
        let repo_reader_token = "repo-read-only-token-67890";
        let hash = crate::server::auth::hash_token(repo_reader_token);
        {
            let conn = crate::server::open_runtime_db(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "repo-reader", &hash, "repos:read")
                .unwrap();
        }

        // GET /v1/admin/repos with repos:read scope should be allowed
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos")
                    .header("Authorization", format!("Bearer {repo_reader_token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_refresh_repos_empty_ok() {
        let (app, _db_path) = test_app().await;

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/refresh")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["status"], "complete");
        assert_eq!(body["synced"], 0);
        assert_eq!(body["skipped"], 0);
        assert_eq!(body["failed"], 0);
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

    #[tokio::test]
    async fn test_sync_repo_missing_returns_not_found() {
        let (app, _db_path) = test_app().await;

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos/missing/sync")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_create_repo_rejects_localhost_url() {
        let (app, _db_path) = test_app().await;

        let create_body = serde_json::json!({
            "name": "bad-repo",
            "url": "http://localhost:8080/repo",
            "parser": {"package_format": "json"}
        });
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_update_repo_rejects_private_content_url() {
        let (app, db_path) = test_app().await;

        let create_body = serde_json::json!({
            "name": "fedora",
            "url": "https://93.184.216.34/fedora",
            "parser": {"package_format": "json"}
        });
        let create_resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_resp.status(), StatusCode::CREATED);

        let app2 = rebuild_app(&db_path);
        let update_body = serde_json::json!({
            "url": "https://93.184.216.34/fedora",
            "content_url": "http://10.0.0.42/content",
            "parser": {"package_format": "json"}
        });
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_update_repo_rejects_create_only_name_field() {
        let (app, db_path) = test_app().await;

        let create_body = serde_json::json!({
            "name": "fedora",
            "url": "https://93.184.216.34/fedora",
            "parser": {"package_format": "json"}
        });
        let create_resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/repos")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_resp.status(), StatusCode::CREATED);

        let update_body = serde_json::json!({
            "name": "removed-create-only-field",
            "url": "https://93.184.216.34/fedora",
            "parser": {"package_format": "json"}
        });
        let response = rebuild_app(&db_path)
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/repos/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
