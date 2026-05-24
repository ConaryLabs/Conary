// crates/conary-mcp/src/stateless_http.rs
//! Framework-neutral raw HTTP proof for the target stateless MCP adapter.

use crate::stateless::{
    CacheableResult, DiscoverResult, HEADER_METHOD, HEADER_NAME, HEADER_PROTOCOL_VERSION,
    ImplementationInfo, JSON_RPC_INVALID_PARAMS, MCP_DRAFT_PROTOCOL_VERSION, ResourceContent,
    ResourceDescriptor, ResourcesListPayload, ResourcesReadPayload, StatelessProtocolError,
    StatelessRequestHeaders, UnsupportedProtocolVersion, validate_stateless_request,
};
use conary_agent_contract::CachePolicy;
use serde::Serialize;
use serde_json::{Value, json};

pub const HTTP_OK: u16 = 200;
pub const HTTP_BAD_REQUEST: u16 = 400;
pub const HTTP_FORBIDDEN: u16 = 403;
pub const HTTP_METHOD_NOT_ALLOWED: u16 = 405;
pub const HTTP_NOT_FOUND: u16 = 404;

// Origin rejection and non-POST are HTTP-layer gates; HTTP status
// disambiguates these server-defined JSON-RPC errors.
pub const JSON_RPC_SERVER_ERROR: i32 = -32000;
pub const JSON_RPC_PARSE_ERROR: i32 = -32700;
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
                "Conary stateless MCP adapter proof exposes discovery. Resources are available when a provider is configured."
                    .to_string(),
            ),
        }
    }
}

pub trait StatelessResourceProvider {
    fn list_resources(&self) -> Vec<ResourceDescriptor>;

    fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, ResourceReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceReadError {
    NotFound { uri: String },
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

fn success_response<T: Serialize>(id: Value, result: T) -> RawStatelessHttpResponse {
    RawStatelessHttpResponse::json(
        HTTP_OK,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
}

fn method_not_found_response(id: Value, method: &str) -> RawStatelessHttpResponse {
    error_response(
        HTTP_NOT_FOUND,
        Some(id),
        JSON_RPC_METHOD_NOT_FOUND,
        format!("Method not found: {method}"),
        None,
    )
}

fn discover_response(
    id: Value,
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

fn resources_list_response(
    id: Value,
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
    id: Value,
    body: &Value,
    provider: &dyn StatelessResourceProvider,
) -> RawStatelessHttpResponse {
    let uri = body
        .get("params")
        .and_then(|params| params.get("uri"))
        .and_then(Value::as_str)
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

fn stateless_headers_from_request(
    request: &RawStatelessHttpRequest,
) -> Result<StatelessRequestHeaders, StatelessProtocolError> {
    Ok(StatelessRequestHeaders::from_optional_parts(
        standard_header_value(request, HEADER_PROTOCOL_VERSION)?,
        standard_header_value(request, HEADER_METHOD)?,
        standard_header_value(request, HEADER_NAME)?,
        accept_media_types(request),
    ))
}

fn origin_header(request: &RawStatelessHttpRequest) -> Option<String> {
    first_header_value(request, "Origin")
}

fn standard_header_value(
    request: &RawStatelessHttpRequest,
    name: &'static str,
) -> Result<Option<String>, StatelessProtocolError> {
    let Some(value) = raw_header_value(request, name) else {
        return Ok(None);
    };

    if !is_valid_http_field_value(value) {
        return Err(StatelessProtocolError::HeaderMismatch {
            header: name,
            expected: "visible ASCII header value".to_string(),
            actual: value.to_string(),
        });
    }

    let trimmed = value.trim_matches(|ch| ch == ' ' || ch == '\t').to_string();
    Ok((!trimmed.is_empty()).then_some(trimmed))
}

fn first_header_value(request: &RawStatelessHttpRequest, name: &str) -> Option<String> {
    raw_header_value(request, name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn raw_header_value<'a>(request: &'a RawStatelessHttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn is_valid_http_field_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| matches!(byte, b'\t' | b' '..=b'~'))
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
        HEADER_NAME, JSON_RPC_HEADER_MISMATCH, JSON_RPC_INVALID_PARAMS,
        JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION, ResourceContent,
        ResourceDescriptor,
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

    struct TestResourceProvider;

    impl StatelessResourceProvider for TestResourceProvider {
        fn list_resources(&self) -> Vec<ResourceDescriptor> {
            vec![ResourceDescriptor {
                uri: "conary-local://bootstrap/status".to_string(),
                name: "bootstrap_status".to_string(),
                title: Some("Local Bootstrap Status".to_string()),
                description:
                    "Read local developer bootstrap prerequisites and smoke-readiness state"
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

    fn valid_discover_headers() -> Vec<(String, String)> {
        vec![
            (
                "Accept".to_string(),
                "application/json, text/event-stream".to_string(),
            ),
            (
                "MCP-Protocol-Version".to_string(),
                MCP_DRAFT_PROTOCOL_VERSION.to_string(),
            ),
            ("Mcp-Method".to_string(), "server/discover".to_string()),
        ]
    }

    fn response_body(response: &RawStatelessHttpResponse) -> &Value {
        response
            .body
            .as_ref()
            .expect("response should include JSON body")
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
        assert_eq!(
            body["result"]["resources"][0]["mimeType"],
            "application/json"
        );
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
        assert_eq!(
            body["result"]["contents"][0]["mimeType"],
            "application/json"
        );
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
        assert_eq!(body["error"]["code"], JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION);
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
    fn malformed_standard_mcp_header_values_return_header_mismatch_code() {
        let mut body = discover_body("malformed-header-1");
        body["method"] = json!("server/discover\nbad");

        let response = handle_stateless_http_request(
            RawStatelessHttpRequest::post(body)
                .with_header("Accept", "application/json, text/event-stream")
                .with_header("MCP-Protocol-Version", MCP_DRAFT_PROTOCOL_VERSION)
                .with_header("Mcp-Method", "server/discover\nbad"),
            &RawStatelessHttpConfig::default(),
        );

        assert_eq!(response.status, HTTP_BAD_REQUEST);
        let body = response_body(&response);
        assert_eq!(body["id"], "malformed-header-1");
        assert_eq!(body["error"]["code"], JSON_RPC_HEADER_MISMATCH);
        assert_eq!(body["error"]["data"]["kind"], "header_mismatch");
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
        assert_eq!(missing_name_body["error"]["code"], JSON_RPC_HEADER_MISMATCH);
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
}
