---
last_updated: 2026-05-23
revision: 2
summary: Follow-on design for a stateless MCP adapter compliance harness before live MCP expansion, with final review fixes
---

# Stateless MCP Adapter Compliance Harness: Design Spec

**Date:** 2026-05-22
**Status:** Approved design for planning; implementation not started
**Goal:** Define the next bounded LLM-native operations slice after the
transport-neutral contract and local bootstrap milestones: a compliance harness
for Conary's future stateless MCP adapter.

---

## Purpose

Conary should continue targeting MCP as the first outward-facing agent
transport, but the next slice should not add more live behavior to the current
session-based `rmcp` path. The durable work now is to make the target adapter
shape testable: current crate dependency state, headers, per-request metadata,
`server/discover`, cache hints, and result envelopes should be represented in
Conary code before any Remi or `conary-test` resource prototype depends on
them.

This slice is intentionally small. It prepares Conary for the MCP draft
stateless model while preserving the earlier decision that
`crates/conary-agent-contract` is the product contract and MCP is an adapter.

## Current Facts

Local repo facts, verified on 2026-05-22:

- `Cargo.toml` still has the workspace requirement `rmcp = "1.1"`.
- `Cargo.lock` resolves the local workspace to `rmcp 1.6.0`.
- Remi and `conary-test` still use `RoleServer`, `ServerHandler`,
  `StreamableHttpService`, and `LocalSessionManager`.
- `server/discover`, `MCP-Protocol-Version`, `Mcp-Method`, and `Mcp-Name`
  appear only in docs/specs/decisions, not in live application code.
- `ttlMs` and `cacheScope` exist only in
  `crates/conary-agent-contract/src/catalog.rs` as catalog metadata.

External MCP facts, refreshed on 2026-05-22:

- The current MCP draft changelog removes protocol-level sessions and the
  `initialize` / `notifications/initialized` handshake, adds
  `server/discover`, moves protocol/client metadata into per-request `_meta`,
  and requires routing-friendly HTTP request headers.
- The draft docs currently use `DRAFT-2026-v1` as the protocol-version example.
  Treat that as the current draft token, not a durable Conary contract.
- The draft Streamable HTTP transport requires standard `Mcp-Method` and
  conditional `Mcp-Name` headers mirrored from the JSON-RPC request body.
- Draft cache metadata requires `ttlMs` and `cacheScope` on list and resource
  read results.
- Public `rmcp` documentation currently lists `rmcp 1.7.0` as latest, but still
  documents session/initialize-era pieces such as `LocalSessionManager` and
  `InitializeResult`. Conary should still move to the latest compatible crate
  version during active development; the upgrade is dependency hygiene, not a
  stateless adapter by itself.

Primary references:

- <https://modelcontextprotocol.io/specification/draft/changelog>
- <https://modelcontextprotocol.io/specification/draft/server/discover>
- <https://modelcontextprotocol.io/specification/draft/basic/transports>
- <https://modelcontextprotocol.io/specification/draft/server/utilities/caching>
- <https://modelcontextprotocol.io/seps/2575-stateless-mcp>
- <https://docs.rs/crate/rmcp/latest>
- <https://docs.rs/rmcp/latest/rmcp/transport/streamable_http_server/session/local/struct.LocalSessionManager.html>

## Decision

The next implementation slice is **Stateless MCP Adapter Compliance Harness**.

It will first move Conary's `rmcp` dependency to the latest compatible crate
version. As of this spec, that target is `rmcp 1.7.0`; if crates.io has a newer
compatible release when implementation starts, use the newer release and record
the evidence in the completed plan.

After the dependency update, the slice will add draft-shaped, unit-tested
compliance helpers under `crates/conary-mcp` without registering new live MCP
resources, tools, prompts, routes, or discovery behavior. The helpers define
the adapter boundary that a future raw HTTP adapter or future `rmcp` stateless
support must satisfy.

## Scope

In scope:

- Move the workspace to the latest compatible `rmcp` crate version at the start
  of the slice. As of this spec, that target is `rmcp 1.7.0`.
- Create a `crates/conary-mcp::stateless` module with no dependency on `rmcp`
  types.
- Validate draft-shaped HTTP request requirements:
  - `Accept` includes `application/json` and `text/event-stream`.
  - `MCP-Protocol-Version` is present and matches
    `_meta.io.modelcontextprotocol/protocolVersion`.
  - `Mcp-Method` is present and matches the JSON-RPC `method`.
  - `Mcp-Name` is required for `tools/call`, `resources/read`, and
    `prompts/get`, and matches the corresponding body field.
  - per-request `_meta` includes protocol version, client info, and client
    capabilities.
- Define draft-shaped `server/discover` response structs and unsupported
  protocol-version error payloads.
- Define cacheable result wrappers for list/read results using the existing
  `CachePolicy` type.
- Add guard tests that keep this module independent from `rmcp` session types.
- Update docs to make clear this is a harness and target mapping, not live
  behavior.

Out of scope:

- No live Remi MCP resource/tool/prompt registration.
- No live `conary-test` MCP resource/tool/prompt registration.
- No route replacement in Remi or `conary-test`.
- No assumption that upgrading `rmcp` creates draft-stateless behavior. The
  dependency should be current, but the compliance harness remains the adapter
  gate.
- No raw HTTP adapter serving real requests.
- No OpenAPI generation.
- No provider-specific tool-calling integration.

## Architecture

`crates/conary-agent-contract` remains the transport-neutral product contract:
operation results, resource references, risk labels, confirmation semantics,
and catalog cache metadata live there.

`crates/conary-mcp` remains the adapter crate. The new `stateless` module is a
draft compliance model, not a server. It owns MCP-edge concepts such as headers,
per-request metadata, discovery payloads, cacheable result wrappers, and
protocol-level errors. It should serialize to the target MCP draft shapes, but
it should not call `rmcp`, `RoleServer`, `ServerHandler`, or
`LocalSessionManager`.

The current Remi and `conary-test` MCP servers remain unchanged in this slice.
They continue to run on the existing session-based transport until a later
adapter decision replaces or upgrades that path.

## Data Flow

Future adapter data flow:

1. A client sends a JSON-RPC request over Streamable HTTP.
2. The adapter validates standard headers and per-request `_meta`.
3. The adapter handles `server/discover` directly or dispatches a valid
   resource/tool/prompt request to Conary operation code.
4. Conary returns transport-neutral contract results.
5. The adapter maps contract results into MCP structured content with cache
   hints and result types appropriate to the draft.

This slice implements only the validation and serialization pieces needed to
test that future flow.

## Error Handling

Protocol errors should be represented separately from Conary domain errors.

- Missing or mismatched headers, missing per-request metadata, unsupported
  protocol versions, and unsupported methods are adapter/protocol failures.
- Missing prerequisites, validation failures, remote unavailability, and unsafe
  confirmations remain `conary-agent-contract` domain outcomes.

The compliance module should expose typed errors with stable machine-readable
codes so a later raw HTTP adapter can translate them into JSON-RPC errors and
HTTP statuses without string parsing.

Protocol error codes from the current MCP draft:

| Error code | Name | Use |
| --- | --- | --- |
| `-32001` | `HeaderMismatch` | Required MCP request headers are missing or malformed, or header values do not match the JSON-RPC request body. |
| `-32004` | `UnsupportedProtocolVersion` | The server does not support the requested MCP protocol version. |

Missing per-request `_meta` fields are malformed request parameters and should
remain distinguishable from header validation failures.

`DiscoverResult` and `CacheableResult` in this slice return
`resultType: "complete"`. Other draft result types, especially
`input_required` from MRTR / SEP-2322, are out of scope for this harness slice
but should not be precluded by the type design.

`CacheableResult` uses `#[serde(flatten)]` to merge `CachePolicy` fields into
the result. The contract crate's `CachePolicy` serde renames (`ttlMs`,
`cacheScope`) are intentionally the MCP draft field names. If the draft changes
these names before the release candidate, update both the contract and adapter
tests together.

## Testing

The implementation plan must use TDD and focused tests in `crates/conary-mcp`.
Required coverage:

- `rmcp` is updated to the latest compatible crate version before the harness
  is added
- valid draft-shaped `tools/list`, `resources/read`, `tools/call`, and
  `server/discover` requests pass validation
- missing `Accept`, `MCP-Protocol-Version`, `Mcp-Method`, required `Mcp-Name`,
  or per-request `_meta` fails with typed errors
- mismatched header/body method, name, or protocol version fails with typed
  errors
- header/protocol errors expose the current draft JSON-RPC numeric error codes
- `server/discover` serializes `resultType: "complete"`, supported versions,
  capabilities, server info, and optional instructions
- unsupported protocol version errors include supported and requested versions
- cacheable wrappers serialize `ttlMs` and `cacheScope`
- the stateless module does not import or depend on `rmcp`, `RoleServer`,
  `ServerHandler`, `LocalSessionManager`, or `Mcp-Session-Id`

## Acceptance Criteria

- A new plan exists at
  `docs/superpowers/plans/2026-05-22-stateless-mcp-adapter-compliance.md`.
- The implementation plan starts by moving `rmcp` to the latest compatible
  crate version, expected to be `1.7.0` as of this spec.
- The implementation plan adds only compliance helpers and tests; it does not
  add live MCP behavior.
- `docs/operations/agent-mcp-adapter-decision.md` records that the next slice
  is a dependency-current compliance harness, not a live adapter.
- The plan's final verification includes `cargo fmt --check`,
  `cargo test -p conary-mcp`, `cargo test -p conary-agent-contract`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and grep checks
  proving no new live MCP registration paths were touched.
- The plan is suitable for Codex `/goal`: it has a short goal prompt, precise
  task boundaries, expected failing/passing test commands, and one focused
  commit per task.

## Risks

- The MCP draft can still change before release. Keep all draft assumptions
  isolated in `conary-mcp::stateless` so release token or field changes have a
  small blast radius.
- `rmcp` may add stateless support during or after this work. The harness still
  remains useful: it becomes the acceptance test surface for whether Conary can
  safely consume that support. Conary should keep `rmcp` at the latest
  compatible crate version during active development.
- Source-code guard tests can be brittle. Keep them narrow and focused on the
  stateless module's dependency boundary, not on all live MCP code.
- Overbuilding a raw adapter now would mix protocol correctness with product
  semantics. This slice deliberately stops at the compliance model.

## Follow-On

After this harness passes, the next slice can choose between:

1. raw HTTP adapter proof using the harness,
2. `rmcp` stateless adoption if that support exists,
3. first read-only resource prototype using the harness as its adapter gate.

The preferred follow-on is the smallest path that can expose one read-only
resource without reintroducing session assumptions.
