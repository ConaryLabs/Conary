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
    if let Some(response) = http_gate_response(&request, config) {
        return response;
    }

    dispatch_stateless_request(request, config, resource_provider)
}

fn dispatch_stateless_request(
    request: RawStatelessHttpRequest,
    config: &RawStatelessHttpConfig,
    resource_provider: Option<&dyn StatelessResourceProvider>,
) -> RawStatelessHttpResponse {
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

    if let Some(response) = http_gate_response(&preflight_request, config) {
        return response;
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

    dispatch_stateless_request(
        RawStatelessHttpRequest {
            method: preflight_request.method,
            headers: preflight_request.headers,
            body: parsed_body,
        },
        config,
        resource_provider,
    )
}

fn http_gate_response(
    request: &RawStatelessHttpRequest,
    config: &RawStatelessHttpConfig,
) -> Option<RawStatelessHttpResponse> {
    if !request.method.eq_ignore_ascii_case("POST") {
        return Some(error_response(
            HTTP_METHOD_NOT_ALLOWED,
            extract_scalar_id(&request.body),
            JSON_RPC_SERVER_ERROR,
            "Only POST is supported for stateless MCP requests",
            None,
        ));
    }

    if !config
        .origin_policy
        .allows(origin_header(request).as_deref())
    {
        return Some(error_response(
            HTTP_FORBIDDEN,
            None,
            JSON_RPC_SERVER_ERROR,
            "Origin is not allowed",
            None,
        ));
    }

    None
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
