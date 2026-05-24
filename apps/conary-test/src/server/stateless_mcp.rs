// conary-test/src/server/stateless_mcp.rs
//! Axum adapter for conary-test's draft stateless MCP discovery route.

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use conary_mcp::stateless::{ImplementationInfo, MCP_DRAFT_PROTOCOL_VERSION};
use conary_mcp::stateless_http::{
    HTTP_BAD_REQUEST, JSON_RPC_PARSE_ERROR, OriginPolicy, RawStatelessHttpConfig,
    RawStatelessHttpResponse, handle_stateless_http_bytes,
};
use serde_json::{Value, json};

const MAX_STATELESS_MCP_BODY_BYTES: usize = 1024 * 1024;

pub async fn handle(request: axum::http::Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let method = parts.method.as_str().to_string();
    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();

    let body = match to_bytes(body, MAX_STATELESS_MCP_BODY_BYTES).await {
        Ok(body) => body,
        Err(err) => {
            return raw_response_to_axum(RawStatelessHttpResponse {
                status: HTTP_BAD_REQUEST,
                content_type: "application/json",
                body: Some(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {
                        "code": JSON_RPC_PARSE_ERROR,
                        "message": format!("Failed to read request body: {err}")
                    }
                })),
            });
        }
    };

    raw_response_to_axum(handle_stateless_http_bytes(
        method,
        headers,
        &body,
        &stateless_config(),
    ))
}

fn stateless_config() -> RawStatelessHttpConfig {
    RawStatelessHttpConfig {
        origin_policy: OriginPolicy::local_non_browser(),
        supported_versions: vec![MCP_DRAFT_PROTOCOL_VERSION.to_string()],
        server_info: ImplementationInfo::new("conary-test-mcp", env!("CARGO_PKG_VERSION")),
        instructions: Some(
            "Conary test infrastructure stateless MCP endpoint exposes discovery only.".to_string(),
        ),
    }
}

fn raw_response_to_axum(response: RawStatelessHttpResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    let mut axum_response = match response.body {
        Some(body) => (status, Json(body)).into_response(),
        None => status.into_response(),
    };
    axum_response
        .headers_mut()
        .insert(header::CONTENT_TYPE, response.content_type.parse().unwrap());
    axum_response
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use conary_mcp::stateless::{
        HEADER_METHOD, HEADER_PROTOCOL_VERSION, JSON_RPC_HEADER_MISMATCH, JSON_RPC_INVALID_PARAMS,
        JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION,
    };
    use conary_mcp::stateless_http::{
        JSON_RPC_METHOD_NOT_FOUND, JSON_RPC_PARSE_ERROR, JSON_RPC_SERVER_ERROR,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::server::routes::create_router;
    use crate::test_fixtures;

    const TEST_TOKEN: &str = "test-secret-token";

    fn valid_meta() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": MCP_DRAFT_PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientInfo": {
                "name": "ConaryTestClient",
                "version": "0.1.0"
            },
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    }

    fn discover_body(id: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "server/discover",
            "params": {
                "_meta": valid_meta()
            }
        })
    }

    fn tools_list_body(id: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/list",
            "params": {
                "_meta": valid_meta()
            }
        })
    }

    fn stateless_request(method: &str) -> axum::http::request::Builder {
        Request::builder()
            .method(method)
            .uri("/mcp/stateless")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
            .header(HEADER_METHOD, "server/discover")
    }

    async fn read_json(response: axum::response::Response) -> Value {
        try_read_json(response)
            .await
            .expect("response should be valid JSON")
    }

    async fn try_read_json(response: axum::response::Response) -> serde_json::Result<Value> {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body)
    }

    #[tokio::test]
    async fn stateless_discover_route_returns_conary_test_discovery() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                stateless_request("POST")
                    .body(Body::from(
                        serde_json::to_vec(&discover_body("discover-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["id"], "discover-1");
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["serverInfo"]["name"], "conary-test-mcp");
        assert_eq!(
            body["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(
            body["result"]["capabilities"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert!(body["result"]["capabilities"]["tools"].is_null());
        assert!(body["result"]["capabilities"]["resources"].is_null());
        assert!(body["result"]["capabilities"]["prompts"].is_null());
    }

    #[tokio::test]
    async fn stateless_route_requires_token_when_router_is_authed() {
        let app = create_router(
            test_fixtures::test_app_state(),
            Some(TEST_TOKEN.to_string()),
        );
        let response = app
            .oneshot(
                stateless_request("POST")
                    .body(Body::from(
                        serde_json::to_vec(&discover_body("auth-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn stateless_route_accepts_correct_token() {
        let app = create_router(
            test_fixtures::test_app_state(),
            Some(TEST_TOKEN.to_string()),
        );
        let response = app
            .oneshot(
                stateless_request("POST")
                    .header("authorization", format!("Bearer {TEST_TOKEN}"))
                    .body(Body::from(
                        serde_json::to_vec(&discover_body("auth-2")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["id"], "auth-2");
    }

    #[tokio::test]
    async fn legacy_mcp_route_still_reaches_rmcp_service_when_authed() {
        let app = create_router(
            test_fixtures::test_app_state(),
            Some(TEST_TOKEN.to_string()),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {TEST_TOKEN}"))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
        assert_ne!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn legacy_mcp_route_does_not_return_stateless_discovery() {
        let app = create_router(
            test_fixtures::test_app_state(),
            Some(TEST_TOKEN.to_string()),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {TEST_TOKEN}"))
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "server/discover")
                    .body(Body::from(
                        serde_json::to_vec(&discover_body("cross-wire-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        if let Ok(body) = try_read_json(response).await {
            assert!(
                body.get("result")
                    .and_then(|result| result.get("resultType"))
                    .is_none(),
                "legacy /mcp should not return stateless discovery"
            );
        }
    }

    #[tokio::test]
    async fn stateless_route_rejects_bad_origin() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                stateless_request("POST")
                    .header("origin", "https://evil.example")
                    .body(Body::from(
                        serde_json::to_vec(&discover_body("origin-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = read_json(response).await;
        assert_eq!(body["error"]["code"], JSON_RPC_SERVER_ERROR);
    }

    #[tokio::test]
    async fn stateless_route_reports_missing_protocol_header() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("accept", "application/json, text/event-stream")
                    .header("content-type", "application/json")
                    .header(HEADER_METHOD, "server/discover")
                    .body(Body::from(
                        serde_json::to_vec(&discover_body("missing-header-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = read_json(response).await;
        assert_eq!(body["error"]["code"], JSON_RPC_HEADER_MISMATCH);
    }

    #[tokio::test]
    async fn stateless_route_reports_missing_meta() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let body = json!({
            "jsonrpc": "2.0",
            "id": "missing-meta-1",
            "method": "server/discover",
            "params": {}
        });
        let response = app
            .oneshot(
                stateless_request("POST")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = read_json(response).await;
        assert_eq!(body["error"]["code"], JSON_RPC_INVALID_PARAMS);
    }

    #[tokio::test]
    async fn stateless_route_reports_unsupported_protocol() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let mut body = discover_body("unsupported-protocol-1");
        body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("DRAFT-OLD");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("accept", "application/json, text/event-stream")
                    .header("content-type", "application/json")
                    .header(HEADER_PROTOCOL_VERSION, "DRAFT-OLD")
                    .header(HEADER_METHOD, "server/discover")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = read_json(response).await;
        assert_eq!(body["error"]["code"], JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(body["error"]["data"]["requested"], "DRAFT-OLD");
        assert_eq!(
            body["error"]["data"]["supported"][0],
            MCP_DRAFT_PROTOCOL_VERSION
        );
    }

    #[tokio::test]
    async fn stateless_route_reports_method_not_found_for_valid_unsupported_methods() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let body = tools_list_body("tools-list-1");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("accept", "application/json, text/event-stream")
                    .header("content-type", "application/json")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "tools/list")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = read_json(response).await;
        assert_eq!(body["error"]["code"], JSON_RPC_METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn stateless_route_non_post_malformed_json_returns_405() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                stateless_request("GET")
                    .body(Body::from(br#"{"jsonrpc": "2.0", "#.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = read_json(response).await;
        assert_eq!(body["error"]["code"], JSON_RPC_SERVER_ERROR);
    }

    #[tokio::test]
    async fn stateless_route_malformed_json_returns_parse_error() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                stateless_request("POST")
                    .body(Body::from(br#"{"jsonrpc": "2.0", "#.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = read_json(response).await;
        assert_eq!(body["id"], Value::Null);
        assert_eq!(body["error"]["code"], JSON_RPC_PARSE_ERROR);
    }
}
