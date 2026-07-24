// apps/remi/src/server/handlers/admin/scriptlet_evidence/reconciliation.rs

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use tokio::sync::RwLock;

use super::super::check_scope;
use crate::server::ServerState;
use crate::server::auth::{Scope, TokenScopes, json_error};

#[derive(Debug, Deserialize)]
pub struct ScriptletEvidenceReconciliationRequest {
    pub dry_run: Option<bool>,
    pub limit: Option<usize>,
    pub after_cluster_key: Option<String>,
}

#[derive(Debug)]
struct ReconciliationOptions {
    dry_run: bool,
    limit: usize,
    after_cluster_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconciliationParamError {
    CursorTooLong,
}

impl ReconciliationParamError {
    fn into_response(self) -> Response {
        match self {
            Self::CursorTooLong => json_error(
                400,
                "after_cluster_key must be at most 256 bytes",
                "INVALID_PARAMETER",
            ),
        }
    }
}

pub async fn reconcile_scriptlet_evidence_unknown_commands(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    request: Option<Json<ScriptletEvidenceReconciliationRequest>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    let options = match reconciliation_options(request) {
        Ok(options) => options,
        Err(error) => return error.into_response(),
    };
    let db_path = {
        let state = state.read().await;
        state.config.db_path.clone()
    };

    match tokio::task::spawn_blocking(move || {
        crate::server::scriptlet_evidence_queue::reconciliation::reconcile_unknown_command_batch(
            &db_path,
            options.dry_run,
            options.limit,
            options.after_cluster_key.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => json_error(
            500,
            &format!("Scriptlet evidence reconciliation failed: {error}"),
            "SCRIPTLET_EVIDENCE_RECONCILIATION_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("Scriptlet evidence reconciliation task failed: {error}"),
            "SCRIPTLET_EVIDENCE_RECONCILIATION_TASK_FAILED",
        ),
    }
}

pub async fn reconcile_scriptlet_evidence_apparmor(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<Extension<TokenScopes>>,
    request: Option<Json<ScriptletEvidenceReconciliationRequest>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    let options = match reconciliation_options(request) {
        Ok(options) => options,
        Err(error) => return error.into_response(),
    };
    let (db_path, cache_dir) = {
        let state = state.read().await;
        (state.config.db_path.clone(), state.config.cache_dir.clone())
    };

    match tokio::task::spawn_blocking(move || {
        crate::server::scriptlet_evidence_queue::reconciliation::reconcile_apparmor_batch(
            &db_path,
            &cache_dir,
            options.dry_run,
            options.limit,
            options.after_cluster_key.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => json_error(
            500,
            &format!("AppArmor evidence reconciliation failed: {error}"),
            "SCRIPTLET_EVIDENCE_APPARMOR_RECONCILIATION_FAILED",
        ),
        Err(error) => json_error(
            500,
            &format!("AppArmor evidence reconciliation task failed: {error}"),
            "SCRIPTLET_EVIDENCE_APPARMOR_RECONCILIATION_TASK_FAILED",
        ),
    }
}

fn reconciliation_options(
    request: Option<Json<ScriptletEvidenceReconciliationRequest>>,
) -> Result<ReconciliationOptions, ReconciliationParamError> {
    let request = request.map(|Json(request)| request);
    let dry_run = request
        .as_ref()
        .and_then(|request| request.dry_run)
        .unwrap_or(true);
    let limit = request
        .as_ref()
        .and_then(|request| request.limit)
        .unwrap_or(500)
        .clamp(1, 5000);
    let after_cluster_key = request.and_then(|request| request.after_cluster_key);
    if after_cluster_key
        .as_ref()
        .is_some_and(|cluster_key| cluster_key.len() > 256)
    {
        return Err(ReconciliationParamError::CursorTooLong);
    }
    Ok(ReconciliationOptions {
        dry_run,
        limit,
        after_cluster_key,
    })
}

#[cfg(test)]
mod tests {
    use super::super::super::test_helpers::test_app;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use conary_core::db::models::{
        NewScriptletEvidenceCluster, NewScriptletEvidenceSample, ScriptletEvidenceCluster,
        ScriptletEvidenceClusterReconciliationLink, ScriptletEvidenceNote, ScriptletEvidenceSample,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn apparmor_reconciliation_defaults_to_private_dry_run() {
        let (app, db_path) = test_app().await;
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        ScriptletEvidenceCluster::upsert(
            &conn,
            &NewScriptletEvidenceCluster {
                cluster_key: "s1-apparmor-legacy".to_string(),
                schema_version: 1,
                normalization_version: 1,
                distro: "ubuntu".to_string(),
                target_profile: "ubuntu-26.04".to_string(),
                blocked_class: "apparmor".to_string(),
                command: "apparmor_parser".to_string(),
                normalized_command_shape: "apparmor_parser -r <path>".to_string(),
                normalized_command_shape_hash: "legacy-shape".to_string(),
                lifecycle_phase: "postinstall".to_string(),
            },
        )
        .unwrap();
        ScriptletEvidenceSample::upsert(
            &conn,
            &NewScriptletEvidenceSample {
                cluster_key: "s1-apparmor-legacy".to_string(),
                converted_package_id: None,
                original_checksum: "sha256:missing".to_string(),
                distro: "ubuntu".to_string(),
                package_name: "apparmor-demo".to_string(),
                package_version: "1.0.0".to_string(),
                package_architecture: Some("amd64".to_string()),
                publication_status: "blocked".to_string(),
                scriptlet_fidelity: "blocked".to_string(),
                target_compatibility: "blocked".to_string(),
                reason_codes_json: r#"["blocked-class-apparmor"]"#.to_string(),
                blocked_classes_json: r#"["apparmor"]"#.to_string(),
                boot_security_intents_json:
                    r#"[{"command":"apparmor_parser","argv":["-r","<path>"]}]"#.to_string(),
                security_policy_intents_json: "[]".to_string(),
                review_artifact_path: Some("/private/review.json".to_string()),
                review_artifact_stale: false,
                evidence_digest: Some("sha256:evidence".to_string()),
                curation_evidence_digest: None,
            },
        )
        .unwrap();
        ScriptletEvidenceNote::insert(
            &conn,
            "s1-apparmor-legacy",
            "maintainer",
            "private rationale",
        )
        .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scriptlet-evidence/reconcile-apparmor")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("\"dry_run\":true"));
        assert!(body.contains("\"unresolved_cluster_count\":1"));
        assert!(body.contains("\"converted-package-missing\""));
        assert!(!body.contains("/private/review.json"));
        assert!(!body.contains("private rationale"));

        let source = ScriptletEvidenceCluster::find(&conn, "s1-apparmor-legacy")
            .unwrap()
            .unwrap();
        assert!(source.superseded_at.is_none());
        assert_eq!(source.normalization_version, 1);
    }

    #[tokio::test]
    async fn cluster_detail_exposes_reconciliation_links() {
        let (app, db_path) = test_app().await;
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        for (cluster_key, normalization_version) in
            [("s1-apparmor-source", 1), ("s1-apparmor-target", 2)]
        {
            ScriptletEvidenceCluster::upsert(
                &conn,
                &NewScriptletEvidenceCluster {
                    cluster_key: cluster_key.to_string(),
                    schema_version: 1,
                    normalization_version,
                    distro: "ubuntu".to_string(),
                    target_profile: "ubuntu-26.04".to_string(),
                    blocked_class: "apparmor".to_string(),
                    command: "apparmor_parser".to_string(),
                    normalized_command_shape: "apparmor_parser -r <path>".to_string(),
                    normalized_command_shape_hash: format!("hash-{cluster_key}"),
                    lifecycle_phase: "postinstall".to_string(),
                },
            )
            .unwrap();
        }
        ScriptletEvidenceClusterReconciliationLink::record(
            &conn,
            "s1-apparmor-source",
            "s1-apparmor-target",
            2,
            4,
        )
        .unwrap();

        let source_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-apparmor-source")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(source_response.status(), StatusCode::OK);
        let source_body = axum::body::to_bytes(source_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let source: serde_json::Value = serde_json::from_slice(&source_body).unwrap();
        assert_eq!(
            source["outgoing_reconciliation_links"][0]["target_cluster_key"],
            "s1-apparmor-target"
        );
        assert_eq!(
            source["outgoing_reconciliation_links"][0]["migrated_sample_count"],
            4
        );
        assert_eq!(
            source["incoming_reconciliation_links"],
            serde_json::json!([])
        );

        let app = super::super::super::test_helpers::rebuild_app(&db_path);
        let target_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/scriptlet-evidence/clusters/s1-apparmor-target")
                    .header("authorization", "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(target_response.status(), StatusCode::OK);
        let target_body = axum::body::to_bytes(target_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let target: serde_json::Value = serde_json::from_slice(&target_body).unwrap();
        assert_eq!(
            target["incoming_reconciliation_links"][0]["source_cluster_key"],
            "s1-apparmor-source"
        );
    }
}
