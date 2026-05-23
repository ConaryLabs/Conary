---
last_updated: 2026-05-22
revision: 1
summary: Decision record for Conary's first stateless MCP adapter path
---

# Agent MCP Adapter Decision

## Decision

The first LLM-native operations milestone remains contract-only plus
inventory/prune. Conary will not add new live MCP resources, tools, prompts, or
discovery behavior on the existing session-based `rmcp` path.

## Current State

- Workspace requirement: `rmcp = "1.1"` in `Cargo.toml`
- Resolved dependency: `rmcp 1.6.0` in `Cargo.lock`
- Current Remi and conary-test wiring uses `RoleServer`, `ServerHandler`,
  `StreamableHttpService`, and `LocalSessionManager`
- Current live MCP surfaces are tool-only from Conary's product perspective
- Local source inspection on 2026-05-22 shows `rmcp 1.6.0` does not implement
  the target stateless MCP draft; it still uses `initialize`,
  `Mcp-Session-Id`, and session-manager based Streamable HTTP code
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
