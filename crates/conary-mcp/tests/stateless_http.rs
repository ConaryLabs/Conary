// crates/conary-mcp/tests/stateless_http.rs
#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use conary_mcp::stateless::{
        HEADER_METHOD, HEADER_NAME, HEADER_PROTOCOL_VERSION, JSON_RPC_HEADER_MISMATCH,
        JSON_RPC_INVALID_PARAMS, JSON_RPC_UNSUPPORTED_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION,
        ResourceContent, ResourceDescriptor,
    };
    use conary_mcp::stateless_http::*;

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
