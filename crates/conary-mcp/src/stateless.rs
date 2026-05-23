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

    let header_version =
        headers
            .protocol_version
            .as_deref()
            .ok_or(StatelessProtocolError::MissingHeader(
                HEADER_PROTOCOL_VERSION,
            ))?;
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
            supported: supported_versions
                .iter()
                .map(|version| version.to_string())
                .collect(),
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

fn meta_string<'a>(
    request: &'a Value,
    field: &'static str,
) -> Result<&'a str, StatelessProtocolError> {
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
    let header_name =
        headers
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

        assert!(
            validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION]).is_ok()
        );
    }

    #[test]
    fn resources_read_requires_matching_name_header() {
        let headers = valid_headers("resources/read").with_name("conary://remi/health");
        let mut request = valid_request("resources/read");
        request["params"]["uri"] = json!("conary://remi/health");

        assert!(
            validate_stateless_request(&headers, &request, &[MCP_DRAFT_PROTOCOL_VERSION]).is_ok()
        );
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
        assert_eq!(
            err.json_rpc_error_code(),
            JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION
        );
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
