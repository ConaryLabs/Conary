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
    self, CreateRepoInput, NativeSourcePolicyInput, UpdateRepoInput,
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

/// Query parameters for one exact repository sync.
#[derive(Debug, Deserialize)]
pub struct SyncQuery {
    #[serde(default)]
    pub force: bool,
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
    Query(query): Query<SyncQuery>,
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

#[cfg(test)]
mod tests;
