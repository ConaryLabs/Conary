// conary-test/src/server/stateless_mcp.rs
//! Axum adapter for conary-test's draft stateless MCP discovery route.

use std::path::Path;

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use conary_agent_contract::{local_bootstrap_status, test_suites};
use conary_mcp::stateless::{
    ImplementationInfo, MCP_DRAFT_PROTOCOL_VERSION, ResourceContent, ResourceDescriptor,
};
use conary_mcp::stateless_http::{
    HTTP_BAD_REQUEST, JSON_RPC_PARSE_ERROR, OriginPolicy, RawStatelessHttpConfig,
    RawStatelessHttpResponse, ResourceReadError, StatelessResourceProvider,
    handle_stateless_http_bytes_with_resources,
};
use serde_json::{Value, json};

use crate::server::state::AppState;

const MAX_STATELESS_MCP_BODY_BYTES: usize = 1024 * 1024;

pub async fn handle(State(state): State<AppState>, request: axum::http::Request<Body>) -> Response {
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

    raw_response_to_axum(handle_stateless_http_bytes_with_resources(
        method,
        headers,
        &body,
        &stateless_config(),
        &ConaryTestResourceProvider { state },
    ))
}

fn stateless_config() -> RawStatelessHttpConfig {
    RawStatelessHttpConfig {
        origin_policy: OriginPolicy::local_non_browser(),
        supported_versions: vec![MCP_DRAFT_PROTOCOL_VERSION.to_string()],
        server_info: ImplementationInfo::new("conary-test-mcp", env!("CARGO_PKG_VERSION")),
        instructions: Some(
            "Conary test infrastructure stateless MCP endpoint exposes discovery plus read-only bootstrap-status and suites resources."
                .to_string(),
        ),
    }
}

#[derive(Clone)]
struct ConaryTestResourceProvider {
    state: AppState,
}

impl StatelessResourceProvider for ConaryTestResourceProvider {
    fn list_resources(&self) -> Vec<ResourceDescriptor> {
        vec![bootstrap_status_descriptor(), suites_descriptor()]
    }

    fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, ResourceReadError> {
        let bootstrap_uri = local_bootstrap_status().uri;
        if uri == bootstrap_uri {
            let inspect = crate::bootstrap::inspect_default();
            return Ok(vec![json_resource_content(uri, &inspect)]);
        }

        let suites_uri = test_suites().uri;
        if uri == suites_uri {
            let inspect =
                crate::suite_inventory::inspect_manifest_dir(Path::new(&self.state.manifest_dir));
            return Ok(vec![json_resource_content(uri, &inspect)]);
        }

        Err(ResourceReadError::NotFound {
            uri: uri.to_string(),
        })
    }
}

fn bootstrap_status_descriptor() -> ResourceDescriptor {
    ResourceDescriptor {
        uri: local_bootstrap_status().uri,
        name: "bootstrap_status".to_string(),
        title: Some("Local Bootstrap Status".to_string()),
        description: "Read local developer bootstrap prerequisites and smoke-readiness state"
            .to_string(),
        mime_type: "application/json".to_string(),
    }
}

fn suites_descriptor() -> ResourceDescriptor {
    ResourceDescriptor {
        uri: test_suites().uri,
        name: "conary_test_suites".to_string(),
        title: Some("Conary-Test Suites".to_string()),
        description: "Read the local conary-test suite manifest inventory".to_string(),
        mime_type: "application/json".to_string(),
    }
}

fn json_resource_content(uri: &str, inspect: &impl serde::Serialize) -> ResourceContent {
    let text = serde_json::to_string_pretty(inspect)
        .expect("Conary InspectResult should serialize to JSON");

    ResourceContent {
        uri: uri.to_string(),
        mime_type: "application/json".to_string(),
        text,
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
    use std::path::Path;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use conary_mcp::stateless::{
        HEADER_METHOD, HEADER_NAME, HEADER_PROTOCOL_VERSION, JSON_RPC_HEADER_MISMATCH,
        JSON_RPC_INVALID_PARAMS, JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION,
    };
    use conary_mcp::stateless_http::{
        JSON_RPC_METHOD_NOT_FOUND, JSON_RPC_PARSE_ERROR, JSON_RPC_SERVER_ERROR,
    };
    use serde_json::{Value, json};
    use tempfile::tempdir;
    use tower::ServiceExt;

    use crate::server::routes::create_router;
    use crate::server::state::AppState;
    use crate::test_fixtures;

    const TEST_TOKEN: &str = "test-secret-token";

    fn state_with_manifest_dir(manifest_dir: &Path) -> AppState {
        let mut state = test_fixtures::test_app_state();
        state.manifest_dir = manifest_dir.display().to_string();
        state
    }

    fn write_test_manifest(dir: &Path, file_name: &str, suite_name: &str, phase: u32) {
        std::fs::write(
            dir.join(file_name),
            format!(
                r#"
[suite]
name = "{suite_name}"
phase = {phase}

[[test]]
id = "T01"
name = "smoke"
description = "Smoke test"
timeout = 10

[[test.step]]
run = "true"
"#
            ),
        )
        .unwrap();
    }

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

    fn resource_list_body(id: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "resources/list",
            "params": {
                "_meta": valid_meta()
            }
        })
    }

    fn resource_read_body(id: &str, uri: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "resources/read",
            "params": {
                "_meta": valid_meta(),
                "uri": uri
            }
        })
    }

    fn suites_resource_read_request(id: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/mcp/stateless")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
            .header(HEADER_METHOD, "resources/read")
            .header(HEADER_NAME, "conary-test://suites")
            .body(Body::from(
                serde_json::to_vec(&resource_read_body(id, "conary-test://suites")).unwrap(),
            ))
            .unwrap()
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
    async fn stateless_discover_route_advertises_resources_only() {
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
        assert_eq!(body["result"]["capabilities"]["resources"], json!({}));
        assert!(body["result"]["capabilities"].get("tools").is_none());
        assert!(body["result"]["capabilities"].get("prompts").is_none());
    }

    #[tokio::test]
    async fn resources_list_route_returns_bootstrap_and_suites_resources() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "resources/list")
                    .body(Body::from(
                        serde_json::to_vec(&resource_list_body("resources-list-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["ttlMs"], 30_000);
        assert_eq!(body["result"]["cacheScope"], "private");
        let resources = body["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0]["uri"], "conary-local://bootstrap/status");
        assert_eq!(resources[0]["name"], "bootstrap_status");
        assert_eq!(resources[1]["uri"], "conary-test://suites");
        assert_eq!(resources[1]["name"], "conary_test_suites");
        assert_eq!(resources[1]["title"], "Conary-Test Suites");
        assert_eq!(
            resources[1]["description"],
            "Read the local conary-test suite manifest inventory"
        );
        assert_eq!(resources[1]["mimeType"], "application/json");
    }

    #[tokio::test]
    async fn resources_read_route_returns_bootstrap_inspect_json() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "resources/read")
                    .header(HEADER_NAME, "conary-local://bootstrap/status")
                    .body(Body::from(
                        serde_json::to_vec(&resource_read_body(
                            "resources-read-1",
                            "conary-local://bootstrap/status",
                        ))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["ttlMs"], 30_000);
        assert_eq!(body["result"]["cacheScope"], "private");
        assert_eq!(body["result"]["contents"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["result"]["contents"][0]["uri"],
            "conary-local://bootstrap/status"
        );
        assert_eq!(
            body["result"]["contents"][0]["mimeType"],
            "application/json"
        );

        let text = body["result"]["contents"][0]["text"]
            .as_str()
            .expect("resource content should be text");
        let payload: serde_json::Value =
            serde_json::from_str(text).expect("bootstrap resource text should be JSON");

        assert_eq!(payload["operation"], "conary-test.bootstrap.inspect");
        assert_eq!(payload["subject"]["uri"], "conary-local://bootstrap/status");
        assert_eq!(payload["risk"], "read_only");
        assert!(payload["data"].get("project_root").is_some());
        assert!(payload["data"].get("required").is_some());
    }

    #[tokio::test]
    async fn resources_read_route_returns_suites_inspect_json_from_state_manifest_dir() {
        let root = tempdir().unwrap();
        write_test_manifest(root.path(), "phase1-core.toml", "phase1-core", 1);
        let app = create_router(state_with_manifest_dir(root.path()), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-read-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["ttlMs"], 30_000);
        assert_eq!(body["result"]["cacheScope"], "private");
        assert_eq!(body["result"]["contents"].as_array().unwrap().len(), 1);
        assert_eq!(body["result"]["contents"][0]["uri"], "conary-test://suites");
        assert_eq!(
            body["result"]["contents"][0]["mimeType"],
            "application/json"
        );

        let text = body["result"]["contents"][0]["text"]
            .as_str()
            .expect("resource content should be text");
        let payload: Value =
            serde_json::from_str(text).expect("suites resource text should be JSON");

        assert_eq!(payload["operation"], "conary-test.suites.inspect");
        assert_eq!(payload["subject"]["uri"], "conary-test://suites");
        assert_eq!(payload["risk"], "read_only");
        assert_eq!(payload["status"], "ok");
        assert_eq!(
            payload["data"]["manifest_dir"],
            root.path().display().to_string()
        );
        assert_eq!(payload["data"]["parsed"], 1);
        assert_eq!(payload["data"]["suites"][0]["id"], "phase1-core");
    }

    #[tokio::test]
    async fn suites_resource_reports_partial_manifest_parse_state_inside_content() {
        let root = tempdir().unwrap();
        write_test_manifest(root.path(), "good.toml", "good", 1);
        std::fs::write(root.path().join("bad.toml"), "not = [valid").unwrap();
        let app = create_router(state_with_manifest_dir(root.path()), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-partial-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "partial");
        assert_eq!(payload["data"]["parsed"], 1);
        assert_eq!(payload["data"]["failed"], 1);
        assert!(
            payload["data"]["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("bad.toml"))
        );
    }

    #[tokio::test]
    async fn suites_resource_reports_all_failed_manifest_parse_state_inside_content() {
        let root = tempdir().unwrap();
        std::fs::write(root.path().join("bad-a.toml"), "not = [valid").unwrap();
        std::fs::write(root.path().join("bad-b.toml"), "also = [broken").unwrap();
        let app = create_router(state_with_manifest_dir(root.path()), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-all-failed-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["data"]["parsed"], 0);
        assert_eq!(payload["data"]["failed"], 2);
        assert!(
            payload["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning
                    .as_str()
                    .unwrap()
                    .contains("no parseable test manifests"))
        );
    }

    #[tokio::test]
    async fn suites_resource_reports_missing_manifest_dir_inside_content() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing");
        let app = create_router(state_with_manifest_dir(&missing), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-missing-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["data"]["dir_exists"], false);
        assert_eq!(payload["data"]["parsed"], 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn suites_resource_reports_unreadable_manifest_dir_inside_content() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let blocked = root.path().join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let original_permissions = std::fs::metadata(&blocked).unwrap().permissions();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let app = create_router(state_with_manifest_dir(&blocked), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-unreadable-1"))
            .await
            .unwrap();

        std::fs::set_permissions(&blocked, original_permissions).unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["data"]["dir_exists"], true);
        assert!(
            payload["data"]["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("unreadable"))
        );
        assert!(
            payload["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap().contains("unreadable"))
        );
    }

    #[tokio::test]
    async fn resources_read_route_unknown_uri_returns_resource_not_found() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "resources/read")
                    .header(HEADER_NAME, "conary-local://missing")
                    .body(Body::from(
                        serde_json::to_vec(&resource_read_body(
                            "resources-read-missing-1",
                            "conary-local://missing",
                        ))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = read_json(response).await;
        assert_eq!(body["error"]["code"], JSON_RPC_INVALID_PARAMS);
        assert_eq!(body["error"]["data"]["uri"], "conary-local://missing");
    }

    #[tokio::test]
    async fn resources_read_route_requires_matching_mcp_name() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "resources/read")
                    .header(HEADER_NAME, "conary-local://other")
                    .body(Body::from(
                        serde_json::to_vec(&resource_read_body(
                            "resources-read-header-mismatch-1",
                            "conary-local://bootstrap/status",
                        ))
                        .unwrap(),
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
    async fn stateless_resource_requires_token_when_router_is_authed() {
        let app = create_router(
            test_fixtures::test_app_state(),
            Some(TEST_TOKEN.to_string()),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "resources/list")
                    .body(Body::from(
                        serde_json::to_vec(&resource_list_body("resources-list-auth-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
    async fn legacy_mcp_route_does_not_return_stateless_resource_list() {
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
                    .header(HEADER_METHOD, "resources/list")
                    .body(Body::from(
                        serde_json::to_vec(&resource_list_body("cross-wire-list-1")).unwrap(),
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
                "legacy /mcp should not return stateless resource list result"
            );
        }
    }

    #[tokio::test]
    async fn legacy_mcp_route_does_not_return_stateless_suites_resource() {
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
                    .header(HEADER_METHOD, "resources/read")
                    .header(HEADER_NAME, "conary-test://suites")
                    .body(Body::from(
                        serde_json::to_vec(&resource_read_body(
                            "cross-wire-suites-1",
                            "conary-test://suites",
                        ))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        // The legacy rmcp endpoint may return non-JSON for malformed or
        // session-era requests. In that case this test passes because the
        // response is definitely not the stateless resource envelope.
        if let Ok(body) = try_read_json(response).await {
            assert!(
                body.get("result")
                    .and_then(|result| result.get("resultType"))
                    .is_none(),
                "legacy /mcp should not return stateless suites resource result"
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
