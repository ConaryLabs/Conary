// apps/conaryd/src/daemon/routes/transactions/tests.rs
//! Tests for daemon transaction and package-operation routes.

use super::super::test_support::{
    body_json, create_test_state, current_process_creds, test_router,
};
use crate::daemon::auth::PeerCredentials;
use crate::daemon::{DaemonJob, JobStatus};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_handler_list_transactions_empty() {
    let (state, _dir) = create_test_state();
    let app = test_router(state, current_process_creds());

    let request = Request::builder()
        .uri("/v1/transactions")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_handler_get_transaction_not_found() {
    let (state, _dir) = create_test_state();
    let app = test_router(state, current_process_creds());

    let request = Request::builder()
        .uri("/v1/transactions/nonexistent-job-id")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let json = body_json(response).await;
    assert_eq!(json["status"], 404);
    assert!(
        json["detail"]
            .as_str()
            .unwrap()
            .contains("nonexistent-job-id")
    );
}

#[tokio::test]
async fn test_handler_create_transaction_queues_package_jobs() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state.clone(), root_creds);

    let body = serde_json::json!({
        "operations": [
            {
                "type": "install",
                "packages": ["nginx"]
            }
        ]
    });

    let request = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let json = body_json(response).await;
    assert_eq!(json["status"], "queued");
    assert_eq!(json["queue_position"], 0);
    let job_id = json["job_id"].as_str().unwrap();
    assert_eq!(json["location"], format!("/v1/transactions/{job_id}"));

    let job = {
        let conn = state.open_db().unwrap();
        DaemonJob::find_by_id(&conn, job_id).unwrap().unwrap()
    };
    assert_eq!(job.kind, crate::daemon::JobKind::Install);
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(
        job.spec,
        serde_json::json!([
            {
                "type": "install",
                "packages": ["nginx"],
                "allow_downgrade": false,
                "skip_deps": false
            }
        ])
    );
    assert_eq!(
        job.requested_by_uid,
        current_process_creds().map(|creds| creds.uid)
    );
}

#[tokio::test]
async fn test_handler_create_transaction_empty_operations() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state.clone(), root_creds);

    let body = serde_json::json!({
        "operations": []
    });

    let request = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let json = body_json(response).await;
    assert_eq!(json["status"], 400);
    assert!(json["detail"].as_str().unwrap().contains("operation"));
}

#[tokio::test]
async fn test_handler_create_transaction_rejects_mixed_package_kinds() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state, root_creds);

    let body = serde_json::json!({
        "operations": [
            {
                "type": "install",
                "packages": ["nginx"]
            },
            {
                "type": "remove",
                "packages": ["vim"]
            }
        ]
    });

    let request = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert!(
        json["detail"]
            .as_str()
            .unwrap()
            .contains("one package operation kind")
    );
}

#[tokio::test]
async fn test_handler_create_transaction_invalid_json() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state, root_creds);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .body(Body::from("not valid json"))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Axum returns 400 Bad Request for JSON deserialization failures
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handler_create_transaction_forbidden() {
    let (state, _dir) = create_test_state();
    // No credentials (simulates TCP connection)
    let app = test_router(state, None);

    let body = serde_json::json!({
        "operations": [
            {
                "type": "install",
                "packages": ["nginx"]
            }
        ]
    });

    let request = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let json = body_json(response).await;
    assert_eq!(json["status"], 403);
}

#[tokio::test]
async fn test_handler_create_transaction_idempotency() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();

    let body = serde_json::json!({
        "operations": [
            {
                "type": "install",
                "packages": ["curl"]
            }
        ]
    });
    let body_str = serde_json::to_string(&body).unwrap();

    let app1 = test_router(state.clone(), root_creds);
    let request1 = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .header("x-idempotency-key", "idem-key-42")
        .body(Body::from(body_str.clone()))
        .unwrap();

    let response1 = app1.oneshot(request1).await.unwrap();
    assert_eq!(response1.status(), StatusCode::ACCEPTED);
    let json1 = body_json(response1).await;
    assert_eq!(json1["status"], "queued");

    let app2 = test_router(state, root_creds);
    let request2 = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .header("x-idempotency-key", "idem-key-42")
        .body(Body::from(body_str))
        .unwrap();

    let response2 = app2.oneshot(request2).await.unwrap();
    assert_eq!(response2.status(), StatusCode::OK);
    let json2 = body_json(response2).await;
    assert_eq!(json2["status"], "queued");
    assert_eq!(json2["job_id"], json1["job_id"]);
    assert_eq!(json2["location"], json1["location"]);
}

#[tokio::test]
async fn test_handler_enhance_idempotency() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let idempotency_key = "enhance-key-42";

    let body = serde_json::json!({
        "batch_size": 1,
        "trove_ids": [],
        "types": [],
        "force": false
    });
    let body_str = serde_json::to_string(&body).unwrap();

    let request1 = Request::builder()
        .method("POST")
        .uri("/v1/enhance")
        .header("content-type", "application/json")
        .header("x-idempotency-key", idempotency_key)
        .body(Body::from(body_str.clone()))
        .unwrap();

    let response1 = test_router(state.clone(), root_creds)
        .oneshot(request1)
        .await
        .unwrap();
    assert_eq!(response1.status(), StatusCode::ACCEPTED);
    let json1 = body_json(response1).await;

    let request2 = Request::builder()
        .method("POST")
        .uri("/v1/enhance")
        .header("content-type", "application/json")
        .header("x-idempotency-key", idempotency_key)
        .body(Body::from(body_str))
        .unwrap();

    let response2 = test_router(state, root_creds)
        .oneshot(request2)
        .await
        .unwrap();
    assert_eq!(response2.status(), StatusCode::OK);
    let json2 = body_json(response2).await;
    assert_eq!(json2["status"], "queued");
    assert_eq!(json2["job_id"], json1["job_id"]);
    assert_eq!(json2["location"], json1["location"]);
}

#[tokio::test]
async fn test_handler_create_transaction_rejects_existing_enhance_idempotency_key() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let idempotency_key = "shared-enhance-key";

    let enhance_body = serde_json::json!({
        "batch_size": 1,
        "trove_ids": [],
        "types": [],
        "force": false
    });
    let enhance_request = Request::builder()
        .method("POST")
        .uri("/v1/enhance")
        .header("content-type", "application/json")
        .header("x-idempotency-key", idempotency_key)
        .body(Body::from(serde_json::to_string(&enhance_body).unwrap()))
        .unwrap();

    let enhance_response = test_router(state.clone(), root_creds)
        .oneshot(enhance_request)
        .await
        .unwrap();
    assert_eq!(enhance_response.status(), StatusCode::ACCEPTED);

    let package_body = serde_json::json!({
        "operations": [
            {
                "type": "install",
                "packages": ["curl"]
            }
        ]
    });
    let package_request = Request::builder()
        .method("POST")
        .uri("/v1/transactions")
        .header("content-type", "application/json")
        .header("x-idempotency-key", idempotency_key)
        .body(Body::from(serde_json::to_string(&package_body).unwrap()))
        .unwrap();

    let package_response = test_router(state, root_creds)
        .oneshot(package_request)
        .await
        .unwrap();

    assert_eq!(package_response.status(), StatusCode::CONFLICT);
    let json = body_json(package_response).await;
    assert_eq!(json["status"], 409);
    assert!(json["detail"].as_str().unwrap().contains("Idempotency key"));
}

#[tokio::test]
async fn test_handler_get_transaction_after_creation() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();

    // Insert a job directly (transaction API rejects unsupported kinds)
    let job = DaemonJob::new(
        crate::daemon::JobKind::Enhance,
        serde_json::json!({"batch_size": 5}),
    )
    .with_uid(nix::unistd::geteuid().as_raw());
    let job_id = job.id.clone();
    {
        let conn = state.open_db().unwrap();
        job.insert(&conn).unwrap();
    }

    let app = test_router(state, root_creds);
    let get_req = Request::builder()
        .uri(format!("/v1/transactions/{}", job_id))
        .body(Body::empty())
        .unwrap();

    let get_resp = app.oneshot(get_req).await.unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);

    let details = body_json(get_resp).await;
    assert_eq!(details["id"].as_str().unwrap(), job_id);
    assert_eq!(details["kind"], "enhance");
    assert_eq!(details["status"], "queued");
}

#[tokio::test]
async fn test_handler_list_transactions_with_status_filter() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();

    // Insert a job directly (transaction API rejects unsupported kinds)
    let job = DaemonJob::new(
        crate::daemon::JobKind::Enhance,
        serde_json::json!({"batch_size": 5}),
    )
    .with_uid(nix::unistd::geteuid().as_raw());
    {
        let conn = state.open_db().unwrap();
        job.insert(&conn).unwrap();
    }

    // List queued transactions
    let app2 = test_router(state.clone(), root_creds);
    let list_req = Request::builder()
        .uri("/v1/transactions?status=queued")
        .body(Body::empty())
        .unwrap();

    let list_resp = app2.oneshot(list_req).await.unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);

    let json = body_json(list_resp).await;
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["status"], "queued");

    // List completed (should be empty)
    let app3 = test_router(state, root_creds);
    let list_req2 = Request::builder()
        .uri("/v1/transactions?status=completed")
        .body(Body::empty())
        .unwrap();

    let list_resp2 = app3.oneshot(list_req2).await.unwrap();
    assert_eq!(list_resp2.status(), StatusCode::OK);

    let json2 = body_json(list_resp2).await;
    assert!(json2.is_array());
    assert_eq!(json2.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_handler_list_transactions_filters_by_requesting_uid() {
    let (state, _dir) = create_test_state();
    let daemon_uid = nix::unistd::geteuid().as_raw();
    let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

    let visible_job = DaemonJob::new(
        crate::daemon::JobKind::Enhance,
        serde_json::json!({"batch_size": 5}),
    )
    .with_uid(daemon_uid);
    let hidden_job = DaemonJob::new(
        crate::daemon::JobKind::Enhance,
        serde_json::json!({"batch_size": 7}),
    )
    .with_uid(other_uid);

    {
        let conn = state.open_db().unwrap();
        visible_job.insert(&conn).unwrap();
        hidden_job.insert(&conn).unwrap();
    }

    let app = test_router(
        state,
        Some(PeerCredentials {
            pid: std::process::id(),
            uid: daemon_uid,
            gid: daemon_uid,
        }),
    );

    let request = Request::builder()
        .uri("/v1/transactions")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["id"], visible_job.id);
}

#[tokio::test]
async fn test_handler_get_transaction_hides_foreign_job() {
    let (state, _dir) = create_test_state();
    let daemon_uid = nix::unistd::geteuid().as_raw();
    let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

    let hidden_job = DaemonJob::new(
        crate::daemon::JobKind::Enhance,
        serde_json::json!({"batch_size": 7}),
    )
    .with_uid(other_uid);
    let hidden_job_id = hidden_job.id.clone();

    {
        let conn = state.open_db().unwrap();
        hidden_job.insert(&conn).unwrap();
    }

    let app = test_router(
        state,
        Some(PeerCredentials {
            pid: std::process::id(),
            uid: daemon_uid,
            gid: daemon_uid,
        }),
    );

    let request = Request::builder()
        .uri(format!("/v1/transactions/{}", hidden_job_id))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handler_transaction_stream_hides_foreign_job() {
    let (state, _dir) = create_test_state();
    let daemon_uid = nix::unistd::geteuid().as_raw();
    let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

    let hidden_job = DaemonJob::new(
        crate::daemon::JobKind::Enhance,
        serde_json::json!({"batch_size": 7}),
    )
    .with_uid(other_uid);
    let hidden_job_id = hidden_job.id.clone();

    {
        let conn = state.open_db().unwrap();
        hidden_job.insert(&conn).unwrap();
    }

    let app = test_router(
        state,
        Some(PeerCredentials {
            pid: std::process::id(),
            uid: daemon_uid,
            gid: daemon_uid,
        }),
    );

    let request = Request::builder()
        .uri(format!("/v1/transactions/{}/stream", hidden_job_id))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handler_cancel_transaction_not_found() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state, root_creds);

    let request = Request::builder()
        .method("DELETE")
        .uri("/v1/transactions/nonexistent-id")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handler_cancel_transaction_hides_foreign_job() {
    let (state, _dir) = create_test_state();
    let daemon_uid = nix::unistd::geteuid().as_raw();
    let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

    let hidden_job = DaemonJob::new(
        crate::daemon::JobKind::Enhance,
        serde_json::json!({"batch_size": 7}),
    )
    .with_uid(other_uid);
    let hidden_job_id = hidden_job.id.clone();

    {
        let conn = state.open_db().unwrap();
        hidden_job.insert(&conn).unwrap();
    }

    let app = test_router(
        state,
        Some(PeerCredentials {
            pid: std::process::id(),
            uid: daemon_uid,
            gid: daemon_uid,
        }),
    );

    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/transactions/{}", hidden_job_id))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_package_routes_queue_package_jobs() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state.clone(), root_creds);

    for (expected_position, (path, operation, expected_kind)) in [
        (
            "/v1/packages/install",
            "install",
            crate::daemon::JobKind::Install,
        ),
        (
            "/v1/packages/remove",
            "remove",
            crate::daemon::JobKind::Remove,
        ),
        (
            "/v1/packages/update",
            "update",
            crate::daemon::JobKind::Update,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let body = serde_json::json!({
            "packages": ["demo"],
            "options": {}
        });
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], "queued");
        assert_eq!(json["queue_position"], expected_position);
        let job_id = json["job_id"].as_str().unwrap();

        let job = {
            let conn = state.open_db().unwrap();
            DaemonJob::find_by_id(&conn, job_id).unwrap().unwrap()
        };
        assert_eq!(
            job.kind, expected_kind,
            "{operation} route queued wrong kind"
        );
        assert_eq!(job.spec[0]["type"], operation);
        assert_eq!(job.spec[0]["packages"], serde_json::json!(["demo"]));
    }
}

#[tokio::test]
async fn test_handler_dry_run_returns_package_summary() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state, root_creds);

    let body = serde_json::json!({
        "operations": [
            {
                "type": "install",
                "packages": ["nginx", "curl"]
            },
            {
                "type": "remove",
                "packages": ["vim"]
            }
        ]
    });

    let request = Request::builder()
        .method("POST")
        .uri("/v1/transactions/dry-run")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(
        json["operations"],
        serde_json::json!([
            {
                "type": "install",
                "packages": ["nginx", "curl"],
                "allow_downgrade": false,
                "skip_deps": false
            },
            {
                "type": "remove",
                "packages": ["vim"],
                "cascade": false,
                "remove_orphans": false
            }
        ])
    );
    assert_eq!(
        json["summary"]["install"],
        serde_json::json!(["nginx", "curl"])
    );
    assert_eq!(json["summary"]["remove"], serde_json::json!(["vim"]));
    assert_eq!(json["summary"]["update"], serde_json::json!([]));
    assert_eq!(json["summary"]["total_affected"], 3);
}

#[tokio::test]
async fn test_handler_dry_run_empty_operations() {
    let (state, _dir) = create_test_state();
    let root_creds = current_process_creds();
    let app = test_router(state, root_creds);

    let body = serde_json::json!({
        "operations": []
    });

    let request = Request::builder()
        .method("POST")
        .uri("/v1/transactions/dry-run")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
