// apps/remi/src/server/handlers/packages/delta.rs

use super::*;

/// Query parameters for delta requests
#[derive(Debug, Deserialize)]
pub(crate) struct DeltaQuery {
    /// Version to upgrade from
    pub from: String,
    /// Version to upgrade to
    pub to: String,
}

/// GET /v1/{distro}/packages/{name}/delta?from=V1&to=V2
///
/// Returns the pre-computed delta manifest between two versions of a package.
/// If no cached delta exists, computes one on the fly.
pub(crate) async fn get_delta(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
    Query(query): Query<DeltaQuery>,
) -> Response {
    if let Err(e) = super::super::validate_distro_and_name(&distro, &name) {
        return e;
    }

    let state_guard = state.read().await;
    let db_path = state_guard.config.db_path.clone();
    drop(state_guard);

    let from = query.from;
    let to = query.to;
    let source_profile =
        match conary_core::repository::supported_profiles::profile_for_remi_route(&distro) {
            Some(profile) => profile.id().to_string(),
            None => return (StatusCode::BAD_REQUEST, "Unknown distribution").into_response(),
        };
    let name_c = name.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db_path)?;

        // Try cached delta first
        if let Some(cached) =
            crate::server::delta_manifests::get_delta(&conn, &source_profile, &name_c, &from, &to)?
        {
            return Ok(cached);
        }

        // Compute on the fly
        crate::server::delta_manifests::compute_delta(&conn, &source_profile, &name_c, &from, &to)
    })
    .await;

    match result {
        Ok(Ok(delta)) => {
            let response = delta.to_response();
            let json = match super::super::serialize_json(&response, "delta response") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::super::json_response(json, 300)
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to compute delta for {}/{}: {}", distro, name, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to compute delta").into_response()
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}
