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
            },
            "/v1/admin/repos": {
                "get": {
                    "operationId": "listRepos",
                    "summary": "List configured repositories",
                    "description": "Returns all configured upstream repositories with their sync status, priority, and enabled state. Use to check which repos are available before triggering a sync. Requires repos:read scope.",
                    "tags": ["repos"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "200": { "description": "Array of repositories" }, "401": { "description": "Invalid or missing token" } }
                },
                "post": {
                    "operationId": "createRepo",
                    "summary": "Add a repository",
                    "description": "Registers a new upstream repository for metadata sync and package fetching. After adding, trigger a sync to pull metadata. Requires repos:write scope.",
                    "tags": ["repos"],
                    "security": [{ "bearerAuth": [] }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": {
                            "type": "object",
                            "required": ["name", "url"],
                            "properties": {
                                "name": { "type": "string", "description": "Unique repository identifier (e.g., 'fedora-41')" },
                                "url": { "type": "string", "description": "Base URL for repository metadata" },
                                "content_url": { "type": "string", "description": "Separate URL for package downloads, if different from metadata URL" },
                                "enabled": { "type": "boolean", "description": "Whether the repo is active. Default: true" },
                                "priority": { "type": "integer", "description": "Lower values are preferred when resolving. Default: 0" },
                                "gpg_check": { "type": "boolean", "description": "Verify GPG signatures on metadata. Default: true" },
                                "metadata_expire": { "type": "integer", "description": "Metadata cache lifetime in seconds. Default: 3600" }
                            }
                        }}}
                    },
                    "responses": { "201": { "description": "Repository created" }, "400": { "description": "Invalid configuration" }, "401": { "description": "Invalid or missing token" }, "409": { "description": "Repository name already exists" } }
                }
            },
            "/v1/admin/repos/{name}": {
                "get": {
                    "operationId": "getRepo",
                    "summary": "Get repository details",
                    "description": "Returns full configuration and sync status for a single repository. Use to inspect settings before updating. Requires repos:read scope.",
                    "tags": ["repos"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Repository identifier" }],
                    "responses": { "200": { "description": "Repository details" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Repository not found" } }
                },
                "put": {
                    "operationId": "updateRepo",
                    "summary": "Update repository configuration",
                    "description": "Replaces repository configuration. Include all fields, not just changed ones. Use getRepo first to fetch current values. Requires repos:write scope.",
                    "tags": ["repos"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Repository identifier" }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": {
                            "type": "object",
                            "required": ["url"],
                            "properties": {
                                "name": { "type": "string", "description": "Ignored (renames not supported). Optional for backwards compatibility." },
                                "url": { "type": "string", "description": "Base URL for repository metadata" },
                                "content_url": { "type": "string", "description": "Separate URL for package downloads" },
                                "enabled": { "type": "boolean", "description": "Whether the repo is active" },
                                "priority": { "type": "integer", "description": "Lower values are preferred when resolving" },
                                "gpg_check": { "type": "boolean", "description": "Verify GPG signatures on metadata" },
                                "metadata_expire": { "type": "integer", "description": "Metadata cache lifetime in seconds" }
                            }
                        }}}
                    },
                    "responses": { "200": { "description": "Repository updated" }, "400": { "description": "Invalid configuration" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Repository not found" } }
                },
                "delete": {
                    "operationId": "deleteRepo",
                    "summary": "Remove a repository",
                    "description": "Deletes a repository and its cached metadata. Packages already converted from this repo remain in the CAS. Requires repos:write scope.",
                    "tags": ["repos"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Repository identifier" }],
                    "responses": { "204": { "description": "Deleted" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Repository not found" } }
                }
            },
            "/v1/admin/repos/{name}/sync": {
                "post": {
                    "operationId": "syncRepo",
                    "summary": "Trigger repository metadata sync",
                    "description": "Starts an asynchronous metadata sync for the specified repository. Returns immediately with 202. Monitor progress via the SSE events endpoint with ?filter=repo. Requires repos:write scope.",
                    "tags": ["repos"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Repository identifier" }],
                    "responses": { "202": { "description": "Sync started" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Repository not found" } }
                }
            },
            "/v1/admin/federation/peers": {
                "get": {
                    "operationId": "listPeers",
                    "summary": "List federation peers",
                    "description": "Returns all configured federation peers with their tier, health status, and last-seen timestamps. Use to monitor cluster topology. Requires federation:read scope.",
                    "tags": ["federation"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "200": { "description": "Array of peers with health info" }, "401": { "description": "Invalid or missing token" } }
                },
                "post": {
                    "operationId": "addPeer",
                    "summary": "Add a federation peer",
                    "description": "Registers a new peer node for CAS chunk sharing. The peer will be health-checked automatically. Use listPeers to verify it came online. Requires federation:write scope.",
                    "tags": ["federation"],
                    "security": [{ "bearerAuth": [] }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": {
                            "type": "object",
                            "required": ["endpoint"],
                            "properties": {
                                "endpoint": { "type": "string", "description": "Base URL of the peer (e.g., 'https://peer1.example.com:8080')" },
                                "tier": { "type": "string", "description": "Peer tier: 'leaf', 'cell_hub', or 'region_hub'. Default: 'leaf'" },
                                "node_name": { "type": "string", "description": "Human-readable name for the peer" }
                            }
                        }}}
                    },
                    "responses": { "201": { "description": "Peer added" }, "400": { "description": "Invalid peer configuration" }, "401": { "description": "Invalid or missing token" } }
                }
            },
            "/v1/admin/federation/peers/{id}": {
                "delete": {
                    "operationId": "deletePeer",
                    "summary": "Remove a federation peer",
                    "description": "Removes a peer from the federation. Chunks stored on that peer become unavailable unless replicated elsewhere. Requires federation:write scope.",
                    "tags": ["federation"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Peer identifier" }],
                    "responses": { "204": { "description": "Deleted" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Peer not found" } }
                }
            },
            "/v1/admin/federation/peers/{id}/health": {
                "get": {
                    "operationId": "peerHealth",
                    "summary": "Get detailed peer health",
                    "description": "Returns detailed health metrics for a specific peer including latency, success rate, circuit breaker state, and current status. Use to diagnose connectivity issues. Requires federation:read scope.",
                    "tags": ["federation"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Peer identifier" }],
                    "responses": { "200": { "description": "Peer health with success_rate and status" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Peer not found" } }
                }
            },
            "/v1/admin/federation/config": {
                "get": {
                    "operationId": "getFederationConfig",
                    "summary": "Get federation configuration",
                    "description": "Returns the current federation configuration including tier, replication settings, and discovery options. Requires federation:read scope.",
                    "tags": ["federation"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "200": { "description": "Federation configuration object" }, "401": { "description": "Invalid or missing token" } }
                },
                "put": {
                    "operationId": "updateFederationConfig",
                    "summary": "Update federation configuration",
                    "description": "Replaces the federation configuration. Changes take effect immediately. Use getFederationConfig first to fetch current values. Requires federation:write scope.",
                    "tags": ["federation"],
                    "security": [{ "bearerAuth": [] }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": {
                            "type": "object",
                            "description": "Federation configuration object. Structure depends on deployment."
                        }}}
                    },
                    "responses": { "200": { "description": "Configuration updated" }, "400": { "description": "Invalid configuration" }, "401": { "description": "Invalid or missing token" } }
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
