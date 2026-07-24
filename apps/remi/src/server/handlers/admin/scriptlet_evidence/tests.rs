// apps/remi/src/server/handlers/admin/scriptlet_evidence/tests.rs

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
            typed_evidence: conary_core::db::models::ScriptletEvidenceRecord::new(
                conary_core::db::models::ScriptletEvidenceKind::MalformedSummary,
            ),
            reason_codes_json: r#"["boot-security-initramfs"]"#.to_string(),
            blocked_classes_json: r#"["initramfs"]"#.to_string(),
            boot_security_intents_json: r#"[{"command":"semodule","argv":["--module=/tmp/foo.pp","--install=/home/remi/private.pp","SECRET=/home/remi/token"],"lifecycle_paths":["/home/remi/private.pp"]}]"#.to_string(),
            security_policy_intents_json: r#"[{"provider":"selinux","source":{"command":"semodule","argv":["--module=/tmp/foo.pp","--install=/home/remi/private.pp","SECRET=/home/remi/token"]},"scope":{"kind":"path","paths":["/home/remi/private.pp"]},"payload_evidence":{"payload_backed":true,"paths":["/tmp/foo.pp","/usr/share/selinux/packages/public.pp"]}}]"#.to_string(),
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
    assert!(private_body.contains("\"security_policy_intents\":[{"));
    assert!(private_body.contains("\"provider\":\"selinux\""));
    assert!(private_body.contains("--module=<path>"));
    assert!(private_body.contains("--install=<path>"));
    assert!(private_body.contains("<env-assignment>"));
    assert!(private_body.contains("/usr/share/selinux/packages/public.pp"));
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
    assert!(public_body.contains("\"security_policy_intents\":[{"));
    assert!(public_body.contains("\"provider\":\"selinux\""));
    assert!(public_body.contains("--module=<path>"));
    assert!(public_body.contains("--install=<path>"));
    assert!(public_body.contains("<env-assignment>"));
    assert!(public_body.contains("/usr/share/selinux/packages/public.pp"));
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
