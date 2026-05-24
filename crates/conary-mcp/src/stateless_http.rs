// crates/conary-mcp/src/stateless_http.rs
//! Framework-neutral raw HTTP proof for the target stateless MCP adapter.

use crate::stateless::{
    DiscoverResult, HEADER_METHOD, HEADER_NAME, HEADER_PROTOCOL_VERSION, ImplementationInfo,
    MCP_DRAFT_PROTOCOL_VERSION, StatelessProtocolError, StatelessRequestHeaders,
    UnsupportedProtocolVersion, validate_stateless_request,
};
use serde_json::{Value, json};

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
            Some(origin) => self.allowed_origins.iter().any(|allowed| allowed == origin),
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::stateless::{
        JSON_RPC_HEADER_MISMATCH, JSON_RPC_INVALID_PARAMS, JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION,
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
        response
            .body
            .as_ref()
            .expect("response should include JSON body")
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

    #[test]
    fn malformed_json_rpc_envelopes_return_invalid_request() {
        let cases = [
            ("batch", json!([]), Value::Null),
            (
                "notification",
                json!({"jsonrpc": "2.0", "method": "server/discover"}),
                Value::Null,
            ),
            (
                "response",
                json!({"jsonrpc": "2.0", "id": "r1", "result": {}}),
                json!("r1"),
            ),
            ("non_object", json!("not an object"), Value::Null),
            (
                "wrong_jsonrpc",
                json!({"jsonrpc": "1.0", "id": "bad-1", "method": "server/discover"}),
                json!("bad-1"),
            ),
            (
                "missing_method",
                json!({"jsonrpc": "2.0", "id": "bad-2"}),
                json!("bad-2"),
            ),
            (
                "non_string_method",
                json!({"jsonrpc": "2.0", "id": "bad-3", "method": 7}),
                json!("bad-3"),
            ),
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
}
