---
last_updated: 2026-05-24
revision: 7
summary: Decision record for Conary's stateless MCP adapter path, compliance harness, raw HTTP proof, and conary-test read-only resources
---

# Agent MCP Adapter Decision

## Decision

The first LLM-native operations milestone remains contract-only plus
inventory/prune. Conary will not add new live MCP resources, tools, prompts, or
discovery behavior on the existing session-based `rmcp` path.

## Current State

- Workspace requirement: `rmcp = "1.7.0"` in `Cargo.toml`
- Resolved dependency: `rmcp 1.7.0` in `Cargo.lock`
- Latest public `rmcp` docs checked on 2026-05-24 list `rmcp 1.7.0`, but still
  document session/initialize-era types such as `LocalSessionManager` and
  `InitializeResult`
- Current Remi and conary-test wiring uses `RoleServer`, `ServerHandler`,
  `StreamableHttpService`, and `LocalSessionManager`
- Current live MCP surfaces are tool-only from Conary's product perspective
- `crates/conary-mcp::stateless` contains the non-live compliance harness for
  request validation, discovery result modeling, cacheable result modeling, and
  adapter-boundary guard tests
- `crates/conary-mcp::stateless_http` contains the framework-neutral raw HTTP
  adapter support for `server/discover`, resource list/read dispatch when a
  provider is configured, origin validation, JSON-RPC envelope validation,
  header extraction, protocol error mapping, and unsupported-method responses
- `apps/conary-test` exposes `POST /mcp/stateless` as the first live
  stateless adapter gate. It handles `server/discover`, `resources/list`, and
  `resources/read` for `conary-local://bootstrap/status` and
  `conary-test://suites`, and keeps the legacy `/mcp` session-based tool
  surface unchanged.
- The raw HTTP adapter support does not mount routes, bind sockets, register
  product resources by itself, register tools, register prompts, or depend on
  `rmcp` / `axum`
- The compliance harness does not add live MCP resources, tools, prompts,
  routes, or discovery behavior
- `crates/conary-agent-contract` may define draft-shaped metadata names such as
  `ttlMs` and `cacheScope`, but those are contract/catalog metadata only and do
  not create live MCP list/read behavior

## Target

Target the current MCP draft stateless direction associated with the 2026-07-28
release candidate. The draft docs currently use `DRAFT-2026-v1` as the
protocol-version token; re-verify the final token before live adapter work.

## Adapter Gate

Before new live MCP registration work begins, implementation must prove one of
these paths:

1. `rmcp` supports the target draft features needed by Conary.
2. A thin raw HTTP adapter can implement the target draft with tests for:
   - per-request `POST`
   - `Accept`
   - `MCP-Protocol-Version`
   - `Mcp-Method`
   - `Mcp-Name`
   - per-request `_meta`
   - `Origin` validation
   - `server/discover`
   - cache metadata before the first live list/read response is exposed

## Current Choice

Do not build new live MCP registrations on the existing session-based path.
After the contract, catalog, local bootstrap, compliance harness, non-live raw
proof, and first live resource slices, the selected live adapter-gate surface
is a `conary-test` route at `POST /mcp/stateless`. It exposes
`server/discover` plus read-only `conary-local://bootstrap/status` and
`conary-test://suites` resources, and advertises no tools or prompts.

The raw HTTP adapter supports list/read through an explicit provider trait.
Cache metadata remains modeled through `CacheableResult<T>` and the shared
contract `CachePolicy`; live list/read behavior is added only when a service
wires a provider.

## Harness Slice

The current implementation slice is the stateless MCP adapter compliance
harness: it moved the workspace to the latest compatible `rmcp` crate version
and added draft-shaped validation, discovery, cacheable-result, and guard tests
in `crates/conary-mcp` without adding live MCP resources, tools, prompts,
routes, or discovery behavior. This keeps Conary aimed at the MCP draft while
avoiding fresh investment in the legacy session path.

Source spec:

- `docs/superpowers/specs/archive/2026-05-22-stateless-mcp-adapter-compliance-design.md`

## Raw HTTP Proof Slice

The raw HTTP proof slice proved Conary can satisfy the current MCP draft
stateless HTTP requirements without waiting for `rmcp` support. It reuses
`crates/conary-mcp::stateless`, adds framework-neutral request/response
adapter support, handles `server/discover`, and can dispatch `resources/list`
and `resources/read` through a configured provider.

Source spec:

- `docs/superpowers/specs/archive/2026-05-24-stateless-raw-http-adapter-proof-design.md`

## Conary-Test Stateless Route Slice

The first live stateless adapter gate is `POST /mcp/stateless` in
`conary-test`. It adapts Axum requests into `crates/conary-mcp::stateless_http`,
uses `serverInfo.name = "conary-test-mcp"`, preserves the existing `/mcp`
session-based service, and stays inside the existing conary-test auth boundary
when a token is configured.

This route supports `server/discover`, `resources/list`, and `resources/read`
for `conary-local://bootstrap/status` and `conary-test://suites`. Bootstrap
status returns the existing local bootstrap `InspectResult` as
`application/json` text content. The suites resource returns a static
manifest-inventory `InspectResult` from `AppState.manifest_dir`. It must not
add tools, prompts, resource templates, subscriptions, SSE, smoke execution,
per-suite resources, or Remi route behavior.

Source specs:

- `docs/superpowers/specs/archive/2026-05-24-conary-test-stateless-discover-route-design.md`
- `docs/superpowers/specs/archive/2026-05-24-conary-test-bootstrap-status-resource-design.md`
- `docs/superpowers/specs/archive/2026-05-24-conary-test-suites-resource-design.md`
