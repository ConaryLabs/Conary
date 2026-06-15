// apps/remi/src/server/handlers/openapi.rs
//! OpenAPI 3.1 specification for the admin API

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn openapi_spec() -> Response {
    let spec = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Remi Admin API",
            "description": "Administration API for the Remi package server. Manage admin tokens, repositories, federation settings, and test-harness operations. Designed for human and LLM agent consumption.",
            "version": env!("CARGO_PKG_VERSION"),
            "contact": { "name": "Conary Labs" }
        },
        "servers": [
            { "url": "/", "description": "Current admin API origin (direct admin listener or reverse-proxied public endpoint)" }
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
                                "scopes": { "type": "string", "description": "Comma-separated scopes. Default: 'admin'. Options: admin, repos:read, repos:write, federation:read, federation:write" }
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
            "/v1/admin/test-fixtures/{path}": {
                "put": {
                    "operationId": "uploadTestFixture",
                    "summary": "Upload an integration-test fixture",
                    "description": "Uploads a fixture blob under the requested catch-all path. Requires admin or test-data write authorization.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Fixture path relative to the test fixture root" }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } }
                    },
                    "responses": { "200": { "description": "Fixture uploaded" }, "401": { "description": "Invalid or missing token" }, "403": { "description": "Insufficient scope" } }
                }
            },
            "/v1/admin/test-artifacts/{path}": {
                "put": {
                    "operationId": "uploadTestArtifact",
                    "summary": "Upload an integration-test artifact",
                    "description": "Uploads a test artifact blob under the requested catch-all path. Requires admin or test-data write authorization.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Artifact path relative to the test artifact root" }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } }
                    },
                    "responses": { "200": { "description": "Artifact uploaded" }, "401": { "description": "Invalid or missing token" }, "403": { "description": "Insufficient scope" } }
                }
            },
            "/v1/admin/packages/{distro}": {
                "post": {
                    "operationId": "uploadPackage",
                    "summary": "Upload a package artifact",
                    "description": "Uploads a package artifact for the named distro so test harnesses and package workflows can publish inputs.",
                    "tags": ["packages"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "distro", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Distribution key for the uploaded package" }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } }
                    },
                    "responses": { "200": { "description": "Package uploaded" }, "401": { "description": "Invalid or missing token" }, "403": { "description": "Insufficient scope" } }
                }
            },
            "/v1/admin/releases/{distro}": {
                "post": {
                    "operationId": "uploadReleasePackage",
                    "summary": "Upload an attested release package",
                    "description": "Uploads a CCS release package after verifying the static publish gate with configured trusted build-attestation signers. Public package metadata, CAS chunk visibility, and TUF targets are committed only after the gate passes.",
                    "tags": ["packages"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "distro", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Distribution key for the release package" }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } }
                    },
                    "responses": {
                        "201": { "description": "Release package accepted and published" },
                        "400": { "description": "Invalid upload body or CCS package" },
                        "401": { "description": "Invalid or missing token" },
                        "403": { "description": "Insufficient scope" },
                        "413": { "description": "Upload exceeds maximum size" },
                        "422": { "description": "Publish gate failed, including missing trusted release signers or rejected attestation signer" }
                    }
                }
            },
            "/v1/admin/packages/{distro}/{package}/scriptlet-review": {
                "get": {
                    "operationId": "getScriptletReviewArtifact",
                    "summary": "Fetch scriptlet review artifact",
                    "description": "Returns the scriptlet review artifact for a package uploaded through the admin package surface.",
                    "tags": ["packages"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [
                        { "name": "distro", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Distribution key" },
                        { "name": "package", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Package identifier" }
                    ],
                    "responses": { "200": { "description": "Scriptlet review artifact" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Artifact not found" } }
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
            "/v1/admin/refresh": {
                "post": {
                    "operationId": "refreshRepos",
                    "summary": "Refresh upstream repository state",
                    "description": "Triggers a repository refresh across configured upstreams. Use repository sync endpoints for per-repo refreshes.",
                    "tags": ["repos"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "202": { "description": "Refresh queued" }, "401": { "description": "Invalid or missing token" }, "403": { "description": "Insufficient scope" } }
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
            },
            "/v1/admin/test-runs/gc": {
                "delete": {
                    "operationId": "testRunGarbageCollect",
                    "summary": "Garbage collect old test-run data",
                    "description": "Deletes expired test-run data and artifacts according to the server's retention policy.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "200": { "description": "Garbage collection completed" }, "401": { "description": "Invalid or missing token" }, "403": { "description": "Insufficient scope" } }
                }
            },
            "/v1/admin/test-health": {
                "get": {
                    "operationId": "testHealth",
                    "summary": "Check test-data service health",
                    "description": "Returns health information for the Remi test-data API.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "200": { "description": "Test-data service health" }, "401": { "description": "Invalid or missing token" } }
                }
            },
            "/v1/admin/test-runs": {
                "get": {
                    "operationId": "listTestRuns",
                    "summary": "List test runs",
                    "description": "Returns recorded integration-test runs with status and timing metadata.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "responses": { "200": { "description": "Array of test runs" }, "401": { "description": "Invalid or missing token" } }
                },
                "post": {
                    "operationId": "createTestRun",
                    "summary": "Create a test run",
                    "description": "Creates a test-run record for harness result ingestion.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": { "type": "object" } } }
                    },
                    "responses": { "201": { "description": "Test run created" }, "400": { "description": "Invalid request" }, "401": { "description": "Invalid or missing token" } }
                }
            },
            "/v1/admin/test-runs/{id}": {
                "get": {
                    "operationId": "getTestRun",
                    "summary": "Get test-run details",
                    "description": "Returns a single test run by identifier.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Test run identifier" }],
                    "responses": { "200": { "description": "Test run" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Test run not found" } }
                },
                "put": {
                    "operationId": "updateTestRun",
                    "summary": "Update a test run",
                    "description": "Updates status or metadata for an existing test run.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Test run identifier" }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": { "type": "object" } } }
                    },
                    "responses": { "200": { "description": "Test run updated" }, "400": { "description": "Invalid request" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Test run not found" } }
                }
            },
            "/v1/admin/test-runs/{id}/results": {
                "post": {
                    "operationId": "pushTestResult",
                    "summary": "Push a test result",
                    "description": "Appends or updates result data for a test run.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Test run identifier" }],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": { "type": "object" } } }
                    },
                    "responses": { "200": { "description": "Result accepted" }, "400": { "description": "Invalid result" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Test run not found" } }
                }
            },
            "/v1/admin/test-runs/{id}/tests/{test_id}": {
                "get": {
                    "operationId": "getTestDetail",
                    "summary": "Get test detail",
                    "description": "Returns one test's result detail within a test run.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [
                        { "name": "id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Test run identifier" },
                        { "name": "test_id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Manifest test identifier" }
                    ],
                    "responses": { "200": { "description": "Test detail" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Test not found" } }
                }
            },
            "/v1/admin/test-runs/{id}/tests/{test_id}/logs": {
                "get": {
                    "operationId": "getTestLogs",
                    "summary": "Get test logs",
                    "description": "Returns captured logs for one test within a test run.",
                    "tags": ["test-harness"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [
                        { "name": "id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Test run identifier" },
                        { "name": "test_id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Manifest test identifier" }
                    ],
                    "responses": { "200": { "description": "Test logs" }, "401": { "description": "Invalid or missing token" }, "404": { "description": "Logs not found" } }
                }
            },
            "/v1/admin/audit": {
                "get": {
                    "operationId": "queryAudit",
                    "summary": "Query audit log",
                    "description": "Returns recent admin API operations. Supports filtering by action, token, and time range. Write operations include request/response bodies.",
                    "tags": ["audit"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [
                        {
                            "name": "limit",
                            "in": "query",
                            "schema": { "type": "integer", "default": 50, "maximum": 500 },
                            "description": "Maximum number of entries to return"
                        },
                        {
                            "name": "action",
                            "in": "query",
                            "schema": { "type": "string" },
                            "description": "Filter by action prefix (e.g., 'repo' matches 'repo.create')"
                        },
                        {
                            "name": "since",
                            "in": "query",
                            "schema": { "type": "string", "format": "date-time" },
                            "description": "Only entries after this ISO 8601 timestamp"
                        },
                        {
                            "name": "token_name",
                            "in": "query",
                            "schema": { "type": "string" },
                            "description": "Filter by token name"
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "Array of audit log entries",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "id": { "type": "integer" },
                                                "timestamp": { "type": "string", "format": "date-time" },
                                                "token_name": { "type": "string", "nullable": true },
                                                "action": { "type": "string" },
                                                "method": { "type": "string" },
                                                "path": { "type": "string" },
                                                "status_code": { "type": "integer" },
                                                "request_body": { "type": "string", "nullable": true },
                                                "response_body": { "type": "string", "nullable": true },
                                                "source_ip": { "type": "string", "nullable": true },
                                                "duration_ms": { "type": "integer", "nullable": true }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Not authenticated" },
                        "403": { "description": "Insufficient scope (requires admin)" }
                    }
                },
                "delete": {
                    "operationId": "purgeAudit",
                    "summary": "Purge old audit entries",
                    "description": "Delete audit log entries older than the specified date. NOT reversible.",
                    "tags": ["audit"],
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [
                        {
                            "name": "before",
                            "in": "query",
                            "required": true,
                            "schema": { "type": "string", "format": "date-time" },
                            "description": "Delete entries with timestamps before this ISO 8601 date"
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "Number of entries deleted",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "deleted": { "type": "integer" },
                                            "before": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Not authenticated" },
                        "403": { "description": "Insufficient scope (requires admin)" }
                    }
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

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json_text = String::from_utf8(body.to_vec()).unwrap();

        serde_json::from_str::<serde_json::Value>(&json_text).unwrap();
        assert!(!json_text.contains("/v1/admin/ci/workflows"));
        assert!(!json_text.contains("ci:read"));
        assert!(!json_text.contains("ci:trigger"));
    }

    #[tokio::test]
    async fn openapi_spec_documents_external_admin_route_families() {
        let resp = openapi_spec().await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let paths = spec
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .expect("OpenAPI spec should contain a paths object");

        for (path, methods) in [
            ("/v1/admin/tokens", &["get", "post"][..]),
            ("/v1/admin/tokens/{id}", &["delete"][..]),
            ("/v1/admin/test-fixtures/{path}", &["put"][..]),
            ("/v1/admin/test-artifacts/{path}", &["put"][..]),
            ("/v1/admin/packages/{distro}", &["post"][..]),
            ("/v1/admin/releases/{distro}", &["post"][..]),
            (
                "/v1/admin/packages/{distro}/{package}/scriptlet-review",
                &["get"][..],
            ),
            ("/v1/admin/repos", &["get", "post"][..]),
            ("/v1/admin/repos/{name}", &["get", "put", "delete"][..]),
            ("/v1/admin/repos/{name}/sync", &["post"][..]),
            ("/v1/admin/refresh", &["post"][..]),
            ("/v1/admin/federation/peers", &["get", "post"][..]),
            ("/v1/admin/federation/peers/{id}", &["delete"][..]),
            ("/v1/admin/federation/peers/{id}/health", &["get"][..]),
            ("/v1/admin/federation/config", &["get", "put"][..]),
            ("/v1/admin/test-runs/gc", &["delete"][..]),
            ("/v1/admin/test-health", &["get"][..]),
            ("/v1/admin/test-runs", &["get", "post"][..]),
            ("/v1/admin/test-runs/{id}", &["get", "put"][..]),
            ("/v1/admin/test-runs/{id}/results", &["post"][..]),
            ("/v1/admin/test-runs/{id}/tests/{test_id}", &["get"][..]),
            (
                "/v1/admin/test-runs/{id}/tests/{test_id}/logs",
                &["get"][..],
            ),
            ("/v1/admin/audit", &["get", "delete"][..]),
            ("/v1/admin/events", &["get"][..]),
        ] {
            let path_item = paths
                .get(path)
                .unwrap_or_else(|| panic!("OpenAPI spec missing path {path}"));

            for method in methods {
                assert!(
                    path_item.get(*method).is_some(),
                    "OpenAPI spec missing {method} operation for {path}"
                );
            }
        }
    }

    #[tokio::test]
    async fn openapi_spec_documents_release_upload_gate_failure() {
        let resp = openapi_spec().await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let release_post = &spec["paths"]["/v1/admin/releases/{distro}"]["post"];

        assert_eq!(
            release_post["operationId"],
            serde_json::Value::String("uploadReleasePackage".to_string())
        );
        assert!(
            release_post["responses"]["422"]["description"]
                .as_str()
                .unwrap()
                .contains("trusted release signers")
        );
    }
}
