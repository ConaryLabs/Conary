// apps/remi/src/server/handlers/admin/repos/tests.rs

use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::test_helpers::{rebuild_app, test_app, test_app_with_database_writer};

#[tokio::test]
async fn test_repo_crud_lifecycle() {
    let (app, db_path) = test_app().await;

    // Create a repo
    let create_body = serde_json::json!({
        "name": "fedora",
        "url": "https://93.184.216.34/fedora",
        "enabled": true,
        "priority": 10,
        "parser": {"package_format": "json"}
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
        "url": "https://example.org/fedora",
        "priority": 20,
        "parser": {"package_format": "json"}
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
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "repository update response: {}",
        String::from_utf8_lossy(&body_bytes)
    );
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
async fn repository_update_waits_for_the_shared_database_writer() {
    let (app, db_path, database_writer) = test_app_with_database_writer().await;
    let create_body = serde_json::json!({
        "name": "fedora",
        "url": "https://93.184.216.34/fedora",
        "parser": {"package_format": "json"}
    });
    let create_resp = app
        .clone()
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
    assert_eq!(create_resp.status(), StatusCode::CREATED);

    // Hold both the declared writer authority and SQLite's real write lock.
    // The old repository path bypassed the former, waited out the five-second
    // SQLite busy timeout on the latter, and returned HTTP 500. The corrected
    // path must remain queued on the shared owner without touching SQLite.
    let mut blocking_connection = conary_core::db::open_fast(&db_path).unwrap();
    let blocking_transaction = blocking_connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let writer_guard = database_writer.hold_for_test();
    let update_body = serde_json::json!({
        "url": "https://93.184.216.35/fedora",
        "priority": 20,
        "parser": {"package_format": "json"}
    });
    let mut update = tokio::spawn(
        app.oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/v1/admin/repos/fedora")
                .header("Authorization", "Bearer test-admin-token-12345")
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(update_body.to_string()))
                .unwrap(),
        ),
    );

    if let Ok(completed) =
        tokio::time::timeout(std::time::Duration::from_millis(5_500), &mut update).await
    {
        let response = completed.unwrap().unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        panic!(
            "repository update bypassed the shared database writer: {status} {}",
            String::from_utf8_lossy(&body)
        );
    }
    drop(blocking_transaction);
    drop(writer_guard);

    let response = update.await.unwrap().unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "repository update response: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn native_repo_create_requires_and_returns_exact_source_policy() {
    let (app, _db_path) = test_app().await;
    let root = serde_json::json!({
        "url": "https://93.184.216.34/keys/repository.gpg",
        "fingerprint": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    });
    let create_body = serde_json::json!({
        "name": "third-party-rpm",
        "url": "https://93.184.216.34/rpm",
        "parser": {"package_format": "rpm", "architecture": "x86_64"},
        "trust": {
            "ecosystem": "rpm",
            "metadata": {"kind": "open-pgp", "keys": [root.clone()]},
            "package_keys": [root]
        },
        "native_source": {
            "source_identity": "third-party:widgets",
            "repository_identity": "widgets:x86_64",
            "stream_kind": "channel",
            "stream_identity": "stable",
            "update_mode": "follow"
        }
    });
    let response = app
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
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["native_source"]["source_identity"],
        "third-party:widgets"
    );
    assert_eq!(body["native_source"]["update_mode"], "follow");
    assert_eq!(
        body["native_source"]["stream_binding_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
}

#[tokio::test]
async fn test_repo_scope_enforcement() {
    let (app, db_path) = test_app().await;

    // Create a token with only repos:read scope
    let repo_reader_token = "repo-read-only-token-67890";
    let hash = crate::server::auth::hash_token(repo_reader_token);
    {
        let conn = crate::server::open_runtime_db(&db_path).unwrap();
        conary_core::db::models::admin_token::create(&conn, "repo-reader", &hash, "repos:read")
            .unwrap();
    }

    // GET /v1/admin/repos with repos:read scope should be allowed
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/v1/admin/repos")
                .header("Authorization", format!("Bearer {repo_reader_token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_sync_repo_missing_returns_not_found() {
    let (app, _db_path) = test_app().await;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/admin/repos/missing/sync")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn repository_creation_defers_dns_authority_to_the_fetch_boundary() {
    let (app, _db_path) = test_app().await;

    let create_body = serde_json::json!({
        "name": "bad-repo",
        "url": "http://localhost:8080/repo",
        "parser": {"package_format": "json"}
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
}

#[tokio::test]
async fn test_update_repo_rejects_private_content_url() {
    let (app, db_path) = test_app().await;

    let create_body = serde_json::json!({
        "name": "fedora",
        "url": "https://93.184.216.34/fedora",
        "parser": {"package_format": "json"}
    });
    let create_resp = app
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
    assert_eq!(create_resp.status(), StatusCode::CREATED);

    let app2 = rebuild_app(&db_path);
    let update_body = serde_json::json!({
        "url": "https://93.184.216.34/fedora",
        "content_url": "http://10.0.0.42/content",
        "parser": {"package_format": "json"}
    });
    let resp = app2
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
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_update_repo_rejects_create_only_name_field() {
    let (app, db_path) = test_app().await;

    let create_body = serde_json::json!({
        "name": "fedora",
        "url": "https://93.184.216.34/fedora",
        "parser": {"package_format": "json"}
    });
    let create_resp = app
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
    assert_eq!(create_resp.status(), StatusCode::CREATED);

    let update_body = serde_json::json!({
        "name": "removed-create-only-field",
        "url": "https://93.184.216.34/fedora",
        "parser": {"package_format": "json"}
    });
    let response = rebuild_app(&db_path)
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

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
