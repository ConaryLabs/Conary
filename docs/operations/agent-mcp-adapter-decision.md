---
last_updated: 2026-05-24
revision: 4
summary: Decision record for Conary's stateless MCP adapter path, compliance harness, and raw HTTP proof
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
After the contract, catalog, local bootstrap, and compliance harness slices,
the selected adapter-gate slice is a non-live raw HTTP proof in
`crates/conary-mcp`. The proof should handle `server/discover`, draft request
validation, origin checks, HTTP status mapping, and JSON-RPC error envelopes
without mounting routes in Remi or `conary-test`.

The raw HTTP proof should not implement list/read stubs. Cache metadata remains
covered by the non-live stateless harness and is deferred to the first
read-only resource slice before any live list/read response ships.

## Harness Slice

The current implementation slice is the stateless MCP adapter compliance
harness: it moved the workspace to the latest compatible `rmcp` crate version
and added draft-shaped validation, discovery, cacheable-result, and guard tests
in `crates/conary-mcp` without adding live MCP resources, tools, prompts,
routes, or discovery behavior. This keeps Conary aimed at the MCP draft while
avoiding fresh investment in the legacy session path.

Source spec:

- `docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md`

## Raw HTTP Proof Slice

The next implementation slice should prove Conary can satisfy the current MCP
draft stateless HTTP requirements without waiting for `rmcp` support. It should
reuse `crates/conary-mcp::stateless`, add a framework-neutral request/response
adapter proof, and keep Remi and `conary-test` live routes unchanged.

Source spec:

- `docs/superpowers/specs/2026-05-24-stateless-raw-http-adapter-proof-design.md`
