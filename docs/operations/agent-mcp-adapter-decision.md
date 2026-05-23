---
last_updated: 2026-05-23
revision: 3
summary: Decision record for Conary's stateless MCP adapter path and compliance harness implementation
---

# Agent MCP Adapter Decision

## Decision

The first LLM-native operations milestone remains contract-only plus
inventory/prune. Conary will not add new live MCP resources, tools, prompts, or
discovery behavior on the existing session-based `rmcp` path.

## Current State

- Workspace requirement: `rmcp = "1.7.0"` in `Cargo.toml`
- Resolved dependency: `rmcp 1.7.0` in `Cargo.lock`
- Latest public `rmcp` docs checked on 2026-05-23 list `rmcp 1.7.0`, but still
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
   - cache metadata on list/read responses

## Current Choice

Do not build new live MCP registrations in the first milestone. Build the
contract crate, catalog metadata, local bootstrap inspection, and cleanup first.

## Harness Slice

The current implementation slice is the stateless MCP adapter compliance
harness: it moved the workspace to the latest compatible `rmcp` crate version
and added draft-shaped validation, discovery, cacheable-result, and guard tests
in `crates/conary-mcp` without adding live MCP resources, tools, prompts,
routes, or discovery behavior. This keeps Conary aimed at the MCP draft while
avoiding fresh investment in the legacy session path.

Source spec:

- `docs/superpowers/specs/2026-05-22-stateless-mcp-adapter-compliance-design.md`
