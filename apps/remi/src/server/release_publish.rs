// apps/remi/src/server/release_publish.rs
//! Remi release artifact upload, gate enforcement, and public metadata commit.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::native_publish::{self, NativePublishError};

const MAX_RELEASE_UPLOAD_SIZE: u64 = 512 * 1024 * 1024;
const RELEASE_PUBLISH_POLICY_DIGEST: &str = "m2-static-publish-policy-v1";

#[derive(Debug, Serialize)]
pub struct ReleaseUploadResponse {
    status: &'static str,
    distro: String,
    package: String,
    version: String,
    release: String,
    architecture: String,
    path: String,
    size: u64,
    content_hash: String,
}

#[derive(Debug)]
struct ReleaseUploadError {
    status: StatusCode,
    code: String,
    message: String,
}

impl ReleaseUploadError {
    fn bad_request(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.into(),
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<NativePublishError> for ReleaseUploadError {
    fn from(error: NativePublishError) -> Self {
        Self {
            status: error.status,
            code: error.code.as_str().to_string(),
            message: error.message,
        }
    }
}

struct StagedRelease {
    path: PathBuf,
}

pub async fn handle_release_upload(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: Request,
) -> Response {
    match release_upload_inner(state, distro, request).await {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(error) => release_upload_error_response(error),
    }
}

async fn release_upload_inner(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: Request,
) -> Result<ReleaseUploadResponse, ReleaseUploadError> {
    native_publish::verify::validate_supported_release_distro(&distro)
        .map_err(ReleaseUploadError::from)?;
    let staged = stage_release_body(&state, request).await?;
    let result = release_upload_after_stage(&state, &distro, &staged).await;
    let _ = tokio::fs::remove_file(&staged.path).await;
    result
}

async fn release_upload_after_stage(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
    staged: &StagedRelease,
) -> Result<ReleaseUploadResponse, ReleaseUploadError> {
    let (cache_dir, chunk_dir, release_publish) = {
        let guard = state.read().await;
        (
            guard.config.cache_dir.clone(),
            guard.config.chunk_dir.clone(),
            guard.config.release_publish.clone(),
        )
    };
    let accepted = native_publish::verify::accepted_release_signers(&release_publish)
        .map_err(ReleaseUploadError::from)?;
    let artifact_path = staged.path.clone();
    let route_slug = distro.to_string();
    let artifact = tokio::task::spawn_blocking(move || {
        native_publish::verify::verify_native_artifact(
            &artifact_path,
            &route_slug,
            &accepted,
            RELEASE_PUBLISH_POLICY_DIGEST,
        )
    })
    .await
    .map_err(|error| {
        ReleaseUploadError::internal(
            format!("join native release verification task: {error}"),
            "INTERNAL_ERROR",
        )
    })?
    .map_err(ReleaseUploadError::from)?;

    let response_package = artifact.name.clone();
    let response_version = artifact.version.clone();
    let response_release = artifact.package_release.clone();
    let response_architecture = artifact.architecture.clone();
    let response_size = artifact.total_size;
    let response_content_hash = artifact.content_hash.clone();
    let promoted = native_publish::storage::promote_native_artifact(
        &cache_dir,
        &chunk_dir,
        distro,
        &staged.path,
        &artifact,
    )
    .await
    .map_err(ReleaseUploadError::from)?;
    let response_path = promoted.package_path.to_string_lossy().to_string();
    let promoted_for_cleanup = promoted.clone();
    let commit =
        native_publish::persistence::commit_native_publication(state, distro, artifact, promoted)
            .await;
    if let Err(error) = commit {
        promoted_for_cleanup.cleanup_public_objects().await;
        return Err(ReleaseUploadError::from(error));
    }

    Ok(ReleaseUploadResponse {
        status: "created",
        distro: distro.to_string(),
        package: response_package,
        version: response_version,
        release: response_release,
        architecture: response_architecture,
        path: response_path,
        size: response_size,
        content_hash: response_content_hash,
    })
}

async fn stage_release_body(
    state: &Arc<RwLock<ServerState>>,
    request: Request,
) -> Result<StagedRelease, ReleaseUploadError> {
    let cache_dir = state.read().await.config.cache_dir.clone();
    let staging_dir = cache_dir.join("releases").join("staging");
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .map_err(|error| {
            ReleaseUploadError::internal(
                format!("create release staging directory: {error}"),
                "IO_ERROR",
            )
        })?;

    let path = staging_dir.join(format!("release-{}.ccs", uuid::Uuid::new_v4().simple()));
    let mut file = tokio::fs::File::create(&path).await.map_err(|error| {
        ReleaseUploadError::internal(format!("create staged release body: {error}"), "IO_ERROR")
    })?;

    let mut size = 0u64;
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|error| {
            ReleaseUploadError::bad_request(format!("invalid upload body: {error}"), "INVALID_BODY")
        })?;
        size += bytes.len() as u64;
        if size > MAX_RELEASE_UPLOAD_SIZE {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ReleaseUploadError {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "PAYLOAD_TOO_LARGE".to_string(),
                message: "Upload exceeds maximum size (512 MB)".to_string(),
            });
        }
        if let Err(error) = file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ReleaseUploadError::internal(
                format!("write staged release body: {error}"),
                "IO_ERROR",
            ));
        }
    }

    if size == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ReleaseUploadError::bad_request(
            "Package body must not be empty",
            "INVALID_BODY",
        ));
    }

    if let Err(error) = file.flush().await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ReleaseUploadError::internal(
            format!("flush staged release body: {error}"),
            "IO_ERROR",
        ));
    }

    Ok(StagedRelease { path })
}

fn release_upload_error_response(error: ReleaseUploadError) -> Response {
    (
        error.status,
        Json(serde_json::json!({
            "error": error.message,
            "code": error.code,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::native_publish::test_support::assert_json_code;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use conary_core::ccs::attestation::{
        BUILD_ATTESTATION_SCHEMA_V1, BuildAttestationPayload, BuildOutputIdentity,
        canonical_json_hash, compute_v3_content_identity, compute_v3_file_merkle_root,
        sign_build_attestation,
    };
    use conary_core::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests;
    use conary_core::ccs::signing::SigningKeyPair;
    use conary_core::ccs::v3::schema::{
        AuthorityDocumentV3, ComponentAuthorityV3, ConflictPolicyV3, FORMAT_VERSION_V3,
        FileAuthorityV3, LifecycleAuthorityV3, PackageDataV3, PackageIdentityV3, PackageKindTagV3,
        PackageKindV3, PackagePolicyV3, ProvenanceAuthorityV3,
    };
    use conary_core::db::schema;
    use conary_core::payload::{PayloadContentAuthority, PayloadNode};
    use conary_core::recipe::hermetic::{
        BuildInputIdentity, BuilderEnvironmentIdentity, BuilderEnvironmentKind, DependencyLock,
        DivergenceReport, HERMETIC_EVIDENCE_SCHEMA, HermeticBuildEvidence, RecipeIdentity,
        ReproducibilityRecord, SourceIdentity,
    };
    use rusqlite::params;
    use std::collections::BTreeMap;
    use std::path::Path;
    use tower::ServiceExt;

    const TEST_DISTRO: &str = "fedora";
    const TEST_PROFILE: &str = "fedora-44";

    struct ReleaseFixture {
        _temp: tempfile::TempDir,
        app: axum::Router,
        db_path: PathBuf,
        chunk_dir: PathBuf,
        keys_dir: PathBuf,
    }

    impl ReleaseFixture {
        fn new(trusted: Vec<crate::server::config::TrustedBuildAttestationSigner>) -> Self {
            Self::new_with_tuf_roles(trusted, &["targets", "snapshot", "timestamp"])
        }

        fn new_with_tuf_roles(
            trusted: Vec<crate::server::config::TrustedBuildAttestationSigner>,
            tuf_roles: &[&str],
        ) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let db_path = temp.path().join("remi.db");
            let chunk_dir = temp.path().join("chunks");
            let cache_dir = temp.path().join("cache");
            let keys_dir = temp.path().join("keys");
            std::fs::create_dir_all(&chunk_dir).unwrap();
            std::fs::create_dir_all(&cache_dir).unwrap();
            write_tuf_role_keys(&keys_dir, TEST_PROFILE, tuf_roles);

            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .unwrap();
            schema::ensure_current(&conn).unwrap();
            drop(conn);

            let release_publish = crate::server::config::ReleasePublishSection {
                repository_keys_dir: Some(keys_dir.clone()),
                trusted_build_attestation_signers: trusted,
            };
            let config = crate::server::ServerConfig {
                db_path: db_path.clone(),
                chunk_dir: chunk_dir.clone(),
                cache_dir,
                release_publish,
                ..Default::default()
            };
            let state = Arc::new(RwLock::new(
                crate::server::ServerState::new(config).expect("test server state"),
            ));
            let app = crate::server::routes::create_external_admin_router(state, None);
            seed_admin_token(&db_path);

            Self {
                _temp: temp,
                app,
                db_path,
                chunk_dir,
                keys_dir,
            }
        }

        async fn upload_release(&self, bytes: Vec<u8>) -> Response {
            self.upload_release_to_distro(TEST_DISTRO, bytes).await
        }

        async fn upload_release_to_distro(&self, distro: &str, bytes: Vec<u8>) -> Response {
            self.app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/v1/admin/releases/{distro}"))
                        .header("Authorization", "Bearer test-admin-token-12345")
                        .body(Body::from(bytes))
                        .unwrap(),
                )
                .await
                .unwrap()
        }

        fn converted_package_row_exists(&self, package: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM converted_packages
                     WHERE source_profile = ?1 AND package_name = ?2",
                    params![TEST_PROFILE, package],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }

        fn native_publication_row_exists(&self, package: &str, package_release: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM native_package_publications
                     WHERE source_profile = ?1 AND name = ?2 AND package_release = ?3
                       AND status = 'public'",
                    params![TEST_PROFILE, package, package_release],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }

        fn native_status_count(&self, package: &str, status: &str) -> i64 {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM native_package_publications
                 WHERE source_profile = ?1 AND name = ?2 AND status = ?3",
                params![TEST_PROFILE, package, status],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn public_package_detail_exists(&self, package: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE r.name = ?1 AND rp.name = ?2",
                    params![TEST_DISTRO, package],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }

        fn public_chunk_exists(&self, content_hash: &str) -> bool {
            crate::server::handlers::cas_object_path(&self.chunk_dir, content_hash).exists()
        }

        fn tuf_target_exists(&self, package: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tuf_targets
                     WHERE target_path LIKE ?1",
                    params![format!("%{package}%")],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }

        fn tuf_target_hash_exists(&self, content_hash: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tuf_targets
                     WHERE repository_id = (
                         SELECT id FROM repositories WHERE name = ?1
                     ) AND sha256 = ?2",
                    params![TEST_DISTRO, content_hash],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }

        fn remove_tuf_role_key(&self, role: &str) {
            let distro_dir = self.keys_dir.join(TEST_PROFILE);
            let _ = std::fs::remove_file(distro_dir.join(format!("{role}.private")));
            let _ = std::fs::remove_file(distro_dir.join(format!("{role}.public")));
        }
    }

    #[tokio::test]
    async fn release_upload_empty_trusted_signers_fail_closed() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new(Vec::new());

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = response_text(response).await;
        assert!(body.contains("no trusted release signers configured"));
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn remi_release_parity_rejected_upload_leaves_no_public_state() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let trusted_other = SigningKeyPair::generate().with_key_id("other");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&trusted_other)]);

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn remi_release_parity_commit_failure_after_promotion_cleans_public_objects() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new_with_tuf_roles(
            vec![trusted_signer(&signer)],
            &["targets", "timestamp"],
        );

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn release_upload_with_accepted_signer_publishes_public_metadata() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            response_text(response).await
        );
        assert!(!fixture.converted_package_row_exists("hello"));
        assert!(fixture.native_publication_row_exists("hello", "1"));
        assert!(fixture.public_package_detail_exists("hello"));
        assert!(fixture.public_chunk_exists(&artifact.content_hash));
        assert!(fixture.tuf_target_exists("hello"));
    }

    #[tokio::test]
    async fn release_upload_with_supported_lifecycle_publishes_public_metadata() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact_with_lifecycle(
            &signer,
            "service-ok",
            "1.0.0",
            "1",
            b"service payload",
            LifecycleAuthorityV3 {
                services: vec![conary_core::ccs::v3::schema::LifecycleServiceV3 {
                    name: "conary-example.service".to_string(),
                    action: conary_core::ccs::v3::schema::LifecycleServiceActionV3::Restart,
                    reversible: None,
                }],
                ..Default::default()
            },
        );
        let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            response_text(response).await
        );
        assert!(fixture.native_publication_row_exists("service-ok", "1"));
        assert!(fixture.public_package_detail_exists("service-ok"));
        assert!(fixture.public_chunk_exists(&artifact.content_hash));
        assert!(fixture.tuf_target_exists("service-ok"));
    }

    #[tokio::test]
    async fn release_upload_accepts_source_independent_lifecycle_without_route_allowlist() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact_with_lifecycle(
            &signer,
            "service-arbitrary",
            "1.0.0",
            "1",
            b"service payload",
            LifecycleAuthorityV3 {
                services: vec![conary_core::ccs::v3::schema::LifecycleServiceV3 {
                    name: "other.service".to_string(),
                    action: conary_core::ccs::v3::schema::LifecycleServiceActionV3::Restart,
                    reversible: None,
                }],
                ..Default::default()
            },
        );
        let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            response_text(response).await
        );
        assert!(fixture.native_publication_row_exists("service-arbitrary", "1"));
        assert!(fixture.public_package_detail_exists("service-arbitrary"));
        assert!(fixture.public_chunk_exists(&artifact.content_hash));
        assert!(fixture.tuf_target_exists("service-arbitrary"));
    }

    #[tokio::test]
    async fn release_upload_rejects_local_dev_artifact_before_public_state() {
        let local_dev = SigningKeyPair::generate().with_key_id("local-dev");
        let artifact = local_dev_release_artifact(&local_dev, "hello", "1.0.0");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&local_dev)]);

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = response_text(response).await;
        assert_json_code(&body, "PUBLISH_GATE_FAILED");
        assert!(
            body.contains("build attestation") || body.contains("local-dev"),
            "{body}"
        );
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn release_upload_unsupported_distro_fails_before_storage() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact =
            attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"payload");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

        let response = fixture
            .upload_release_to_distro("not-a-target", artifact.bytes)
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_text(response).await;
        assert_json_code(&body, "UNKNOWN_DISTRIBUTION");
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn native_release_replacement_supersedes_old_row_and_target() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let first =
            attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"first");
        let second =
            attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"second");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

        assert_eq!(
            fixture.upload_release(first.bytes).await.status(),
            StatusCode::CREATED
        );
        assert_eq!(
            fixture.upload_release(second.bytes).await.status(),
            StatusCode::CREATED
        );

        assert_eq!(fixture.native_status_count("hello", "public"), 1);
        assert_eq!(fixture.native_status_count("hello", "superseded"), 1);
        assert!(!fixture.tuf_target_hash_exists(&first.content_hash));
        assert!(fixture.tuf_target_hash_exists(&second.content_hash));
    }

    #[tokio::test]
    async fn native_release_replacement_failure_keeps_last_public_row() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let first =
            attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"first");
        let second =
            attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"second");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

        assert_eq!(
            fixture.upload_release(first.bytes).await.status(),
            StatusCode::CREATED
        );
        fixture.remove_tuf_role_key("snapshot");
        assert_eq!(
            fixture.upload_release(second.bytes).await.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        assert_eq!(fixture.native_status_count("hello", "public"), 1);
        assert!(fixture.tuf_target_hash_exists(&first.content_hash));
        assert!(!fixture.tuf_target_hash_exists(&second.content_hash));
        assert!(fixture.public_chunk_exists(&first.content_hash));
        assert!(!fixture.public_chunk_exists(&second.content_hash));
    }

    fn assert_no_public_state(fixture: &ReleaseFixture, package: &str, content_hash: &str) {
        assert!(!fixture.converted_package_row_exists(package));
        assert!(!fixture.native_publication_row_exists(package, "1"));
        assert!(!fixture.public_package_detail_exists(package));
        assert!(!fixture.public_chunk_exists(content_hash));
        assert!(!fixture.tuf_target_exists(package));
    }

    async fn response_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn trusted_signer(
        key: &SigningKeyPair,
    ) -> crate::server::config::TrustedBuildAttestationSigner {
        crate::server::config::TrustedBuildAttestationSigner {
            key_id: key.key_id().unwrap_or("publisher").to_string(),
            public_key: key.public_key_base64(),
        }
    }

    fn seed_admin_token(db_path: &Path) {
        let token = "test-admin-token-12345";
        let hash = crate::server::auth::hash_token(token);
        let conn = rusqlite::Connection::open(db_path).unwrap();
        conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin").unwrap();
    }

    fn write_tuf_role_keys(keys_dir: &Path, distro: &str, roles: &[&str]) {
        use std::os::unix::fs::PermissionsExt;

        let distro_dir = keys_dir.join(distro);
        std::fs::create_dir_all(&distro_dir).unwrap();
        std::fs::set_permissions(keys_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&distro_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        for role in roles {
            SigningKeyPair::generate()
                .with_key_id(role)
                .save_to_files(
                    &distro_dir.join(format!("{role}.private")),
                    &distro_dir.join(format!("{role}.public")),
                )
                .unwrap();
        }
    }

    struct TestArtifact {
        bytes: Vec<u8>,
        content_hash: String,
    }

    fn attested_release_artifact(
        signer: &SigningKeyPair,
        name: &str,
        version: &str,
    ) -> TestArtifact {
        attested_release_artifact_with_release(signer, name, version, "1", b"release payload")
    }

    fn attested_release_artifact_with_release(
        signer: &SigningKeyPair,
        name: &str,
        version: &str,
        release: &str,
        payload: &[u8],
    ) -> TestArtifact {
        attested_release_artifact_with_lifecycle(
            signer,
            name,
            version,
            release,
            payload,
            LifecycleAuthorityV3::default(),
        )
    }

    fn attested_release_artifact_with_lifecycle(
        signer: &SigningKeyPair,
        name: &str,
        version: &str,
        release: &str,
        payload: &[u8],
        lifecycle: LifecycleAuthorityV3,
    ) -> TestArtifact {
        let temp = tempfile::tempdir().unwrap();
        let evidence = sample_hermetic_evidence_for_tests(name, version);
        let hermetic_evidence_hash = canonical_json_hash(&evidence).unwrap();
        let payload_path = "/usr/share/payload".to_string();
        let payload_hash = conary_core::hash::sha256(payload);
        let payloads = BTreeMap::from([(payload_path.clone(), payload.to_vec())]);
        let authority = AuthorityDocumentV3 {
            format_version: FORMAT_VERSION_V3,
            identity: PackageIdentityV3 {
                name: name.to_string(),
                version: version.to_string(),
                version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
                release: release.to_string(),
                architecture: Some("x86_64".to_string()),
                debian_multi_arch: None,
                platform: Some("linux".to_string()),
                kind: PackageKindTagV3::Package,
            },
            kind: PackageKindV3::Package(PackageDataV3 {
                files: vec![FileAuthorityV3 {
                    path: payload_path,
                    node: PayloadNode::regular(0o644),
                    content: Some(PayloadContentAuthority {
                        sha256: payload_hash,
                        size: payload.len() as u64,
                    }),
                    component: "main".to_string(),
                    config: None,
                    conflict: ConflictPolicyV3::Error,
                }],
                config: Vec::new(),
                policy: PackagePolicyV3::default(),
            }),
            provided_capabilities: Vec::new(),
            requirements: Vec::new(),
            relations: Vec::new(),
            execution_capabilities: None,
            file_capabilities: Vec::new(),
            components: BTreeMap::from([(
                "main".to_string(),
                ComponentAuthorityV3 {
                    name: "main".to_string(),
                    default: true,
                    file_count: 1,
                    total_size: payload.len() as u64,
                },
            )]),
            lifecycle,
            provenance: ProvenanceAuthorityV3 {
                origin_class: Some("native-built".to_string()),
                hardening_level: Some("hermetic".to_string()),
                build_input_identity: Some("sha256:build-input".to_string()),
                hermetic_evidence_hash: Some(hermetic_evidence_hash.clone()),
                foreign_conversion_boundary_hash: None,
            },
            debug_toml_sha256: None,
        };
        let output_identity = BuildOutputIdentity {
            file_merkle_root: compute_v3_file_merkle_root(&authority).unwrap(),
            package_name: name.to_string(),
            package_version: version.to_string(),
            package_release: release.to_string(),
            architecture: Some("x86_64".to_string()),
            origin_class: "native-built".to_string(),
            hardening_level: "hermetic".to_string(),
            hermetic_evidence_hash: hermetic_evidence_hash.clone(),
            canonical_content_identity: compute_v3_content_identity(&authority).unwrap(),
        };
        let payload = BuildAttestationPayload {
            schema_version: BUILD_ATTESTATION_SCHEMA_V1,
            origin_class: output_identity.origin_class.clone(),
            hardening_level: output_identity.hardening_level.clone(),
            build_input: evidence.build_input.clone(),
            dependency_lock: evidence.dependency_lock.clone(),
            hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
            output_identity,
            build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
            scriptlet_risk_report_hash: None,
            conversion_boundary_hash: None,
            publish_policy_digest: RELEASE_PUBLISH_POLICY_DIGEST.to_string(),
            command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
            sandbox_profile: "kitchen-pristine-network-none".to_string(),
            seccomp_profile: None,
            builder_identity: "remi-release-test-builder".to_string(),
            conary_version: "test".to_string(),
            issued_at: "2026-06-14T00:00:00Z".to_string(),
        };
        let envelope = sign_build_attestation(payload, signer).unwrap();
        let package_path = temp.path().join("release.ccs");
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority,
            &payloads,
            &package_path,
            signer,
            None,
            Some(&envelope),
            None,
        )
        .unwrap();
        let bytes = std::fs::read(package_path).unwrap();
        let content_hash = conary_core::hash::sha256(&bytes);
        TestArtifact {
            bytes,
            content_hash,
        }
    }

    fn local_dev_release_artifact(
        signer: &SigningKeyPair,
        name: &str,
        version: &str,
    ) -> TestArtifact {
        let temp = tempfile::tempdir().unwrap();
        let evidence = sample_hermetic_evidence_for_tests(name, version);
        let hermetic_evidence_hash = canonical_json_hash(&evidence).unwrap();
        let payload = b"local-dev payload";
        let payload_path = "/usr/share/payload".to_string();
        let payload_hash = conary_core::hash::sha256(payload);
        let payloads = BTreeMap::from([(payload_path.clone(), payload.to_vec())]);
        let authority = AuthorityDocumentV3 {
            format_version: FORMAT_VERSION_V3,
            identity: PackageIdentityV3 {
                name: name.to_string(),
                version: version.to_string(),
                version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
                release: "1".to_string(),
                architecture: Some("x86_64".to_string()),
                debian_multi_arch: None,
                platform: Some("linux".to_string()),
                kind: PackageKindTagV3::Package,
            },
            kind: PackageKindV3::Package(PackageDataV3 {
                files: vec![FileAuthorityV3 {
                    path: payload_path,
                    node: PayloadNode::regular(0o644),
                    content: Some(PayloadContentAuthority {
                        sha256: payload_hash,
                        size: payload.len() as u64,
                    }),
                    component: "main".to_string(),
                    config: None,
                    conflict: ConflictPolicyV3::Error,
                }],
                config: Vec::new(),
                policy: PackagePolicyV3::default(),
            }),
            provided_capabilities: Vec::new(),
            requirements: Vec::new(),
            relations: Vec::new(),
            execution_capabilities: None,
            file_capabilities: Vec::new(),
            components: BTreeMap::from([(
                "main".to_string(),
                ComponentAuthorityV3 {
                    name: "main".to_string(),
                    default: true,
                    file_count: 1,
                    total_size: payload.len() as u64,
                },
            )]),
            lifecycle: LifecycleAuthorityV3::default(),
            provenance: ProvenanceAuthorityV3 {
                origin_class: Some("native-built".to_string()),
                hardening_level: Some("hermetic".to_string()),
                build_input_identity: Some("sha256:build-input".to_string()),
                hermetic_evidence_hash: Some(hermetic_evidence_hash),
                foreign_conversion_boundary_hash: None,
            },
            debug_toml_sha256: None,
        };
        let package_path = temp.path().join("local-dev.ccs");
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority,
            &payloads,
            &package_path,
            signer,
            None,
            None,
            None,
        )
        .unwrap();
        let bytes = std::fs::read(package_path).unwrap();
        let content_hash = conary_core::hash::sha256(&bytes);
        TestArtifact {
            bytes,
            content_hash,
        }
    }

    fn sample_hermetic_evidence_for_tests(name: &str, version: &str) -> HermeticBuildEvidence {
        HermeticBuildEvidence {
            schema_version: HERMETIC_EVIDENCE_SCHEMA,
            build_input: BuildInputIdentity {
                recipe: RecipeIdentity::ExplicitRecipe {
                    path: "recipe.toml".to_string(),
                    hash: conary_core::hash::sha256_prefixed(
                        format!("{name}:{version}").as_bytes(),
                    ),
                },
                source: SourceIdentity::Archive {
                    url: "https://example.invalid/source.tar.gz".to_string(),
                    checksum: "sha256:source".to_string(),
                },
                additional_sources: Vec::new(),
                patches: Vec::new(),
                local_tree: None,
                builder_environment: BuilderEnvironmentIdentity {
                    kind: BuilderEnvironmentKind::Pristine,
                    sysroot_hash: Some("sha256:sysroot".to_string()),
                    toolchain_hash: None,
                    diagnostics: Vec::new(),
                },
            },
            dependency_lock: DependencyLock::default(),
            command_risk: conary_core::recipe::hermetic::BuildCommandRiskReport::no_findings(),
            reproducibility: ReproducibilityRecord {
                source_date_epoch: Some(1),
                path_remap_count: 1,
                env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
            },
            divergence: DivergenceReport::default(),
            diagnostics: Vec::new(),
        }
    }
}
