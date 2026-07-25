// apps/remi/src/server/handlers/admin/packages/tests.rs

use crate::server::handlers::admin::test_helpers::{rebuild_app, test_app};
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use conary_core::ccs::builder::write_v2_ccs_package;
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::ccs::v2::schema::{
    AuthorityDocumentV2, ComponentAuthorityV2, ConflictPolicyV2, FORMAT_VERSION_V2,
    FileAuthorityV2, LifecycleAuthorityV2, PackageDataV2, PackageIdentityV2, PackageKindTagV2,
    PackageKindV2, ProvenanceAuthorityV2,
};
use conary_core::payload::{PayloadContentAuthority, PayloadNode};
use conary_core::repository::versioning::VersionScheme;
use std::collections::BTreeMap;
use tower::ServiceExt;

fn minimal_ccs(db_path: &std::path::Path, distro: &str, name: &str, version: &str) -> Vec<u8> {
    let key = SigningKeyPair::load_from_file(
        &db_path
            .parent()
            .unwrap()
            .join("keys")
            .join(distro)
            .join("targets.private"),
    )
    .unwrap();
    signed_ccs(&key, name, version)
}

fn signed_ccs(key: &SigningKeyPair, name: &str, version: &str) -> Vec<u8> {
    let payload_path = format!("/usr/share/{name}/fixture");
    let payload = b"fixture\n".to_vec();
    let authority = AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: name.to_string(),
            version: version.to_string(),
            version_scheme: VersionScheme::Conary,
            release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            platform: Some("linux".to_string()),
            kind: PackageKindTagV2::Package,
        },
        kind: PackageKindV2::Package(PackageDataV2 {
            files: vec![FileAuthorityV2 {
                path: payload_path.clone(),
                node: PayloadNode::regular(0o644),
                content: Some(PayloadContentAuthority {
                    sha256: conary_core::hash::sha256(&payload),
                    size: payload.len() as u64,
                }),
                component: "runtime".to_string(),
                config: None,
                conflict: ConflictPolicyV2::Error,
            }],
            ..PackageDataV2::default()
        }),
        provides: Vec::new(),
        requirements: Vec::new(),
        relations: Vec::new(),
        components: BTreeMap::from([(
            "runtime".to_string(),
            ComponentAuthorityV2 {
                name: "runtime".to_string(),
                default: true,
                file_count: 1,
                total_size: payload.len() as u64,
            },
        )]),
        lifecycle: LifecycleAuthorityV2::default(),
        provenance: ProvenanceAuthorityV2 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("hermetic".to_string()),
            build_input_identity: Some("sha256:test-build-input".to_string()),
            hermetic_evidence_hash: Some("sha256:test-hermetic-evidence".to_string()),
            foreign_conversion_boundary_hash: None,
        },
        debug_toml_sha256: None,
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("package.ccs");
    let payloads = BTreeMap::from([(payload_path, payload)]);
    write_v2_ccs_package(&authority, &payloads, &path, key, None, None, None).unwrap();
    std::fs::read(path).unwrap()
}

async fn assert_status_named(
    label: &str,
    response: axum::response::Response,
    expected: StatusCode,
) {
    let status = response.status();
    if status != expected {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        panic!(
            "{label}: unexpected status {status}, expected {expected}; body: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[tokio::test]
async fn test_upload_package_registers_converted_record() {
    let (app, db_path) = test_app().await;
    let body = minimal_ccs(&db_path, "fedora", "fixture-demo", "1.0.0");
    let body_len = body.len();
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_status_named("fedora upload", response, StatusCode::CREATED).await;

    let conn = conary_core::db::open(&db_path).unwrap();
    let found = conary_core::db::models::ConvertedPackage::find_by_package_identity(
        &conn,
        "fedora",
        "fixture-demo",
        Some("1.0.0"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.original_format, "ccs");
    assert_eq!(found.total_size, Some(body_len as i64));
}

#[tokio::test]
async fn test_upload_package_allows_same_fixture_for_multiple_distros() {
    let (app, db_path) = test_app().await;
    let fedora_body = minimal_ccs(&db_path, "fedora", "fixture-demo", "1.0.0");
    let ubuntu_body = minimal_ccs(&db_path, "ubuntu", "fixture-demo", "1.0.0");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(fedora_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_status_named("fedora upload", response, StatusCode::CREATED).await;

    let response = rebuild_app(&db_path)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/ubuntu")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(ubuntu_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_status_named("ubuntu upload", response, StatusCode::CREATED).await;

    let conn = conary_core::db::open(&db_path).unwrap();
    for distro in ["fedora", "ubuntu"] {
        assert!(
            conary_core::db::models::ConvertedPackage::find_by_package_identity(
                &conn,
                distro,
                "fixture-demo",
                Some("1.0.0"),
            )
            .unwrap()
            .is_some()
        );
    }
}

#[tokio::test]
async fn test_upload_package_rejects_unauthenticated() {
    let (app, _db_path) = test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_package_upload_rejects_untrusted_v2_before_publication() {
    let (app, db_path) = test_app().await;
    let untrusted = SigningKeyPair::generate().with_key_id("untrusted");
    let body = signed_ccs(&untrusted, "untrusted-demo", "1.0.0");
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        conary_core::db::models::ConvertedPackage::find_by_package_identity(
            &conn,
            "fedora",
            "untrusted-demo",
            Some("1.0.0"),
        )
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn admin_package_upload_rejects_unsupported_distro_before_cache_paths() {
    let (app, db_path) = test_app().await;
    let cache_dir = db_path.parent().unwrap().join("cache");
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/debian")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        !cache_dir.join("packages").join("debian").exists(),
        "unsupported distro route must not create cache/packages/debian"
    );
}
