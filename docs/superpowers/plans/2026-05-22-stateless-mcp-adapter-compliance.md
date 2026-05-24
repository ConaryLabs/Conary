# Stateless MCP Adapter Compliance Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Conary to the latest compatible `rmcp` crate and add a tested, non-live stateless MCP compliance harness that future MCP adapters must satisfy.

**Architecture:** Keep `crates/conary-agent-contract` as the transport-neutral product contract. Add draft-shaped protocol validation, discovery, and cache helpers in `crates/conary-mcp::stateless` without registering live MCP resources, tools, prompts, routes, or discovery behavior. The current Remi and `conary-test` session-based MCP servers remain unchanged until a later adapter slice.

**Tech Stack:** Rust 1.94, Cargo workspace dependencies, `rmcp`, serde, serde_json, schemars, `conary-agent-contract`, Codex `/goal`.

---

## Codex `/goal` Operating Model

Use this plan with Codex Goal mode. Recommended goal text:

```text
Implement docs/superpowers/plans/2026-05-22-stateless-mcp-adapter-compliance.md task-by-task. Source spec: docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md. First move rmcp to the latest compatible crate version, expected rmcp 1.7.0 as of 2026-05-22. Then add only non-live conary-mcp stateless compliance helpers and tests. Do not add live MCP resources, tools, prompts, routes, or discovery behavior. Make one focused commit per task, run verification before each commit, and update checkboxes as tasks complete.
```

Goal-mode checkpoint rules:

- Keep each task as one commit unless the task says otherwise.
- After every commit, run `git status --short`.
- If the latest `rmcp` version is newer than `1.7.0` when implementation starts, use the newer compatible version and update this plan's completed-task notes with the evidence.
- Use `/goal pause` before leaving long-running verification unattended.
- Do not start a live MCP adapter implementation from this plan.

## Source Specs And Constraints

- Primary spec: `docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md`
- Parent spec: `docs/superpowers/specs/2026-05-22-llm-native-operations-surface-design.md`
- Adapter decision: `docs/operations/agent-mcp-adapter-decision.md`
- Existing contract crate: `crates/conary-agent-contract`
- Existing adapter crate: `crates/conary-mcp`

Hard constraints:

- Use latest compatible Rust crate versions during active development. For this plan, update `rmcp` from the stale local lock state to the latest compatible published version, expected `1.7.0` as of 2026-05-22.
- Do not add new live MCP resources, tools, prompts, routes, `server/discover`, or app registration behavior.
- Do not deepen the existing `RoleServer` / `ServerHandler` / `LocalSessionManager` path.
- Keep the stateless compliance module independent of `rmcp` types.
- Treat `DRAFT-2026-v1` as the current MCP draft token, not a durable Conary contract.
- Keep all protocol-draft assumptions isolated to `crates/conary-mcp::stateless`.

## File Structure

Modify:

- `Cargo.toml`: update workspace `rmcp` dependency to the latest compatible version.
- `Cargo.lock`: update resolved `rmcp` and `rmcp-macros` versions.
- `crates/conary-mcp/src/lib.rs`: export the new `stateless` module.
- `docs/operations/agent-mcp-adapter-decision.md`: record the implemented dependency update and harness boundary if needed.

Create:

- `crates/conary-mcp/src/stateless.rs`: draft-shaped request validation, discovery result, unsupported-version payload, cacheable result wrapper, and unit tests.
- `crates/conary-mcp/tests/stateless_dependency_boundary.rs`: source guard test proving the stateless module stays independent from session-era `rmcp` types.

Do not modify:

- `apps/remi/src/server/mcp.rs`
- `apps/remi/src/server/routes/mcp.rs`
- `apps/conary-test/src/server/mcp.rs`
- `apps/conary-test/src/server/routes.rs`

Those files are allowed to remain legacy session-based until a later live adapter slice.

## Task 1: Move `rmcp` To Latest Compatible Version

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

- [x] **Step 1: Confirm current stale local state**

Run:

```bash
cargo tree -p conary-mcp -i rmcp
```

Expected before this task:

```text
rmcp v1.6.0
└── conary-mcp v0.8.0 (.../crates/conary-mcp)
```

- [x] **Step 2: Check the latest published crate version**

Run:

```bash
cargo search rmcp --limit 1
```

Expected on 2026-05-22:

```text
rmcp = "1.7.0"    # Rust SDK for Model Context Protocol
```

If the command reports a newer compatible `1.x` version, use that newer version in the rest of this task and record the observed version in the completed-task notes.

- [x] **Step 3: Update the workspace dependency**

In `Cargo.toml`, change the MCP dependency line to the latest compatible version. Expected edit as of 2026-05-22:

```toml
# MCP
rmcp = "1.7.0"
schemars = "1.0"
```

- [x] **Step 4: Update the lockfile**

Run:

```bash
cargo update -p rmcp --precise 1.7.0
```

Expected: Cargo updates `rmcp` and any matching `rmcp-macros` dependency to `1.7.0` or reports that the lockfile is already current after the workspace dependency edit.

- [x] **Step 5: Verify the dependency graph**

Run:

```bash
cargo tree -p conary-mcp -i rmcp
rg -n 'name = "rmcp"|name = "rmcp-macros"|version = "1\.7\.0"' Cargo.lock
```

Expected:

```text
rmcp v1.7.0
└── conary-mcp v0.8.0 (.../crates/conary-mcp)
```

The `rg` output should show `rmcp` and `rmcp-macros` lock entries at `1.7.0`.

- [x] **Step 6: Run focused tests**

Run:

```bash
cargo test -p conary-mcp
cargo check -p remi -p conary-test
```

Expected: PASS. If this fails due to `rmcp` API changes, update the existing
adapter helper code to match the new crate API without changing live MCP
registration behavior.

Highest-risk symbol: `crates/conary-mcp/src/lib.rs` currently builds
`server_info()` with `InitializeResult`. If `rmcp 1.7.0` removed or renamed
`InitializeResult`, replace the helper with the equivalent new API or remove
the helper only if the live Remi and `conary-test` servers no longer need it.
The `cargo check -p remi -p conary-test` command is required because those live
servers consume this helper.

- [x] **Step 7: Commit**

Run:

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps(mcp): update rmcp"
git status --short
```

Expected: commit succeeds and `git status --short` is clean.

## Task 2: Add Stateless Request Validation

**Files:**
- Modify: `crates/conary-mcp/src/lib.rs`
- Create: `crates/conary-mcp/src/stateless.rs`

- [x] **Step 1: Write failing validation tests**

Add this export to `crates/conary-mcp/src/lib.rs`:

```rust
pub mod stateless;
```

Create `crates/conary-mcp/src/stateless.rs` with tests first:

```rust
// crates/conary-mcp/src/stateless.rs
//! Stateless MCP draft compliance helpers.

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn valid_headers(method: &str) -> StatelessRequestHeaders {
        StatelessRequestHeaders::new(MCP_DRAFT_PROTOCOL_VERSION, method)
            .with_accepts(["application/json", "text/event-stream"])
    }

    fn valid_request(method: &str) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": "test-1",
            "method": method,
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": MCP_DRAFT_PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "ConaryTestClient",
                        "version": "0.1.0"
                    },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        })
    }

    #[test]
    fn tools_list_request_validates_without_name() {
        let headers = valid_headers("tools/list");
        let request = valid_request("tools/list");

        assert!(validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION]).is_ok());
    }

    #[test]
    fn resources_read_requires_matching_name_header() {
        let headers = valid_headers("resources/read").with_name("conary://remi/health");
        let mut request = valid_request("resources/read");
        request["params"]["uri"] = json!("conary://remi/health");

        assert!(validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION]).is_ok());
    }

    #[test]
    fn missing_protocol_header_fails() {
        let headers = StatelessRequestHeaders::missing_protocol("tools/list")
            .with_accepts(["application/json", "text/event-stream"]);
        let request = valid_request("tools/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("missing protocol header should fail");
        assert_eq!(err.code(), "missing_header");
        assert!(err.to_string().contains("MCP-Protocol-Version"));
    }

    #[test]
    fn missing_meta_fails() {
        let headers = valid_headers("tools/list");
        let request = json!({
            "jsonrpc": "2.0",
            "id": "test-1",
            "method": "tools/list",
            "params": {}
        });

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("missing _meta should fail");
        assert_eq!(err.code(), "missing_meta_field");
    }

    #[test]
    fn protocol_header_must_match_meta() {
        let headers = StatelessRequestHeaders::new("DRAFT-OTHER", "tools/list")
            .with_accepts(["application/json", "text/event-stream"]);
        let request = valid_request("tools/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("mismatched protocol should fail");
        assert_eq!(err.code(), "unsupported_protocol_version");
        assert_eq!(err.json_rpc_error_code(), JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION);
    }

    #[test]
    fn method_header_must_match_body() {
        let headers = valid_headers("tools/list");
        let request = valid_request("resources/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("mismatched method should fail");
        assert_eq!(err.code(), "header_mismatch");
        assert_eq!(err.json_rpc_error_code(), JSON_RPC_HEADER_MISMATCH);
        assert!(err.to_string().contains("Mcp-Method"));
    }

    #[test]
    fn required_name_header_must_match_body() {
        let headers = valid_headers("tools/call").with_name("wrong_tool");
        let mut request = valid_request("tools/call");
        request["params"]["name"] = json!("right_tool");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("mismatched name should fail");
        assert_eq!(err.code(), "header_mismatch");
        assert!(err.to_string().contains("Mcp-Name"));
    }

    #[test]
    fn accept_must_include_json_and_event_stream() {
        let headers = StatelessRequestHeaders::new(MCP_DRAFT_PROTOCOL_VERSION, "tools/list")
            .with_accepts(["application/json"]);
        let request = valid_request("tools/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("missing event-stream accept should fail");
        assert_eq!(err.code(), "missing_accept");
    }
}
```

- [x] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p conary-mcp stateless
```

Expected: FAIL with unresolved names such as `StatelessRequestHeaders`, `MCP_DRAFT_PROTOCOL_VERSION`, and `validate_stateless_request`.

- [x] **Step 3: Implement validation helpers**

Replace `crates/conary-mcp/src/stateless.rs` with:

```rust
// crates/conary-mcp/src/stateless.rs
//! Stateless MCP draft compliance helpers.
//!
//! This module models the target draft boundary for future Conary MCP adapters.
//! It is intentionally independent from `rmcp` so it can validate either a
//! future SDK adapter or a thin raw HTTP adapter.

use std::{error::Error, fmt};

use serde_json::Value;

pub const MCP_DRAFT_PROTOCOL_VERSION: &str = "DRAFT-2026-v1";
pub const HEADER_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
pub const HEADER_METHOD: &str = "Mcp-Method";
pub const HEADER_NAME: &str = "Mcp-Name";
pub const JSON_RPC_HEADER_MISMATCH: i32 = -32001;
pub const JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION: i32 = -32004;
pub const JSON_RPC_INVALID_PARAMS: i32 = -32602;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatelessRequestHeaders {
    protocol_version: Option<String>,
    method: Option<String>,
    name: Option<String>,
    accepts: Vec<String>,
}

impl StatelessRequestHeaders {
    pub fn new(protocol_version: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            protocol_version: Some(protocol_version.into()),
            method: Some(method.into()),
            name: None,
            accepts: Vec::new(),
        }
    }

    pub fn missing_protocol(method: impl Into<String>) -> Self {
        Self {
            protocol_version: None,
            method: Some(method.into()),
            name: None,
            accepts: Vec::new(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_accepts<I, S>(mut self, accepts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.accepts = accepts.into_iter().map(Into::into).collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatelessProtocolError {
    MissingHeader(&'static str),
    MissingAccept(&'static str),
    MissingMetaField(&'static str),
    HeaderMismatch {
        header: &'static str,
        expected: String,
        actual: String,
    },
    MissingName {
        method: String,
    },
    UnsupportedProtocolVersion {
        requested: String,
        supported: Vec<String>,
    },
}

impl StatelessProtocolError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingHeader(_) => "missing_header",
            Self::MissingAccept(_) => "missing_accept",
            Self::MissingMetaField(_) => "missing_meta_field",
            Self::HeaderMismatch { .. } => "header_mismatch",
            Self::MissingName { .. } => "missing_name",
            Self::UnsupportedProtocolVersion { .. } => "unsupported_protocol_version",
        }
    }

    pub fn json_rpc_error_code(&self) -> i32 {
        match self {
            Self::UnsupportedProtocolVersion { .. } => JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION,
            Self::MissingMetaField(_) => JSON_RPC_INVALID_PARAMS,
            Self::MissingHeader(_)
            | Self::MissingAccept(_)
            | Self::HeaderMismatch { .. }
            | Self::MissingName { .. } => JSON_RPC_HEADER_MISMATCH,
        }
    }
}

impl fmt::Display for StatelessProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader(header) => write!(f, "missing required MCP header {header}"),
            Self::MissingAccept(media_type) => {
                write!(f, "Accept must include {media_type}")
            }
            Self::MissingMetaField(field) => {
                write!(f, "missing required MCP request _meta field {field}")
            }
            Self::HeaderMismatch {
                header,
                expected,
                actual,
            } => write!(
                f,
                "{header} header value {actual:?} does not match body value {expected:?}"
            ),
            Self::MissingName { method } => {
                write!(f, "{HEADER_NAME} is required for {method}")
            }
            Self::UnsupportedProtocolVersion {
                requested,
                supported,
            } => write!(
                f,
                "unsupported MCP protocol version {requested:?}; supported versions: {}",
                supported.join(", ")
            ),
        }
    }
}

impl Error for StatelessProtocolError {}

pub fn validate_stateless_request(
    headers: &StatelessRequestHeaders,
    request: &Value,
    supported_versions: &[&str],
) -> Result<(), StatelessProtocolError> {
    require_accept(headers, "application/json")?;
    require_accept(headers, "text/event-stream")?;

    let header_version = headers
        .protocol_version
        .as_deref()
        .ok_or(StatelessProtocolError::MissingHeader(HEADER_PROTOCOL_VERSION))?;
    let method_header = headers
        .method
        .as_deref()
        .ok_or(StatelessProtocolError::MissingHeader(HEADER_METHOD))?;
    let body_method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or(StatelessProtocolError::MissingMetaField("method"))?;

    if !supported_versions.contains(&header_version) {
        return Err(StatelessProtocolError::UnsupportedProtocolVersion {
            requested: header_version.to_string(),
            supported: supported_versions.iter().map(|version| version.to_string()).collect(),
        });
    }

    if method_header != body_method {
        return Err(StatelessProtocolError::HeaderMismatch {
            header: HEADER_METHOD,
            expected: body_method.to_string(),
            actual: method_header.to_string(),
        });
    }

    let meta_version = meta_string(request, "io.modelcontextprotocol/protocolVersion")?;
    if header_version != meta_version {
        return Err(StatelessProtocolError::HeaderMismatch {
            header: HEADER_PROTOCOL_VERSION,
            expected: meta_version.to_string(),
            actual: header_version.to_string(),
        });
    }

    require_meta_object(request, "io.modelcontextprotocol/clientInfo")?;
    require_meta_object(request, "io.modelcontextprotocol/clientCapabilities")?;
    validate_name_header(headers, request, body_method)?;

    Ok(())
}

fn require_accept(
    headers: &StatelessRequestHeaders,
    media_type: &'static str,
) -> Result<(), StatelessProtocolError> {
    if headers.accepts.iter().any(|value| value == media_type) {
        Ok(())
    } else {
        Err(StatelessProtocolError::MissingAccept(media_type))
    }
}

fn request_meta(request: &Value) -> Result<&Value, StatelessProtocolError> {
    request
        .get("params")
        .and_then(|params| params.get("_meta"))
        .ok_or(StatelessProtocolError::MissingMetaField("_meta"))
}

fn meta_string<'a>(request: &'a Value, field: &'static str) -> Result<&'a str, StatelessProtocolError> {
    request_meta(request)?
        .get(field)
        .and_then(Value::as_str)
        .ok_or(StatelessProtocolError::MissingMetaField(field))
}

fn require_meta_object(request: &Value, field: &'static str) -> Result<(), StatelessProtocolError> {
    request_meta(request)?
        .get(field)
        .and_then(Value::as_object)
        .map(|_| ())
        .ok_or(StatelessProtocolError::MissingMetaField(field))
}

fn validate_name_header(
    headers: &StatelessRequestHeaders,
    request: &Value,
    method: &str,
) -> Result<(), StatelessProtocolError> {
    let Some(field) = required_name_field(method) else {
        return Ok(());
    };

    let body_name = request
        .get("params")
        .and_then(|params| params.get(field))
        .and_then(Value::as_str)
        .ok_or_else(|| StatelessProtocolError::MissingName {
            method: method.to_string(),
        })?;
    let header_name = headers
        .name
        .as_deref()
        .ok_or_else(|| StatelessProtocolError::MissingName {
            method: method.to_string(),
        })?;

    if header_name == body_name {
        Ok(())
    } else {
        Err(StatelessProtocolError::HeaderMismatch {
            header: HEADER_NAME,
            expected: body_name.to_string(),
            actual: header_name.to_string(),
        })
    }
}

fn required_name_field(method: &str) -> Option<&'static str> {
    match method {
        "tools/call" | "prompts/get" => Some("name"),
        "resources/read" => Some("uri"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn valid_headers(method: &str) -> StatelessRequestHeaders {
        StatelessRequestHeaders::new(MCP_DRAFT_PROTOCOL_VERSION, method)
            .with_accepts(["application/json", "text/event-stream"])
    }

    fn valid_request(method: &str) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": "test-1",
            "method": method,
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": MCP_DRAFT_PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "ConaryTestClient",
                        "version": "0.1.0"
                    },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        })
    }

    #[test]
    fn tools_list_request_validates_without_name() {
        let headers = valid_headers("tools/list");
        let request = valid_request("tools/list");

        assert!(validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION]).is_ok());
    }

    #[test]
    fn resources_read_requires_matching_name_header() {
        let headers = valid_headers("resources/read").with_name("conary://remi/health");
        let mut request = valid_request("resources/read");
        request["params"]["uri"] = json!("conary://remi/health");

        assert!(validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION]).is_ok());
    }

    #[test]
    fn missing_protocol_header_fails() {
        let headers = StatelessRequestHeaders::missing_protocol("tools/list")
            .with_accepts(["application/json", "text/event-stream"]);
        let request = valid_request("tools/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("missing protocol header should fail");
        assert_eq!(err.code(), "missing_header");
        assert!(err.to_string().contains("MCP-Protocol-Version"));
    }

    #[test]
    fn missing_meta_fails() {
        let headers = valid_headers("tools/list");
        let request = json!({
            "jsonrpc": "2.0",
            "id": "test-1",
            "method": "tools/list",
            "params": {}
        });

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("missing _meta should fail");
        assert_eq!(err.code(), "missing_meta_field");
    }

    #[test]
    fn protocol_header_must_match_meta() {
        let headers = StatelessRequestHeaders::new("DRAFT-OTHER", "tools/list")
            .with_accepts(["application/json", "text/event-stream"]);
        let request = valid_request("tools/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("mismatched protocol should fail");
        assert_eq!(err.code(), "unsupported_protocol_version");
        assert_eq!(err.json_rpc_error_code(), JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION);
    }

    #[test]
    fn method_header_must_match_body() {
        let headers = valid_headers("tools/list");
        let request = valid_request("resources/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("mismatched method should fail");
        assert_eq!(err.code(), "header_mismatch");
        assert_eq!(err.json_rpc_error_code(), JSON_RPC_HEADER_MISMATCH);
        assert!(err.to_string().contains("Mcp-Method"));
    }

    #[test]
    fn required_name_header_must_match_body() {
        let headers = valid_headers("tools/call").with_name("wrong_tool");
        let mut request = valid_request("tools/call");
        request["params"]["name"] = json!("right_tool");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("mismatched name should fail");
        assert_eq!(err.code(), "header_mismatch");
        assert!(err.to_string().contains("Mcp-Name"));
    }

    #[test]
    fn accept_must_include_json_and_event_stream() {
        let headers = StatelessRequestHeaders::new(MCP_DRAFT_PROTOCOL_VERSION, "tools/list")
            .with_accepts(["application/json"]);
        let request = valid_request("tools/list");

        let err = validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION])
            .expect_err("missing event-stream accept should fail");
        assert_eq!(err.code(), "missing_accept");
    }
}
```

- [x] **Step 4: Run validation tests**

Run:

```bash
cargo test -p conary-mcp stateless
```

Expected: PASS.

- [x] **Step 5: Commit**

Run:

```bash
git add crates/conary-mcp/src/lib.rs crates/conary-mcp/src/stateless.rs
git commit -m "feat(mcp): add stateless request validation"
git status --short
```

Expected: commit succeeds and `git status --short` is clean.

## Task 3: Add Discovery And Cacheable Result Models

**Files:**
- Modify: `crates/conary-mcp/src/stateless.rs`

- [x] **Step 1: Add failing serialization tests**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/conary-mcp/src/stateless.rs`:

```rust
    #[test]
    fn discover_result_serializes_target_shape() {
        let result = DiscoverResult::new(
            [MCP_DRAFT_PROTOCOL_VERSION],
            serde_json::json!({"tools": {}, "resources": {}}),
            ImplementationInfo::new("conary-mcp", "0.8.0"),
        )
        .with_instructions("Conary exposes package, repository, and test operations.");

        let value = serde_json::to_value(result).expect("discover result serializes");
        assert_eq!(value["resultType"], "complete");
        assert_eq!(value["supportedVersions"][0], MCP_DRAFT_PROTOCOL_VERSION);
        assert_eq!(value["serverInfo"]["name"], "conary-mcp");
        assert_eq!(value["capabilities"]["resources"], serde_json::json!({}));
    }

    #[test]
    fn discover_result_type_is_complete() {
        let result = DiscoverResult::new(
            [MCP_DRAFT_PROTOCOL_VERSION],
            serde_json::json!({}),
            ImplementationInfo::new("test", "0.1.0"),
        );

        assert_eq!(result.result_type, "complete");
    }

    #[test]
    fn unsupported_version_payload_lists_supported_and_requested() {
        let payload = UnsupportedProtocolVersion::new("OLD", [MCP_DRAFT_PROTOCOL_VERSION]);

        let value = serde_json::to_value(payload).expect("payload serializes");
        assert_eq!(value["requested"], "OLD");
        assert_eq!(value["supported"][0], MCP_DRAFT_PROTOCOL_VERSION);
    }

    #[test]
    fn cacheable_result_serializes_cache_metadata() {
        let result = CacheableResult::new(
            conary_agent_contract::CachePolicy::private_short(),
            serde_json::json!({"resources": [{"uri": "conary://remi/health"}]}),
        );

        let value = serde_json::to_value(result).expect("cacheable result serializes");
        assert_eq!(value["resultType"], "complete");
        assert_eq!(value["ttlMs"], 30_000);
        assert_eq!(value["cacheScope"], "private");
        assert_eq!(value["resources"][0]["uri"], "conary://remi/health");
    }
```

- [x] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p conary-mcp stateless
```

Expected: FAIL with unresolved names such as `DiscoverResult`, `ImplementationInfo`, `UnsupportedProtocolVersion`, and `CacheableResult`.

- [x] **Step 3: Add discovery and cache models**

Add these imports near the top of `crates/conary-mcp/src/stateless.rs`:

```rust
use conary_agent_contract::CachePolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
```

Add these types before the test module:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImplementationInfo {
    pub name: String,
    pub version: String,
}

impl ImplementationInfo {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResult {
    #[serde(rename = "resultType")]
    pub result_type: String,
    pub supported_versions: Vec<String>,
    pub capabilities: Value,
    pub server_info: ImplementationInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl DiscoverResult {
    pub fn new<I, S>(
        supported_versions: I,
        capabilities: Value,
        server_info: ImplementationInfo,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            result_type: "complete".to_string(),
            supported_versions: supported_versions.into_iter().map(Into::into).collect(),
            capabilities,
            server_info,
            instructions: None,
        }
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnsupportedProtocolVersion {
    pub requested: String,
    pub supported: Vec<String>,
}

impl UnsupportedProtocolVersion {
    pub fn new<I, S>(requested: impl Into<String>, supported: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            requested: requested.into(),
            supported: supported.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CacheableResult<T> {
    #[serde(rename = "resultType")]
    pub result_type: String,
    #[serde(flatten)]
    pub cache: CachePolicy,
    #[serde(flatten)]
    pub payload: T,
}

impl<T> CacheableResult<T> {
    pub fn new(cache: CachePolicy, payload: T) -> Self {
        Self {
            result_type: "complete".to_string(),
            cache,
            payload,
        }
    }
}
```

`CacheableResult` deliberately flattens `CachePolicy` into the MCP result
object. The `ttlMs` and `cacheScope` serde field names are owned by
`conary-agent-contract::CachePolicy` and are intentionally the current MCP
draft field names. If implementation discovers payload collisions with
`resultType`, `ttlMs`, or `cacheScope`, reserve those names for adapter fields
and add a regression test for the observed serde behavior before committing.

- [x] **Step 4: Run tests**

Run:

```bash
cargo test -p conary-mcp stateless
```

Expected: PASS.

- [x] **Step 5: Commit**

Run:

```bash
git add crates/conary-mcp/src/stateless.rs
git commit -m "feat(mcp): model stateless discovery results"
git status --short
```

Expected: commit succeeds and `git status --short` is clean.

## Task 4: Add Dependency-Boundary Guard Tests

**Files:**
- Create: `crates/conary-mcp/tests/stateless_dependency_boundary.rs`

- [x] **Step 1: Write guard tests**

Create `crates/conary-mcp/tests/stateless_dependency_boundary.rs`:

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
fn stateless_module_does_not_use_rmcp_or_session_types() {
    let source = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/stateless.rs"))
        .expect("stateless module should be readable");

    for forbidden in [
        "use rmcp",
        "rmcp::",
        "RoleServer",
        "ServerHandler",
        "LocalSessionManager",
        "Mcp-Session-Id",
        "InitializeResult",
    ] {
        assert!(
            !source.contains(forbidden),
            "stateless module must not depend on legacy/session MCP type {forbidden}"
        );
    }
}

#[test]
fn live_mcp_route_files_do_not_contain_draft_stateless_identifiers() {
    let root = repo_root();
    for path in [
        "apps/remi/src/server/routes/mcp.rs",
        "apps/conary-test/src/server/routes.rs",
    ] {
        let source = fs::read_to_string(root.join(path))
            .expect("live MCP route file should be readable");
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

- [x] **Step 2: Run the guard tests**

Run:

```bash
cargo test -p conary-mcp --test stateless_dependency_boundary
```

Expected: PASS.

- [x] **Step 3: Run all MCP tests**

Run:

```bash
cargo test -p conary-mcp
```

Expected: PASS.

- [x] **Step 4: Commit**

Run:

```bash
git add crates/conary-mcp/tests/stateless_dependency_boundary.rs
git commit -m "test(mcp): guard stateless adapter boundary"
git status --short
```

Expected: commit succeeds and `git status --short` is clean.

## Task 5: Update Adapter Decision Docs

**Files:**
- Modify: `docs/operations/agent-mcp-adapter-decision.md`
- Modify: `docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md`

- [x] **Step 1: Update the decision record after implementation facts are known**

In `docs/operations/agent-mcp-adapter-decision.md`, keep revision `2` or increment it if the file has changed since this plan was written. Ensure the `Current State` section says:

```markdown
- Workspace requirement: `rmcp = "1.7.0"` in `Cargo.toml`
- Resolved dependency: `rmcp 1.7.0` in `Cargo.lock`
- Latest public `rmcp` docs checked on 2026-05-22 list `rmcp 1.7.0`, but still
  document session/initialize-era types such as `LocalSessionManager` and
  `InitializeResult`
```

Ensure the `Next Slice` section remains clear that the harness added no live MCP resources, tools, prompts, routes, or discovery behavior.

- [x] **Step 2: Update the spec with completed dependency facts**

If Task 1 used a newer compatible `rmcp` version than `1.7.0`, update `docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md` so its current facts and acceptance criteria name the actual version used.

If Task 1 used `1.7.0`, update the status line:

```markdown
**Status:** Implemented plan source; live adapter not started
```

Also update the local repo facts in that spec from the pre-plan state to the completed dependency state:

```markdown
- `Cargo.toml` has the workspace requirement `rmcp = "1.7.0"`.
- `Cargo.lock` resolves the local workspace to `rmcp 1.7.0`.
```

- [x] **Step 3: Run docs consistency checks**

Run:

```bash
rg -n "rmcp 1\\.6\\.0|rmcp = \"1\\.1\"" docs/operations/agent-mcp-adapter-decision.md docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md Cargo.toml Cargo.lock
rg -n "live MCP resources|live MCP tools|server/discover" docs/operations/agent-mcp-adapter-decision.md docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md
```

Expected:

- The first command should show no stale `rmcp 1.6.0` or `rmcp = "1.1"` hits in the updated decision/spec and dependency files.
- The second command should show only language that this slice did not add live MCP behavior and that `server/discover` remains target/harness behavior.

- [x] **Step 4: Commit**

Run:

```bash
git add docs/operations/agent-mcp-adapter-decision.md docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md
git commit -m "docs(mcp): record stateless harness boundary"
git status --short
```

Expected: commit succeeds and `git status --short` is clean.

## Task 6: Final Verification

**Files:**
- Inspect all changed files.

- [x] **Step 1: Run formatting**

Run:

```bash
cargo fmt --check
```

Expected: PASS.

- [x] **Step 2: Run focused tests**

Run:

```bash
cargo test -p conary-mcp
cargo test -p conary-agent-contract
```

Expected: PASS.

- [x] **Step 3: Run workspace lint**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [x] **Step 4: Verify no live MCP behavior was added**

Run:

```bash
git diff --name-only main...HEAD
rg -n "server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/remi/src/server apps/conary-test/src/server
rg -n "pub mod stateless|DRAFT-2026-v1|CacheableResult|DiscoverResult" crates/conary-mcp/src crates/conary-mcp/tests
```

Expected:

- `git diff --name-only main...HEAD` should list dependency files, `crates/conary-mcp/src/lib.rs`, `crates/conary-mcp/src/stateless.rs`, `crates/conary-mcp/tests/stateless_dependency_boundary.rs`, and the docs updated by this plan.
- The `apps/...` search should return no hits for draft-stateless headers or live `server/discover`.
- The `crates/conary-mcp` search should show the harness code and tests only.

- [x] **Step 5: Inspect final status**

Run:

```bash
git status --short --branch
git log --oneline -6
```

Expected: branch is ahead of `main` by the task commits and `git status --short` shows no uncommitted changes.

- [x] **Step 6: Request review before merge**

Summarize:

- `rmcp` version chosen and evidence
- tests run
- confirmation that no live MCP behavior was added
- the next likely goal after merge

Do not merge until the reviewer approves the branch.
