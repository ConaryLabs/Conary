---
last_updated: 2026-05-24
revision: 1
summary: Design for mounting Conary's stateless MCP discover proof as a conary-test-only live route
---

# Conary-Test Stateless MCP Discover Route: Design Spec

**Date:** 2026-05-24
**Status:** Draft for review
**Goal:** Mount the tested stateless raw HTTP proof in `conary-test` as a
narrow live route that exposes only `server/discover`, while leaving Remi and
the legacy session-based `/mcp` endpoint unchanged.

---

## Purpose

Conary has now proved the current MCP draft shape twice without live route
wiring:

- `conary_mcp::stateless` models the draft protocol constants, request
  validation, discovery payloads, cache metadata, and JSON-RPC protocol error
  codes.
- `conary_mcp::stateless_http` maps a framework-neutral HTTP request into that
  harness, validates `Origin`, handles parsed JSON-RPC requests, returns
  `server/discover`, and rejects unsupported methods.

The next risk is no longer "can Conary model the draft?" It is "can a real app
server call that model without leaking into the legacy `rmcp` session path?"

This slice should answer that with the smallest useful live route:
`conary-test` exposes a stateless MCP discovery endpoint backed by the raw
proof. It does not expose tools, resources, prompts, SSE streaming,
subscriptions, or mutations. It proves live HTTP adaptation only.

## Current Facts

Repo facts, verified on 2026-05-24:

- `main` and `origin/main` are clean at
  `c6298372da52e4b16d1703c259634a38826e6304` before this design work.
- `crates/conary-mcp::stateless` defines:
  - `MCP_DRAFT_PROTOCOL_VERSION`
  - `HEADER_PROTOCOL_VERSION`
  - `HEADER_METHOD`
  - `HEADER_NAME`
  - `validate_stateless_request`
  - draft-shaped `DiscoverResult`
  - draft JSON-RPC error codes for header mismatch, invalid params, and
    unsupported protocol version
- `crates/conary-mcp::stateless_http` defines the non-live raw HTTP proof:
  - `RawStatelessHttpRequest`
  - `RawStatelessHttpResponse`
  - `RawStatelessHttpConfig`
  - `OriginPolicy`
  - `handle_stateless_http_request`
- `apps/conary-test` already depends on `conary-mcp` and `axum`.
- `apps/conary-test/src/server/routes.rs` currently mounts the legacy
  session-based `rmcp` service at `/mcp` with `StreamableHttpService` and
  `LocalSessionManager`.
- The existing `/mcp` endpoint is inside the authenticated API router. It
  requires `Authorization: Bearer <token>` when the server is started with a
  token, and it is open when no token is configured.
- `apps/conary-test/src/server/mod.rs` currently binds the server to
  `0.0.0.0:{port}`. This slice must not claim network isolation from bind
  address alone.

External facts, refreshed on 2026-05-24:

- The MCP draft discovery page requires `server/discover` and shows
  `DRAFT-2026-v1` in request `_meta` and response `supportedVersions`.
- The MCP draft transport page says Streamable HTTP messages are independent
  `POST` requests.
- The transport page requires `Accept` to include both `application/json` and
  `text/event-stream`.
- The transport page requires `MCP-Protocol-Version`, `Mcp-Method`, and
  conditional `Mcp-Name` request headers.
- The transport page requires servers to validate `Origin` to prevent DNS
  rebinding attacks.
- `rmcp 1.7.0` remains the workspace dependency and still documents
  session-era transport types, so this slice should use Conary's raw adapter
  path rather than deepening the `rmcp` path.

Primary references:

- <https://modelcontextprotocol.io/specification/draft/server/discover>
- <https://modelcontextprotocol.io/specification/draft/basic/transports>
- <https://docs.rs/crate/rmcp/latest>
- <https://docs.rs/rmcp/latest/rmcp/transport/streamable_http_server/session/local/struct.LocalSessionManager.html>
- <https://docs.rs/rmcp/latest/rmcp/transport/streamable_http_server/tower/struct.StreamableHttpService.html>

## Decision

Add a `conary-test` live route at:

```text
POST /mcp/stateless
```

The route uses the stateless raw HTTP proof and exposes only
`server/discover`.

It is a `conary-test`-only local-dev surface, not a Remi route and not a
replacement for the legacy `/mcp` route. "Local" in this slice means scoped to
the local developer/test harness, with missing `Origin` accepted for CLI and
non-browser clients. It does not mean the TCP listener is restricted to
`127.0.0.1`; changing the server bind address is a separate operational
hardening decision.

The route should be mounted inside the existing authenticated API router so
token behavior matches `/mcp`: when a token is configured, missing or wrong
bearer tokens return the existing HTTP `401` response before the MCP handler
runs. Auth middleware errors do not need JSON-RPC envelopes in this slice.

## Scope

In scope:

- Add an Axum adapter for the raw stateless HTTP proof in `apps/conary-test`.
- Mount `POST /mcp/stateless` in `create_router`.
- Keep the existing legacy `/mcp` `rmcp` service mounted and behaviorally
  unchanged.
- Preserve existing token-auth behavior by keeping the new route in the same
  API router as `/mcp`.
- Use `OriginPolicy::local_non_browser()` for the first live route.
- Use `serverInfo.name = "conary-test-mcp"` so discovery identifies the
  product surface, not the helper crate.
- Use the `conary-test` package version in discovery `serverInfo`.
- Return an empty `capabilities` object from `server/discover`.
- Return brief `instructions` saying this stateless endpoint currently exposes
  discovery only.
- Add or reuse a framework-neutral byte-entry helper so malformed JSON returns
  a JSON-RPC parse error instead of an Axum extractor error.
- Update guard tests so Remi and legacy MCP files remain draft-free, while the
  new `conary-test` stateless adapter file is allowed to contain draft
  identifiers.
- Update `docs/operations/agent-mcp-adapter-decision.md` and
  `docs/operations/infrastructure.md` to describe the new route accurately.

Out of scope:

- No Remi route changes.
- No replacement of `/mcp`.
- No live MCP resources.
- No live MCP tools.
- No live MCP prompts.
- No SSE streaming implementation.
- No subscriptions, progress notifications, cancellation, MRTR, or
  `input_required` handling.
- No `tools/list`, `resources/list`, `resources/read`, `prompts/list`, or
  `prompts/get` stubs.
- No change to the conary-test bind address or CLI serve flags.
- No new authentication system.
- No provider-specific SDK integration.

## Architecture

`conary_mcp::stateless` remains the source of truth for draft protocol
constants and protocol validation.

`conary_mcp::stateless_http` remains the framework-neutral HTTP proof. This
slice may extend it with a small byte-entry helper for invalid JSON handling,
but it must stay independent from `axum`, `rmcp`, `RoleServer`,
`ServerHandler`, `StreamableHttpService`, and `LocalSessionManager`.

`apps/conary-test/src/server/stateless_mcp.rs` should own the Axum adapter:

- read request method, headers, and body bytes
- preserve repeated header values when building the raw request
- parse JSON into `serde_json::Value` through the framework-neutral helper
- configure `RawStatelessHttpConfig` for `conary-test`
- call the raw proof
- map `RawStatelessHttpResponse` back into an Axum response

`apps/conary-test/src/server/routes.rs` should only mount the route. It should
not grow protocol validation logic.

The existing `apps/conary-test/src/server/mcp.rs` file stays the legacy
session-based tool server. This slice should not add draft stateless behavior
there.

Implementation plan notes:

- Add `pub mod stateless_mcp;` in `apps/conary-test/src/server/mod.rs`.
- Start with a route-order regression test for `/mcp/stateless` alongside the
  existing `/mcp` nested service.
- Reconcile the documentation audit inventory and ledger after docs are
  updated, because the active adapter decision and current active plan/spec
  files must be represented before the ledger gate can pass.

## Route Placement

The intended public path for the slice is `/mcp/stateless`.

Because the legacy service is mounted at `/mcp`, the implementation plan must
start with a route-level test proving that a valid request to `/mcp/stateless`
is handled by the stateless discover adapter, not by the legacy `rmcp`
service. If Axum route composition requires a small restructuring around the
existing `/mcp` mount, preserve both externally visible paths:

- `/mcp` continues to serve the legacy session-based MCP endpoint.
- `/mcp/stateless` serves only the stateless discovery route.

Do not move the stateless route to a different path without updating this spec
and asking for review.

## Data Flow

1. A client sends `POST /mcp/stateless`.
2. If the conary-test server was started with a token, existing bearer auth
   middleware runs first.
3. The Axum adapter collects method, headers, and body bytes.
4. The byte-entry helper applies HTTP method and `Origin` gates before JSON
   parsing.
5. For allowed `POST` requests, malformed JSON returns HTTP `400` with
   JSON-RPC parse error `-32700` and `id: null`.
6. Parsed JSON, method, and headers are passed into
   `conary_mcp::stateless_http`.
7. `stateless_http` validates HTTP method, `Origin`, JSON-RPC envelope, MCP
   headers, `Accept`, protocol version, and required `_meta`.
8. A valid `server/discover` request returns HTTP `200` with JSON-RPC
   `result`.
9. Valid but unsupported methods return HTTP `404` with JSON-RPC method-not-
   found `-32601`.
10. Protocol validation failures return the same HTTP and JSON-RPC mappings
   already proven in the raw adapter tests.

## Request Shape

A valid `server/discover` request to this route must include:

```json
{
  "jsonrpc": "2.0",
  "id": "discover-1",
  "method": "server/discover",
  "params": {
    "_meta": {
      "io.modelcontextprotocol/protocolVersion": "DRAFT-2026-v1",
      "io.modelcontextprotocol/clientInfo": {
        "name": "example-client",
        "version": "0.1.0"
      },
      "io.modelcontextprotocol/clientCapabilities": {}
    }
  }
}
```

Example headers:

```text
Accept: application/json, text/event-stream
Content-Type: application/json
MCP-Protocol-Version: DRAFT-2026-v1
Mcp-Method: server/discover
```

`Mcp-Name` is not required for `server/discover`.

This slice does not add a separate `Content-Type` validator. The handler reads
body bytes and parses JSON directly; malformed or non-JSON bodies fail through
the JSON-RPC parse-error path.

## Discovery Response

The successful response should be:

```json
{
  "jsonrpc": "2.0",
  "id": "discover-1",
  "result": {
    "resultType": "complete",
    "supportedVersions": ["DRAFT-2026-v1"],
    "capabilities": {},
    "serverInfo": {
      "name": "conary-test-mcp",
      "version": "0.8.0"
    },
    "instructions": "Conary test infrastructure stateless MCP endpoint exposes discovery only."
  }
}
```

The version shown above is illustrative; implementation should use the current
`conary-test` package version.

The response must not advertise `tools`, `resources`, or `prompts`.

All stateless route responses, including errors, use
`Content-Type: application/json`. SSE is not supported by this route.

## Error Model

Use the existing raw proof mappings for parsed requests:

| Condition | HTTP status | JSON-RPC code | Notes |
| --- | --- | --- | --- |
| valid `server/discover` | `200` | none | JSON-RPC `result` contains `DiscoverResult`. |
| invalid present `Origin` | `403` | `-32000` | Rejected before trusting request metadata. |
| non-`POST` method | `405` | `-32000` | HTTP transport gate. |
| malformed JSON | `400` | `-32700` | Applies only after method and `Origin` gates pass. Live Axum adapter must not leak an Axum extractor error. |
| malformed JSON-RPC envelope | `400` | `-32600` | Single request objects with `jsonrpc: "2.0"`, present `id`, and string `method` are accepted. |
| missing or mismatched MCP header | `400` | `-32001` | Use `StatelessProtocolError::json_rpc_error_code()`. |
| unsupported protocol version | `400` | `-32004` | Error `data` includes `{ requested, supported }`. |
| missing `_meta` fields | `400` | `-32602` | Preserve current stateless distinction. |
| unsupported validated RPC method | `404` | `-32601` | Used until live dispatch exists. |
| missing or wrong bearer token | `401` | none | Existing app auth middleware response, outside MCP handler. Body is `{"error":"unauthorized"}` plain JSON, not a JSON-RPC envelope; clients must handle it separately. |

## Testing

The implementation plan must use TDD.

Required `conary-mcp` coverage if a byte-entry helper is added:

- malformed JSON bytes return HTTP `400`, JSON-RPC `-32700`, and `id: null`
- valid JSON bytes delegate to the existing parsed-body handler
- non-`POST` bytes return HTTP `405` before JSON parsing, including malformed
  JSON bytes
- invalid present `Origin` returns HTTP `403` before JSON parsing, including
  malformed JSON bytes
- the stateless modules still do not import `axum`, `rmcp`, session types, or
  live HTTP framework types

Required `conary-test` route coverage:

- `POST /mcp/stateless` with a valid `server/discover` request returns HTTP
  `200` and JSON-RPC discovery
- discovery `serverInfo.name` is `conary-test-mcp`
- discovery `capabilities` is an empty object and does not include `tools`,
  `resources`, or `prompts`
- `/mcp` still exists and still uses the legacy auth behavior
- `/mcp` still reaches the legacy `rmcp` service when a valid bearer token is
  supplied; a route composition change must not turn it into a `404`, `405`,
  `401`, or stateless discovery response
- a valid `server/discover` request sent to `/mcp`, not `/mcp/stateless`, does
  not return discovery JSON; this proves the routes are not cross-wired
- `/mcp/stateless` requires the bearer token when `create_router` receives a
  token
- `/mcp/stateless` accepts missing `Origin` for non-browser local clients
- `/mcp/stateless` rejects a non-matching present `Origin` with HTTP `403`
- missing `MCP-Protocol-Version` returns HTTP `400` and JSON-RPC `-32001`
- missing `_meta` returns HTTP `400` and JSON-RPC `-32602`
- unsupported protocol version returns HTTP `400`, JSON-RPC `-32004`, and
  `data.requested` / `data.supported`
- valid but unsupported `tools/list` returns HTTP `404` and JSON-RPC `-32601`
- non-`POST` requests return HTTP `405`
- non-`POST` requests with malformed JSON still return HTTP `405`
- malformed JSON returns HTTP `400` and JSON-RPC `-32700`
- repeated and comma-separated `Accept` headers survive Axum conversion
- the route test proves `/mcp/stateless` is not swallowed by the legacy `/mcp`
  service

Guard coverage:

- Remi route and MCP server files must remain free of draft stateless
  identifiers.
- `apps/conary-test/src/server/mcp.rs` must remain free of draft stateless
  identifiers.
- `apps/conary-test/src/server/routes.rs` may contain only the mount for the
  stateless route, not protocol constants or `_meta` validation.
- The new `apps/conary-test/src/server/stateless_mcp.rs` file is the only
  `conary-test` server file that should contain `server/discover`,
  `MCP-Protocol-Version`, `Mcp-Method`, or `Mcp-Name` outside tests.

Final verification should include:

```bash
cargo fmt --check
cargo test -p conary-mcp
cargo test -p conary-test stateless
cargo test -p conary-test mcp_endpoint_requires_token
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
if rg -n "server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/remi/src/server; then exit 1; fi
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

The final Remi grep is inverted intentionally: any match is a failure.

## Documentation Updates

Update `docs/operations/agent-mcp-adapter-decision.md` after implementation to
record that the adapter gate has moved from non-live raw proof to a
`conary-test` live discovery-only route.

Update `docs/operations/infrastructure.md` to say:

- the legacy `/mcp` route remains session-based and tool-only
- `conary-test` also exposes `/mcp/stateless` for draft stateless discovery
  only
- no live resources, tools, or prompts are exposed through the stateless route
  yet
- first read-only resources remain a follow-on slice

Update `apps/conary-test/README.md` to mention `/mcp/stateless` as a
discovery-only, draft-shaped preview endpoint. Keep the existing `/mcp`
description intact.

Update `docs/superpowers/documentation-accuracy-audit-inventory.tsv` and
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` so the docs audit
ledger check passes after the operations docs and app README are updated.

## Acceptance Criteria

- `POST /mcp/stateless` exists in `conary-test`.
- Valid `server/discover` requests return draft-shaped discovery JSON.
- Discovery advertises no tools, resources, or prompts.
- Existing `/mcp` behavior is unchanged.
- A regression test proves authenticated `/mcp` requests still reach the
  legacy `rmcp` service after `/mcp/stateless` is mounted.
- Remi has no live stateless route changes.
- The route is behind the existing conary-test auth boundary when auth is
  configured.
- Malformed JSON and protocol failures return structured JSON-RPC errors.
- Guard tests clearly distinguish allowed `conary-test` stateless files from
  forbidden Remi and legacy MCP files.
- `docs/operations/agent-mcp-adapter-decision.md` records this route as the
  first live adapter gate.
- The documentation audit ledger passes in `--require-complete` mode.
- The implementation plan is suitable for Codex `/goal` and keeps one focused
  commit per task.

## Risks

- The MCP draft can still change before the 2026-07-28 release candidate.
  Keeping the route discover-only limits the blast radius.
- `conary-test` currently binds `0.0.0.0`. This slice should be honest about
  that and should not describe the route as network-isolated. Bind-address
  hardening can be a later server-ops slice.
- Axum route composition around `/mcp` and `/mcp/stateless` must be verified by
  tests because the legacy `/mcp` endpoint is a nested service.
- Auth middleware returns ordinary HTTP JSON, not JSON-RPC. That is acceptable
  because auth gates run outside the MCP handler, but it should be documented
  so later client tests do not expect a JSON-RPC envelope for `401`.
- A discover-only live route is intentionally not useful to end users yet. Its
  value is proving the adapter gate before resources are added.

## Follow-On

After this route passes, Conary can add the first read-only stateless MCP
resource. The likely first resource should stay in `conary-test` and should be
cacheable, deterministic, and safe for agent orientation, such as bootstrap
status or service health.

Do not add read-only resources until this route is live, tested, and recorded
in the adapter decision doc.
