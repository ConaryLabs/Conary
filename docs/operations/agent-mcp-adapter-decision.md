---
last_updated: 2026-08-09
revision: 12
summary: Decision record for Remi's modern stateless MCP adapter and the framework-neutral compliance proof
---

# Agent MCP Adapter Decision

## Decision

The live MCP surface is Remi's modern-only stateless Streamable HTTP `/mcp`.
`conary-test` is a local CLI and integration-test engine; its former network
server and both former MCP adapters were removed in issue #351. The
transport-neutral operation contract and framework-neutral MCP compliance
harness remain in their owning crates.

## Current State

- Workspace `rmcp = "3.1.2"` supports Remi's modern-only stateless Streamable
  HTTP implementation; the authoritative product description is
  `docs/modules/remi.md`.
- `crates/conary-mcp::stateless` contains the non-live compliance harness for
  request validation, discovery result modeling, and cacheable result modeling.
- `crates/conary-mcp::stateless_http` contains framework-neutral raw HTTP
  adapter support for discovery, resource list/read dispatch, origin
  validation, JSON-RPC envelope validation, header extraction, protocol error
  mapping, and unsupported-method responses.
- The conary-test engine retains its Remi test-data client and local WAL result
  buffer for the planned streaming path. The local CLI currently does not
  construct that path or stream results; issue #354 tracks the wiring. It does
  not bind a network listener or register MCP tools, resources, prompts, or
  routes.
- The raw HTTP adapter support does not mount routes, bind sockets, register
  product resources, register tools, register prompts, or depend on `rmcp` or
  `axum`.

## Target

Keep the current Remi MCP protocol direction associated with protocol revision
`2026-07-28`. Any future live MCP surface must be owned by the service that
actually provides the operation and must use the shared agent contract as its
durable vocabulary.

## Adapter Gate

Before adding another live MCP registration, implementation must prove one of
these paths:

1. `rmcp` supports the target protocol features needed by the owning service.
2. A thin raw HTTP adapter can implement the target protocol with tests for:
   - per-request `POST`
   - `Accept`
   - `MCP-Protocol-Version`
   - `Mcp-Method`
   - `Mcp-Name`
   - per-request `_meta`
   - `Origin` validation
   - discovery
   - cache metadata before the first live list/read response is exposed

## Current Choice

Do not recreate a conary-test network or MCP adapter. Remi owns the live
`/mcp` service surface, while `crates/conary-agent-contract` owns the
transport-neutral operation vocabulary and `crates/conary-mcp` remains adapter
glue plus compliance proof.

## Harness Slice

The stateless MCP work remains a framework-neutral compliance harness in
`crates/conary-mcp`. It validates draft-shaped requests, discovery, cacheable
results, raw HTTP policy, and adapter boundaries without adding a live route or
service listener. Its current evidence lives in
`crates/conary-mcp/src/stateless.rs`,
`crates/conary-mcp/src/stateless_http.rs`,
`crates/conary-mcp/tests/stateless_dependency_boundary.rs`, and the owning
crate tests.

## Conary-Test Server Cut

Issue #351 removed the conary-test HTTP orchestration layer, session-era MCP
adapter, stateless MCP prior-art adapter, and `serve` CLI entry point. The
surviving CLI commands, run engine, Remi test-data client, retained WAL path,
manifests, fixtures, and image/deploy tooling remain local-owned surfaces. The
result-streaming path is currently unconstructed for local runs; issue #354
tracks its wiring.
