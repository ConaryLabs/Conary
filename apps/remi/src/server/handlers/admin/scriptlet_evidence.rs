// apps/remi/src/server/handlers/admin/scriptlet_evidence.rs

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{check_scope, validate_path_param};
use crate::server::ServerState;
use crate::server::auth::{Scope, TokenName, TokenScopes, json_error};

#[derive(Debug, Deserialize)]
pub struct BackfillRequest {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ClusterListQuery {
    pub state: Option<String>,
    pub distro: Option<String>,
    pub blocked_class: Option<String>,
    pub command: Option<String>,
    pub package: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct StateUpdateRequest {
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NoteRequest {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct PacketQuery {
    pub visibility: Option<String>,
}

#[derive(Debug, Serialize)]
struct ClusterListResponse {
    clusters: Vec<ClusterSummaryResponse>,
}

#[derive(Debug, Serialize)]
struct ClusterSummaryResponse {
    cluster_key: String,
    schema_version: i64,
    distro: String,
    target_profile: String,
    blocked_class: String,
    command: String,
    normalized_command_shape: String,
    normalized_command_shape_hash: String,
    lifecycle_phase: String,
    state: String,
    first_seen: String,
    last_seen: String,
    updated_at: String,
    attempt_count: i64,
    unique_package_count: i64,
    architectures: Vec<String>,
    stale_sample_count: i64,
}

#[derive(Debug, Serialize)]
struct ClusterDetailResponse {
    cluster: ClusterSummaryResponse,
    samples: Vec<SampleSummaryResponse>,
    state_events: Vec<StateEventResponse>,
    notes: Vec<NoteResponse>,
}

#[derive(Debug, Serialize)]
struct SampleSummaryResponse {
    id: i64,
    package_name: String,
    package_version: String,
    package_architecture: Option<String>,
    publication_status: String,
    scriptlet_fidelity: String,
    target_compatibility: String,
    reason_codes: serde_json::Value,
    blocked_classes: serde_json::Value,
    boot_security_intents: serde_json::Value,
    review_artifact_available: bool,
    review_artifact_stale: bool,
    evidence_digest: Option<String>,
    curation_evidence_digest: Option<String>,
    observed_at: String,
}

#[derive(Debug, Serialize)]
struct StateEventResponse {
    id: i64,
    from_state: Option<String>,
    to_state: String,
    actor: String,
    reason: Option<String>,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct NoteResponse {
    id: i64,
    actor: String,
    body: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptletEvidenceParamError {
    InvalidState,
    InvalidPacketVisibility,
}

impl ScriptletEvidenceParamError {
    fn into_response(self) -> Response {
        match self {
            Self::InvalidState => {
                json_error(400, "Invalid scriptlet evidence state", "INVALID_PARAMETER")
            }
            Self::InvalidPacketVisibility => json_error(
                400,
                "Invalid scriptlet evidence packet visibility",
                "INVALID_PARAMETER",
            ),
        }
    }
}

pub async fn scriptlet_evidence_backfill(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    request: Option<Json<BackfillRequest>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    let limit = request
        .and_then(|Json(request)| request.limit)
        .unwrap_or(500)
        .clamp(1, 5000);
    let (db_path, cache_dir) = {
        let state = state.read().await;
        (state.config.db_path.clone(), state.config.cache_dir.clone())
    };

    match tokio::task::spawn_blocking(move || {
        crate::server::scriptlet_evidence_queue::backfill::run_backfill_batch(
            &db_path, &cache_dir, limit,
        )
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => json_error(
            500,
            &format!("Scriptlet evidence backfill failed: {error}"),
            "SCRIPTLET_EVIDENCE_BACKFILL_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("Scriptlet evidence backfill task failed: {error}"),
            "SCRIPTLET_EVIDENCE_BACKFILL_TASK_FAILED",
        ),
    }
}

pub async fn list_scriptlet_evidence_clusters(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    Query(query): Query<ClusterListQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }

    let filter = match cluster_filter(query) {
        Ok(filter) => filter,
        Err(error) => return error.into_response(),
    };
    let db_path = {
        let state = state.read().await;
        state.config.db_path.clone()
    };
    match tokio::task::spawn_blocking(move || {
        let conn = crate::server::open_runtime_db(db_path)?;
        let rows = conary_core::db::models::ScriptletEvidenceCluster::list(&conn, &filter)?;
        Ok::<_, anyhow::Error>(rows)
    })
    .await
    {
        Ok(Ok(rows)) => Json(ClusterListResponse {
            clusters: rows.into_iter().map(ClusterSummaryResponse::from).collect(),
        })
        .into_response(),
        Ok(Err(error)) => json_error(
            500,
            &format!("Failed to list scriptlet evidence clusters: {error}"),
            "SCRIPTLET_EVIDENCE_LIST_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("Scriptlet evidence list task failed: {error}"),
            "SCRIPTLET_EVIDENCE_LIST_TASK_FAILED",
        ),
    }
}

pub async fn get_scriptlet_evidence_cluster(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    Path(cluster_key): Path<String>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    if let Some(err) = validate_path_param(&cluster_key, "cluster_key") {
        return err;
    }

    let db_path = {
        let state = state.read().await;
        state.config.db_path.clone()
    };
    match tokio::task::spawn_blocking(move || {
        let conn = crate::server::open_runtime_db(db_path)?;
        conary_core::db::models::ScriptletEvidenceCluster::detail(&conn, &cluster_key)
            .map_err(anyhow::Error::from)
    })
    .await
    {
        Ok(Ok(Some(detail))) => Json(ClusterDetailResponse::from(detail)).into_response(),
        Ok(Ok(None)) => json_error(
            404,
            "Scriptlet evidence cluster not found",
            "SCRIPTLET_EVIDENCE_CLUSTER_NOT_FOUND",
        ),
        Ok(Err(error)) => json_error(
            500,
            &format!("Failed to load scriptlet evidence cluster: {error}"),
            "SCRIPTLET_EVIDENCE_DETAIL_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("Scriptlet evidence detail task failed: {error}"),
            "SCRIPTLET_EVIDENCE_DETAIL_TASK_FAILED",
        ),
    }
}

pub async fn get_scriptlet_evidence_packet(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    Path(cluster_key): Path<String>,
    Query(query): Query<PacketQuery>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    if let Some(err) = validate_path_param(&cluster_key, "cluster_key") {
        return err;
    }
    let visibility = match packet_visibility(query.visibility.as_deref()) {
        Ok(visibility) => visibility,
        Err(error) => return error.into_response(),
    };

    let db_path = {
        let state = state.read().await;
        state.config.db_path.clone()
    };
    match tokio::task::spawn_blocking(move || {
        let conn = crate::server::open_runtime_db(db_path)?;
        conary_core::db::models::ScriptletEvidenceCluster::detail(&conn, &cluster_key)
            .map_err(anyhow::Error::from)
    })
    .await
    {
        Ok(Ok(Some(detail))) => {
            let packet =
                crate::server::scriptlet_evidence_queue::packet::build_packet(detail, visibility);
            Json(packet).into_response()
        }
        Ok(Ok(None)) => json_error(
            404,
            "Scriptlet evidence cluster not found",
            "SCRIPTLET_EVIDENCE_CLUSTER_NOT_FOUND",
        ),
        Ok(Err(error)) => json_error(
            500,
            &format!("Failed to build scriptlet evidence packet: {error}"),
            "SCRIPTLET_EVIDENCE_PACKET_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("Scriptlet evidence packet task failed: {error}"),
            "SCRIPTLET_EVIDENCE_PACKET_TASK_FAILED",
        ),
    }
}

pub async fn update_scriptlet_evidence_cluster_state(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    token_name: Option<Extension<TokenName>>,
    Path(cluster_key): Path<String>,
    Json(request): Json<StateUpdateRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    if let Some(err) = validate_path_param(&cluster_key, "cluster_key") {
        return err;
    }
    let Some(next_state) = parse_state(&request.state) else {
        return json_error(400, "Invalid scriptlet evidence state", "INVALID_PARAMETER");
    };
    let actor = token_name
        .map(|Extension(name)| name.0)
        .unwrap_or_else(|| "unknown-admin".to_string());
    let reason = request.reason;
    let db_path = {
        let state = state.read().await;
        state.config.db_path.clone()
    };

    match tokio::task::spawn_blocking(move || {
        let conn = crate::server::open_runtime_db(db_path)?;
        conary_core::db::models::ScriptletEvidenceCluster::update_state(
            &conn,
            &cluster_key,
            next_state,
            &actor,
            reason.as_deref(),
        )?;
        conary_core::db::models::ScriptletEvidenceCluster::detail(&conn, &cluster_key)
    })
    .await
    {
        Ok(Ok(Some(detail))) => Json(ClusterDetailResponse::from(detail)).into_response(),
        Ok(Ok(None)) => json_error(
            404,
            "Scriptlet evidence cluster not found",
            "SCRIPTLET_EVIDENCE_CLUSTER_NOT_FOUND",
        ),
        Ok(Err(conary_core::Error::NotFound(_))) => json_error(
            404,
            "Scriptlet evidence cluster not found",
            "SCRIPTLET_EVIDENCE_CLUSTER_NOT_FOUND",
        ),
        Ok(Err(error)) => json_error(
            500,
            &format!("Failed to update scriptlet evidence state: {error}"),
            "SCRIPTLET_EVIDENCE_STATE_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("Scriptlet evidence state task failed: {error}"),
            "SCRIPTLET_EVIDENCE_STATE_TASK_FAILED",
        ),
    }
}

pub async fn add_scriptlet_evidence_cluster_note(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    token_name: Option<Extension<TokenName>>,
    Path(cluster_key): Path<String>,
    Json(request): Json<NoteRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    if let Some(err) = validate_path_param(&cluster_key, "cluster_key") {
        return err;
    }
    let body = request.body.trim().to_string();
    if body.is_empty() || body.len() > 4096 {
        return json_error(
            400,
            "Note body is required and must be at most 4096 bytes",
            "INVALID_PARAMETER",
        );
    }
    let actor = token_name
        .map(|Extension(name)| name.0)
        .unwrap_or_else(|| "unknown-admin".to_string());
    let db_path = {
        let state = state.read().await;
        state.config.db_path.clone()
    };

    match tokio::task::spawn_blocking(move || {
        let conn = crate::server::open_runtime_db(db_path)?;
        if conary_core::db::models::ScriptletEvidenceCluster::find(&conn, &cluster_key)?.is_none() {
            return Err(conary_core::Error::NotFound(format!(
                "scriptlet evidence cluster {cluster_key}"
            )));
        }
        conary_core::db::models::ScriptletEvidenceNote::insert(&conn, &cluster_key, &actor, &body)
    })
    .await
    {
        Ok(Ok(note)) => Json(NoteResponse::from(note)).into_response(),
        Ok(Err(conary_core::Error::NotFound(_))) => json_error(
            404,
            "Scriptlet evidence cluster not found",
            "SCRIPTLET_EVIDENCE_CLUSTER_NOT_FOUND",
        ),
        Ok(Err(error)) => json_error(
            500,
            &format!("Failed to add scriptlet evidence note: {error}"),
            "SCRIPTLET_EVIDENCE_NOTE_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("Scriptlet evidence note task failed: {error}"),
            "SCRIPTLET_EVIDENCE_NOTE_TASK_FAILED",
        ),
    }
}

fn cluster_filter(
    query: ClusterListQuery,
) -> Result<conary_core::db::models::ScriptletEvidenceClusterListFilter, ScriptletEvidenceParamError>
{
    let state = match query.state.as_deref() {
        Some(value) => Some(parse_state(value).ok_or(ScriptletEvidenceParamError::InvalidState)?),
        None => None,
    };
    Ok(
        conary_core::db::models::ScriptletEvidenceClusterListFilter {
            state,
            distro: query.distro,
            blocked_class: query.blocked_class,
            command: query.command,
            package: query.package,
            limit: Some(query.limit.unwrap_or(100).clamp(1, 1000)),
            offset: Some(query.offset.unwrap_or(0).max(0)),
        },
    )
}

fn parse_state(value: &str) -> Option<conary_core::db::models::ScriptletEvidenceState> {
    use conary_core::db::models::ScriptletEvidenceState as State;
    match value {
        "needs-triage" => Some(State::NeedsTriage),
        "adapter-candidate" => Some(State::AdapterCandidate),
        "in-design" => Some(State::InDesign),
        "in-implementation" => Some(State::InImplementation),
        "covered-partial" => Some(State::CoveredPartial),
        "covered-public-ready" => Some(State::CoveredPublicReady),
        "wont-support" => Some(State::WontSupport),
        _ => None,
    }
}

fn packet_visibility(
    value: Option<&str>,
) -> Result<
    crate::server::scriptlet_evidence_queue::packet::PacketVisibility,
    ScriptletEvidenceParamError,
> {
    use crate::server::scriptlet_evidence_queue::packet::PacketVisibility;
    match value.unwrap_or("private") {
        "private" => Ok(PacketVisibility::Private),
        "public-sanitized" => Ok(PacketVisibility::PublicSanitized),
        _ => Err(ScriptletEvidenceParamError::InvalidPacketVisibility),
    }
}

impl From<conary_core::db::models::ScriptletEvidenceClusterSummary> for ClusterSummaryResponse {
    fn from(value: conary_core::db::models::ScriptletEvidenceClusterSummary) -> Self {
        let cluster = value.cluster;
        Self {
            cluster_key: cluster.cluster_key,
            schema_version: cluster.schema_version,
            distro: cluster.distro,
            target_profile: cluster.target_profile,
            blocked_class: cluster.blocked_class,
            command: cluster.command,
            normalized_command_shape: cluster.normalized_command_shape,
            normalized_command_shape_hash: cluster.normalized_command_shape_hash,
            lifecycle_phase: cluster.lifecycle_phase,
            state: cluster.state.as_str().to_string(),
            first_seen: cluster.first_seen,
            last_seen: cluster.last_seen,
            updated_at: cluster.updated_at,
            attempt_count: value.attempt_count,
            unique_package_count: value.unique_package_count,
            architectures: value.architectures,
            stale_sample_count: value.stale_sample_count,
        }
    }
}

impl From<conary_core::db::models::ScriptletEvidenceClusterDetail> for ClusterDetailResponse {
    fn from(value: conary_core::db::models::ScriptletEvidenceClusterDetail) -> Self {
        let attempt_count = value.samples.len() as i64;
        let unique_package_count = value
            .samples
            .iter()
            .map(|sample| {
                format!(
                    "{}|{}|{}",
                    sample.package_name,
                    sample.package_version,
                    sample.package_architecture.as_deref().unwrap_or("")
                )
            })
            .collect::<std::collections::BTreeSet<_>>()
            .len() as i64;
        let architectures = value
            .samples
            .iter()
            .filter_map(|sample| sample.package_architecture.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let stale_sample_count = value
            .samples
            .iter()
            .filter(|sample| sample.review_artifact_stale)
            .count() as i64;
        let cluster = ClusterSummaryResponse::from(
            conary_core::db::models::ScriptletEvidenceClusterSummary {
                cluster: value.cluster,
                attempt_count,
                unique_package_count,
                architectures,
                stale_sample_count,
            },
        );
        Self {
            cluster,
            samples: value
                .samples
                .into_iter()
                .map(SampleSummaryResponse::from)
                .collect(),
            state_events: value
                .state_events
                .into_iter()
                .map(StateEventResponse::from)
                .collect(),
            notes: value.notes.into_iter().map(NoteResponse::from).collect(),
        }
    }
}

impl From<conary_core::db::models::ScriptletEvidenceSample> for SampleSummaryResponse {
    fn from(value: conary_core::db::models::ScriptletEvidenceSample) -> Self {
        Self {
            id: value.id,
            package_name: value.package_name,
            package_version: value.package_version,
            package_architecture: value.package_architecture,
            publication_status: value.publication_status,
            scriptlet_fidelity: value.scriptlet_fidelity,
            target_compatibility: value.target_compatibility,
            reason_codes: parse_json_array(&value.reason_codes_json),
            blocked_classes: parse_json_array(&value.blocked_classes_json),
            boot_security_intents:
                crate::server::scriptlet_evidence_queue::normalization::sanitize_boot_security_intents_value(
                    &value.boot_security_intents_json,
                ),
            review_artifact_available: value.review_artifact_path.is_some(),
            review_artifact_stale: value.review_artifact_stale,
            evidence_digest: value.evidence_digest,
            curation_evidence_digest: value.curation_evidence_digest,
            observed_at: value.observed_at,
        }
    }
}

impl From<conary_core::db::models::ScriptletEvidenceStateEvent> for StateEventResponse {
    fn from(value: conary_core::db::models::ScriptletEvidenceStateEvent) -> Self {
        Self {
            id: value.id,
            from_state: value.from_state.map(|state| state.as_str().to_string()),
            to_state: value.to_state.as_str().to_string(),
            actor: value.actor,
            reason: value.reason,
            created_at: value.created_at,
        }
    }
}

impl From<conary_core::db::models::ScriptletEvidenceNote> for NoteResponse {
    fn from(value: conary_core::db::models::ScriptletEvidenceNote) -> Self {
        Self {
            id: value.id,
            actor: value.actor,
            body: value.body,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

fn parse_json_array(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

#[cfg(test)]
mod tests;
