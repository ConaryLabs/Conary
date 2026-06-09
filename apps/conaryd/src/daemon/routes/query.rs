// apps/conaryd/src/daemon/routes/query.rs
//! Daemon read-only package and query routes.

use super::db::run_db_query;
use super::errors::{ApiResult, not_found_error};
use super::types::{
    DependencyInfo, HistoryEntry, PackageDetails, PackageSummary, SearchQuery, SharedState,
};
use axum::{
    Router,
    extract::{Path, Query, State},
    response::Json,
    routing::get,
};
use conary_core::db::models::{Changeset, DependencyEntry, GenerationPublication, Trove};

pub(super) fn router() -> Router<SharedState> {
    Router::new()
        .route("/packages", get(list_packages_handler))
        .route("/packages/{name}", get(get_package_handler))
        .route("/packages/{name}/files", get(get_package_files_handler))
        .route("/search", get(search_handler))
        .route("/depends/{name}", get(depends_handler))
        .route("/rdepends/{name}", get(rdepends_handler))
        .route("/history", get(history_handler))
}

async fn list_packages_handler(
    State(state): State<SharedState>,
) -> ApiResult<Json<Vec<PackageSummary>>> {
    let troves = run_db_query(&state, Trove::list_all).await?;
    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

async fn get_package_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<PackageDetails>> {
    let pkg_name = name.clone();

    let result = run_db_query(&state, move |conn| {
        let trove = Trove::find_one_by_name(conn, &pkg_name)?;
        match trove {
            None => Ok(None),
            Some(t) => {
                let deps = if let Some(id) = t.id {
                    DependencyEntry::find_by_trove(conn, id)?
                } else {
                    vec![]
                };
                Ok(Some((t, deps)))
            }
        }
    })
    .await?;

    let (trove, deps) = result.ok_or_else(|| not_found_error("package", &name))?;
    let details = PackageDetails {
        name: trove.name,
        version: trove.version,
        package_type: trove.trove_type.as_str().to_string(),
        architecture: trove.architecture,
        description: trove.description,
        installed_at: trove.installed_at,
        install_source: trove.install_source.as_str().to_string(),
        install_reason: trove.install_reason.as_str().to_string(),
        selection_reason: trove.selection_reason,
        flavor: trove.flavor_spec,
        pinned: trove.pinned,
        dependencies: deps.iter().map(DependencyInfo::from).collect(),
    };
    Ok(Json(details))
}

async fn get_package_files_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<String>>> {
    let pkg_name = name.clone();

    let result = run_db_query(&state, move |conn| {
        let trove = Trove::find_one_by_name(conn, &pkg_name)?;
        match trove {
            Some(t) => {
                let trove_id = t
                    .id
                    .ok_or_else(|| conary_core::Error::NotFound("Package has no ID".to_string()))?;
                let mut stmt =
                    conn.prepare("SELECT path FROM files WHERE trove_id = ?1 ORDER BY path")?;
                let files: Vec<String> = stmt
                    .query_map([trove_id], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(Some(files))
            }
            None => Ok(None),
        }
    })
    .await?;

    result
        .map(Json)
        .ok_or_else(|| not_found_error("package", &name))
}

async fn search_handler(
    State(state): State<SharedState>,
    Query(params): Query<SearchQuery>,
) -> ApiResult<Json<Vec<PackageSummary>>> {
    let query = params.q.unwrap_or_default();

    let troves = run_db_query(&state, move |conn| {
        let pattern = if query.is_empty() {
            "%".to_string()
        } else {
            format!("%{}%", query)
        };

        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, \
             installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, \
             selection_reason, label_id \
             FROM troves WHERE name LIKE ?1 ORDER BY name, version",
        )?;

        let troves: Vec<Trove> = stmt
            .query_map([pattern], Trove::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(troves)
    })
    .await?;

    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

async fn depends_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<DependencyInfo>>> {
    let pkg_name = name.clone();

    let result = run_db_query(&state, move |conn| {
        let trove = Trove::find_one_by_name(conn, &pkg_name)?;
        match trove {
            None => Ok(None),
            Some(t) => {
                let deps = if let Some(id) = t.id {
                    DependencyEntry::find_by_trove(conn, id)?
                } else {
                    vec![]
                };
                Ok(Some(deps))
            }
        }
    })
    .await?;

    let deps = result.ok_or_else(|| not_found_error("package", &name))?;
    Ok(Json(deps.iter().map(DependencyInfo::from).collect()))
}

async fn rdepends_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<PackageSummary>>> {
    let pkg_name = name.clone();

    let troves = run_db_query(&state, move |conn| {
        let dep_entries = DependencyEntry::find_dependents(conn, &pkg_name)?;
        let mut troves = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        for dep in dep_entries {
            if !seen_ids.contains(&dep.trove_id)
                && let Some(trove) = Trove::find_by_id(conn, dep.trove_id)?
            {
                seen_ids.insert(dep.trove_id);
                troves.push(trove);
            }
        }
        Ok(troves)
    })
    .await?;

    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

async fn history_handler(State(state): State<SharedState>) -> ApiResult<Json<Vec<HistoryEntry>>> {
    let (changesets, publications) = run_db_query(&state, |conn| {
        Ok((
            Changeset::list_all(conn)?,
            GenerationPublication::pending_recoverable(conn)?,
        ))
    })
    .await?;
    let history: Vec<HistoryEntry> = changesets
        .iter()
        .map(|changeset| HistoryEntry::from_changeset_with_publication(changeset, &publications))
        .collect();
    Ok(Json(history))
}

#[cfg(test)]
mod tests {
    use super::super::errors::INTERNAL_ERROR_DETAIL;
    use super::super::test_support::{
        body_bytes, body_json, create_test_state, create_test_state_with_db_path,
        current_process_creds, test_router,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_handler_list_packages_empty_db() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_handler_get_package_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/packages/nonexistent-pkg")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let json = body_json(response).await;
        assert_eq!(json["status"], 404);
        assert!(json["detail"].as_str().unwrap().contains("nonexistent-pkg"));
    }

    #[tokio::test]
    async fn test_handler_get_package_files_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/packages/nonexistent-pkg/files")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_search_empty_results() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/search?q=nonexistent")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_handler_search_no_query_param() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/search")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should succeed with empty results (matches all with wildcard)
        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
    }

    #[tokio::test]
    async fn test_handler_depends_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/depends/nonexistent-pkg")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_rdepends_empty() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/rdepends/nonexistent-pkg")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // rdepends returns empty array (not 404) when no dependents exist
        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_handler_history_empty() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/history")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_internal_errors_are_sanitized_for_clients() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let bad_db_path = temp_dir.path().join("db-dir");
        std::fs::create_dir_all(&bad_db_path).unwrap();
        let (state, _guard) = create_test_state_with_db_path(bad_db_path.clone());
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(body.contains(INTERNAL_ERROR_DETAIL));
        assert!(!body.contains("Database error"));
        assert!(!body.contains(&bad_db_path.display().to_string()));
    }
}
