// conary-server/src/server/handlers/admin/repos.rs
//! Repository management handlers

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::admin_service::{self, CreateRepoInput, UpdateRepoInput};
use crate::server::auth::{Scope, TokenScopes, json_error};

use super::{check_scope, validate_path_param};

/// Request body for creating or updating a repository.
#[derive(Debug, Deserialize)]
pub struct RepoRequest {
    pub name: Option<String>,
    pub url: String,
    pub content_url: Option<String>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub gpg_check: Option<bool>,
    pub metadata_expire: Option<i32>,
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
    pub gpg_check: bool,
    pub metadata_expire: i32,
    pub last_sync: Option<String>,
    pub created_at: Option<String>,
}

impl From<conary_core::db::models::Repository> for RepoResponse {
    fn from(r: conary_core::db::models::Repository) -> Self {
        Self {
            id: r.id.unwrap_or(0),
            name: r.name,
            url: r.url,
            content_url: r.content_url,
            enabled: r.enabled,
            priority: r.priority,
            gpg_check: r.gpg_check,
            metadata_expire: r.metadata_expire,
            last_sync: r.last_sync,
            created_at: r.created_at,
        }
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
            let response: Vec<RepoResponse> = repos.into_iter().map(RepoResponse::from).collect();
            Json(response).into_response()
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
    Json(body): Json<RepoRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::ReposWrite) {
        return err;
    }

    let name = match body.name.as_deref() {
        Some(n) => n.trim(),
        None => return json_error(400, "Name is required", "INVALID_INPUT"),
    };
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
        gpg_check: body.gpg_check.unwrap_or(true),
        metadata_expire: body.metadata_expire.unwrap_or(3600),
    };

    match admin_service::create_repo(&state, input).await {
        Ok(repo) => {
            let guard = state.read().await;
            guard.publish_event("repo.created", serde_json::json!({"name": &name}));
            drop(guard);
            (StatusCode::CREATED, Json(RepoResponse::from(repo))).into_response()
        }
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
        Ok(Some(repo)) => Json(RepoResponse::from(repo)).into_response(),
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
    Json(body): Json<RepoRequest>,
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

    if let Some(ref n) = body.name {
        let n = n.trim();
        if !n.is_empty()
            && let Some(err) = validate_path_param(n, "repo name")
        {
            return err;
        }
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
        gpg_check: body.gpg_check,
        metadata_expire: body.metadata_expire,
    };

    match admin_service::update_repo(&state, &name, input).await {
        Ok(Some(repo)) => {
            let guard = state.read().await;
            guard.publish_event("repo.updated", serde_json::json!({"name": &repo.name}));
            drop(guard);
            Json(RepoResponse::from(repo)).into_response()
        }
        Ok(None) => json_error(404, "Repository not found", "NOT_FOUND"),
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
/// Trigger a manual sync for a repository. Currently a stub that verifies
/// the repo exists and publishes a `repo.sync_requested` event.
/// Requires the "repos:write" scope.
pub async fn sync_repo(
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

    match admin_service::repo_exists(&state, &name).await {
        Ok(true) => {
            let guard = state.read().await;
            guard.publish_event("repo.sync_requested", serde_json::json!({"name": &name}));
            drop(guard);
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"status": "sync_requested", "name": name})),
            )
                .into_response()
        }
        Ok(false) => json_error(404, "Repository not found", "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to find repo for sync: {e}");
            json_error(500, "Failed to sync repository", "INTERNAL_ERROR")
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    /// Helper to rebuild a fresh router against the same DB (oneshot consumes the app).
    fn rebuild_app(db_path: &std::path::Path) -> axum::Router {
        let config = crate::server::ServerConfig {
            db_path: db_path.to_path_buf(),
            chunk_dir: db_path.parent().unwrap().join("chunks"),
            cache_dir: db_path.parent().unwrap().join("cache"),
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));
        crate::server::routes::create_external_admin_router(state, None)
    }

    async fn test_app() -> (axum::Router, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .unwrap();
            conary_core::db::schema::migrate(&conn).unwrap();
        }

        let config = crate::server::ServerConfig {
            db_path: db_path.clone(),
            chunk_dir: tmp.path().join("chunks"),
            cache_dir: tmp.path().join("cache"),
            ..Default::default()
        };
        std::fs::create_dir_all(&config.chunk_dir).unwrap();
        std::fs::create_dir_all(&config.cache_dir).unwrap();

        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));
        let app = crate::server::routes::create_external_admin_router(state, None);

        let test_token = "test-admin-token-12345";
        let hash = crate::server::auth::hash_token(test_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin")
                .unwrap();
        }

        std::mem::forget(tmp);
        (app, db_path)
    }

    #[tokio::test]
    async fn test_repo_crud_lifecycle() {
        let (app, db_path) = test_app().await;

        // Create a repo
        let create_body = serde_json::json!({
            "name": "fedora",
            "url": "https://mirrors.example.com/fedora",
            "enabled": true,
            "priority": 10
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
            "name": "fedora",
            "url": "https://mirrors2.example.com/fedora",
            "priority": 20
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
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
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
    async fn test_repo_scope_enforcement() {
        let (app, db_path) = test_app().await;

        // Create a token with only ci:read scope
        let ci_token = "ci-read-only-token-67890";
        let hash = crate::server::auth::hash_token(ci_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "ci-reader", &hash, "ci:read")
                .unwrap();
        }

        // GET /v1/admin/repos with ci:read scope should be 403
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/repos")
                    .header("Authorization", format!("Bearer {ci_token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
