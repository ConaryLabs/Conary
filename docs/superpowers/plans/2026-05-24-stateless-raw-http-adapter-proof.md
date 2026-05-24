# Stateless Raw HTTP Adapter Proof Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a tested, non-live raw HTTP proof for Conary's target stateless MCP adapter that handles `server/discover`, protocol validation, origin checks, and JSON-RPC error envelopes.

**Architecture:** Keep `crates/conary-agent-contract` as the transport-neutral product contract and keep `crates/conary-mcp::stateless` as the draft protocol-shape harness. Add `crates/conary-mcp::stateless_http` as a framework-neutral adapter proof that depends on `serde_json` and the existing stateless helpers, not on `rmcp`, `axum`, Remi routes, or `conary-test` routes. The proof only succeeds for `server/discover`; validated resource/tool/prompt methods return JSON-RPC Method not found until a later live slice adds dispatch.

**Tech Stack:** Rust 1.94, `serde`, `serde_json`, existing `conary-mcp` and `conary-agent-contract` crates, current MCP draft stateless HTTP shape, Codex `/goal`.

---

## Codex `/goal` Operating Model

Use this plan with Codex Goal mode. Recommended goal text:

```text
Implement docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md task-by-task. Source spec: docs/superpowers/specs/2026-05-24-stateless-raw-http-adapter-proof-design.md. Add only a non-live, framework-neutral raw HTTP proof in crates/conary-mcp. Do not add Remi or conary-test routes, live MCP resources, live MCP tools, live MCP prompts, SSE streaming, or rmcp session-path changes. For implementation tasks, write tests before code and verify the expected failure before implementation. Tasks 2-4 add conformance tests over the Task 1 proof. Make one focused commit per task, update checkboxes, and stop only when final acceptance passes. In Task 1 Step 4, use crate::stateless::HEADER_PROTOCOL_VERSION, HEADER_METHOD, and HEADER_NAME instead of hard-coded MCP header strings in stateless_headers_from_request.
```

Goal-mode checkpoint rules:

- Keep each task as one commit unless the task says otherwise.
- After every commit, run `git status --short`.
- Use `/goal pause` before leaving long-running verification unattended.
- Do not mount this proof in Remi, `conary-test`, or any app route from this plan.
- Treat `DRAFT-2026-v1` as the current MCP draft token, not a durable Conary contract.

## Source Specs And Constraints

- Primary spec: `docs/superpowers/specs/2026-05-24-stateless-raw-http-adapter-proof-design.md`
- Adapter decision: `docs/operations/agent-mcp-adapter-decision.md`
- Existing protocol harness: `crates/conary-mcp/src/stateless.rs`
- Existing guard tests: `crates/conary-mcp/tests/stateless_dependency_boundary.rs`

Hard constraints:

- Do not add live Remi route behavior.
- Do not add live `conary-test` route behavior.
- Do not register live MCP resources, tools, prompts, or app discovery.
- Do not add `axum::Router`, bind a socket, implement SSE, or wire app routes.
- Do not depend on `rmcp` from the new raw proof module.
- `server/discover` must return an empty capabilities object and must not advertise `tools`, `resources`, or `prompts`.
- Cache metadata remains covered by the existing non-live harness and is deferred until the first live read-only resource slice.

## File Structure

Modify:

- `crates/conary-mcp/src/lib.rs`: export `stateless_http`.
- `crates/conary-mcp/src/stateless.rs`: add a public constructor for optional extracted headers so raw HTTP extraction can represent missing required headers.
- `crates/conary-mcp/tests/stateless_dependency_boundary.rs`: extend source guard tests to include `stateless_http.rs`.
- `docs/operations/agent-mcp-adapter-decision.md`: record that the raw HTTP proof is implemented and remains non-live.
- `docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md`: update task checkboxes as each step completes.

Create:

- `crates/conary-mcp/src/stateless_http.rs`: framework-neutral raw HTTP proof, request/response types, origin policy, header extraction, JSON-RPC envelope validation, response mapping, and unit tests.

Do not modify:

- `apps/remi/src/server/mcp.rs`
- `apps/remi/src/server/routes/mcp.rs`
- `apps/conary-test/src/server/mcp.rs`
- `apps/conary-test/src/server/routes.rs`

## Shared Type Shape

Use these names consistently in every task:

- `RawStatelessHttpRequest`
- `RawStatelessHttpResponse`
- `RawStatelessHttpConfig`
- `OriginPolicy`
- `handle_stateless_http_request`
- `HTTP_OK`, `HTTP_BAD_REQUEST`, `HTTP_FORBIDDEN`, `HTTP_METHOD_NOT_ALLOWED`, `HTTP_NOT_FOUND`
- `JSON_RPC_SERVER_ERROR`, `JSON_RPC_INVALID_REQUEST`, `JSON_RPC_METHOD_NOT_FOUND`

## Task 1: Add Raw Proof Success Path And Origin/Method Transport Gates

**Files:**
- Modify: `crates/conary-mcp/src/lib.rs`
- Modify: `crates/conary-mcp/src/stateless.rs`
- Create: `crates/conary-mcp/src/stateless_http.rs`

- [x] **Step 1: Export the planned module and add optional header construction**

In `crates/conary-mcp/src/lib.rs`, add the new module export next to `stateless`:

```rust
pub mod stateless;
pub mod stateless_http;
```

In `crates/conary-mcp/src/stateless.rs`, add this method to the `impl StatelessRequestHeaders` block:

```rust
    pub fn from_optional_parts(
        protocol_version: Option<String>,
        method: Option<String>,
        name: Option<String>,
        accepts: Vec<String>,
    ) -> Self {
        Self {
            protocol_version,
            method,
            name,
            accepts,
        }
    }
```

- [x] **Step 2: Write failing success-path tests**

Create `crates/conary-mcp/src/stateless_http.rs` with the path comment, module docs, and these tests:

```rust
// crates/conary-mcp/src/stateless_http.rs
//! Framework-neutral raw HTTP proof for the target stateless MCP adapter.

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::stateless::{
        JSON_RPC_HEADER_MISMATCH, JSON_RPC_INVALID_PARAMS,
        JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION,
        MCP_DRAFT_PROTOCOL_VERSION,
    };

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

    fn valid_discover_request(id: &str) -> RawStatelessHttpRequest {
        RawStatelessHttpRequest::post(discover_body(id))
            .with_header("Accept", "application/json")
            .with_header("Accept", "text/event-stream")
            .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
            .with_header("Mcp-Method", "server/discover")
    }

    fn response_body(response: &RawStatelessHttpResponse) -> &Value {
        response.body.as_ref().expect("response should include JSON body")
    }

    #[test]
    fn server_discover_returns_empty_capabilities() {
        let response = handle_stateless_http_request(
            valid_discover_request("discover-1"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_OK);
        assert_eq!(response.content_type, "application/json");
        let body = response_body(&response);
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], "discover-1");
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(
            body["result"]["supportedVersions"][0],
            MCP_DRAFT_PROTOCOL_VERSION
        );
        assert_eq!(body["result"]["serverInfo"]["name"], "conary-mcp");

        let capabilities = body["result"]["capabilities"]
            .as_object()
            .expect("capabilities should be an object");
        assert!(capabilities.is_empty());
        assert!(capabilities.get("tools").is_none());
        assert!(capabilities.get("resources").is_none());
        assert!(capabilities.get("prompts").is_none());
    }

    #[test]
    fn invalid_present_origin_is_rejected_before_body_trust() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(json!({
                "jsonrpc": "1.0",
                "id": "must-not-leak",
                "method": 7
            }))
            .with_header("Origin", "https://evil.example"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_FORBIDDEN);
        let body = response_body(&response);
        assert_eq!(body["id"], Value::Null);
        assert_eq!(body["error"]["code"], JSON_RPC_SERVER_ERROR);
    }

    #[test]
    fn missing_origin_is_accepted_for_local_non_browser_clients() {
        let response = handle_stateless_http_request(
            valid_discover_request("discover-3"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_OK);
    }

    #[test]
    fn configured_origin_is_accepted_exactly() {
        let config = RawStatelessHttpConfig {
            origin_policy: OriginPolicy::exact_origins(["https://forge.local"]),
            ..RawStatelessHttpConfig::default()
        };

        let response = handle_stateless_http_request(
            valid_discover_request("discover-4").with_header("Origin", "https://forge.local"),
            &config,
        );

        assert_eq!(response.status, HTTP_OK);
    }

    #[test]
    fn non_matching_exact_origin_is_rejected() {
        let config = RawStatelessHttpConfig {
            origin_policy: OriginPolicy::exact_origins(["https://forge.local"]),
            ..RawStatelessHttpConfig::default()
        };

        let response = handle_stateless_http_request(
            valid_discover_request("bad-origin-1").with_header("Origin", "https://evil.example"),
            &config,
        );

        assert_eq!(response.status, HTTP_FORBIDDEN);
    }

    #[test]
    fn non_post_request_returns_method_not_allowed() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::new("GET", discover_body("discover-5")),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_METHOD_NOT_ALLOWED);
        let body = response_body(&response);
        assert_eq!(body["id"], "discover-5");
        assert_eq!(body["error"]["code"], JSON_RPC_SERVER_ERROR);
    }
}
```

- [x] **Step 3: Run tests and verify they fail**

Run:

```bash
cargo test -p conary-mcp stateless_http
```

Expected: FAIL with unresolved names such as `RawStatelessHttpRequest`, `RawStatelessHttpResponse`, `RawStatelessHttpConfig`, `OriginPolicy`, `handle_stateless_http_request`, and the HTTP/JSON-RPC constants.

- [x] **Step 4: Implement the success path and transport gates**

Add this implementation above the tests in `crates/conary-mcp/src/stateless_http.rs`:

```rust
use crate::stateless::{
    validate_stateless_request, DiscoverResult, ImplementationInfo, StatelessProtocolError,
    StatelessRequestHeaders, UnsupportedProtocolVersion, HEADER_METHOD, HEADER_NAME,
    HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION,
};
use serde_json::{json, Value};

pub const HTTP_OK: u16 = 200;
pub const HTTP_BAD_REQUEST: u16 = 400;
pub const HTTP_FORBIDDEN: u16 = 403;
pub const HTTP_METHOD_NOT_ALLOWED: u16 = 405;
pub const HTTP_NOT_FOUND: u16 = 404;

// Origin rejection and non-POST are HTTP-layer gates; HTTP status
// disambiguates these server-defined JSON-RPC errors.
pub const JSON_RPC_SERVER_ERROR: i32 = -32000;
pub const JSON_RPC_INVALID_REQUEST: i32 = -32600;
pub const JSON_RPC_METHOD_NOT_FOUND: i32 = -32601;

#[derive(Debug, Clone, PartialEq)]
pub struct RawStatelessHttpRequest {
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl RawStatelessHttpRequest {
    pub fn new(method: impl Into<String>, body: Value) -> Self {
        Self {
            method: method.into(),
            headers: Vec::new(),
            body,
        }
    }

    pub fn post(body: Value) -> Self {
        Self::new("POST", body)
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawStatelessHttpResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Option<Value>,
}

impl RawStatelessHttpResponse {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: Some(body),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginPolicy {
    allow_missing: bool,
    allowed_origins: Vec<String>,
}

impl OriginPolicy {
    pub fn local_non_browser() -> Self {
        Self {
            allow_missing: true,
            allowed_origins: Vec::new(),
        }
    }

    pub fn exact_origins<I, S>(origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allow_missing: false,
            allowed_origins: origins.into_iter().map(Into::into).collect(),
        }
    }

    fn allows(&self, origin: Option<&str>) -> bool {
        match origin {
            Some(origin) => self
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin),
            None => self.allow_missing,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawStatelessHttpConfig {
    pub origin_policy: OriginPolicy,
    pub supported_versions: Vec<String>,
    pub server_info: ImplementationInfo,
    pub instructions: Option<String>,
}

impl Default for RawStatelessHttpConfig {
    fn default() -> Self {
        Self {
            origin_policy: OriginPolicy::local_non_browser(),
            supported_versions: vec![MCP_DRAFT_PROTOCOL_VERSION.to_string()],
            server_info: ImplementationInfo::new("conary-mcp", env!("CARGO_PKG_VERSION")),
            instructions: Some(
                "Conary stateless MCP adapter proof exposes discovery only.".to_string(),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct JsonRpcRequestEnvelope {
    id: Value,
    method: String,
}

pub fn handle_stateless_http_request(
    request: RawStatelessHttpRequest,
    config: &RawStatelessHttpConfig,
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

    if !config.origin_policy.allows(origin_header(&request).as_deref()) {
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

    let headers = stateless_headers_from_request(&request);
    let supported_versions: Vec<&str> = config
        .supported_versions
        .iter()
        .map(String::as_str)
        .collect();

    if let Err(err) = validate_stateless_request(&headers, &request.body, &supported_versions) {
        return stateless_protocol_error_response(envelope.id, err);
    }

    match envelope.method.as_str() {
        "server/discover" => discover_response(envelope.id, config),
        method => error_response(
            HTTP_NOT_FOUND,
            Some(envelope.id),
            JSON_RPC_METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
            None,
        ),
    }
}

fn discover_response(id: Value, config: &RawStatelessHttpConfig) -> RawStatelessHttpResponse {
    let mut result = DiscoverResult::new(
        config.supported_versions.clone(),
        json!({}),
        config.server_info.clone(),
    );

    if let Some(instructions) = &config.instructions {
        result = result.with_instructions(instructions);
    }

    RawStatelessHttpResponse::json(
        HTTP_OK,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
}

fn stateless_protocol_error_response(
    id: Value,
    err: StatelessProtocolError,
) -> RawStatelessHttpResponse {
    let code = err.json_rpc_error_code();
    let data = match &err {
        StatelessProtocolError::UnsupportedProtocolVersion {
            requested,
            supported,
        } => Some(json!(UnsupportedProtocolVersion::new(
            requested.clone(),
            supported.clone()
        ))),
        _ => Some(json!({ "kind": err.code() })),
    };

    error_response(HTTP_BAD_REQUEST, Some(id), code, err.to_string(), data)
}

fn error_response(
    status: u16,
    id: Option<Value>,
    code: i32,
    message: impl Into<String>,
    data: Option<Value>,
) -> RawStatelessHttpResponse {
    let mut error = json!({
        "code": code,
        "message": message.into(),
    });

    if let Some(data) = data {
        error["data"] = data;
    }

    RawStatelessHttpResponse::json(
        status,
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": error,
        }),
    )
}

fn validate_json_rpc_envelope(body: &Value) -> Result<JsonRpcRequestEnvelope, &'static str> {
    let Some(object) = body.as_object() else {
        return Err("JSON-RPC body must be an object");
    };

    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("JSON-RPC version must be 2.0");
    }

    let Some(id) = object.get("id") else {
        return Err("JSON-RPC request id is required");
    };

    if !is_valid_request_id(id) {
        return Err("JSON-RPC request id must be a string, number, or null");
    }

    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Err("JSON-RPC method is required");
    };

    Ok(JsonRpcRequestEnvelope {
        id: id.clone(),
        method: method.to_string(),
    })
}

fn is_valid_request_id(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_) | Value::Null)
}

fn extract_scalar_id(body: &Value) -> Option<Value> {
    body.as_object()
        .and_then(|object| object.get("id"))
        .filter(|id| is_valid_request_id(id))
        .cloned()
}

fn stateless_headers_from_request(request: &RawStatelessHttpRequest) -> StatelessRequestHeaders {
    StatelessRequestHeaders::from_optional_parts(
        first_header_value(request, HEADER_PROTOCOL_VERSION),
        first_header_value(request, HEADER_METHOD),
        first_header_value(request, HEADER_NAME),
        accept_media_types(request),
    )
}

fn origin_header(request: &RawStatelessHttpRequest) -> Option<String> {
    first_header_value(request, "Origin")
}

fn first_header_value(request: &RawStatelessHttpRequest, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn accept_media_types(request: &RawStatelessHttpRequest) -> Vec<String> {
    request
        .headers
        .iter()
        .filter(|(header_name, _)| header_name.eq_ignore_ascii_case("Accept"))
        .flat_map(|(_, value)| value.split(','))
        .filter_map(|part| {
            // This proof strips today's simple media-type parameters, not the
            // full quoted-parameter grammar from HTTP content negotiation.
            let media_type = part
                .trim()
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            (!media_type.is_empty()).then_some(media_type)
        })
        .collect()
}
```

- [x] **Step 5: Run success-path tests**

Run:

```bash
cargo test -p conary-mcp stateless_http::tests::server_discover_returns_empty_capabilities
cargo test -p conary-mcp stateless_http::tests::invalid_present_origin_is_rejected_before_body_trust
cargo test -p conary-mcp stateless_http::tests::missing_origin_is_accepted_for_local_non_browser_clients
cargo test -p conary-mcp stateless_http::tests::configured_origin_is_accepted_exactly
cargo test -p conary-mcp stateless_http::tests::non_matching_exact_origin_is_rejected
cargo test -p conary-mcp stateless_http::tests::non_post_request_returns_method_not_allowed
```

Expected: PASS.

- [x] **Step 6: Commit**

Run:

```bash
cargo fmt
cargo test -p conary-mcp stateless_http
git add crates/conary-mcp/src/lib.rs crates/conary-mcp/src/stateless.rs crates/conary-mcp/src/stateless_http.rs docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md
git commit -m "feat(mcp): add raw stateless HTTP proof"
git status --short
```

Expected: tests pass, commit succeeds, and status is clean.

## Task 2: Prove Draft Header Extraction Rules

**Files:**
- Modify: `crates/conary-mcp/src/stateless_http.rs`

- [x] **Step 1: Add header parsing conformance tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/conary-mcp/src/stateless_http.rs`:

```rust
    #[test]
    fn lowercase_mcp_headers_are_accepted() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(discover_body("lowercase-1"))
                .with_header("accept", "application/json")
                .with_header("accept", "text/event-stream")
                .with_header("mcp-protocol-version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("mcp-method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_OK);
    }

    #[test]
    fn comma_separated_accept_header_is_parsed() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(discover_body("accept-1"))
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_OK);
    }

    #[test]
    fn repeated_accept_headers_are_parsed() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(discover_body("accept-2"))
                .with_header("Accept", "application/json")
                .with_header("Accept", "text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_OK);
    }

    #[test]
    fn accept_parameters_and_quality_values_are_ignored_for_matching() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(discover_body("accept-3"))
                .with_header(
                    "Accept",
                    "application/json; charset=utf-8, text/event-stream; q=0.9",
                )
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_OK);
    }
```

- [x] **Step 2: Run header tests and verify behavior**

Run:

```bash
cargo test -p conary-mcp stateless_http::tests::lowercase_mcp_headers_are_accepted
cargo test -p conary-mcp stateless_http::tests::comma_separated_accept_header_is_parsed
cargo test -p conary-mcp stateless_http::tests::repeated_accept_headers_are_parsed
cargo test -p conary-mcp stateless_http::tests::accept_parameters_and_quality_values_are_ignored_for_matching
```

Expected: PASS. If any command fails, fix only `first_header_value` or `accept_media_types` in `crates/conary-mcp/src/stateless_http.rs` so header names are case-insensitive and `Accept` parsing strips parameters after `;`.

- [x] **Step 3: Commit**

Run:

```bash
cargo fmt
cargo test -p conary-mcp stateless_http
git add crates/conary-mcp/src/stateless_http.rs docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md
git commit -m "test(mcp): prove raw stateless header extraction"
git status --short
```

Expected: tests pass, commit succeeds, and status is clean.

## Task 3: Prove JSON-RPC Envelope Rejection Policy

**Files:**
- Modify: `crates/conary-mcp/src/stateless_http.rs`

- [ ] **Step 1: Add JSON-RPC envelope conformance tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/conary-mcp/src/stateless_http.rs`:

```rust
    #[test]
    fn malformed_json_rpc_envelopes_return_invalid_request() {
        let cases = [
            ("batch", json!([]), Value::Null),
            ("notification", json!({"jsonrpc": "2.0", "method": "server/discover"}), Value::Null),
            ("response", json!({"jsonrpc": "2.0", "id": "r1", "result": {}}), json!("r1")),
            ("non_object", json!("not an object"), Value::Null),
            ("wrong_jsonrpc", json!({"jsonrpc": "1.0", "id": "bad-1", "method": "server/discover"}), json!("bad-1")),
            ("missing_method", json!({"jsonrpc": "2.0", "id": "bad-2"}), json!("bad-2")),
            ("non_string_method", json!({"jsonrpc": "2.0", "id": "bad-3", "method": 7}), json!("bad-3")),
        ];

        for (name, body, expected_id) in cases {
            let response = handle_stateless_http_request(
                RawStatelessHttpRequest::post(body)
                    .with_header("Accept", "application/json, text/event-stream")
                    .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                    .with_header("Mcp-Method", "server/discover"),
                &RawStatelessHttpConfig::default(),
            );

            assert_eq!(response.status, HTTP_BAD_REQUEST, "{name}");
            let body = response_body(&response);
            assert_eq!(body["id"], expected_id, "{name}");
            assert_eq!(body["error"]["code"], JSON_RPC_INVALID_REQUEST, "{name}");
        }
    }

    #[test]
    fn invalid_json_rpc_id_is_rejected() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(json!({
                "jsonrpc": "2.0",
                "id": {"nested": true},
                "method": "server/discover",
                "params": {"_meta": valid_meta()}
            }))
            .with_header("Accept", "application/json, text/event-stream")
            .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
            .with_header("Mcp-Method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], Value::Null);
        assert_eq!(body["error"]["code"], JSON_RPC_INVALID_REQUEST);
    }
```

- [ ] **Step 2: Run envelope tests**

Run:

```bash
cargo test -p conary-mcp stateless_http::tests::malformed_json_rpc_envelopes_return_invalid_request
cargo test -p conary-mcp stateless_http::tests::invalid_json_rpc_id_is_rejected
```

Expected: PASS. If either command fails, fix only `validate_json_rpc_envelope`, `extract_scalar_id`, or `is_valid_request_id` in `crates/conary-mcp/src/stateless_http.rs`.

- [ ] **Step 3: Commit**

Run:

```bash
cargo fmt
cargo test -p conary-mcp stateless_http
git add crates/conary-mcp/src/stateless_http.rs docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md
git commit -m "test(mcp): prove raw stateless JSON-RPC envelope policy"
git status --short
```

Expected: tests pass, commit succeeds, and status is clean.

## Task 4: Prove Protocol Error And Unsupported Method Mapping

**Files:**
- Modify: `crates/conary-mcp/src/stateless_http.rs`

- [ ] **Step 1: Add protocol mapping conformance tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/conary-mcp/src/stateless_http.rs`:

```rust
    #[test]
    fn missing_protocol_version_header_returns_header_mismatch_code() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(discover_body("missing-protocol-1"))
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("Mcp-Method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], "missing-protocol-1");
        assert_eq!(body["error"]["code"], JSON_RPC_HEADER_MISMATCH);
        assert_eq!(body["error"]["data"]["kind"], "missing_header");
    }

    #[test]
    fn unsupported_protocol_version_returns_supported_and_requested_data() {
        let mut body = discover_body("unsupported-protocol-1");
        body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("DRAFT-OLD");

        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(body)
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", "DRAFT-OLD")
                .with_header("Mcp-Method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], "unsupported-protocol-1");
        assert_eq!(
            body["error"]["code"],
            JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION
        );
        assert_eq!(body["error"]["data"]["requested"], "DRAFT-OLD");
        assert_eq!(
            body["error"]["data"]["supported"][0],
            MCP_DRAFT_PROTOCOL_VERSION
        );
    }

    #[test]
    fn mismatched_mcp_method_header_returns_header_mismatch_code() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(discover_body("method-mismatch-1"))
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "tools/list"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], "method-mismatch-1");
        assert_eq!(body["error"]["code"], JSON_RPC_HEADER_MISMATCH);
        assert_eq!(body["error"]["data"]["kind"], "header_mismatch");
    }

    #[test]
    fn unsupported_validated_method_returns_json_rpc_method_not_found() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": "tools-list-1",
            "method": "tools/list",
            "params": {
                "_meta": valid_meta()
            }
        });

        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(body)
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "tools/list"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_NOT_FOUND);
        let body = response_body(&response);
        assert_eq!(body["id"], "tools-list-1");
        assert_eq!(body["error"]["code"], JSON_RPC_METHOD_NOT_FOUND);
    }

    #[test]
    fn missing_mcp_method_header_returns_header_mismatch_code() {
        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(discover_body("missing-method-1"))
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], "missing-method-1");
        assert_eq!(body["error"]["code"], JSON_RPC_HEADER_MISMATCH);
        assert_eq!(body["error"]["data"]["kind"], "missing_header");
    }

    #[test]
    fn missing_meta_fields_return_invalid_params_code() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": "no-meta-1",
            "method": "server/discover",
            "params": {}
        });

        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(body)
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "server/discover"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], "no-meta-1");
        assert_eq!(body["error"]["code"], JSON_RPC_INVALID_PARAMS);
        assert_eq!(body["error"]["data"]["kind"], "missing_meta_field");
    }

    #[test]
    fn resources_read_requires_mcp_name_before_unsupported_method_mapping() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": "resources-read-1",
            "method": "resources/read",
            "params": {
                "uri": "conary://remi/health",
                "_meta": valid_meta()
            }
        });

        let missing_name = handle_stateless_http_request(
            RawStatelessHttpRequest::post(body.clone())
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "resources/read"),
            &RawStatelessHttpConfig::default(),
        );
        assert_eq!(missing_name.status, HTTP_BAD_REQUEST);
        let missing_name_body = response_body(&missing_name);
        assert_eq!(
            missing_name_body["error"]["code"],
            JSON_RPC_HEADER_MISMATCH
        );
        assert_eq!(missing_name_body["error"]["data"]["kind"], "missing_name");

        let with_name = handle_stateless_http_request(
            RawStatelessHttpRequest::post(body)
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "resources/read")
                .with_header("Mcp-Name", "conary://remi/health"),
            &RawStatelessHttpConfig::default(),
        );
        assert_eq!(with_name.status, HTTP_NOT_FOUND);
        let with_name_body = response_body(&with_name);
        assert_eq!(with_name_body["error"]["code"], JSON_RPC_METHOD_NOT_FOUND);
    }
```

- [ ] **Step 2: Run protocol mapping tests**

Run:

```bash
cargo test -p conary-mcp stateless_http::tests::missing_protocol_version_header_returns_header_mismatch_code
cargo test -p conary-mcp stateless_http::tests::unsupported_protocol_version_returns_supported_and_requested_data
cargo test -p conary-mcp stateless_http::tests::mismatched_mcp_method_header_returns_header_mismatch_code
cargo test -p conary-mcp stateless_http::tests::unsupported_validated_method_returns_json_rpc_method_not_found
cargo test -p conary-mcp stateless_http::tests::missing_mcp_method_header_returns_header_mismatch_code
cargo test -p conary-mcp stateless_http::tests::missing_meta_fields_return_invalid_params_code
cargo test -p conary-mcp stateless_http::tests::resources_read_requires_mcp_name_before_unsupported_method_mapping
```

Expected: PASS. If any command fails, fix only `stateless_protocol_error_response`, `error_response`, or `handle_stateless_http_request` dispatch in `crates/conary-mcp/src/stateless_http.rs`.

- [ ] **Step 3: Run all raw proof tests**

Run:

```bash
cargo test -p conary-mcp stateless_http
```

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
cargo fmt
cargo test -p conary-mcp stateless_http
git add crates/conary-mcp/src/stateless_http.rs docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md
git commit -m "test(mcp): prove raw stateless error mapping"
git status --short
```

Expected: tests pass, commit succeeds, and status is clean.

## Task 5: Add Guard Coverage And Implementation Docs

**Files:**
- Modify: `crates/conary-mcp/tests/stateless_dependency_boundary.rs`
- Modify: `docs/operations/agent-mcp-adapter-decision.md`

- [ ] **Step 1: Extend guard tests to cover the raw proof module**

Replace the entire contents of `crates/conary-mcp/tests/stateless_dependency_boundary.rs` with:

```rust
// crates/conary-mcp/tests/stateless_dependency_boundary.rs
//! Guard tests for the stateless MCP compliance harness boundary.

use std::{fs, path::PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("conary-mcp should live under crates/")
        .to_path_buf()
}

#[test]
fn stateless_modules_do_not_use_rmcp_or_live_http_framework_types() {
    for module_path in ["src/stateless.rs", "src/stateless_http.rs"] {
        let source = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(module_path),
        )
        .expect("stateless module should be readable");

        for forbidden in [
            "use rmcp",
            "rmcp::",
            "RoleServer",
            "ServerHandler",
            "StreamableHttpService",
            "LocalSessionManager",
            "Mcp-Session-Id",
            "InitializeResult",
            "use axum",
            "axum::",
        ] {
            assert!(
                !source.contains(forbidden),
                "{module_path} must not depend on legacy/session/live HTTP type {forbidden}"
            );
        }
    }
}

#[test]
fn live_mcp_server_files_do_not_contain_draft_stateless_identifiers() {
    let root = repo_root();
    for path in [
        "apps/remi/src/server/mcp.rs",
        "apps/remi/src/server/routes/mcp.rs",
        "apps/conary-test/src/server/mcp.rs",
        "apps/conary-test/src/server/routes.rs",
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
                "{path} must not contain draft stateless identifier '{forbidden}' until a live adapter slice adds it"
            );
        }
    }
}
```

- [ ] **Step 2: Run guard tests**

Run:

```bash
cargo test -p conary-mcp --test stateless_dependency_boundary
```

Expected: PASS.

- [ ] **Step 3: Update the adapter decision record with implementation facts**

In `docs/operations/agent-mcp-adapter-decision.md`, update the frontmatter:

```yaml
last_updated: 2026-05-24
revision: 5
summary: Decision record for Conary's stateless MCP adapter path, compliance harness, and non-live raw HTTP proof implementation
```

In `## Current State`, add these bullets after the existing `crates/conary-mcp::stateless` bullet:

```markdown
- `crates/conary-mcp::stateless_http` contains the non-live raw HTTP proof for
  `server/discover`, origin validation, JSON-RPC envelope validation, header
  extraction, protocol error mapping, and unsupported-method responses
- The raw HTTP proof does not mount routes, bind sockets, register resources,
  register tools, register prompts, or depend on `rmcp` / `axum`
```

Replace the first paragraph under `## Raw HTTP Proof Slice` with:

```markdown
The current raw HTTP proof slice proves Conary can satisfy the current MCP
draft stateless HTTP requirements without waiting for `rmcp` support. It reuses
`crates/conary-mcp::stateless`, adds a framework-neutral request/response
adapter proof, handles only `server/discover` successfully, and keeps Remi and
`conary-test` live routes unchanged.
```

- [ ] **Step 4: Run docs and stale-surface checks**

Run:

```bash
rg -n "stateless_http|raw HTTP proof|server/discover" docs/operations/agent-mcp-adapter-decision.md docs/superpowers/specs/2026-05-24-stateless-raw-http-adapter-proof-design.md
rg -n "server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/remi/src/server apps/conary-test/src/server
git diff --check
```

Expected:

- First `rg` shows the decision/spec language for the non-live proof.
- Second `rg` exits with no hits.
- `git diff --check` exits 0.

- [ ] **Step 5: Run all MCP tests**

Run:

```bash
cargo test -p conary-mcp
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/conary-mcp/tests/stateless_dependency_boundary.rs docs/operations/agent-mcp-adapter-decision.md docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md
git commit -m "docs(mcp): record raw stateless proof boundary"
git status --short
```

Expected: commit succeeds and status is clean.

## Task 6: Final Verification

**Files:**
- Inspect all changed files.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 2: Run focused tests**

Run:

```bash
cargo test -p conary-mcp
cargo test -p conary-agent-contract
```

Expected: PASS.

- [ ] **Step 3: Run workspace lint**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Verify no live MCP behavior was added**

Run:

```bash
BASE=$(git merge-base HEAD main)
git diff --name-only "$BASE"...HEAD
rg -n "server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/remi/src/server apps/conary-test/src/server
rg -n "pub mod stateless_http|RawStatelessHttpRequest|OriginPolicy|handle_stateless_http_request" crates/conary-mcp/src crates/conary-mcp/tests
```

Expected:

- `git diff --name-only "$BASE"...HEAD` lists `crates/conary-mcp/src/lib.rs`, `crates/conary-mcp/src/stateless.rs`, `crates/conary-mcp/src/stateless_http.rs`, `crates/conary-mcp/tests/stateless_dependency_boundary.rs`, this plan, and the decision record.
- The `apps/...` search exits with no hits.
- The `crates/conary-mcp` search shows only the non-live proof module, module export, and guard tests.

- [ ] **Step 5: Inspect final status**

Run:

```bash
git status --short --branch
git log --oneline -6
```

Expected: branch is ahead of `main` by the task commits. If Task 6 checkboxes have already been updated, `git status --short` may show only `docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md`; no code files should be dirty.

- [ ] **Step 6: Request review before merge**

Summarize:

- proof module added
- tests run
- confirmation that `server/discover` is non-live and capabilities are empty
- confirmation that Remi and `conary-test` route files were not changed
- next goal after merge

Do not merge until the reviewer approves the branch.

- [ ] **Step 7: Commit final verification checklist**

After Step 6 is complete and all Task 6 plus final acceptance checkboxes are checked, run:

```bash
git add docs/superpowers/plans/2026-05-24-stateless-raw-http-adapter-proof.md
git commit -m "docs(mcp): mark raw stateless proof verified"
git status --short
```

Expected: commit succeeds and `git status --short` is clean.

## Final Acceptance Checklist

- [ ] `crates/conary-mcp/src/stateless_http.rs` exists.
- [ ] `crates/conary-mcp/src/lib.rs` exports `pub mod stateless_http;`.
- [ ] `server/discover` returns HTTP `200` with JSON-RPC `result`.
- [ ] `server/discover` response capabilities are empty and omit `tools`, `resources`, and `prompts`.
- [ ] Invalid present `Origin` returns HTTP `403`.
- [ ] Missing `Origin` is accepted by the local/non-browser policy.
- [ ] Non-matching configured exact `Origin` returns HTTP `403`.
- [ ] Non-`POST` requests return HTTP `405`.
- [ ] Header extraction handles lowercase header names.
- [ ] Header extraction handles comma-separated and repeated `Accept` headers.
- [ ] Header extraction strips `Accept` media-type parameters and quality values.
- [ ] Malformed JSON-RPC envelopes return HTTP `400` with JSON-RPC `-32600`.
- [ ] Missing or mismatched MCP headers return HTTP `400` with JSON-RPC `-32001`.
- [ ] Missing `_meta` fields return HTTP `400` with JSON-RPC `-32602`.
- [ ] Conditional `Mcp-Name` validation runs before unsupported method mapping.
- [ ] Unsupported protocol versions return HTTP `400` with JSON-RPC `-32004` and structured data.
- [ ] Unsupported validated RPC methods return HTTP `404` with JSON-RPC `-32601`.
- [ ] Guard tests prove `stateless_http.rs` does not import `rmcp`, session-era types, or `axum`.
- [ ] Guard tests prove live Remi and `conary-test` route files do not contain draft stateless identifiers.
- [ ] `cargo fmt --check` passes.
- [ ] `cargo test -p conary-mcp` passes.
- [ ] `cargo test -p conary-agent-contract` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
