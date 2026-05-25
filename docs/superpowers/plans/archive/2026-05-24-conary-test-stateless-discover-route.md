# Conary-Test Stateless MCP Discover Route Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mount Conary's stateless MCP raw HTTP proof in `conary-test` at `POST /mcp/stateless`, exposing only `server/discover` while keeping legacy `/mcp` behavior unchanged.

**Architecture:** Keep `conary_mcp::stateless` as the protocol harness and `conary_mcp::stateless_http` as the framework-neutral HTTP adapter. Add a tiny Axum adapter in `apps/conary-test/src/server/stateless_mcp.rs`, mount it beside the existing `/mcp` nested `rmcp` service, and document that the stateless route is discovery-only. Remi remains untouched.

**Tech Stack:** Rust 1.94, Axum 0.8, `serde_json`, `conary-mcp`, current MCP draft stateless HTTP shape, Codex `/goal`.

---

## Codex `/goal` Operating Model

Recommended goal text:

```text
Implement docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md task-by-task. Source spec: docs/superpowers/specs/archive/2026-05-24-conary-test-stateless-discover-route-design.md. Add only a conary-test live stateless MCP discovery route at /mcp/stateless. Do not add Remi routes, live MCP resources, live MCP tools, live MCP prompts, SSE streaming, provider SDK integration, or rmcp stateless assumptions. Keep legacy /mcp behavior unchanged. For every task, write tests first, verify the expected failure, implement the smallest code/docs change, run the listed verification, update task checkboxes, and make one focused commit. Stop only when final acceptance passes.
```

Goal-mode checkpoint rules:

- Keep each task as one commit unless a task explicitly says otherwise.
- After every commit, run `git status --short`.
- Use `/goal pause` before leaving long-running verification unattended.
- Treat `DRAFT-2026-v1` as the current MCP draft token, not a durable Conary contract.
- The implementation plan is part of the work record; update checkboxes as tasks complete.

## Source Specs And Constraints

- Primary spec: `docs/superpowers/specs/archive/2026-05-24-conary-test-stateless-discover-route-design.md`
- Previous raw proof spec: `docs/superpowers/specs/archive/2026-05-24-stateless-raw-http-adapter-proof-design.md`
- Adapter decision: `docs/operations/agent-mcp-adapter-decision.md`
- Existing protocol harness: `crates/conary-mcp/src/stateless.rs`
- Existing raw proof: `crates/conary-mcp/src/stateless_http.rs`
- Existing conary-test router: `apps/conary-test/src/server/routes.rs`

Hard constraints:

- Do not modify Remi routes or Remi MCP behavior.
- Do not replace the existing `/mcp` route.
- Do not register live MCP resources, tools, prompts, or SSE streaming.
- Do not add `tools/list`, `resources/list`, `resources/read`, `prompts/list`, or `prompts/get` stubs.
- Do not change the conary-test bind address or CLI serve flags.
- Keep the new stateless route inside the existing conary-test auth boundary.
- `server/discover` must return an empty `capabilities` object.

## File Structure

Modify:

- `crates/conary-mcp/src/stateless_http.rs`: add byte-entry parsing helper and JSON-RPC parse-error mapping.
- `crates/conary-mcp/tests/stateless_dependency_boundary.rs`: update guard tests so only the new conary-test stateless adapter file can contain draft identifiers.
- `apps/conary-test/src/server/mod.rs`: export the new `stateless_mcp` module.
- `apps/conary-test/src/server/routes.rs`: mount `/mcp/stateless` beside legacy `/mcp`.
- `apps/conary-test/README.md`: document `/mcp/stateless` as discovery-only preview surface.
- `docs/operations/agent-mcp-adapter-decision.md`: record the first live conary-test stateless discovery route.
- `docs/operations/infrastructure.md`: describe legacy `/mcp` and stateless `/mcp/stateless` split.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`: refresh current tracked docs inventory.
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`: reconcile retained rows for active MCP plans/specs and changed docs.
- `docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md`: update checkboxes.

Create:

- `apps/conary-test/src/server/stateless_mcp.rs`: Axum adapter and route-level tests.

Do not modify:

- `apps/remi/src/server/mcp.rs`
- `apps/remi/src/server/routes/mcp.rs`

## Shared Test Helpers

Use this JSON-RPC request body in tests that need a valid stateless discovery request:

```rust
fn valid_discover_body() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "discover-1",
        "method": "server/discover",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": conary_mcp::stateless::MCP_DRAFT_PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientInfo": {
                    "name": "ConaryTestClient",
                    "version": "0.1.0"
                },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}
```

Use these headers for valid stateless requests:

```rust
.header("accept", "application/json, text/event-stream")
.header(
    conary_mcp::stateless::HEADER_PROTOCOL_VERSION,
    conary_mcp::stateless::MCP_DRAFT_PROTOCOL_VERSION,
)
.header(conary_mcp::stateless::HEADER_METHOD, "server/discover")
.header("content-type", "application/json")
```

## Task 1: Add Byte-Entry JSON Parsing To The Raw Proof

**Files:**
- Modify: `crates/conary-mcp/src/stateless_http.rs`
- Modify: `docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md`

- [x] **Step 1: Write failing byte-entry tests**

In `crates/conary-mcp/src/stateless_http.rs`, add these helpers and tests near the existing test helpers:

```rust
    fn valid_discover_headers() -> Vec<(String, String)> {
        vec![
            ("Accept".to_string(), "application/json, text/event-stream".to_string()),
            (
                "MCP-Protocol-Version".to_string(),
                MCP_DRAFT_PROTOCOL_VERSION.to_string(),
            ),
            ("Mcp-Method".to_string(), "server/discover".to_string()),
        ]
    }

    #[test]
    fn malformed_json_bytes_return_parse_error() {
        let response = handle_stateless_http_bytes(
            "POST",
            valid_discover_headers(),
            br#"{"jsonrpc": "2.0", "#,
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], Value::Null);
        assert_eq!(body["error"]["code"], JSON_RPC_PARSE_ERROR);
    }

    #[test]
    fn valid_json_bytes_delegate_to_parsed_handler() {
        let bytes = serde_json::to_vec(&discover_body("bytes-1")).unwrap();

        let response = handle_stateless_http_bytes(
            "POST",
            valid_discover_headers(),
            &bytes,
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_OK);
        let body = response_body(&response);
        assert_eq!(body["id"], "bytes-1");
        assert_eq!(body["result"]["serverInfo"]["name"], "conary-mcp");
    }

    #[test]
    fn non_post_byte_request_is_rejected_before_json_parse() {
        let response = handle_stateless_http_bytes(
            "GET",
            valid_discover_headers(),
            br#"{"jsonrpc": "2.0", "#,
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_METHOD_NOT_ALLOWED);
        let body = response_body(&response);
        assert_eq!(body["id"], Value::Null);
        assert_eq!(body["error"]["code"], JSON_RPC_SERVER_ERROR);
    }

    #[test]
    fn origin_byte_gate_runs_before_json_parse() {
        let mut headers = valid_discover_headers();
        headers.push(("Origin".to_string(), "https://evil.example".to_string()));

        let response = handle_stateless_http_bytes(
            "POST",
            headers,
            br#"{"jsonrpc": "2.0", "#,
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_FORBIDDEN);
        let body = response_body(&response);
        assert_eq!(body["id"], Value::Null);
        assert_eq!(body["error"]["code"], JSON_RPC_SERVER_ERROR);
    }
```

- [x] **Step 2: Run byte-entry tests and verify failure**

Run:

```bash
cargo test -p conary-mcp byte
```

Expected: compile failure mentioning missing `handle_stateless_http_bytes` and `JSON_RPC_PARSE_ERROR`.

- [x] **Step 3: Implement byte-entry helper**

In `crates/conary-mcp/src/stateless_http.rs`, add the parse-error constant near the existing JSON-RPC constants:

```rust
pub const JSON_RPC_PARSE_ERROR: i32 = -32700;
```

Add this public function after `handle_stateless_http_request`:

```rust
pub fn handle_stateless_http_bytes(
    method: impl Into<String>,
    headers: Vec<(String, String)>,
    body: &[u8],
    config: &RawStatelessHttpConfig,
) -> RawStatelessHttpResponse {
    let method = method.into();
    let preflight_request = RawStatelessHttpRequest {
        method,
        headers,
        body: Value::Null,
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

    handle_stateless_http_request(
        RawStatelessHttpRequest {
            method: preflight_request.method,
            headers: preflight_request.headers,
            body: parsed_body,
        },
        config,
    )
}
```

- [x] **Step 4: Run byte-entry tests and verify pass**

Run:

```bash
cargo test -p conary-mcp byte
```

Expected: all four tests pass.

- [x] **Step 5: Run the full conary-mcp package tests**

Run:

```bash
cargo test -p conary-mcp
```

Expected: all `conary-mcp` unit, integration, and doc tests pass.

- [x] **Step 6: Commit Task 1**

Update this task's checkboxes, then commit:

```bash
git add crates/conary-mcp/src/stateless_http.rs docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md
git commit -m "feat(mcp): parse raw stateless HTTP request bytes"
git status --short
```

Expected: commit succeeds and the worktree is clean.

## Task 2: Mount `/mcp/stateless` In Conary-Test

**Files:**
- Create: `apps/conary-test/src/server/stateless_mcp.rs`
- Modify: `apps/conary-test/src/server/mod.rs`
- Modify: `apps/conary-test/src/server/routes.rs`
- Modify: `docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md`

- [x] **Step 1: Create route tests before implementation**

In `apps/conary-test/src/server/mod.rs`, add the module declaration:

```rust
pub mod stateless_mcp;
```

Create `apps/conary-test/src/server/stateless_mcp.rs` with the path comment and route-level tests:

```rust
// conary-test/src/server/stateless_mcp.rs
//! Axum adapter for conary-test's draft stateless MCP discovery route.

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
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn stateless_discover_route_returns_conary_test_discovery() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                stateless_request("POST")
                    .body(Body::from(serde_json::to_vec(&discover_body("discover-1")).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["id"], "discover-1");
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["serverInfo"]["name"], "conary-test-mcp");
        assert_eq!(body["result"]["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(body["result"]["capabilities"].as_object().unwrap().is_empty());
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
                    .body(Body::from(serde_json::to_vec(&discover_body("auth-1")).unwrap()))
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
                    .body(Body::from(serde_json::to_vec(&discover_body("auth-2")).unwrap()))
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

        let body = read_json(response).await;
        assert!(
            body.get("result")
                .and_then(|result| result.get("resultType"))
                .is_none(),
            "legacy /mcp should not return stateless discovery"
        );
    }

    #[tokio::test]
    async fn stateless_route_rejects_bad_origin() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                stateless_request("POST")
                    .header("origin", "https://evil.example")
                    .body(Body::from(serde_json::to_vec(&discover_body("origin-1")).unwrap()))
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
                    .body(Body::from(serde_json::to_vec(&discover_body("missing-header-1")).unwrap()))
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
        assert_eq!(body["error"]["data"]["supported"][0], MCP_DRAFT_PROTOCOL_VERSION);
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
```

- [x] **Step 2: Run route tests and verify expected failures**

Run:

```bash
cargo test -p conary-test stateless_mcp
```

Expected: tests compile, and route tests fail because `/mcp/stateless` is not mounted yet.

- [x] **Step 3: Implement the Axum adapter**

Replace the top of `apps/conary-test/src/server/stateless_mcp.rs`, keeping the tests below it:

```rust
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
            "Conary test infrastructure stateless MCP endpoint exposes discovery only."
                .to_string(),
        ),
    }
}

fn raw_response_to_axum(response: RawStatelessHttpResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    let mut axum_response = match response.body {
        Some(body) => (status, Json(body)).into_response(),
        None => status.into_response(),
    };
    axum_response.headers_mut().insert(
        header::CONTENT_TYPE,
        response.content_type.parse().unwrap(),
    );
    axum_response
}
```

- [x] **Step 4: Mount the route next to legacy `/mcp`**

In `apps/conary-test/src/server/routes.rs`, add the module import:

```rust
use crate::server::stateless_mcp;
```

Change the routing import:

```rust
use axum::routing::{any, get, post};
```

Mount `/mcp/stateless` before the legacy nested service:

```rust
        .route("/mcp/stateless", any(stateless_mcp::handle))
        .nest_service("/mcp", mcp_service);
```

- [x] **Step 5: Run route tests and verify pass**

Run:

```bash
cargo test -p conary-test stateless_mcp
```

Expected: all `stateless_mcp` tests pass.

- [x] **Step 6: Re-run legacy auth regression**

Run:

```bash
cargo test -p conary-test mcp_endpoint_requires_token
```

Expected: existing legacy `/mcp` auth regression still passes.

- [x] **Step 7: Commit Task 2**

Update this task's checkboxes, then commit:

```bash
git add apps/conary-test/src/server/stateless_mcp.rs apps/conary-test/src/server/mod.rs apps/conary-test/src/server/routes.rs docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md
git commit -m "feat(conary-test): mount stateless MCP discovery route"
git status --short
```

Expected: commit succeeds and the worktree is clean.

## Task 3: Update Guard Tests For The New Live Route Boundary

**Files:**
- Modify: `crates/conary-mcp/tests/stateless_dependency_boundary.rs`
- Modify: `docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md`

- [x] **Step 1: Replace the live-route guard test**

In `crates/conary-mcp/tests/stateless_dependency_boundary.rs`, replace `live_mcp_server_files_do_not_contain_draft_stateless_identifiers` with these tests:

```rust
#[test]
fn remi_and_legacy_mcp_files_do_not_contain_draft_stateless_identifiers() {
    let root = repo_root();
    for path in [
        "apps/remi/src/server/mcp.rs",
        "apps/remi/src/server/routes/mcp.rs",
        "apps/conary-test/src/server/mcp.rs",
    ] {
        let source =
            fs::read_to_string(root.join(path)).expect("live MCP server file should be readable");
        for forbidden in [
            "Mcp-Method",
            "Mcp-Name",
            "server/discover",
            "MCP-Protocol-Version",
            "DRAFT-2026-v1",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} must not contain draft stateless identifier '{forbidden}'"
            );
        }
    }
}

#[test]
fn conary_test_routes_only_mounts_stateless_adapter() {
    let root = repo_root();
    let path = "apps/conary-test/src/server/routes.rs";
    let source = fs::read_to_string(root.join(path)).expect("routes file should be readable");

    assert!(
        source.contains("\"/mcp/stateless\""),
        "{path} should mount the stateless discovery route"
    );
    assert!(
        source.contains("stateless_mcp::handle"),
        "{path} should delegate stateless protocol handling to stateless_mcp"
    );

    for forbidden in [
        "MCP-Protocol-Version",
        "Mcp-Method",
        "Mcp-Name",
        "DRAFT-2026-v1",
        "io.modelcontextprotocol/",
        "handle_stateless_http_request",
        "handle_stateless_http_bytes",
        "server/discover",
    ] {
        assert!(
            !source.contains(forbidden),
            "{path} must only mount the stateless adapter, not contain protocol logic '{forbidden}'"
        );
    }
}

#[test]
fn conary_test_stateless_adapter_does_not_use_rmcp_session_types() {
    let root = repo_root();
    let path = "apps/conary-test/src/server/stateless_mcp.rs";
    let source =
        fs::read_to_string(root.join(path)).expect("stateless MCP adapter file should be readable");

    for forbidden in [
        "use rmcp",
        "rmcp::",
        "RoleServer",
        "ServerHandler",
        "StreamableHttpService",
        "LocalSessionManager",
        "Mcp-Session-Id",
        "InitializeResult",
    ] {
        assert!(
            !source.contains(forbidden),
            "{path} must not depend on legacy/session type {forbidden}"
        );
    }
}
```

- [x] **Step 2: Run guard tests and verify pass**

Run:

```bash
cargo test -p conary-mcp --test stateless_dependency_boundary
```

Expected: all guard tests pass.

- [x] **Step 3: Run route tests again**

Run:

```bash
cargo test -p conary-test stateless_mcp
```

Expected: all `stateless_mcp` tests still pass.

- [x] **Step 4: Commit Task 3**

Update this task's checkboxes, then commit:

```bash
git add crates/conary-mcp/tests/stateless_dependency_boundary.rs docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md
git commit -m "test(mcp): guard stateless discovery route boundary"
git status --short
```

Expected: commit succeeds and the worktree is clean.

## Task 4: Update Docs And Reconcile The Documentation Audit Ledger

**Files:**
- Modify: `apps/conary-test/README.md`
- Modify: `docs/operations/agent-mcp-adapter-decision.md`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md`

- [x] **Step 1: Update conary-test README endpoint wording**

In `apps/conary-test/README.md`, replace:

```markdown
The MCP endpoint is mounted at `/mcp` (Streamable HTTP transport).
```

with:

```markdown
The legacy MCP endpoint is mounted at `/mcp` through `rmcp`'s session-based
Streamable HTTP transport. The draft stateless preview endpoint is mounted at
`/mcp/stateless` and currently exposes only `server/discover` with empty
capabilities; it does not expose live tools, resources, prompts, or SSE
streaming.
```

- [x] **Step 2: Update adapter decision current state**

In `docs/operations/agent-mcp-adapter-decision.md`, update the frontmatter to:

```yaml
---
last_updated: 2026-05-24
revision: 6
summary: Decision record for Conary's stateless MCP adapter path, compliance harness, raw HTTP proof, and conary-test discovery route
---
```

Add this bullet to **Current State** after the `stateless_http` bullet:

```markdown
- `apps/conary-test` exposes `POST /mcp/stateless` as the first live
  stateless adapter gate. It handles only `server/discover`, returns empty
  capabilities, and keeps the legacy `/mcp` session-based tool surface
  unchanged.
```

Replace the **Current Choice** paragraph that says the selected adapter-gate slice is non-live with:

```markdown
Do not build new live MCP registrations on the existing session-based path.
After the contract, catalog, local bootstrap, compliance harness, and non-live
raw proof slices, the selected live adapter-gate slice is a `conary-test` route
at `POST /mcp/stateless`. It exposes only `server/discover` and advertises no
tools, resources, or prompts.
```

Add a new section after **Raw HTTP Proof Slice**:

```markdown
## Conary-Test Stateless Discovery Slice

The first live stateless adapter gate is `POST /mcp/stateless` in
`conary-test`. It adapts Axum requests into `crates/conary-mcp::stateless_http`,
uses `serverInfo.name = "conary-test-mcp"`, preserves the existing `/mcp`
session-based service, and stays inside the existing conary-test auth boundary
when a token is configured.

This route is discovery-only. It must not add resources, tools, prompts, SSE,
or Remi route behavior. First read-only resources remain a follow-on slice.

Source spec:

- `docs/superpowers/specs/archive/2026-05-24-conary-test-stateless-discover-route-design.md`
```

- [x] **Step 3: Update infrastructure MCP wording**

In `docs/operations/infrastructure.md`, replace the first paragraph under **Agent Operations And MCP** with:

```markdown
Today, the live Remi MCP endpoint and the legacy `conary-test` `/mcp` endpoint
are session-based, tool-only surfaces. `conary-test` also exposes
`/mcp/stateless` as a draft stateless discovery-only preview route; it returns
empty capabilities and does not expose live resources, tools, prompts, or SSE
streaming.
```

Replace:

```markdown
For the next stateless adapter, prefer MCP resources for read-only state
inspection and MCP tools for audited mutations. MCP is the adapter, not the
durable product contract:
```

with:

```markdown
For the next stateless slice, prefer MCP resources for read-only state
inspection and MCP tools for audited mutations. MCP is the adapter, not the
durable product contract:
```

- [x] **Step 4: Refresh inventory and reconcile ledger**

Run this script from the repo root:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
cp docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv.bak
python - <<'PY'
from pathlib import Path

inventory_path = Path("docs/superpowers/documentation-accuracy-audit-inventory.tsv")
ledger_path = Path("docs/superpowers/documentation-accuracy-audit-ledger.tsv")

inventory_rows = {}
for line in inventory_path.read_text().splitlines()[1:]:
    path, family, audience = line.split("\t")
    inventory_rows[path] = (family, audience)

header, *existing_lines = ledger_path.read_text().splitlines()
rows = {}
for line in existing_lines:
    parts = line.split("\t")
    origin = parts[0]
    if origin in inventory_rows:
        rows[origin] = parts

required_upsert_paths = [
    "apps/conary-test/README.md",
    "docs/operations/agent-mcp-adapter-decision.md",
    "docs/operations/infrastructure.md",
    "docs/superpowers/plans/archive/2026-05-22-llm-native-operations-surface.md",
    "docs/superpowers/plans/archive/2026-05-22-local-bootstrap-smoke-proof-loop.md",
    "docs/superpowers/plans/archive/2026-05-22-stateless-mcp-adapter-compliance.md",
    "docs/superpowers/plans/archive/2026-05-24-stateless-raw-http-adapter-proof.md",
    "docs/superpowers/specs/archive/2026-05-22-llm-native-operations-surface-design.md",
    "docs/superpowers/specs/archive/2026-05-22-stateless-mcp-adapter-compliance-design.md",
    "docs/superpowers/specs/archive/2026-05-24-stateless-raw-http-adapter-proof-design.md",
    "docs/superpowers/specs/archive/2026-05-24-conary-test-stateless-discover-route-design.md",
    "docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md",
]
for path in required_upsert_paths:
    if path not in inventory_rows:
        raise SystemExit(f"upserted path not in refreshed inventory: {path}")

def upsert(path, claim_clusters, evidence_sources, disposition, notes):
    family, audience = inventory_rows[path]
    rows[path] = [
        path,
        path,
        family,
        audience,
        claim_clusters,
        evidence_sources,
        "verified",
        disposition,
        notes,
    ]

upsert(
    "apps/conary-test/README.md",
    "test-harness; mcp; stateless-discovery; local-bootstrap",
    "apps/conary-test/src/server/routes.rs; apps/conary-test/src/server/stateless_mcp.rs; crates/conary-mcp/src/stateless_http.rs",
    "corrected",
    "Documented the legacy /mcp route and the draft /mcp/stateless discovery-only preview endpoint.",
)
upsert(
    "docs/operations/agent-mcp-adapter-decision.md",
    "mcp-adapter; stateless-discovery; conary-test",
    "crates/conary-mcp/src/stateless.rs; crates/conary-mcp/src/stateless_http.rs; apps/conary-test/src/server/stateless_mcp.rs; apps/conary-test/src/server/routes.rs",
    "corrected",
    "Recorded the conary-test /mcp/stateless route as the first live stateless adapter gate while keeping resources/tools/prompts deferred.",
)
upsert(
    "docs/operations/infrastructure.md",
    "operations; mcp; conary-test; stateless-discovery",
    "docs/operations/agent-mcp-adapter-decision.md; apps/conary-test/src/server/routes.rs; apps/conary-test/src/server/stateless_mcp.rs",
    "corrected",
    "Clarified the split between legacy session-based MCP routes and the conary-test stateless discovery-only preview route.",
)

for path in [
    "docs/superpowers/plans/archive/2026-05-22-llm-native-operations-surface.md",
    "docs/superpowers/plans/archive/2026-05-22-local-bootstrap-smoke-proof-loop.md",
    "docs/superpowers/plans/archive/2026-05-22-stateless-mcp-adapter-compliance.md",
    "docs/superpowers/plans/archive/2026-05-24-stateless-raw-http-adapter-proof.md",
    "docs/superpowers/specs/archive/2026-05-22-llm-native-operations-surface-design.md",
    "docs/superpowers/specs/archive/2026-05-22-stateless-mcp-adapter-compliance-design.md",
    "docs/superpowers/specs/archive/2026-05-24-stateless-raw-http-adapter-proof-design.md",
]:
    upsert(
        path,
        "llm-native-operations; mcp-adapter; active-planning-record",
        "docs/operations/agent-mcp-adapter-decision.md; crates/conary-mcp/src/stateless.rs; crates/conary-mcp/src/stateless_http.rs; apps/conary-test/src/bootstrap.rs",
        "verified-no-change",
        "Retained as an active planning/specification record for the current LLM-native MCP adapter workstream.",
    )

upsert(
    "docs/superpowers/specs/archive/2026-05-24-conary-test-stateless-discover-route-design.md",
    "llm-native-operations; mcp-adapter; conary-test; stateless-discovery",
    "crates/conary-mcp/src/stateless_http.rs; apps/conary-test/src/server/routes.rs; apps/conary-test/src/server/mcp.rs; docs/operations/agent-mcp-adapter-decision.md",
    "verified-no-change",
    "Active reviewed design for the conary-test stateless discovery route.",
)
upsert(
    "docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md",
    "llm-native-operations; mcp-adapter; conary-test; stateless-discovery",
    "docs/superpowers/specs/archive/2026-05-24-conary-test-stateless-discover-route-design.md; apps/conary-test/src/server/routes.rs; crates/conary-mcp/src/stateless_http.rs",
    "corrected",
    "Active goal-ready implementation plan for the conary-test stateless discovery route.",
)

missing = sorted(set(inventory_rows) - set(rows))
if missing:
    raise SystemExit("ledger missing current paths: " + ", ".join(missing))

ordered = [header]
for path in sorted(inventory_rows):
    ordered.append("\t".join(rows[path]))
ledger_path.write_text("\n".join(ordered) + "\n")
PY
```

This intentionally removes stale ledger rows whose origin paths are no longer tracked, including the retired `CLAUDE.md` row, because the refreshed inventory is the current baseline.
The committed inventory is known to be stale before this task; do not treat
`--require-complete` failures before this task as implementation failures.

- [x] **Step 5: Run docs audit verification**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected: `Documentation audit ledger check passed (--require-complete).`

After the check passes, remove the backup:

```bash
rm docs/superpowers/documentation-accuracy-audit-ledger.tsv.bak
```

- [x] **Step 6: Run focused documentation sweeps**

Run:

```bash
rg -n "/mcp/stateless|stateless discovery|server/discover" apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
rg -n "resources/list|resources/read|prompts/list|prompts/get|live tools|live resources|live prompts" apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md
```

Expected: first command finds the new discovery-only documentation; second command finds only wording that says live resources/tools/prompts are not exposed yet.

- [x] **Step 7: Commit Task 4**

Update this task's checkboxes, then commit:

```bash
git add apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md
git commit -m "docs(mcp): record conary-test stateless discovery route"
git status --short
```

Expected: commit succeeds and the worktree is clean.

## Task 5: Final Acceptance

**Files:**
- Modify: `docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md`

- [x] **Step 1: Run final Rust verification**

Run:

```bash
cargo fmt --check
cargo test -p conary-mcp
cargo test -p conary-test stateless
cargo test -p conary-test mcp_endpoint_requires_token
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every command succeeds.

- [x] **Step 2: Verify Remi has no draft stateless identifiers**

Run:

```bash
if rg -n "server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/remi/src/server; then
  echo "unexpected draft stateless identifier in Remi server code" >&2
  exit 1
fi
```

Expected: no output and exit code `0`.

- [x] **Step 3: Verify docs audit ledger**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected: `Documentation audit ledger check passed (--require-complete).`

- [x] **Step 4: Verify scope boundaries**

Run:

```bash
rg -n "/mcp/stateless|server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/conary-test/src/server
rg -n "server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/remi/src/server || true
rg -n "resources/list|resources/read|prompts/list|prompts/get|tools/list" apps/conary-test/src/server/stateless_mcp.rs crates/conary-mcp/src/stateless_http.rs
```

Expected:

- First command shows draft stateless identifiers only in `apps/conary-test/src/server/stateless_mcp.rs`, plus the `/mcp/stateless` mount in `routes.rs`.
- Second command returns no Remi matches.
- Third command finds tests for unsupported methods only; no live list/read/get route registrations exist.

- [x] **Step 5: Check formatting and worktree**

Run:

```bash
git diff --check
git status --short --branch
```

Expected: `git diff --check` succeeds. `git status --short --branch` shows only this plan file modified for final checkbox updates.

- [x] **Step 6: Commit final plan checkbox update**

Update this task's checkboxes, then commit:

```bash
git add docs/superpowers/plans/archive/2026-05-24-conary-test-stateless-discover-route.md
git commit -m "docs(plan): complete conary-test stateless discovery route"
git status --short --branch
```

Expected: commit succeeds and the worktree is clean.

## Final Acceptance Criteria

- `POST /mcp/stateless` exists in `conary-test`.
- Valid `server/discover` requests return draft-shaped discovery JSON.
- Discovery uses `serverInfo.name = "conary-test-mcp"`.
- Discovery advertises no tools, resources, or prompts.
- Existing `/mcp` behavior is unchanged and remains behind existing auth when configured.
- Remi has no live stateless route changes.
- Malformed JSON returns JSON-RPC parse error `-32700` after method and `Origin` gates pass.
- Guard tests distinguish the allowed conary-test stateless adapter from forbidden Remi and legacy MCP files.
- `docs/operations/agent-mcp-adapter-decision.md`, `docs/operations/infrastructure.md`, and `apps/conary-test/README.md` document the discovery-only route.
- Documentation audit ledger passes in `--require-complete` mode.
- Final verification commands in Task 5 pass.
