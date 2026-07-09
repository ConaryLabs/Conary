// apps/remi/src/server/handlers/admin/non_public_test_serving.rs
//! Admin/test serving for converted artifacts that are not public-ready.

use super::{check_scope, validate_path_param, validate_supported_admin_distro_route};
use crate::server::ServerState;
use crate::server::auth::{Scope, TokenScopes, json_error};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

#[derive(Debug, Deserialize)]
pub struct NonPublicTestServingQuery {
    pub version: String,
    #[serde(alias = "architecture")]
    pub arch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NonPublicTestManifestResponse {
    pub status: &'static str,
    pub distro: String,
    pub package: String,
    pub version: String,
    pub arch: Option<String>,
    pub public_ready: bool,
    pub ccs: NonPublicTestCcsInfo,
    pub scriptlets: crate::server::publication::PublicationGateReport,
}

#[derive(Debug, Serialize)]
pub struct NonPublicTestCcsInfo {
    pub content_hash: Option<String>,
    pub total_size: u64,
}

enum NonPublicTestLookup {
    Eligible {
        manifest: Box<NonPublicTestManifestResponse>,
        ccs_path: PathBuf,
    },
    Disabled,
    PublicReady,
    Malformed,
    Stale,
    AmbiguousArchitecture,
    Missing,
}

fn lookup_non_public_test_candidate(
    conn: &rusqlite::Connection,
    distro: &str,
    package: &str,
    version: &str,
    architecture: Option<&str>,
) -> anyhow::Result<Option<conary_core::db::models::ConvertedPackage>> {
    if architecture.is_some() {
        return conary_core::db::models::ConvertedPackage::find_by_package_identity_with_arch(
            conn,
            distro,
            package,
            Some(version),
            architecture,
        )
        .map_err(anyhow::Error::from);
    }

    let current: Vec<_> = conary_core::db::models::ConvertedPackage::find_publication_candidates(
        conn,
        distro,
        Some(package),
    )?
    .into_iter()
    .filter(|converted| converted.package_version.as_deref() == Some(version))
    .collect();

    if current.len() > 1 {
        anyhow::bail!("ambiguous-architecture");
    }
    if let Some(converted) = current.into_iter().next() {
        return Ok(Some(converted));
    }

    conary_core::db::models::ConvertedPackage::find_by_package_identity_with_arch(
        conn,
        distro,
        package,
        Some(version),
        None,
    )
    .map_err(anyhow::Error::from)
}

fn lookup_non_public_test_package_blocking(
    db_path: &FsPath,
    enabled: bool,
    distro: &str,
    package: &str,
    version: &str,
    architecture: Option<&str>,
) -> anyhow::Result<NonPublicTestLookup> {
    if !enabled {
        return Ok(NonPublicTestLookup::Disabled);
    }

    let conn = crate::server::open_runtime_db(db_path)?;
    let converted =
        match lookup_non_public_test_candidate(&conn, distro, package, version, architecture) {
            Ok(Some(converted)) => converted,
            Ok(None) => return Ok(NonPublicTestLookup::Missing),
            Err(error) if error.to_string() == "ambiguous-architecture" => {
                return Ok(NonPublicTestLookup::AmbiguousArchitecture);
            }
            Err(error) => return Err(error),
        };

    if converted.needs_reconversion() {
        return Ok(NonPublicTestLookup::Stale);
    }

    let Some(ccs_path) = converted.ccs_path.as_ref().map(PathBuf::from) else {
        return Ok(NonPublicTestLookup::Missing);
    };
    if !ccs_path.is_file() {
        return Ok(NonPublicTestLookup::Missing);
    }

    let publication = converted.scriptlet_summary_for_publication();
    if !publication.valid {
        return Ok(NonPublicTestLookup::Malformed);
    }

    Ok(
        match crate::server::publication::classify_converted_package(&converted) {
            crate::server::publication::PublicationDecision::Ready => {
                NonPublicTestLookup::PublicReady
            }
            crate::server::publication::PublicationDecision::ReviewRequired(report)
            | crate::server::publication::PublicationDecision::Blocked(report) => {
                NonPublicTestLookup::Eligible {
                    manifest: Box::new(NonPublicTestManifestResponse {
                        status: "non-public-test-serving",
                        distro: converted.distro.unwrap_or_else(|| distro.to_string()),
                        package: converted
                            .package_name
                            .unwrap_or_else(|| package.to_string()),
                        version: converted
                            .package_version
                            .unwrap_or_else(|| version.to_string()),
                        arch: converted.package_architecture,
                        public_ready: false,
                        ccs: NonPublicTestCcsInfo {
                            content_hash: converted.content_hash,
                            total_size: converted.total_size.unwrap_or(0) as u64,
                        },
                        scriptlets: report,
                    }),
                    ccs_path,
                }
            }
        },
    )
}

async fn lookup_non_public_test_package(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    package: String,
    version: String,
    architecture: Option<String>,
) -> Result<NonPublicTestLookup, Response> {
    let (db_path, enabled) = {
        let guard = state.read().await;
        (
            guard.config.db_path.clone(),
            guard.config.non_public_test_serving.enabled,
        )
    };

    tokio::task::spawn_blocking(move || {
        lookup_non_public_test_package_blocking(
            &db_path,
            enabled,
            &distro,
            &package,
            &version,
            architecture.as_deref(),
        )
    })
    .await
    .map_err(|error| {
        tracing::error!("Failed to join non-public test-serving lookup task: {error}");
        json_error(
            500,
            "Failed to query test-serving package",
            "INTERNAL_ERROR",
        )
    })?
    .map_err(|error| {
        tracing::error!("Failed to query non-public test-serving package: {error}");
        json_error(500, "Failed to query test-serving package", "DB_ERROR")
    })
}

fn non_public_test_lookup_error(lookup: NonPublicTestLookup) -> Response {
    match lookup {
        NonPublicTestLookup::Disabled => json_error(
            403,
            "Non-public test serving is disabled",
            "NON_PUBLIC_TEST_SERVING_DISABLED",
        ),
        NonPublicTestLookup::PublicReady => json_error(
            409,
            "Converted package is already public-ready; use the public package route",
            "ALREADY_PUBLIC",
        ),
        NonPublicTestLookup::Malformed => json_error(
            409,
            "Converted package has malformed scriptlet publication metadata",
            "MALFORMED_SCRIPTLET_SUMMARY",
        ),
        NonPublicTestLookup::Stale => json_error(
            409,
            "Converted package needs reconversion",
            "STALE_CONVERSION",
        ),
        NonPublicTestLookup::AmbiguousArchitecture => json_error(
            409,
            "Architecture is required for this package/version",
            "AMBIGUOUS_ARCHITECTURE",
        ),
        NonPublicTestLookup::Missing => json_error(404, "Converted package not found", "NOT_FOUND"),
        NonPublicTestLookup::Eligible { .. } => {
            json_error(500, "Unexpected test-serving state", "INTERNAL_ERROR")
        }
    }
}

fn safe_download_filename(path: &FsPath) -> String {
    let raw = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("package.ccs");
    let safe: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(*ch, '.' | '_' | '-'))
        .collect();
    if safe.is_empty() {
        "package.ccs".to_string()
    } else {
        safe
    }
}

async fn stream_non_public_test_ccs(ccs_path: PathBuf) -> Response {
    let file = match File::open(&ccs_path).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(
                "Failed to open non-public test CCS {}: {}",
                ccs_path.display(),
                error
            );
            return json_error(404, "Converted package file not found", "NOT_FOUND");
        }
    };
    let metadata = match file.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(
                "Failed to inspect non-public test CCS {}: {}",
                ccs_path.display(),
                error
            );
            return json_error(500, "Failed to inspect package file", "IO_ERROR");
        }
    };
    if !metadata.is_file() {
        return json_error(404, "Converted package file not found", "NOT_FOUND");
    }

    let filename = safe_download_filename(&ccs_path);
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| json_error(500, "Failed to stream package file", "INTERNAL_ERROR"))
}

fn validate_non_public_test_request(
    distro: &str,
    package: &str,
    query: &NonPublicTestServingQuery,
    scopes: &Option<axum::Extension<TokenScopes>>,
) -> Option<Response> {
    if let Some(err) = check_scope(scopes, Scope::Admin) {
        return Some(err);
    }
    if let Some(err) = validate_supported_admin_distro_route(distro) {
        return Some(err);
    }
    if let Some(err) = validate_path_param(package, "package") {
        return Some(err);
    }
    if query.version.is_empty() {
        return Some(json_error(400, "Version is required", "INVALID_PARAMETER"));
    }
    None
}

pub async fn get_non_public_test_manifest(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, package)): Path<(String, String)>,
    Query(query): Query<NonPublicTestServingQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = validate_non_public_test_request(&distro, &package, &query, &scopes) {
        return err;
    }

    match lookup_non_public_test_package(state, distro, package, query.version, query.arch).await {
        Ok(NonPublicTestLookup::Eligible { manifest, .. }) => Json(*manifest).into_response(),
        Ok(other) => non_public_test_lookup_error(other),
        Err(response) => response,
    }
}

pub async fn download_non_public_test_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, package)): Path<(String, String)>,
    Query(query): Query<NonPublicTestServingQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = validate_non_public_test_request(&distro, &package, &query, &scopes) {
        return err;
    }

    match lookup_non_public_test_package(state, distro, package, query.version, query.arch).await {
        Ok(NonPublicTestLookup::Eligible { ccs_path, .. }) => {
            stream_non_public_test_ccs(ccs_path).await
        }
        Ok(other) => non_public_test_lookup_error(other),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use crate::server::handlers::admin::test_helpers::test_app;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
    use conary_core::ccs::convert::ScriptletBundleSummary;
    use conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence;
    use conary_core::ccs::security_policy::{
        SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyFallback, SecurityPolicyIntent,
        SecurityPolicyPayloadEvidence, SecurityPolicyProvider, SecurityPolicyReconciliation,
        SecurityPolicyReconciliationState, SecurityPolicyRequirements, SecurityPolicyScope,
        SecurityPolicySource,
    };
    use conary_core::db::models::ConvertedPackage;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    async fn test_app_with_non_public_test_serving(
        enabled: bool,
    ) -> (axum::Router, std::path::PathBuf) {
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
            non_public_test_serving: crate::server::config::NonPublicTestServingSection { enabled },
            ..Default::default()
        };
        std::fs::create_dir_all(&config.chunk_dir).unwrap();
        std::fs::create_dir_all(&config.cache_dir).unwrap();

        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("test server state"),
        ));
        let app = crate::server::routes::create_external_admin_router(state, None);

        let hash = crate::server::auth::hash_token("test-admin-token-12345");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin")
                .unwrap();
        }

        std::mem::forget(tmp);
        (app, db_path)
    }

    fn seed_non_public_test_row_with_summary(
        db_path: &std::path::Path,
        architecture: &str,
        ccs_name: &str,
        mut summary: ScriptletBundleSummary,
        summary_valid: bool,
    ) {
        let ccs_path = db_path.parent().unwrap().join(ccs_name);
        std::fs::write(&ccs_path, b"non-public ccs").unwrap();
        let conn = rusqlite::Connection::open(db_path).unwrap();
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "pkg".to_string(),
            "1.0".to_string(),
            "rpm".to_string(),
            format!("sha256:source-{architecture}"),
            "high".to_string(),
            &["abc".to_string()],
            14,
            format!("sha256:content-{architecture}"),
            ccs_path.to_string_lossy().to_string(),
        );
        converted.package_architecture = Some(architecture.to_string());
        converted.set_scriptlet_metadata(&summary).unwrap();
        if !summary_valid {
            summary.publication_status = "public".to_string();
            converted.scriptlet_summary_json = serde_json::to_string(&summary).unwrap();
        }
        converted.insert(&conn).unwrap();
    }

    fn blocked_summary(status: &str) -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            publication_status: status.to_string(),
            scriptlet_fidelity: status.to_string(),
            target_compatibility: status.to_string(),
            blocked_reason_codes: if status == "blocked" {
                vec!["blocked-class-network".to_string()]
            } else {
                Vec::new()
            },
            review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
            ..ScriptletBundleSummary::default()
        }
    }

    fn local_only_summary() -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            publication_status: "local-only".to_string(),
            scriptlet_fidelity: "local-only".to_string(),
            target_compatibility: "local-only".to_string(),
            review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
            ..ScriptletBundleSummary::default()
        }
    }

    fn issue35_boot_security_summary() -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            evidence_digest: Some("sha256:issue35-evidence".to_string()),
            blocked_reason_codes: vec![
                "blocked-class-kernel-module".to_string(),
                "blocked-class-initramfs".to_string(),
                "blocked-class-bootloader".to_string(),
            ],
            blocked_classes: vec![
                "kernel-module".to_string(),
                "initramfs".to_string(),
                "bootloader".to_string(),
            ],
            boot_security_intents: vec![
                BootSecurityIntentEvidence {
                    class_id: "kernel-module".to_string(),
                    reason_code: "blocked-class-kernel-module".to_string(),
                    command: "depmod".to_string(),
                    argv: vec!["6.10.12-200.fc40.x86_64".to_string()],
                    phase: Some("postinstall".to_string()),
                    lifecycle_paths: vec!["/lib/modules/6.10.12-200.fc40.x86_64".to_string()],
                },
                BootSecurityIntentEvidence {
                    class_id: "initramfs".to_string(),
                    reason_code: "blocked-class-initramfs".to_string(),
                    command: "dracut".to_string(),
                    argv: vec![
                        "--force".to_string(),
                        "/boot/initramfs-6.10.12-200.fc40.x86_64.img".to_string(),
                    ],
                    phase: Some("postinstall".to_string()),
                    lifecycle_paths: vec!["/boot".to_string()],
                },
                BootSecurityIntentEvidence {
                    class_id: "bootloader".to_string(),
                    reason_code: "blocked-class-bootloader".to_string(),
                    command: "grub2-mkconfig".to_string(),
                    argv: vec!["-o".to_string(), "/boot/grub2/grub.cfg".to_string()],
                    phase: Some("postinstall".to_string()),
                    lifecycle_paths: vec!["/boot/grub2/grub.cfg".to_string()],
                },
            ],
            review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
            ..ScriptletBundleSummary::default()
        }
    }

    fn apparmor_review_policy_intent() -> SecurityPolicyIntent {
        SecurityPolicyIntent {
            schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
            id: "scriptlet:0:post-install:apparmor:apparmor_parser".to_string(),
            source: SecurityPolicySource {
                source_format: Some("deb".to_string()),
                source_distro: Some("ubuntu".to_string()),
                entry_id: Some("scriptlet:0:post-install".to_string()),
                command: Some("apparmor_parser".to_string()),
                argv: vec!["-r".to_string(), "/etc/apparmor.d/usr.bin.demo".to_string()],
                adapter_id: None,
            },
            provider: SecurityPolicyProvider::Apparmor,
            operation: "profile-reload".to_string(),
            scope: SecurityPolicyScope {
                kind: "profile".to_string(),
                name: Some("/etc/apparmor.d/usr.bin.demo".to_string()),
                paths: vec!["/etc/apparmor.d/usr.bin.demo".to_string()],
                service: None,
                port: None,
                extra: BTreeMap::new(),
            },
            desired_state: BTreeMap::new(),
            requirements: SecurityPolicyRequirements {
                required_on_active_provider: false,
                provider_mode: None,
                tools: vec!["apparmor_parser".to_string()],
                modules: Vec::new(),
            },
            fallback: SecurityPolicyFallback::BlockOnEnforcingTarget,
            payload_evidence: SecurityPolicyPayloadEvidence {
                payload_backed: true,
                paths: vec!["/etc/apparmor.d/usr.bin.demo".to_string()],
                digest: None,
            },
            reconciliation: SecurityPolicyReconciliation {
                state: SecurityPolicyReconciliationState::Review,
                reason: Some("blocked-class-apparmor".to_string()),
                target_provider: None,
            },
            extra: BTreeMap::new(),
        }
    }

    fn apparmor_review_summary() -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            evidence_digest: Some("sha256:apparmor-review-evidence".to_string()),
            blocked_reason_codes: vec!["blocked-class-apparmor".to_string()],
            blocked_classes: vec!["apparmor".to_string()],
            security_policy_intents: vec![apparmor_review_policy_intent()],
            review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
            ..ScriptletBundleSummary::default()
        }
    }

    fn seed_non_public_test_row(db_path: &std::path::Path, status: &str, summary_valid: bool) {
        seed_non_public_test_row_with_summary(
            db_path,
            "x86_64",
            "pkg.ccs",
            blocked_summary(status),
            summary_valid,
        );
    }

    #[tokio::test]
    async fn non_public_test_serving_manifest_is_disabled_by_default() {
        let (app, db_path) = test_app().await;
        seed_non_public_test_row(&db_path, "blocked", true);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("NON_PUBLIC_TEST_SERVING_DISABLED"));
    }

    #[tokio::test]
    async fn non_public_test_serving_manifest_returns_sanitized_blocked_metadata() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        seed_non_public_test_row(&db_path, "blocked", true);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("\"status\":\"non-public-test-serving\""));
        assert!(body.contains("\"publication_status\":\"blocked\""));
        assert!(body.contains("\"blocked-class-network\""));
        assert!(body.contains("\"review_artifact_available\":true"));
        assert!(!body.contains("review_artifact_path"));
        assert!(!body.contains("private-review-secret"));
    }

    #[tokio::test]
    async fn non_public_test_serving_manifest_and_download_allow_local_only_rows() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        seed_non_public_test_row_with_summary(
            &db_path,
            "x86_64",
            "pkg-local-only.ccs",
            local_only_summary(),
            true,
        );

        let manifest_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(manifest_response.status(), StatusCode::OK);
        let body = to_bytes(manifest_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("\"status\":\"non-public-test-serving\""));
        assert!(body.contains("\"publication_status\":\"local-only\""));
        assert!(body.contains("\"review_artifact_available\":true"));
        assert!(!body.contains("review_artifact_path"));
        assert!(!body.contains("private-review-secret"));

        let download_response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-download?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(download_response.status(), StatusCode::OK);
        assert_eq!(
            download_response
                .headers()
                .get(header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
        let body = to_bytes(download_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"non-public ccs");
    }

    #[tokio::test]
    async fn non_public_test_manifest_preserves_apparmor_review_policy_fallback() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        seed_non_public_test_row_with_summary(
            &db_path,
            "x86_64",
            "pkg-apparmor.ccs",
            apparmor_review_summary(),
            true,
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("\"provider\":\"apparmor\""));
        assert!(body.contains("\"operation\":\"profile-reload\""));
        assert!(body.contains("\"fallback\":\"block-on-enforcing-target\""));
        assert!(body.contains("\"state\":\"review\""));
        assert!(body.contains("\"blocked-class-apparmor\""));
        assert!(!body.contains("review_artifact_path"));
        assert!(!body.contains("private-review-secret"));
    }

    #[tokio::test]
    async fn non_public_test_serving_manifest_sanitizes_private_intent_values() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        let mut summary = apparmor_review_summary();
        summary.security_policy_intents[0]
            .source
            .argv
            .push("SECRET=/home/remi/token".to_string());
        summary.security_policy_intents[0]
            .scope
            .paths
            .push("/home/remi/private.pp".to_string());
        summary.security_policy_intents[0]
            .payload_evidence
            .paths
            .push("/home/remi/private.pp".to_string());
        seed_non_public_test_row_with_summary(&db_path, "x86_64", "pkg.ccs", summary, true);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("\"provider\":\"apparmor\""));
        assert!(body.contains("\"operation\":\"profile-reload\""));
        assert!(body.contains("/etc/apparmor.d/usr.bin.demo"));
        assert!(body.contains("\"<path>\""));
        assert!(!body.contains("/home/remi"));
        assert!(!body.contains("SECRET="));
        assert!(!body.contains("review_artifact_path"));
        assert!(!body.contains("private-review-secret"));
    }

    #[tokio::test]
    async fn non_public_test_manifest_accepts_architecture_alias_for_boot_security_evidence() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        seed_non_public_test_row_with_summary(
            &db_path,
            "x86_64",
            "pkg-x86_64.ccs",
            issue35_boot_security_summary(),
            true,
        );
        seed_non_public_test_row_with_summary(
            &db_path,
            "aarch64",
            "pkg-aarch64.ccs",
            issue35_boot_security_summary(),
            true,
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(
                        "/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&architecture=x86_64",
                    )
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("\"arch\":\"x86_64\""));
        assert!(body.contains("\"blocked-class-kernel-module\""));
        assert!(body.contains("\"blocked-class-initramfs\""));
        assert!(body.contains("\"blocked-class-bootloader\""));
        assert!(body.contains("\"command\":\"depmod\""));
        assert!(body.contains("\"command\":\"dracut\""));
        assert!(body.contains("\"command\":\"grub2-mkconfig\""));
        assert!(body.contains("\"review_artifact_available\":true"));
        assert!(!body.contains("review_artifact_path"));
        assert!(!body.contains("private-review-secret"));
    }

    #[tokio::test]
    async fn non_public_test_serving_manifest_rejects_public_rows() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        seed_non_public_test_row(&db_path, "public", true);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("ALREADY_PUBLIC"));
    }

    #[tokio::test]
    async fn non_public_test_serving_manifest_rejects_malformed_summary() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        seed_non_public_test_row(&db_path, "blocked", false);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("MALFORMED_SCRIPTLET_SUMMARY"));
    }

    #[tokio::test]
    async fn non_public_test_serving_download_streams_blocked_ccs_bytes() {
        let (app, db_path) = test_app_with_non_public_test_serving(true).await;
        seed_non_public_test_row(&db_path, "blocked", true);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/packages/fedora/pkg/test-download?version=1.0&arch=x86_64")
                    .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"non-public ccs");
    }
}
