// conary-server/src/server/handlers/openapi.rs
//! OpenAPI 3.1 specification for the admin API

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn openapi_spec() -> Response {
    let spec = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Remi Admin API",
            "description": "Administration API for the Remi package server. Manage CI/CD pipelines, admin tokens, and monitor server operations. Designed for human and LLM agent consumption.",
            "version": env!("CARGO_PKG_VERSION"),
            "contact": { "name": "Conary Labs" }
        },
        "servers": [
            { "url": "https://packages.conary.io:8082", "description": "Production" }
        ],
        "security": [{ "bearerAuth": [] }],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Admin API token. Create via POST /v1/admin/tokens or set REMI_ADMIN_TOKEN env var at server startup."
                }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string", "description": "Human-readable error message" },
                        "code": { "type": "string", "description": "Machine-readable error code: UNAUTHORIZED, INSUFFICIENT_SCOPE, NOT_FOUND, UPSTREAM_ERROR, INTERNAL_ERROR" }
                    },
                    "required": ["error", "code"]
                }
            }
        },
        "paths": {
            "/v1/admin/tokens": {
                "get": {
                    "operationId": "listTokens",
                    "summary": "List all admin API tokens",
                    "description": "Returns all tokens with names, scopes, and last-used timestamps. Token hashes are never returned. Use to audit existing tokens.",
                    "tags": ["tokens"],
                    "responses": { "200": { "description": "Array of tokens" }, "401": { "description": "Invalid or missing token" } }
                },
                "post": {
                    "operationId": "createToken",
                    "summary": "Create a new admin API token",
                    "description": "Creates a token and returns the plaintext value ONCE. Store it securely. Requires 'admin' scope.",
                    "tags": ["tokens"],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": {
                            "type": "object",
                            "required": ["name"],
                            "properties": {
                                "name": { "type": "string", "description": "Label for this token (1-128 chars)" },
                                "scopes": { "type": "string", "description": "Comma-separated scopes. Default: 'admin'. Options: admin, ci:read, ci:trigger, repos:read, repos:write, federation:read, federation:write" }
                            }
                        }}}
                    },
                    "responses": { "201": { "description": "Token created with plaintext value" }, "401": { "description": "Invalid token" } }
                }
            },
            "/v1/admin/tokens/{id}": {
                "delete": {
                    "operationId": "deleteToken",
                    "summary": "Revoke an admin API token",
                    "description": "Permanently deletes a token. Requests using it will immediately fail.",
                    "tags": ["tokens"],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }],
                    "responses": { "204": { "description": "Deleted" }, "404": { "description": "Not found" } }
                }
            },
            "/v1/admin/ci/workflows": {
                "get": {
                    "operationId": "ciListWorkflows",
                    "summary": "List CI workflows",
                    "description": "Returns all CI/CD workflows. Use the workflow filename (e.g., 'ci.yaml') with other CI endpoints. Requires ci:read scope.",
                    "tags": ["ci"],
                    "responses": { "200": { "description": "Workflow list" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/workflows/{name}/runs": {
                "get": {
                    "operationId": "ciListRuns",
                    "summary": "List CI runs for a workflow",
                    "description": "Returns recent runs for a workflow. Requires ci:read scope.",
                    "tags": ["ci"],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Workflow filename (e.g., ci.yaml, integration.yaml, e2e.yaml)" }],
                    "responses": { "200": { "description": "Run list" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/runs/{id}": {
                "get": {
                    "operationId": "ciGetRun",
                    "summary": "Get CI run details",
                    "description": "Full details for a run including job statuses. Requires ci:read scope.",
                    "tags": ["ci"],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }],
                    "responses": { "200": { "description": "Run details" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/runs/{id}/logs": {
                "get": {
                    "operationId": "ciGetLogs",
                    "summary": "Get CI run logs",
                    "description": "Raw log output as plain text. Can be large. Requires ci:read scope.",
                    "tags": ["ci"],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }],
                    "responses": { "200": { "description": "Plain text logs" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/workflows/{name}/dispatch": {
                "post": {
                    "operationId": "ciDispatch",
                    "summary": "Trigger a CI workflow",
                    "description": "Dispatches a new run on main branch. NOT idempotent. Requires ci:trigger scope.",
                    "tags": ["ci"],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" } }],
                    "responses": { "200": { "description": "Dispatched" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/ci/mirror-sync": {
                "post": {
                    "operationId": "ciMirrorSync",
                    "summary": "Force GitHub mirror sync",
                    "description": "Triggers immediate GitHub mirror sync instead of waiting for 10-minute poll. Requires ci:trigger scope.",
                    "tags": ["ci"],
                    "responses": { "200": { "description": "Sync triggered" }, "502": { "description": "Forgejo error" } }
                }
            },
            "/v1/admin/events": {
                "get": {
                    "operationId": "sseEvents",
                    "summary": "Subscribe to admin events (SSE)",
                    "description": "Server-Sent Events stream. Filter with ?filter=ci,repo,federation,cache,conversion. Any valid token can subscribe.",
                    "tags": ["events"],
                    "parameters": [{ "name": "filter", "in": "query", "required": false, "schema": { "type": "string" }, "description": "Comma-separated event types" }],
                    "responses": { "200": { "description": "SSE stream" } }
                }
            }
        }
    });

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string_pretty(&spec).unwrap_or_default(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_openapi_spec_returns_valid_json() {
        let resp = openapi_spec().await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
