---
last_updated: 2026-05-24
revision: 1
summary: Design for adding the first conary-test stateless MCP read-only resource, conary-local://bootstrap/status
---

# Conary-Test Bootstrap Status Stateless Resource: Design Spec

**Date:** 2026-05-24
**Status:** Draft for review
**Goal:** Add the first live read-only resource to the conary-test stateless
MCP preview route: `conary-local://bootstrap/status`.

---

## Purpose

The previous slice proved that `conary-test` can expose a live stateless MCP
route at `POST /mcp/stateless` without depending on the legacy `rmcp` session
path. That route currently handles only `server/discover` and advertises no
tools, resources, or prompts.

This slice should prove the next primitive: an assistant can inspect useful
read-only state before taking action. The first resource is deliberately local,
cheap, and non-mutating:

```text
conary-local://bootstrap/status
```

It returns the existing local developer bootstrap inspection result from
`apps/conary-test/src/bootstrap.rs`. The resource is for developer orientation:
which prerequisites exist, whether manifests/config parse, whether container
smoke validation is ready, and which warnings remain.

## External Facts

MCP draft facts refreshed on 2026-05-24:

- Servers that support resources declare a `resources` capability.
- `resources/list` returns resource descriptors and supports pagination.
- `resources/read` returns one or more resource content blocks.
- Resource descriptors include `uri`, `name`, optional `title`,
  `description`, and `mimeType`.
- Text resource content uses `uri`, `mimeType`, and `text`.
- Cache hints are required on `resources/list` and `resources/read` results.
- Cache hints use `ttlMs` and `cacheScope`.
- Current draft resource-not-found errors use JSON-RPC `-32602` Invalid
  Params, with older `-32002` treated as backwards-compatible client input.
- `resources/read` requires `Mcp-Name` to match `params.uri` under the current
  stateless transport header rules.

Primary references:

- <https://modelcontextprotocol.io/specification/draft/server/resources>
- <https://modelcontextprotocol.io/specification/draft/server/utilities/caching>
- <https://modelcontextprotocol.io/specification/draft/basic/transports>

## Current Repo Facts

- `crates/conary-mcp::stateless` already validates `resources/read` with a
  matching `Mcp-Name` / `params.uri` pair.
- `crates/conary-mcp::stateless::CacheableResult<T>` already serializes
  `resultType`, `ttlMs`, and `cacheScope`.
- `crates/conary-agent-contract::CachePolicy::private_short()` serializes to
  `ttlMs = 30000` and `cacheScope = "private"`.
- `crates/conary-agent-contract::local_bootstrap_status()` returns
  `conary-local://bootstrap/status`.
- `apps/conary-test::bootstrap::inspect_default()` returns an `InspectResult`
  whose subject is `conary-local://bootstrap/status`.
- `apps/conary-test/src/server/stateless_mcp.rs` adapts Axum requests into
  `crates/conary-mcp::stateless_http`.
- `apps/conary-test/src/server/routes.rs` mounts `/mcp/stateless` inside the
  same auth boundary as the legacy `/mcp` route.
- Remi route and MCP server files are guarded against draft stateless
  identifiers.

## Decision

Add `resources/list` and `resources/read` support to the raw stateless adapter
with exactly one live resource provider in `conary-test`.

The live route remains:

```text
POST /mcp/stateless
```

Successful methods after this slice:

- `server/discover`
- `resources/list`
- `resources/read` for `conary-local://bootstrap/status`

Everything else remains unsupported. This slice does not add resource
templates, subscriptions, prompts, tools, SSE streaming, Remi routes, or
mutation behavior.

## Discovery

`server/discover` for the conary-test stateless route should advertise resource
support and no optional resource features:

```json
{
  "capabilities": {
    "resources": {}
  }
}
```

It must still not advertise:

- `tools`
- `prompts`
- `resources.subscribe`
- `resources.listChanged`

The default non-live `conary-mcp` proof may keep empty capabilities unless a
resource provider is passed into the raw adapter.

## Resource List Shape

`resources/list` should return exactly one resource:

```json
{
  "jsonrpc": "2.0",
  "id": "list-1",
  "result": {
    "resultType": "complete",
    "resources": [
      {
        "uri": "conary-local://bootstrap/status",
        "name": "bootstrap_status",
        "title": "Local Bootstrap Status",
        "description": "Read local developer bootstrap prerequisites and smoke-readiness state",
        "mimeType": "application/json"
      }
    ],
    "ttlMs": 30000,
    "cacheScope": "private"
  }
}
```

No `nextCursor` is required for this first slice because the list is one item
and not paginated. If a client sends a cursor, this slice should ignore it and
return the same stable one-item list.

## Resource Read Shape

`resources/read` for `conary-local://bootstrap/status` should return one text
content block:

```json
{
  "jsonrpc": "2.0",
  "id": "read-1",
  "result": {
    "resultType": "complete",
    "contents": [
      {
        "uri": "conary-local://bootstrap/status",
        "mimeType": "application/json",
        "text": "{ ... pretty JSON InspectResult ... }"
      }
    ],
    "ttlMs": 30000,
    "cacheScope": "private"
  }
}
```

The `text` field should be valid JSON text produced by serializing the existing
`InspectResult`. The contained result may report `ok`, `partial`,
`unavailable`, or `failed` through the Conary operation envelope. Those are
resource state values, not MCP transport failures.

The resource content should not wrap the `InspectResult` in an extra Conary
object. The MCP response envelope already provides the protocol wrapper.

## Architecture

Use `crates/conary-mcp` for MCP protocol/result modeling and
`apps/conary-test` for product data.

Recommended implementation split:

- `crates/conary-mcp::stateless`
  - Add serializable MCP resource descriptor/content/result structs.
  - Reuse `CacheableResult<T>` for cacheable `resources/list` and
    `resources/read` results.

- `crates/conary-mcp::stateless_http`
  - Add a small resource-provider interface.
  - Keep existing `handle_stateless_http_request` behavior unchanged for
    callers that do not pass resources.
  - Add a resource-aware request handler used by conary-test.
  - Map `resources/list` and `resources/read`.
  - Keep all `rmcp` and `axum` dependencies out of this crate.

- `apps/conary-test/src/server/stateless_mcp.rs`
  - Provide the single bootstrap-status resource provider.
  - Read bootstrap state by calling `crate::bootstrap::inspect_default()`.
  - Pass the provider into the resource-aware raw handler.
  - Keep Axum-specific request/response conversion here only.

Avoid putting protocol dispatch in `routes.rs`; it should continue to mount the
adapter and nothing else.

## Provider Contract

The raw adapter needs a tiny, read-only contract with these names:

```rust
pub trait StatelessResourceProvider {
    fn list_resources(&self) -> Vec<ResourceDescriptor>;
    fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, ResourceReadError>;
}
```

The provider should return data, not JSON-RPC envelopes. The raw adapter owns
JSON-RPC IDs, HTTP status codes, cache wrappers, and error mapping.

For this slice, the only provider implementation is a conary-test bootstrap
provider with one resource. It should not reach into Remi, run smoke tests,
start containers, or perform mutations.

## Error Model

Preserve existing stateless HTTP behavior for method, origin, JSON parse,
JSON-RPC envelope, header, protocol-version, and `_meta` errors.

New resource-specific behavior:

| Condition | HTTP status | JSON-RPC code | Notes |
| --- | --- | --- | --- |
| valid `resources/list` | `200` | none | Returns one-item resource list with private cache hints. |
| valid bootstrap `resources/read` | `200` | none | Returns one JSON text content block. |
| unknown resource URI | `404` | `-32602` | Current draft resource-not-found shape; include `{ "uri": ... }` in `error.data`. |
| missing `params.uri` for `resources/read` | `400` | `-32001` | Existing `Mcp-Name` / required-name validation path. |
| missing or mismatched `Mcp-Name` for `resources/read` | `400` | `-32001` | Existing header mismatch path. |
| valid but unsupported resource method | `404` | `-32601` | Examples: `resources/templates/list`, subscriptions/listen. |
| bootstrap inspection reports unavailable | `200` | none | The resource exists; unavailable state is inside the JSON content. |

Do not return an empty `contents` array for an unknown resource.

## Auth And Privacy

The route remains inside conary-test's existing auth middleware:

- with no configured token, local non-browser clients can read the resource
- with a configured token, missing/wrong bearer token returns existing HTTP
  `401` plain JSON before the MCP handler runs

Cache scope must be `private` because bootstrap status can include local paths,
available toolchain state, and environment-selected manifest/config paths. It
must not be cached across authorization contexts.

The resource is read-only but local-machine specific. It should not include
secrets or credential values. Existing bootstrap inspection already reports
paths, booleans, manifest summaries, warnings, and command evidence; this is
acceptable for the local developer surface.

## Out Of Scope

- Remi resources
- conary-test suite resources
- run/artifact/log resources
- resource templates
- resource subscriptions or list-changed notifications
- live MCP tools
- live MCP prompts
- SSE streaming
- mutation or smoke execution
- provider-native SDK integration
- changing `/mcp` legacy behavior
- changing conary-test auth or bind address

## Testing

The implementation plan must use TDD.

Required `conary-mcp` coverage:

- resource list result serializes `resultType`, `resources`, `ttlMs`, and
  `cacheScope`
- resource read result serializes JSON text content with matching `uri` and
  `mimeType`
- default raw handler without a provider still does not advertise resources
- resource-aware handler advertises `capabilities.resources = {}`
- resource-aware handler returns method-not-found if no resource provider is
  available
- `resources/list` returns cache hints
- `resources/read` requires matching `Mcp-Name`
- unknown resource URI returns HTTP `404`, JSON-RPC `-32602`, and `error.data.uri`
- unsupported resource methods remain method-not-found
- stateless modules still do not import `rmcp`, `axum`, or session types

Required `conary-test` coverage:

- `server/discover` on `/mcp/stateless` advertises resources and no tools or
  prompts
- `resources/list` returns exactly `conary-local://bootstrap/status`
- `resources/read` returns one `application/json` text content block
- resource text parses as JSON and contains:
  - `operation = "conary-test.bootstrap.inspect"`
  - `risk = "read_only"`
  - subject URI `conary-local://bootstrap/status`
- unknown resource URI returns the specified resource-not-found error
- missing or mismatched `Mcp-Name` fails before dispatch
- token auth still gates `/mcp/stateless`
- legacy `/mcp` still does not return stateless resource responses

Required docs checks:

- `apps/conary-test/README.md` documents that `/mcp/stateless` now exposes
  discovery plus exactly one read-only bootstrap-status resource.
- `docs/operations/agent-mcp-adapter-decision.md` records this as the first
  live read-only resource slice.
- `docs/operations/infrastructure.md` no longer says the conary-test stateless
  route exposes only discovery after the implementation lands.
- Documentation audit inventory and ledger are reconciled.

## Acceptance Criteria

- `/mcp/stateless` supports `server/discover`, `resources/list`, and
  `resources/read` for `conary-local://bootstrap/status`.
- Discovery advertises `resources: {}` and no tools/prompts.
- `resources/list` returns one resource and private cache hints.
- `resources/read` returns valid JSON text for the existing bootstrap
  `InspectResult`.
- Unknown resource URI returns current draft resource-not-found behavior:
  HTTP `404` with JSON-RPC `-32602`.
- Existing legacy `/mcp` behavior remains unchanged.
- Remi remains untouched.
- Final verification passes:
  - `cargo fmt --check`
  - `cargo test -p conary-mcp`
  - `cargo test -p conary-test stateless`
  - `cargo test -p conary-test bootstrap`
  - `cargo test -p conary-test mcp_endpoint_requires_token`
  - `cargo run -p conary-test -- bootstrap check --json`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - docs audit ledger check with `--require-complete`

## Follow-On Slices

After this slice, the next resource candidates are:

- `conary-test://suites`
- `conary-test://runs/{run_id}`
- `conary-test://runs/{run_id}/artifacts/{artifact_id}`

Prompts should remain deferred until at least the bootstrap resource and one
test-run evidence resource exist.
