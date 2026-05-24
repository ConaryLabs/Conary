# Conary-Test Bootstrap Status Stateless Resource Implementation Plan

> **For /goal:** Implement this plan task-by-task. Source spec: `docs/superpowers/specs/2026-05-24-conary-test-bootstrap-status-resource-design.md`. Add only one read-only stateless MCP resource to conary-test: `conary-local://bootstrap/status`. Keep Remi untouched. Do not add live tools, prompts, resource templates, subscriptions, SSE streaming, mutations, smoke execution, or rmcp stateless assumptions. Use tests-first for every code task, verify the expected failure before implementation, update checkboxes as tasks complete, and make one focused commit per task.

**Goal:** Extend the conary-test `/mcp/stateless` preview from discovery-only to discovery plus the first live read-only MCP resource, `conary-local://bootstrap/status`, backed by the existing bootstrap `InspectResult`.

**Architecture:** Keep protocol shape in `crates/conary-mcp`, keep the live conary-test provider in `apps/conary-test`, and keep the route as a thin Axum adapter. The raw HTTP proof remains framework-neutral and rmcp-free.

**Hard Constraints:**
- Do not modify Remi server routes or Remi MCP behavior.
- Do not add live MCP tools, prompts, resource templates, resource subscriptions, listChanged notifications, or SSE streaming.
- Do not deepen the legacy `RoleServer` / `ServerHandler` / `LocalSessionManager` path.
- Do not call smoke suites from the resource read path.
- Unknown resource URIs return HTTP `404` with JSON-RPC `-32602`.
- Missing or mismatched `Mcp-Name` keeps using the existing header mismatch path, HTTP `400` with JSON-RPC `-32001`.
- Auth remains enforced by the existing conary-test router middleware before the stateless handler runs.

**Expected User-Facing Result:**
- `POST /mcp/stateless` with method `server/discover` returns `capabilities.resources = {}` and no tools or prompts.
- `POST /mcp/stateless` with method `resources/list` returns exactly one resource descriptor for `conary-local://bootstrap/status`.
- `POST /mcp/stateless` with method `resources/read` and `Mcp-Name: conary-local://bootstrap/status` returns one JSON text content block containing the existing bootstrap `InspectResult`.

## Task 1: Add Stateless Resource Result Types

**Files:**
- `crates/conary-mcp/src/stateless.rs`

### 1.1 Add failing serialization tests first

Add these tests inside `#[cfg(test)] mod tests` in `crates/conary-mcp/src/stateless.rs`:

```rust
#[test]
fn resource_descriptor_serializes_mcp_shape() {
    let descriptor = ResourceDescriptor {
        uri: "conary-local://bootstrap/status".to_string(),
        name: "bootstrap_status".to_string(),
        title: Some("Local Bootstrap Status".to_string()),
        description: "Read local developer bootstrap prerequisites and smoke-readiness state"
            .to_string(),
        mime_type: "application/json".to_string(),
    };

    let value = serde_json::to_value(descriptor).expect("descriptor should serialize");

    assert_eq!(value["uri"], "conary-local://bootstrap/status");
    assert_eq!(value["name"], "bootstrap_status");
    assert_eq!(value["title"], "Local Bootstrap Status");
    assert_eq!(
        value["description"],
        "Read local developer bootstrap prerequisites and smoke-readiness state"
    );
    assert_eq!(value["mimeType"], "application/json");
    assert!(value.get("mime_type").is_none());
}

#[test]
fn resource_descriptor_omits_absent_title() {
    let descriptor = ResourceDescriptor {
        uri: "conary-local://minimal".to_string(),
        name: "minimal".to_string(),
        title: None,
        description: "Minimal resource".to_string(),
        mime_type: "application/json".to_string(),
    };

    let value = serde_json::to_value(descriptor).expect("descriptor should serialize");

    assert!(value.get("title").is_none());
}

#[test]
fn resources_list_payload_serializes_resource_array() {
    let result = CacheableResult::new(
        CachePolicy::private_short(),
        ResourcesListPayload {
            resources: vec![ResourceDescriptor {
                uri: "conary-local://bootstrap/status".to_string(),
                name: "bootstrap_status".to_string(),
                title: Some("Local Bootstrap Status".to_string()),
                description: "Read local developer bootstrap prerequisites and smoke-readiness state"
                    .to_string(),
                mime_type: "application/json".to_string(),
            }],
        },
    );

    let value = serde_json::to_value(result).expect("resource list should serialize");

    assert_eq!(value["resultType"], "complete");
    assert_eq!(value["ttlMs"], 30_000);
    assert_eq!(value["cacheScope"], "private");
    assert_eq!(
        value["resources"][0]["uri"],
        "conary-local://bootstrap/status"
    );
    assert_eq!(value["resources"][0]["mimeType"], "application/json");
}

#[test]
fn resource_content_serializes_text_content_shape() {
    let content = ResourceContent {
        uri: "conary-local://bootstrap/status".to_string(),
        mime_type: "application/json".to_string(),
        text: "{\n  \"status\": \"ok\"\n}".to_string(),
    };

    let value = serde_json::to_value(content).expect("content should serialize");

    assert_eq!(value["uri"], "conary-local://bootstrap/status");
    assert_eq!(value["mimeType"], "application/json");
    assert_eq!(value["text"], "{\n  \"status\": \"ok\"\n}");
    assert!(value.get("mime_type").is_none());
}

#[test]
fn resources_read_payload_serializes_contents_array() {
    let result = CacheableResult::new(
        CachePolicy::private_short(),
        ResourcesReadPayload {
            contents: vec![ResourceContent {
                uri: "conary-local://bootstrap/status".to_string(),
                mime_type: "application/json".to_string(),
                text: "{}".to_string(),
            }],
        },
    );

    let value = serde_json::to_value(result).expect("resource read should serialize");

    assert_eq!(value["resultType"], "complete");
    assert_eq!(value["ttlMs"], 30_000);
    assert_eq!(value["cacheScope"], "private");
    assert_eq!(
        value["contents"][0]["uri"],
        "conary-local://bootstrap/status"
    );
    assert_eq!(value["contents"][0]["mimeType"], "application/json");
    assert_eq!(value["contents"][0]["text"], "{}");
}
```

Run the focused test command and confirm it fails because the new types do not exist yet:

```bash
cargo test -p conary-mcp resource_descriptor_serializes_mcp_shape
```

### 1.2 Add the resource result structs

In `crates/conary-mcp/src/stateless.rs`, add these structs near `CacheableResult<T>`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDescriptor {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourcesListPayload {
    pub resources: Vec<ResourceDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourcesReadPayload {
    pub contents: Vec<ResourceContent>,
}
```

The `mime_type` fields must serialize as `mimeType` through `rename_all = "camelCase"`.

### 1.3 Verify Task 1

```bash
cargo fmt --check
cargo test -p conary-mcp stateless::tests::resource
```

### 1.4 Commit Task 1

```bash
git status --short
git add crates/conary-mcp/src/stateless.rs
git commit -m "feat(mcp): model stateless resource results"
```

Update this checkbox after the commit:

- [x] Task 1 complete

## Task 2: Dispatch Resource Methods In The Raw Stateless Adapter

**Files:**
- `crates/conary-mcp/src/stateless_http.rs`

### 2.1 Add failing raw-adapter resource tests first

Add these helpers inside `#[cfg(test)] mod tests` in `crates/conary-mcp/src/stateless_http.rs`:

```rust
struct TestResourceProvider;

impl StatelessResourceProvider for TestResourceProvider {
    fn list_resources(&self) -> Vec<ResourceDescriptor> {
        vec![ResourceDescriptor {
            uri: "conary-local://bootstrap/status".to_string(),
            name: "bootstrap_status".to_string(),
            title: Some("Local Bootstrap Status".to_string()),
            description: "Read local developer bootstrap prerequisites and smoke-readiness state"
                .to_string(),
            mime_type: "application/json".to_string(),
        }]
    }

    fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, ResourceReadError> {
        if uri != "conary-local://bootstrap/status" {
            return Err(ResourceReadError::NotFound {
                uri: uri.to_string(),
            });
        }

        Ok(vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: "application/json".to_string(),
            text: "{\n  \"operation\": \"conary-test.bootstrap.inspect\"\n}".to_string(),
        }])
    }
}

fn resource_request(method: &str, params: serde_json::Value) -> RawStatelessHttpRequest {
    RawStatelessHttpRequest::post(json!({
        "jsonrpc": "2.0",
        "id": format!("{method}-1"),
        "method": method,
        "params": params,
    }))
    .with_header("Accept", "application/json, text/event-stream")
    .with_header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
    .with_header(HEADER_METHOD, method)
}
```

Add these tests in the same module:

```rust
#[test]
fn resource_aware_discovery_advertises_resources() {
    let request = valid_discover_request("discover-resource-1");
    let response = handle_stateless_http_request_with_resources(
        request,
        &RawStatelessHttpConfig::default(),
        &TestResourceProvider,
    );
    let body = response_body(&response);

    assert_eq!(response.status, HTTP_OK);
    assert_eq!(body["result"]["capabilities"]["resources"], json!({}));
    assert!(body["result"]["capabilities"].get("tools").is_none());
    assert!(body["result"]["capabilities"].get("prompts").is_none());
}

#[test]
fn resources_list_returns_provider_resources_and_cache_hints() {
    let response = handle_stateless_http_request_with_resources(
        resource_request(
            "resources/list",
            json!({
                "_meta": valid_meta(),
            }),
        ),
        &RawStatelessHttpConfig::default(),
        &TestResourceProvider,
    );
    let body = response_body(&response);

    assert_eq!(response.status, HTTP_OK);
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(body["result"]["ttlMs"], 30_000);
    assert_eq!(body["result"]["cacheScope"], "private");
    assert_eq!(
        body["result"]["resources"][0]["uri"],
        "conary-local://bootstrap/status"
    );
    assert_eq!(body["result"]["resources"][0]["name"], "bootstrap_status");
    assert_eq!(
        body["result"]["resources"][0]["title"],
        "Local Bootstrap Status"
    );
    assert_eq!(body["result"]["resources"][0]["mimeType"], "application/json");
}

#[test]
fn resources_list_accepts_cursor_but_returns_static_single_page() {
    let response = handle_stateless_http_request_with_resources(
        resource_request(
            "resources/list",
            json!({
                "_meta": valid_meta(),
                "cursor": "ignored-for-static-preview"
            }),
        ),
        &RawStatelessHttpConfig::default(),
        &TestResourceProvider,
    );
    let body = response_body(&response);

    assert_eq!(response.status, HTTP_OK);
    assert_eq!(body["result"]["resources"].as_array().unwrap().len(), 1);
    assert!(body["result"].get("nextCursor").is_none());
}

#[test]
fn resources_read_returns_provider_content_and_cache_hints() {
    let response = handle_stateless_http_request_with_resources(
        resource_request(
            "resources/read",
            json!({
                "_meta": valid_meta(),
                "uri": "conary-local://bootstrap/status"
            }),
        )
        .with_header(HEADER_NAME, "conary-local://bootstrap/status"),
        &RawStatelessHttpConfig::default(),
        &TestResourceProvider,
    );
    let body = response_body(&response);

    assert_eq!(response.status, HTTP_OK);
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(body["result"]["ttlMs"], 30_000);
    assert_eq!(body["result"]["cacheScope"], "private");
    assert_eq!(
        body["result"]["contents"][0]["uri"],
        "conary-local://bootstrap/status"
    );
    assert_eq!(body["result"]["contents"][0]["mimeType"], "application/json");
    assert!(
        body["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("conary-test.bootstrap.inspect")
    );
}

#[test]
fn resources_read_unknown_uri_returns_invalid_params_resource_not_found() {
    let response = handle_stateless_http_request_with_resources(
        resource_request(
            "resources/read",
            json!({
                "_meta": valid_meta(),
                "uri": "conary-local://missing"
            }),
        )
        .with_header(HEADER_NAME, "conary-local://missing"),
        &RawStatelessHttpConfig::default(),
        &TestResourceProvider,
    );
    let body = response_body(&response);

    assert_eq!(response.status, HTTP_NOT_FOUND);
    assert_eq!(body["error"]["code"], JSON_RPC_INVALID_PARAMS);
    assert_eq!(body["error"]["data"]["uri"], "conary-local://missing");
}

#[test]
fn resource_methods_without_provider_remain_method_not_found() {
    let response = handle_stateless_http_request(
        resource_request(
            "resources/list",
            json!({
                "_meta": valid_meta(),
            }),
        ),
        &RawStatelessHttpConfig::default(),
    );
    let body = response_body(&response);

    assert_eq!(response.status, HTTP_NOT_FOUND);
    assert_eq!(body["error"]["code"], JSON_RPC_METHOD_NOT_FOUND);
}

#[test]
fn resources_read_still_requires_matching_name_header_before_provider_lookup() {
    let response = handle_stateless_http_request_with_resources(
        resource_request(
            "resources/read",
            json!({
                "_meta": valid_meta(),
                "uri": "conary-local://bootstrap/status"
            }),
        )
        .with_header(HEADER_NAME, "conary-local://other"),
        &RawStatelessHttpConfig::default(),
        &TestResourceProvider,
    );
    let body = response_body(&response);

    assert_eq!(response.status, HTTP_BAD_REQUEST);
    assert_eq!(body["error"]["code"], JSON_RPC_HEADER_MISMATCH);
}
```

Update the existing discovery test only after this red check has been observed; the no-provider discovery path must keep returning empty capabilities.

Run the focused test command and confirm it fails because `StatelessResourceProvider`, resource dispatch, and resource-aware handler functions do not exist:

```bash
cargo test -p conary-mcp stateless_http::tests::resource_aware_discovery_advertises_resources
```

### 2.2 Add provider and error types

In `crates/conary-mcp/src/stateless_http.rs`, extend the `use crate::stateless::{ ... }` list with:

```rust
    CacheableResult, JSON_RPC_INVALID_PARAMS, ResourceContent, ResourceDescriptor,
    ResourcesListPayload, ResourcesReadPayload,
```

Add these imports near the top of the file:

```rust
use conary_agent_contract::CachePolicy;
use serde::Serialize;
```

Add this public trait and error enum near the config types:

```rust
pub trait StatelessResourceProvider {
    fn list_resources(&self) -> Vec<ResourceDescriptor>;

    fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, ResourceReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceReadError {
    NotFound { uri: String },
}
```

Update `RawStatelessHttpConfig::default()` so its instructions do not claim the raw proof is discovery-only:

```rust
instructions: Some(
    "Conary stateless MCP adapter proof exposes discovery. Resources are available when a provider is configured.".to_string(),
),
```

### 2.3 Refactor request dispatch to accept an optional provider

Replace the current `handle_stateless_http_request` body with this delegation pattern:

```rust
pub fn handle_stateless_http_request(
    request: RawStatelessHttpRequest,
    config: &RawStatelessHttpConfig,
) -> RawStatelessHttpResponse {
    handle_stateless_http_request_inner(request, config, None)
}

pub fn handle_stateless_http_request_with_resources<P: StatelessResourceProvider>(
    request: RawStatelessHttpRequest,
    config: &RawStatelessHttpConfig,
    resource_provider: &P,
) -> RawStatelessHttpResponse {
    handle_stateless_http_request_inner(
        request,
        config,
        Some(resource_provider as &dyn StatelessResourceProvider),
    )
}
```

Add this private function and move the existing validation and method match into it:

```rust
fn handle_stateless_http_request_inner(
    request: RawStatelessHttpRequest,
    config: &RawStatelessHttpConfig,
    resource_provider: Option<&dyn StatelessResourceProvider>,
) -> RawStatelessHttpResponse {
    if !request.method.eq_ignore_ascii_case("POST") {
        return error_response(
            HTTP_METHOD_NOT_ALLOWED,
            extract_scalar_id(&request.body),
            JSON_RPC_SERVER_ERROR,
            "Only POST is supported for stateless MCP requests",
            None,
        );
    }

    if !config
        .origin_policy
        .allows(origin_header(&request).as_deref())
    {
        return error_response(
            HTTP_FORBIDDEN,
            None,
            JSON_RPC_SERVER_ERROR,
            "Origin is not allowed",
            None,
        );
    }

    let envelope = match validate_json_rpc_envelope(&request.body) {
        Ok(envelope) => envelope,
        Err(message) => {
            return error_response(
                HTTP_BAD_REQUEST,
                extract_scalar_id(&request.body),
                JSON_RPC_INVALID_REQUEST,
                message,
                None,
            );
        }
    };

    let headers = match stateless_headers_from_request(&request) {
        Ok(headers) => headers,
        Err(err) => return stateless_protocol_error_response(envelope.id, err),
    };
    let supported_versions: Vec<&str> = config
        .supported_versions
        .iter()
        .map(String::as_str)
        .collect();

    if let Err(err) = validate_stateless_request(&headers, &request.body, &supported_versions) {
        return stateless_protocol_error_response(envelope.id, err);
    }

    let JsonRpcRequestEnvelope { id, method } = envelope;

    match method.as_str() {
        "server/discover" => discover_response(id, config, resource_provider.is_some()),
        "resources/list" => match resource_provider {
            Some(provider) => resources_list_response(id, provider),
            None => method_not_found_response(id, &method),
        },
        "resources/read" => match resource_provider {
            Some(provider) => resources_read_response(id, &request.body, provider),
            None => method_not_found_response(id, &method),
        },
        method => method_not_found_response(id, method),
    }
}
```

Change `discover_response` so it accepts a `resources_enabled: bool` argument:

```rust
fn discover_response(
    id: serde_json::Value,
    config: &RawStatelessHttpConfig,
    resources_enabled: bool,
) -> RawStatelessHttpResponse {
    let capabilities = if resources_enabled {
        json!({ "resources": {} })
    } else {
        json!({})
    };

    let mut result = DiscoverResult::new(
        config.supported_versions.clone(),
        capabilities,
        config.server_info.clone(),
    );

    if let Some(instructions) = &config.instructions {
        result = result.with_instructions(instructions);
    }

    success_response(id, result)
}
```

Add these private response helpers:

```rust
fn success_response<T: Serialize>(id: serde_json::Value, result: T) -> RawStatelessHttpResponse {
    RawStatelessHttpResponse::json(
        HTTP_OK,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
}

fn method_not_found_response(id: serde_json::Value, method: &str) -> RawStatelessHttpResponse {
    error_response(
        HTTP_NOT_FOUND,
        Some(id),
        JSON_RPC_METHOD_NOT_FOUND,
        format!("Method not found: {method}"),
        None,
    )
}

fn resources_list_response(
    id: serde_json::Value,
    provider: &dyn StatelessResourceProvider,
) -> RawStatelessHttpResponse {
    success_response(
        id,
        CacheableResult::new(
            CachePolicy::private_short(),
            ResourcesListPayload {
                resources: provider.list_resources(),
            },
        ),
    )
}

fn resources_read_response(
    id: serde_json::Value,
    body: &serde_json::Value,
    provider: &dyn StatelessResourceProvider,
) -> RawStatelessHttpResponse {
    let uri = body
        .get("params")
        .and_then(|params| params.get("uri"))
        .and_then(serde_json::Value::as_str)
        .expect("resources/read validation requires params.uri");

    match provider.read_resource(uri) {
        Ok(contents) => success_response(
            id,
            CacheableResult::new(
                CachePolicy::private_short(),
                ResourcesReadPayload { contents },
            ),
        ),
        Err(ResourceReadError::NotFound { uri }) => error_response(
            HTTP_NOT_FOUND,
            Some(id),
            JSON_RPC_INVALID_PARAMS,
            format!("Resource not found: {uri}"),
            Some(json!({ "uri": uri })),
        ),
    }
}
```

### 2.4 Add byte-entry resource dispatch

Keep the existing `handle_stateless_http_bytes` public function, but make it delegate to an inner helper:

```rust
pub fn handle_stateless_http_bytes(
    method: impl Into<String>,
    headers: Vec<(String, String)>,
    body: &[u8],
    config: &RawStatelessHttpConfig,
) -> RawStatelessHttpResponse {
    handle_stateless_http_bytes_inner(method, headers, body, config, None)
}

pub fn handle_stateless_http_bytes_with_resources<P: StatelessResourceProvider>(
    method: impl Into<String>,
    headers: Vec<(String, String)>,
    body: &[u8],
    config: &RawStatelessHttpConfig,
    resource_provider: &P,
) -> RawStatelessHttpResponse {
    handle_stateless_http_bytes_inner(
        method,
        headers,
        body,
        config,
        Some(resource_provider as &dyn StatelessResourceProvider),
    )
}
```

Add this private helper by moving the current byte parsing logic into it:

```rust
fn handle_stateless_http_bytes_inner(
    method: impl Into<String>,
    headers: Vec<(String, String)>,
    body: &[u8],
    config: &RawStatelessHttpConfig,
    resource_provider: Option<&dyn StatelessResourceProvider>,
) -> RawStatelessHttpResponse {
    let preflight_request = RawStatelessHttpRequest {
        method: method.into(),
        headers,
        body: serde_json::Value::Null,
    };

    if !preflight_request.method.eq_ignore_ascii_case("POST") {
        return error_response(
            HTTP_METHOD_NOT_ALLOWED,
            None,
            JSON_RPC_SERVER_ERROR,
            "Only POST is supported for stateless MCP requests",
            None,
        );
    }

    if !config
        .origin_policy
        .allows(origin_header(&preflight_request).as_deref())
    {
        return error_response(
            HTTP_FORBIDDEN,
            None,
            JSON_RPC_SERVER_ERROR,
            "Origin is not allowed",
            None,
        );
    }

    let parsed_body = match serde_json::from_slice(body) {
        Ok(body) => body,
        Err(_) => {
            return error_response(
                HTTP_BAD_REQUEST,
                None,
                JSON_RPC_PARSE_ERROR,
                "Parse error",
                None,
            );
        }
    };

    handle_stateless_http_request_inner(
        RawStatelessHttpRequest {
            method: preflight_request.method,
            headers: preflight_request.headers,
            body: parsed_body,
        },
        config,
        resource_provider,
    )
}
```

This preserves the existing pre-body Origin rejection behavior.

### 2.5 Verify Task 2

```bash
cargo fmt --check
cargo test -p conary-mcp stateless_http::tests::resource
cargo test -p conary-mcp stateless_http::tests::server_discover_returns_empty_capabilities
cargo test -p conary-mcp
```

### 2.6 Commit Task 2

```bash
git status --short
git add crates/conary-mcp/src/stateless_http.rs
git commit -m "feat(mcp): dispatch stateless resource requests"
```

Update this checkbox after the commit:

- [x] Task 2 complete

## Task 3: Expose Bootstrap Status From The Conary-Test Route

**Files:**
- `apps/conary-test/src/server/stateless_mcp.rs`

### 3.1 Add failing conary-test route tests first

In `apps/conary-test/src/server/stateless_mcp.rs`, update the existing discovery test name and assertions to expect resources:

```rust
#[tokio::test]
async fn stateless_discover_route_advertises_resources_only() {
    let app = create_router(test_fixtures::test_app_state(), None);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp/stateless")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                .header(HEADER_METHOD, "server/discover")
                .body(Body::from(
                    serde_json::to_vec(&discover_body("discover-route-1")).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["result"]["capabilities"]["resources"], json!({}));
    assert!(body["result"]["capabilities"].get("tools").is_none());
    assert!(body["result"]["capabilities"].get("prompts").is_none());
    assert_eq!(body["result"]["serverInfo"]["name"], "conary-test-mcp");
}
```

Add these tests in the same test module:

```rust
fn resource_list_body(id: &str) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "resources/list",
        "params": {
            "_meta": valid_meta()
        }
    })
}

fn resource_read_body(id: &str, uri: &str) -> serde_json::Value {
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

#[tokio::test]
async fn resources_list_route_returns_bootstrap_status_resource() {
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
    assert_eq!(body["result"]["resources"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["result"]["resources"][0]["uri"],
        "conary-local://bootstrap/status"
    );
    assert_eq!(body["result"]["resources"][0]["name"], "bootstrap_status");
    assert_eq!(
        body["result"]["resources"][0]["title"],
        "Local Bootstrap Status"
    );
    assert_eq!(
        body["result"]["resources"][0]["description"],
        "Read local developer bootstrap prerequisites and smoke-readiness state"
    );
    assert_eq!(body["result"]["resources"][0]["mimeType"], "application/json");
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
    assert_eq!(body["result"]["contents"][0]["mimeType"], "application/json");

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
```

Add this route-split regression test after the existing `legacy_mcp_route_does_not_return_stateless_discovery` test:

```rust
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
```

At the top of the test module, import `HEADER_NAME`, `JSON_RPC_HEADER_MISMATCH`, and `JSON_RPC_INVALID_PARAMS` from `conary_mcp::stateless`.

Run the focused test command and confirm it fails because the route is still discovery-only:

```bash
cargo test -p conary-test stateless_mcp::tests::resources_list_route_returns_bootstrap_status_resource
```

### 3.2 Implement the conary-test bootstrap resource provider

In `apps/conary-test/src/server/stateless_mcp.rs`, extend the existing `conary_mcp` imports so they include these names while preserving the current error/status imports used by the handler:

```rust
use conary_mcp::stateless::{
    ImplementationInfo, ResourceContent, ResourceDescriptor, MCP_DRAFT_PROTOCOL_VERSION,
};
use conary_mcp::stateless_http::{
    HTTP_BAD_REQUEST, JSON_RPC_PARSE_ERROR, OriginPolicy, RawStatelessHttpConfig,
    RawStatelessHttpResponse, ResourceReadError, StatelessResourceProvider,
    handle_stateless_http_bytes_with_resources,
};
```

Add this constant and provider near `stateless_config()`:

```rust
const BOOTSTRAP_STATUS_URI: &str = "conary-local://bootstrap/status";

#[derive(Debug, Default)]
struct BootstrapStatusResourceProvider;

impl StatelessResourceProvider for BootstrapStatusResourceProvider {
    fn list_resources(&self) -> Vec<ResourceDescriptor> {
        vec![ResourceDescriptor {
            uri: BOOTSTRAP_STATUS_URI.to_string(),
            name: "bootstrap_status".to_string(),
            title: Some("Local Bootstrap Status".to_string()),
            description: "Read local developer bootstrap prerequisites and smoke-readiness state"
                .to_string(),
            mime_type: "application/json".to_string(),
        }]
    }

    fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, ResourceReadError> {
        if uri != BOOTSTRAP_STATUS_URI {
            return Err(ResourceReadError::NotFound {
                uri: uri.to_string(),
            });
        }

        let inspect = crate::bootstrap::inspect_default();
        let text = serde_json::to_string_pretty(&inspect)
            .expect("bootstrap InspectResult should serialize to JSON");

        Ok(vec![ResourceContent {
            uri: BOOTSTRAP_STATUS_URI.to_string(),
            mime_type: "application/json".to_string(),
            text,
        }])
    }
}
```

Update `stateless_config()` so its instructions describe discovery plus the resource:

```rust
instructions: Some(
    "Conary test infrastructure stateless MCP endpoint exposes discovery plus one read-only bootstrap-status resource.".to_string(),
),
```

Update the handler body so it uses the resource-aware raw HTTP entrypoint:

```rust
let response = handle_stateless_http_bytes_with_resources(
    method.as_str(),
    headers,
    &body,
    &stateless_config(),
    &BootstrapStatusResourceProvider,
);
```

Keep the existing response conversion and `Content-Type: application/json` behavior unchanged.

### 3.3 Verify Task 3

```bash
cargo fmt --check
cargo test -p conary-test stateless_mcp
cargo test -p conary-test bootstrap
cargo test -p conary-test mcp_endpoint_requires_token
```

### 3.4 Commit Task 3

```bash
git status --short
git add apps/conary-test/src/server/stateless_mcp.rs
git commit -m "feat(conary-test): expose bootstrap status stateless resource"
```

Update this checkbox after the commit:

- [x] Task 3 complete

## Task 4: Run Scope Guards And Boundary Checks

**Files:**
- `crates/conary-mcp/tests/stateless_dependency_boundary.rs`

This task is primarily verification. Edit the guard file only if a guard assertion has become stale because of the intended `resources/list` and `resources/read` additions in `apps/conary-test/src/server/stateless_mcp.rs`.

### 4.1 Verify dependency boundaries

```bash
cargo test -p conary-mcp --test stateless_dependency_boundary
```

Expected result:
- `crates/conary-mcp/src/stateless.rs` and `crates/conary-mcp/src/stateless_http.rs` remain free of `rmcp`, `axum`, and session-era identifiers.
- Remi route and legacy MCP files remain free of draft stateless identifiers.
- `apps/conary-test/src/server/routes.rs` remains a route mount only.
- `apps/conary-test/src/server/stateless_mcp.rs` remains free of rmcp session types.

### 4.2 Verify no out-of-scope live surfaces were added

```bash
rg -n "tools/call|prompts/list|prompts/get|resources/templates|subscribe|listChanged" apps/conary-test/src/server/stateless_mcp.rs crates/conary-mcp/src/stateless_http.rs
```

Expected result:
- No matches.

```bash
rg -n "resources/list|resources/read|conary-local://bootstrap/status" apps/remi/src/server
```

Expected result:
- No matches.

```bash
rg -n "handle_stateless_http_bytes_with_resources|StatelessResourceProvider|conary-local://bootstrap/status" apps/conary-test/src/server crates/conary-mcp/src
```

Expected result:
- Matches only in the conary-test stateless route, raw stateless HTTP adapter, stateless types/tests, and related tests.

### 4.3 Commit Task 4 only if guard code changed

If `crates/conary-mcp/tests/stateless_dependency_boundary.rs` was edited:

```bash
git status --short
git add crates/conary-mcp/tests/stateless_dependency_boundary.rs
git commit -m "test(mcp): guard bootstrap status resource scope"
```

If the guard file was not edited, update this checkbox after the verification passes:

- [x] Task 4 complete

## Task 5: Update Docs And Reconcile Docs Audit Metadata

**Files:**
- `apps/conary-test/README.md`
- `docs/operations/agent-mcp-adapter-decision.md`
- `docs/operations/infrastructure.md`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

### 5.1 Update conary-test README

Change the `/mcp/stateless` section from discovery-only to discovery plus one read-only bootstrap resource. The section must say:
- `/mcp/stateless` supports `server/discover`, `resources/list`, and `resources/read`.
- The only live resource is `conary-local://bootstrap/status`.
- The resource returns the same structured bootstrap `InspectResult` used by `conary-test bootstrap check --json`.
- The route still does not expose tools, prompts, resource templates, subscriptions, SSE, or smoke execution.
- `/mcp` remains the legacy session-based rmcp endpoint.

### 5.2 Update adapter decision doc

In `docs/operations/agent-mcp-adapter-decision.md`, update the current-state text so it says:
- The raw stateless proof is live in conary-test at `/mcp/stateless`.
- The route now exposes discovery plus the `conary-local://bootstrap/status` read-only resource.
- Remi is still not wired to the stateless adapter.
- The live route still avoids tools, prompts, templates, subscriptions, SSE, and rmcp stateless assumptions.

### 5.3 Update infrastructure doc

In `docs/operations/infrastructure.md`, update the "Agent Operations And MCP" section so it says:
- Prefer MCP resources for read-only state inspection and MCP tools for audited mutations.
- conary-test currently exposes a stateless preview route with `server/discover`, `resources/list`, and `resources/read` for `conary-local://bootstrap/status`.
- Remi and legacy MCP endpoints remain session-based until the stateless adapter work is intentionally expanded.

### 5.4 Refresh docs audit inventory and ledger

First refresh inventory:

```bash
bash scripts/docs-audit-inventory.sh
```

Back up the ledger:

```bash
cp docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv.bak
```

Run this Python reconciliation script:

```bash
python - <<'PY'
from pathlib import Path

inventory_path = Path("docs/superpowers/documentation-accuracy-audit-inventory.tsv")
ledger_path = Path("docs/superpowers/documentation-accuracy-audit-ledger.tsv")

updated_paths = {
    "apps/conary-test/README.md": (
        "corrected",
        "Updated stateless MCP route docs for server/discover plus bootstrap status resource; verified no tools/prompts/SSE/templates."
    ),
    "docs/operations/agent-mcp-adapter-decision.md": (
        "corrected",
        "Updated adapter decision current state for conary-test bootstrap/status resource while Remi remains unwired."
    ),
    "docs/operations/infrastructure.md": (
        "corrected",
        "Updated agent operations guidance to prefer resources for read-only inspection and record bootstrap/status preview."
    ),
    "docs/superpowers/specs/2026-05-24-conary-test-bootstrap-status-resource-design.md": (
        "verified-no-change",
        "Source spec for bootstrap status resource slice remains active and accurate."
    ),
    "docs/superpowers/plans/2026-05-24-conary-test-bootstrap-status-resource.md": (
        "corrected",
        "Added implementation plan for the bootstrap status stateless resource slice."
    ),
}

inventory_lines = inventory_path.read_text().splitlines()
header, *inventory_rows_raw = inventory_lines
inventory_rows = {}
for line in inventory_rows_raw:
    if not line:
        continue
    path = line.split("\t", 1)[0]
    inventory_rows[path] = line

for path in updated_paths:
    if path not in inventory_rows:
        raise SystemExit(f"updated path not in refreshed inventory: {path}")

ledger_lines = ledger_path.read_text().splitlines()
ledger_header, *ledger_rows_raw = ledger_lines
ledger_rows = {}
for line in ledger_rows_raw:
    if not line:
        continue
    parts = line.split("\t")
    ledger_rows[parts[0]] = parts

output = [ledger_header]
for path in sorted(inventory_rows):
    if path in updated_paths:
        disposition, notes = updated_paths[path]
        output.append(f"{path}\t{disposition}\t2026-05-24\t{notes}")
        continue

    existing = ledger_rows.get(path)
    if existing:
        output.append("\t".join(existing))
    else:
        output.append(
            f"{path}\tverified-no-change\t2026-05-24\tPresent in refreshed inventory; no bootstrap/status changes required."
        )

ledger_path.write_text("\n".join(output) + "\n")
PY
```

Validate the ledger:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

If validation passes, remove the backup:

```bash
rm docs/superpowers/documentation-accuracy-audit-ledger.tsv.bak
```

### 5.5 Verify docs wording and scope

```bash
rg -n "/mcp/stateless|conary-local://bootstrap/status|resources/list|resources/read" apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md
```

Expected result:
- Active docs mention the bootstrap status resource and methods.

```bash
rg -n "discovery-only|only handles server/discover|no live resources" apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md
```

Expected result:
- No stale active-doc claim says `/mcp/stateless` is discovery-only or has no live resources.

### 5.6 Commit Task 5

```bash
git status --short
git add apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(mcp): record bootstrap status resource"
```

Update this checkbox after the commit:

- [x] Task 5 complete

## Task 6: Final Acceptance

### 6.1 Run final verification

```bash
cargo fmt --check
cargo test -p conary-mcp
cargo test -p conary-test stateless_mcp
cargo test -p conary-test bootstrap
cargo test -p conary-test mcp_endpoint_requires_token
cargo run -p conary-test -- bootstrap check --json
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

### 6.2 Run final scope sweeps

```bash
rg -n "tools/call|prompts/list|prompts/get|resources/templates|subscribe|listChanged" apps/conary-test/src/server/stateless_mcp.rs crates/conary-mcp/src/stateless_http.rs
```

Expected result:
- No matches.

```bash
rg -n "resources/list|resources/read|conary-local://bootstrap/status" apps/remi/src/server
```

Expected result:
- No matches.

```bash
rg -n "discovery-only|only handles server/discover|no live resources" apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md
```

Expected result:
- No stale active-doc claim says `/mcp/stateless` is discovery-only or has no live resources.

### 6.3 Mark the plan complete and commit the checkbox updates

Update all remaining checkboxes in this file from `- [ ]` to `- [x]`, then run:

```bash
git status --short
git add docs/superpowers/plans/2026-05-24-conary-test-bootstrap-status-resource.md
git commit -m "docs(plan): complete bootstrap status resource plan"
```

### 6.4 Confirm clean state

```bash
git status --short
git log --oneline -5
```

Expected result:
- Worktree is clean.
- Recent commits include the Task 1 through Task 6 commits.

- [ ] Task 6 complete

## Rollback Notes

If this slice must be reverted before merge, revert the task commits in reverse order. Reverting Task 3 removes the live conary-test route behavior while leaving reusable raw-adapter support from Tasks 1 and 2 in place. Reverting Tasks 1 and 2 removes the reusable resource support entirely.
