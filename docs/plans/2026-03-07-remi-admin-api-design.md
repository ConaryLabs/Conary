# Remi Admin API Design

**Goal:** Expose a token-authenticated REST API for remote administration of Remi server, repositories, federation, and CI monitoring — no web frontend. Includes MCP (Model Context Protocol) endpoint for direct LLM agent integration and OpenAPI spec for discoverability.

**Date:** 2026-03-07

---

## Architecture

### Listeners

| Port | Purpose | Auth |
|------|---------|------|
| :8080 | Public package API (existing) | None |
| :8081 | Localhost admin (existing) | None (loopback only) |
| :8082 | External admin API (new) | Bearer token |

The existing localhost admin router (:8081) stays unchanged. A new external admin listener on :8082 serves the same admin functionality plus new endpoints, protected by bearer token middleware.

### Auth Model

- Tokens stored in `admin_tokens` SQLite table: `id`, `name`, `token_hash` (SHA-256), `created_at`, `last_used_at`, `scopes`
- Scopes: `admin` (full access), `ci:read`, `ci:trigger`, `repos:read`, `repos:write`, `federation:read`, `federation:write`
- Bootstrap: first token created via CLI (`conary-server admin token create`) or `REMI_ADMIN_TOKEN` env var
- Auth middleware extracts `Authorization: Bearer <token>`, hashes it, looks up in DB, checks scopes
- Localhost :8081 bypasses auth entirely (backwards compatible)

### Configuration

New fields in `ServerConfig`:

```toml
[admin]
external_bind = "0.0.0.0:8082"  # New external admin port
forgejo_url = "http://forge.conarylabs.com:3000"
forgejo_token = "..."  # Stored in credentials file, not main config
```

---

## Endpoints

### P0: Auth Management — `/v1/admin/tokens`

| Method | Path | Scope | Description |
|--------|------|-------|-------------|
| POST | `/v1/admin/tokens` | `admin` | Create token (returns plaintext once) |
| GET | `/v1/admin/tokens` | `admin` | List tokens (name, scopes, last_used) |
| DELETE | `/v1/admin/tokens/:id` | `admin` | Revoke token |

### P0: CI/Build Monitoring — `/v1/admin/ci`

Proxies Forgejo API. Remi injects the Forgejo token server-side; clients never see it.

| Method | Path | Scope | Description |
|--------|------|-------|-------------|
| GET | `/v1/admin/ci/workflows` | `ci:read` | List workflows |
| GET | `/v1/admin/ci/workflows/:name/runs` | `ci:read` | List runs for workflow |
| GET | `/v1/admin/ci/runs/:id` | `ci:read` | Get run details + job logs |
| GET | `/v1/admin/ci/runs/:id/logs` | `ci:read` | Stream job logs |
| POST | `/v1/admin/ci/workflows/:name/dispatch` | `ci:trigger` | Trigger workflow run |
| POST | `/v1/admin/ci/mirror-sync` | `ci:trigger` | Force mirror sync |

### P1: Repository Management — `/v1/admin/repos`

| Method | Path | Scope | Description |
|--------|------|-------|-------------|
| GET | `/v1/admin/repos` | `repos:read` | List configured repos |
| POST | `/v1/admin/repos` | `repos:write` | Add repository |
| GET | `/v1/admin/repos/:name` | `repos:read` | Get repo details + sync status |
| PUT | `/v1/admin/repos/:name` | `repos:write` | Update repo config |
| DELETE | `/v1/admin/repos/:name` | `repos:write` | Remove repository |
| POST | `/v1/admin/repos/:name/sync` | `repos:write` | Trigger manual sync |

### P1: Federation Management — `/v1/admin/federation`

| Method | Path | Scope | Description |
|--------|------|-------|-------------|
| GET | `/v1/admin/federation/peers` | `federation:read` | List peers + health |
| POST | `/v1/admin/federation/peers` | `federation:write` | Add peer |
| DELETE | `/v1/admin/federation/peers/:id` | `federation:write` | Remove peer |
| GET | `/v1/admin/federation/peers/:id/health` | `federation:read` | Detailed peer health |
| GET | `/v1/admin/federation/config` | `federation:read` | Get federation config |
| PUT | `/v1/admin/federation/config` | `federation:write` | Update federation config |

### P1: SSE Event Stream — `/v1/admin/events`

| Method | Path | Scope | Description |
|--------|------|-------|-------------|
| GET | `/v1/admin/events` | any valid token | SSE stream of admin events |
| GET | `/v1/admin/events?filter=ci,repo` | any valid token | Filtered SSE stream |

Event types: `ci`, `repo`, `federation`, `cache`, `conversion`

---

## Data Flow

### CI Proxy

```
Client --[Bearer token]--> Remi :8082 --[Forgejo token]--> Forgejo :3000
                           (auth middleware)              (injected server-side)
```

Forgejo credentials stored in `deploy/.credentials.toml` (already gitignored), loaded into `ServerConfig` at startup. Clients authenticate to Remi only — never see Forgejo credentials.

### SSE Architecture

```
Server components --> tokio::broadcast channel --> SSE endpoint --> Clients
(conversion jobs,    (typed JSON events)         (filtered by
 repo sync,                                      query param)
 federation health)
```

Live-only streaming, no persistence. Uses `tokio::sync::broadcast` with bounded buffer (1024 events). Slow consumers get `Lagged` error and reconnect.

---

## Error Handling

All endpoints return consistent JSON errors:

```json
{"error": "Human-readable message", "code": "ERROR_CODE"}
```

| Status | Code | When |
|--------|------|------|
| 401 | `UNAUTHORIZED` | Missing or invalid token |
| 403 | `INSUFFICIENT_SCOPE` | Token lacks required scope |
| 404 | `NOT_FOUND` | Resource not found |
| 502 | `UPSTREAM_ERROR` | Forgejo unreachable/error |
| 500 | `INTERNAL_ERROR` | Server error |

No retry logic in CI proxy — clients handle retries. No rate limiting in initial implementation.

---

## Testing Strategy

- **Unit tests**: Token hashing, auth middleware (mock requests), scope validation
- **Integration tests**: Start test server on random port, exercise CRUD with real SQLite
- **CI proxy tests**: Mock Forgejo HTTP server, verify proxy forwarding and error handling
- **Smoke test**: Add admin health check to `remi-health.sh --full`

---

## LLM Integration Layer

### OpenAPI Spec — `/v1/admin/openapi.json`

Auto-served OpenAPI 3.1 spec describing all admin endpoints. Enables:
- LLM agents to discover available operations without prior knowledge
- Auto-generation of MCP tool definitions from the spec
- Standard API client generation for any language

Served as a static JSON response, generated at build time or startup from route definitions.

### MCP Server — `/mcp` on :8082

A Model Context Protocol endpoint using the `rmcp` crate (official Rust MCP SDK). Exposes admin operations as MCP tools that LLM agents can discover and invoke directly.

**Transport:** Streamable HTTP (single endpoint, stateless, MCP spec 2025-03-26). Clients connect via:
```
claude mcp add remi-admin --transport http --url https://packages.conary.io:8082/mcp
```

**Tools exposed (map 1:1 to REST endpoints):**

| Tool Name | Scope | Description |
|-----------|-------|-------------|
| `list_tokens` | `admin` | List admin API tokens |
| `create_token` | `admin` | Create a new admin token |
| `delete_token` | `admin` | Revoke an admin token |
| `ci_list_workflows` | `ci:read` | List CI workflows |
| `ci_list_runs` | `ci:read` | List runs for a workflow |
| `ci_get_run` | `ci:read` | Get run details |
| `ci_get_logs` | `ci:read` | Get run logs |
| `ci_dispatch` | `ci:trigger` | Trigger a workflow run |
| `ci_mirror_sync` | `ci:trigger` | Force mirror sync |
| `sse_subscribe` | any | Subscribe to admin events (converted to polling for MCP) |

**Auth:** Bearer token passed as MCP tool parameter or via HTTP Authorization header on the transport connection. The MCP handler reuses the same auth middleware as REST endpoints.

**Architecture:** The MCP tool handlers call the same internal functions as the REST handlers — no logic duplication. The MCP layer is purely a protocol adapter (~200-300 lines).

```
LLM Agent <--[MCP/Streamable HTTP]--> rmcp server <--[internal]--> same handler functions
                                                                          |
curl/scripts <--[REST/JSON]-------> axum router ----[internal]-----> same handler functions
```

### Design Principles for LLM-Friendliness

Applied across both REST and MCP surfaces:

1. **Rich descriptions**: Every endpoint/tool has a human-readable description explaining what it does, when to use it, and what to expect
2. **Consistent error shapes**: All errors are `{"error": "message", "code": "CODE"}` — LLMs parse this reliably
3. **Semantic naming**: Tool/endpoint names describe the action (`ci_list_runs` not `get_v1_admin_ci_runs`)
4. **Minimal payloads**: Responses include only what's needed — no deeply nested structures that waste context window
5. **Idempotent reads**: GET/list operations are safe to retry — LLMs can re-call without side effects
6. **Actionable responses**: Error messages suggest what to do next (e.g., "Token requires ci:trigger scope")

---

## Priority / Phasing

| Phase | What | Depends on |
|-------|------|------------|
| P0 | Auth middleware + token management + CI proxy | Nothing |
| P0 | OpenAPI spec endpoint | P0 auth |
| P1 | Repo management + federation management + SSE | P0 (auth) |
| P1 | MCP server endpoint (wraps P0 + P1 tools) | P0 + P1 |
| P2 | Advanced features (rate limiting, audit log, webhooks) | P1 |
