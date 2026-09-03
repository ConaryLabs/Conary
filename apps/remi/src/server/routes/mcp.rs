// apps/remi/src/server/routes/mcp.rs
//! External admin MCP endpoint wiring.

use super::*;

pub(super) fn create_mcp_router(
    state: Arc<RwLock<ServerState>>,
) -> Router<Arc<RwLock<ServerState>>> {
    let state_for_mcp = state;
    // Remi deliberately exposes only the modern, stateless MCP transport.
    // Keep rmcp's loopback host default; Origin is separately restricted to
    // local browser origins and the two configured admin-port variants.
    let config = rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_origins([
            "http://127.0.0.1",
            "http://localhost",
            "http://127.0.0.1:8081",
            "http://localhost:8081",
            "http://127.0.0.1:8082",
            "http://localhost:8082",
        ])
        .with_stateless_protocol_metadata_required(true);
    let mcp_service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || {
            Ok(crate::server::mcp::RemiMcpServer::new(
                state_for_mcp.clone(),
            ))
        },
        Arc::new(
            rmcp::transport::streamable_http_server::session::never::NeverSessionManager::default(),
        ),
        config,
    );
    let mcp_service = tower::service_fn(move |request: Request<Body>| {
        let mut service = mcp_service.clone();
        async move {
            if let Some(err) = mcp_scope_error(&request) {
                return Ok::<Response, Infallible>(err);
            }
            service
                .call(request)
                .await
                .map(|response| response.map(Body::new))
        }
    });

    Router::<Arc<RwLock<ServerState>>>::new()
        .nest_service("/mcp", mcp_service)
        .route_layer(middleware::from_fn(
            |request: Request<Body>, next: Next| async move {
                if let Some(err) = mcp_scope_error(&request) {
                    return err;
                }
                next.run(request).await
            },
        ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use axum::response::Response;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
    const MCP_TEST_TOKEN: &str = "test-admin-token-12345";

    fn mcp_post(
        id: i64,
        method: &str,
        header_version: &str,
        body_version: &str,
        request_params: Value,
        name: Option<&str>,
        origin: Option<&str>,
    ) -> Request<Body> {
        let mut params = request_params
            .as_object()
            .cloned()
            .expect("MCP request parameters must be an object");
        params.insert(
            "_meta".to_string(),
            json!({
                "io.modelcontextprotocol/protocolVersion": body_version,
                "io.modelcontextprotocol/clientInfo": {
                    "name": "remi-router-test",
                    "version": "1.0.0"
                },
                "io.modelcontextprotocol/clientCapabilities": {}
            }),
        );

        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("Host", "localhost")
            .header("Authorization", format!("Bearer {MCP_TEST_TOKEN}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", header_version)
            .header("Mcp-Method", method);
        if let Some(name) = name {
            builder = builder.header("Mcp-Name", name);
        }
        if let Some(origin) = origin {
            builder = builder.header("Origin", origin);
        }

        builder
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                    "params": Value::Object(params),
                }))
                .unwrap(),
            ))
            .unwrap()
    }

    async fn mcp_json(response: Response) -> Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap_or_else(|error| {
            panic!(
                "MCP response should be JSON: {error}; body={}",
                String::from_utf8_lossy(&body)
            )
        })
    }

    fn tools_list_request(id: i64) -> Request<Body> {
        mcp_post(
            id,
            "tools/list",
            MCP_PROTOCOL_VERSION,
            MCP_PROTOCOL_VERSION,
            json!({}),
            None,
            None,
        )
    }

    fn tools_call_request(id: i64) -> Request<Body> {
        mcp_post(
            id,
            "tools/call",
            MCP_PROTOCOL_VERSION,
            MCP_PROTOCOL_VERSION,
            json!({
                "name": "test_health",
                "arguments": {}
            }),
            Some("test_health"),
            None,
        )
    }

    fn tool_names(body: &Value) -> Vec<String> {
        body["result"]["tools"]
            .as_array()
            .expect("tools/list should return a tools array")
            .iter()
            .map(|tool| {
                tool["name"]
                    .as_str()
                    .expect("MCP tool should have a name")
                    .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn test_mcp_route_rejects_unauthenticated_requests() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

        let response = app
            .oneshot(Request::builder().uri("/mcp").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_mcp_route_rejects_invalid_bearer() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .header("Authorization", "Bearer invalid-mcp-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_mcp_route_rejects_non_admin_scope() {
        let (_app, db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
        let token = "test-repo-reader-token-54321";
        let hash = crate::server::auth::hash_token(token);
        let conn = crate::server::open_runtime_db(&db_path).unwrap();
        conary_core::db::models::admin_token::create(
            &conn,
            "test-repo-reader",
            &hash,
            "repos:read",
        )
        .unwrap();
        drop(conn);

        let app = crate::server::handlers::admin::test_helpers::rebuild_app(&db_path);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn modern_mcp_posts_are_stateless_and_tools_are_sorted() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

        let first = app.clone().oneshot(tools_list_request(1)).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert!(
            first.headers().get("Mcp-Session-Id").is_none(),
            "stateless MCP responses must not echo a session id"
        );
        let first_body = mcp_json(first).await;
        assert_eq!(first_body["result"]["resultType"], "complete");
        let names = tool_names(&first_body);
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(
            names, sorted_names,
            "tools/list order must be deterministic"
        );

        let second = app.oneshot(tools_list_request(2)).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        assert!(second.headers().get("Mcp-Session-Id").is_none());
        let second_body = mcp_json(second).await;
        assert_eq!(second_body["result"]["resultType"], "complete");
        assert_eq!(tool_names(&second_body), names);
    }

    #[tokio::test]
    async fn modern_mcp_tool_call_returns_complete_result() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

        let response = app.oneshot(tools_call_request(3)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("Mcp-Session-Id").is_none());
        let body = mcp_json(response).await;
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["isError"], false);
    }

    #[tokio::test]
    async fn modern_mcp_rejects_unsupported_protocol_version_with_supported_data() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
        let response = app
            .oneshot(mcp_post(
                4,
                "tools/list",
                "2025-11-25",
                "2025-11-25",
                json!({}),
                None,
                None,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = mcp_json(response).await;
        assert_eq!(body["error"]["code"], -32022);
        assert_eq!(
            body["error"]["data"]["supported"],
            json!([MCP_PROTOCOL_VERSION])
        );
    }

    #[tokio::test]
    async fn modern_mcp_rejects_header_body_protocol_mismatch() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
        let response = app
            .oneshot(mcp_post(
                5,
                "tools/list",
                MCP_PROTOCOL_VERSION,
                "2025-11-25",
                json!({}),
                None,
                None,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = mcp_json(response).await;
        assert_eq!(body["error"]["code"], -32020);
    }

    #[tokio::test]
    async fn modern_mcp_get_and_delete_are_not_allowed() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

        for method in [Method::GET, Method::DELETE] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/mcp")
                        .header("Host", "localhost")
                        .header("Authorization", format!("Bearer {MCP_TEST_TOKEN}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        }
    }

    #[tokio::test]
    async fn modern_mcp_origin_policy_rejects_bad_origin_and_allows_missing_origin() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

        let bad_origin = app
            .clone()
            .oneshot(mcp_post(
                6,
                "tools/list",
                MCP_PROTOCOL_VERSION,
                MCP_PROTOCOL_VERSION,
                json!({}),
                None,
                Some("https://evil.example"),
            ))
            .await
            .unwrap();
        assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);

        let missing_origin = app.oneshot(tools_list_request(7)).await.unwrap();
        assert_eq!(missing_origin.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn modern_mcp_origin_policy_allows_configured_local_admin_port() {
        let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
        let response = app
            .oneshot(mcp_post(
                8,
                "tools/list",
                MCP_PROTOCOL_VERSION,
                MCP_PROTOCOL_VERSION,
                json!({}),
                None,
                Some("http://localhost:8082"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
