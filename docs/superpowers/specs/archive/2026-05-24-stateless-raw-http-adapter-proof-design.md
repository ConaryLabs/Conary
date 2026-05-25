---
last_updated: 2026-05-24
revision: 1
summary: Design for a non-live raw HTTP proof of Conary's target stateless MCP adapter
---

# Stateless Raw HTTP Adapter Proof: Design Spec

**Date:** 2026-05-24
**Status:** Draft for review
**Goal:** Prove Conary can implement the current stateless MCP draft over raw
HTTP without deepening the legacy session-based `rmcp` path or registering live
MCP behavior.

---

## Purpose

Conary has a transport-neutral agent contract and a non-live stateless MCP
compliance harness. The next risk is the adapter gate: `rmcp` is current, but
it still does not expose the target stateless draft shape Conary needs. Conary
should prove the raw HTTP path now, while the proof can stay small and
contained.

This slice should build a tested adapter proof in `crates/conary-mcp` that
accepts a framework-neutral HTTP request shape, validates the current MCP draft
headers and request metadata, handles `server/discover`, returns draft-shaped
JSON-RPC errors, and rejects unsupported methods. It must not mount a route in
Remi or `conary-test`.

## Current Facts

Repo facts, verified on 2026-05-24:

- The previous implementation baseline was clean and pushed at `4c3c89ee`
  before this design work began.
- `Cargo.toml` has workspace requirement `rmcp = "1.7.0"`.
- `Cargo.lock` resolves the workspace to `rmcp 1.7.0`.
- `crates/conary-mcp::stateless` defines the current draft protocol token,
  standard header names, request validation, `DiscoverResult`,
  `UnsupportedProtocolVersion`, and `CacheableResult`.
- Remi and `conary-test` still use the session-era `rmcp` server path.
- Draft stateless identifiers do not appear in live Remi or `conary-test`
  route files.

External facts, refreshed on 2026-05-24:

- The MCP draft discovery page says servers must implement `server/discover`
  and shows `DRAFT-2026-v1` in the request `_meta` and response
  `supportedVersions`.
- The MCP draft transport page says every Streamable HTTP message is a new
  `POST`, `Accept` must include both `application/json` and
  `text/event-stream`, and every POST must include request metadata headers.
- The same transport page requires `MCP-Protocol-Version`, `Mcp-Method`, and
  conditional `Mcp-Name`; it also requires servers to validate `Origin` to
  prevent DNS rebinding.
- `cargo search rmcp --limit 3` and `cargo info rmcp` report `rmcp 1.7.0` as
  current.
- Public `rmcp 1.7.0` docs still document session-era types such as
  `LocalSessionManager` and `StreamableHttpService`.

Primary references:

- <https://modelcontextprotocol.io/specification/draft/server/discover>
- <https://modelcontextprotocol.io/specification/draft/basic/transports>
- <https://docs.rs/crate/rmcp/latest>
- <https://docs.rs/rmcp/latest/rmcp/transport/streamable_http_server/session/local/struct.LocalSessionManager.html>
- <https://docs.rs/rmcp/latest/rmcp/transport/streamable_http_server/tower/struct.StreamableHttpService.html>

## Decision

Build a **non-live raw HTTP adapter proof** in `crates/conary-mcp`.

The proof should not depend on `rmcp`, `axum`, or live app route wiring. It
should use a small framework-neutral request/response model so later Remi or
`conary-test` routes can adapt their HTTP framework request into this proof
layer without changing protocol behavior.

The only successful RPC method in this slice is `server/discover`. Every other
validated method returns JSON-RPC `Method not found` with HTTP `404`. This
proves the draft transport and negotiation shape before Conary exposes real
resources, prompts, or tools.

## Scope

In scope:

- Add a raw stateless HTTP proof module at
  `crates/conary-mcp/src/stateless_http.rs`.
- Keep the proof module independent from `rmcp` and session-era types.
- Define a framework-neutral request type with:
  - HTTP method string,
  - headers,
  - JSON body.
- Define a framework-neutral response type with:
  - HTTP status code,
  - content type,
  - optional JSON body.
- Define an `OriginPolicy` that accepts missing `Origin` for non-browser
  clients, accepts exact configured origins, and rejects invalid present
  origins with HTTP `403`.
- Build request headers into the existing `StatelessRequestHeaders` type and
  reuse `validate_stateless_request`.
- Handle `server/discover` by returning a JSON-RPC success response containing
  the existing `DiscoverResult` type.
- For this proof, return an empty capabilities object from `server/discover`.
  Do not advertise `tools`, `resources`, or `prompts` until a live slice
  actually exposes those features.
- Convert `StatelessProtocolError` into JSON-RPC error responses with draft
  numeric codes and useful `data` for unsupported protocol versions.
- Return HTTP `400` for invalid protocol/header/metadata requests.
- Return HTTP `404` with JSON-RPC `-32601` for unsupported validated RPC
  methods.
- Return HTTP `405` for non-`POST` requests.
- Update `docs/operations/agent-mcp-adapter-decision.md` to record that the
  raw adapter proof is the selected next slice.
- Add guard tests proving the proof remains non-live and does not touch Remi or
  `conary-test` route files.

Out of scope:

- No live Remi route changes.
- No live `conary-test` route changes.
- No live MCP resources, prompts, tools, or discovery endpoint.
- No `axum::Router` or server binding.
- No SSE streaming implementation.
- No subscriptions, progress notifications, cancellation, MRTR, or
  `input_required` handling.
- No `x-mcp-header` custom tool parameter mirroring.
- No authentication design beyond exact `Origin` validation.
- No provider-specific SDK integration.

## Architecture

`crates/conary-agent-contract` remains the product contract.

`crates/conary-mcp::stateless` remains the protocol-shape harness: constants,
header validation, discovery payloads, cacheable results, and typed protocol
errors.

The new raw proof module is a thin orchestration layer. It owns HTTP status
mapping, JSON-RPC response wrapping, origin checks, and dispatch for the tiny
method set in scope. It should call into `stateless` instead of duplicating
header or `_meta` validation logic.

The live app servers remain unchanged. A future live adapter slice can map
`axum` request parts into the framework-neutral request type and can map the
proof response back to `axum::response::Response`.

## Data Flow

1. A test constructs a raw stateless HTTP request with method, headers, and
   JSON-RPC body.
2. The proof layer validates HTTP method and `Origin`.
3. The proof layer validates that the body is a single JSON-RPC 2.0 request
   object with an `id` and string `method`.
4. The proof layer extracts MCP standard headers and `Accept` values into
   `StatelessRequestHeaders`.
5. The existing `validate_stateless_request` function validates MCP header/body
   consistency and per-request `_meta`.
6. If the JSON-RPC method is `server/discover`, the proof layer returns
   `DiscoverResult` inside a JSON-RPC success envelope.
7. If the method is validated but unsupported, the proof layer returns HTTP
   `404` with JSON-RPC `Method not found`.
8. If protocol validation fails, the proof layer returns HTTP `400` or `403`
   with a JSON-RPC error envelope.

## Header Handling

The framework-neutral request type should store headers as pairs, not as a map
that silently drops duplicates. Header extraction must follow the HTTP and MCP
draft rules:

- Header names are case-insensitive.
- Header values are case-sensitive unless the relevant HTTP field defines
  otherwise.
- Repeated `Accept` headers and comma-separated `Accept` values both count.
- `Accept` media types should ignore parameters and quality values, so
  `application/json; charset=utf-8` and `text/event-stream;q=0.9` satisfy the
  MCP requirements.
- Standard MCP routing headers should be trimmed for surrounding optional
  whitespace before comparison, but their inner values remain case-sensitive.
- The implementation plan must include tests for lowercase header names,
  duplicate `Accept` headers, and one comma-separated `Accept:
  application/json, text/event-stream` header.

This header extraction layer feeds normalized values into
`StatelessRequestHeaders`; it should not duplicate the deeper MCP body/header
validation already owned by `validate_stateless_request`.

## JSON-RPC Envelope Policy

This proof accepts only a single JSON-RPC 2.0 request object. The body is
already parsed as `serde_json::Value`; invalid JSON parsing belongs to the
future framework adapter that constructs this value.

Accepted request envelope:

- body is a JSON object
- `jsonrpc` is exactly `"2.0"`
- `id` is present
- `method` is a string

Rejected for this proof:

- batch arrays
- notifications with no `id`
- JSON-RPC response objects
- non-object JSON values
- invalid or missing `jsonrpc`
- missing or non-string `method`

Rejected envelope shapes return HTTP `400` with JSON-RPC `-32600`
(`Invalid Request`). Use the request `id` when a scalar `id` is present in the
object; otherwise use `id: null`.

## Error Model

The raw proof should use explicit HTTP status constants or a small enum, but it
does not need the `http` crate for this slice.

Required mappings:

| Condition | HTTP status | JSON-RPC code | Notes |
| --- | --- | --- | --- |
| valid `server/discover` | `200` | none | JSON-RPC `result` contains `DiscoverResult`. |
| invalid present `Origin` | `403` | `-32000` | Response body uses `id: null` because origin is rejected before trusting request metadata. |
| non-`POST` method | `405` | `-32000` | This is an HTTP transport failure before MCP validation. Preserve request id only if the parsed body has a scalar id. |
| malformed JSON-RPC envelope | `400` | `-32600` | Single request objects with `jsonrpc: "2.0"`, present `id`, and string `method` are the only accepted shape. |
| missing or mismatched MCP header | `400` | `-32001` | Use `StatelessProtocolError::json_rpc_error_code()`. |
| unsupported protocol version | `400` | `-32004` | Error `data` includes `{ requested, supported }`. |
| missing `_meta` fields | `400` | `-32602` | Preserve current `StatelessProtocolError` distinction. |
| unsupported validated RPC method | `404` | `-32601` | Used for `tools/list`, `resources/read`, etc. until live dispatch exists. |

The JSON-RPC response should preserve the request id when one is present and
use `null` when the id is absent or cannot be read.

## Testing

The implementation plan must use TDD and focused tests in `crates/conary-mcp`.
Required coverage:

- valid `server/discover` POST returns HTTP `200`, content type
  `application/json`, request id, and `DiscoverResult`
- `server/discover` returns empty capabilities and does not advertise `tools`,
  `resources`, or `prompts`
- invalid present `Origin` returns HTTP `403`
- missing `Origin` is accepted by the configured local/non-browser policy
- non-matching configured exact `Origin` returns HTTP `403`
- non-`POST` request returns HTTP `405`
- lowercase MCP header names validate the same as canonical header names
- comma-separated and repeated `Accept` headers are parsed correctly
- `Accept` media-type parameters and quality values do not prevent matching
- batch arrays, notifications, response objects, non-object JSON values, invalid
  `jsonrpc`, and missing `method` return HTTP `400` with JSON-RPC code `-32600`
- missing `MCP-Protocol-Version` returns HTTP `400` with JSON-RPC code
  `-32001`
- malformed standard MCP header values return HTTP `400` with JSON-RPC code
  `-32001`
- unsupported protocol version returns HTTP `400` with JSON-RPC code `-32004`
  and `data.requested` / `data.supported`
- missing `_meta` fields return HTTP `400` with JSON-RPC code `-32602`
- mismatched `Mcp-Method` returns HTTP `400` with JSON-RPC code `-32001`
- unsupported validated method returns HTTP `404` with JSON-RPC code `-32601`
- the raw proof module source does not import `rmcp`, `RoleServer`,
  `ServerHandler`, `StreamableHttpService`, `LocalSessionManager`,
  `Mcp-Session-Id`, or `axum`
- live Remi and `conary-test` route files still do not contain
  `server/discover`, `MCP-Protocol-Version`, `Mcp-Method`, or `Mcp-Name`

Final verification should include:

- `cargo fmt --check`
- `cargo test -p conary-mcp`
- `cargo test -p conary-agent-contract`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `rg -n "server/discover|MCP-Protocol-Version|Mcp-Method|Mcp-Name" apps/remi/src/server apps/conary-test/src/server`

`Origin` rejection and non-`POST` requests are HTTP transport gates rather
than MCP protocol validation failures. They may both use JSON-RPC `-32000`
because the HTTP status code disambiguates `403` from `405`.

## Acceptance Criteria

- A new implementation plan exists at
  `docs/superpowers/plans/archive/2026-05-24-stateless-raw-http-adapter-proof.md`.
- The plan is suitable for Codex `/goal` and has one focused commit per task.
- The proof handles `server/discover` only; it does not expose real
  resources, prompts, tools, routes, or app registrations.
- The proof's `server/discover` response does not advertise `tools`,
  `resources`, or `prompts`.
- The proof converts existing stateless validation errors into HTTP and
  JSON-RPC response shapes without string parsing.
- The decision record names raw HTTP proof as the selected adapter-gate slice.
- Guard tests prove no live MCP route files were touched.

## Risks

- The MCP draft can still change before the 2026-07-28 release candidate. Keep
  the proof isolated in `crates/conary-mcp` so token/header/status changes have
  a small blast radius.
- A pure framework-neutral proof is not a full HTTP server. This is deliberate:
  it proves protocol behavior without forcing Remi or `conary-test` route
  migration too early.
- `Origin` policy can become subtle when deployed behind proxies. This slice
  only proves exact origin validation; proxy-aware origin policy belongs in the
  first live adapter slice.
- `rmcp` may add stateless support later. If it does, this proof becomes the
  acceptance harness for deciding whether Conary can use that SDK support.

## Follow-On

After this proof passes, the next slice can choose one of two paths:

1. Map a single live local-only route to the raw proof and expose only
   `server/discover`.
2. If `rmcp` gains stateless support first, use this proof as the conformance
   harness for adopting that support.

Only after one of those paths passes should Conary add the first read-only MCP
resource.
