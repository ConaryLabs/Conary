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
mod tests {
    use super::super::test_helpers::{rebuild_app, test_app};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use conary_core::ccs::convert::ScriptletBundleSummary;
    use conary_core::db::models::{
        ConvertedPackage, NewScriptletEvidenceCluster, NewScriptletEvidenceSample,
        ScriptletEvidenceCluster, ScriptletEvidenceSample,
    };
    use tower::ServiceExt;

    fn seed_blocked_converted_package(db_path: &std::path::Path) {
        let conn = rusqlite::Connection::open(db_path).unwrap();
        let summary = ScriptletBundleSummary {
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            publication_status: "blocked".to_string(),
            blocked_reason_codes: vec!["network-egress".to_string()],
            blocked_classes: vec!["network".to_string()],
            ..ScriptletBundleSummary::default()
        };
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "blocked-pkg".to_string(),
            "1.0.0".to_string(),
            "rpm".to_string(),
            "sha256:blocked-pkg".to_string(),
            "partial".to_string(),
            &["sha256:chunk".to_string()],
            42,
            "sha256:content".to_string(),
            "/tmp/blocked-pkg.ccs".to_string(),
        );
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(&conn).unwrap();
    }

    #[tokio::test]
    async fn scriptlet_evidence_backfill_requires_admin_scope() {
        let (app, _db_path) = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scriptlet-evidence/backfill")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"limit": 100}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn scriptlet_evidence_backfill_materializes_existing_rows() {
        let (app, db_path) = test_app().await;
        seed_blocked_converted_package(&db_path);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scriptlet-evidence/backfill")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"limit": 100}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let sample_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scriptlet_evidence_cluster_samples",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sample_count, 1);
    }

    #[tokio::test]
    async fn scriptlet_evidence_backfill_accepts_missing_request_body() {
        let (app, db_path) = test_app().await;
        seed_blocked_converted_package(&db_path);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scriptlet-evidence/backfill")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let sample_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scriptlet_evidence_cluster_samples",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sample_count, 1);
    }

    fn seed_evidence_cluster(db_path: &std::path::Path) {
        let conn = rusqlite::Connection::open(db_path).unwrap();
        let cluster = NewScriptletEvidenceCluster {
            cluster_key: "s1-test".to_string(),
            schema_version: 1,
            distro: "fedora".to_string(),
            target_profile: "fedora-44".to_string(),
            blocked_class: "initramfs".to_string(),
            command: "dracut".to_string(),
            normalized_command_shape: "dracut --force <boot>/initramfs-<kver>.img".to_string(),
            normalized_command_shape_hash: "shape-hash".to_string(),
            lifecycle_phase: "postinstall".to_string(),
        };
        ScriptletEvidenceCluster::upsert(&conn, &cluster).unwrap();
        ScriptletEvidenceSample::upsert(
            &conn,
            &NewScriptletEvidenceSample {
                cluster_key: "s1-test".to_string(),
                converted_package_id: None,
                original_checksum: "sha256:sample".to_string(),
                distro: "fedora".to_string(),
                package_name: "kernel".to_string(),
                package_version: "1.0.0".to_string(),
                package_architecture: Some("x86_64".to_string()),
                publication_status: "blocked".to_string(),
                scriptlet_fidelity: "blocked".to_string(),
                target_compatibility: "blocked".to_string(),
                reason_codes_json: r#"["boot-security-initramfs"]"#.to_string(),
                blocked_classes_json: r#"["initramfs"]"#.to_string(),
                boot_security_intents_json: r#"[{"command":"semodule","argv":["--module=/tmp/foo.pp","--install=/home/remi/private.pp","SECRET=/home/remi/token"],"lifecycle_paths":["/home/remi/private.pp"]}]"#.to_string(),
                review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
                review_artifact_stale: true,
                evidence_digest: Some("sha256:evidence".to_string()),
                curation_evidence_digest: None,
            },
        )
        .unwrap();
    }

    async fn response_body(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn scriptlet_evidence_admin_list_rejects_missing_and_insufficient_scope() {
        let (app, db_path) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let hash = crate::server::auth::hash_token("repos-read-token");
        conary_core::db::models::admin_token::create(&conn, "reader", &hash, "repos:read").unwrap();
        let app = rebuild_app(&db_path);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters")
                    .header("authorization", "Bearer repos-read-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn scriptlet_evidence_admin_lists_clusters_with_stale_counts() {
        let (app, db_path) = test_app().await;
        seed_evidence_cluster(&db_path);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        assert!(body.contains("\"cluster_key\":\"s1-test\""));
        assert!(body.contains("\"stale_sample_count\":1"));
    }

    #[tokio::test]
    async fn scriptlet_evidence_admin_detail_hides_private_paths_and_validates_key() {
        let (app, db_path) = test_app().await;
        seed_evidence_cluster(&db_path);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        assert!(body.contains("\"cluster_key\":\"s1-test\""));
        assert!(body.contains("\"review_artifact_available\":true"));
        assert!(body.contains("--module=<path>"));
        assert!(body.contains("--install=<path>"));
        assert!(body.contains("<env-assignment>"));
        assert!(!body.contains("/tmp/private-review-secret"));
        assert!(!body.contains("/tmp/foo.pp"));
        assert!(!body.contains("/home/remi"));
        assert!(!body.contains("SECRET=/home"));
        assert!(!body.contains("review_artifact_path"));

        let app = rebuild_app(&db_path);
        let invalid = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/bad..key")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let app = rebuild_app(&db_path);
        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-missing")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn scriptlet_evidence_state_updates_events_and_notes_are_admin_only() {
        let (app, db_path) = test_app().await;
        seed_evidence_cluster(&db_path);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test/state")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"state":"adapter-candidate","reason":"repeated dracut shape"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let app = rebuild_app(&db_path);
        let note = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test/notes")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"body":"Check Fedora kernel fixture first."}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(note.status(), StatusCode::OK);

        let app = rebuild_app(&db_path);
        let detail = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response_body(detail).await;
        assert!(body.contains("\"state\":\"adapter-candidate\""));
        assert!(body.contains("repeated dracut shape"));
        assert!(body.contains("Check Fedora kernel fixture first."));
    }

    #[tokio::test]
    async fn scriptlet_evidence_state_and_note_validation_fail_closed() {
        let (app, db_path) = test_app().await;
        seed_evidence_cluster(&db_path);

        let invalid = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test/state")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"state":"public-ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let app = rebuild_app(&db_path);
        let empty_note = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test/notes")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"body":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(empty_note.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn scriptlet_evidence_state_covered_does_not_publish_converted_packages() {
        let (app, db_path) = test_app().await;
        seed_evidence_cluster(&db_path);
        seed_blocked_converted_package(&db_path);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test/state")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"state":"covered-public-ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let status: String = conn
            .query_row(
                "SELECT publication_status FROM converted_packages WHERE package_name = 'blocked-pkg'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "blocked");
    }

    #[tokio::test]
    async fn scriptlet_evidence_packet_private_and_public_visibility_are_sanitized() {
        let (app, db_path) = test_app().await;
        seed_evidence_cluster(&db_path);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::models::ScriptletEvidenceNote::insert(
            &conn,
            "s1-test",
            "maintainer",
            "Private maintainer note",
        )
        .unwrap();

        let private = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test/packet")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(private.status(), StatusCode::OK);
        let private_body = response_body(private).await;
        assert!(private_body.contains("conary.remi.scriptlet-evidence-packet.v1"));
        assert!(private_body.contains("Private maintainer note"));
        assert!(private_body.contains("blocked-class-initramfs"));
        assert!(!private_body.contains("/tmp/private-review-secret"));
        assert!(!private_body.contains("/tmp/foo.pp"));
        assert!(!private_body.contains("/home/remi"));
        assert!(!private_body.contains("SECRET=/home"));

        let app = rebuild_app(&db_path);
        let public = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-test/packet?visibility=public-sanitized")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(public.status(), StatusCode::OK);
        let public_body = response_body(public).await;
        assert!(public_body.contains("\"visibility\":\"public-sanitized\""));
        assert!(!public_body.contains("Private maintainer note"));
        assert!(!public_body.contains("maintainer_notes"));
        assert!(!public_body.contains("review_artifacts"));
        assert!(!public_body.contains("/tmp/"));
        assert!(!public_body.contains("/home/"));
        assert!(!public_body.contains("SECRET=/home"));
        assert!(!public_body.contains("6.10.12-200"));

        let app = rebuild_app(&db_path);
        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-missing/packet")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
