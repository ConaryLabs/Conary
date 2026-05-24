---
last_updated: 2026-05-24
revision: 1
summary: Design for adding a static conary-test stateless MCP suites resource, conary-test://suites
---

# Conary-Test Suites Stateless Resource: Design Spec

**Date:** 2026-05-24
**Status:** Reviewed draft for approval
**Goal:** Add one read-only stateless MCP resource to `conary-test`:
`conary-test://suites`.

---

## Purpose

The stateless `conary-test` route now exposes `server/discover`,
`resources/list`, and `resources/read` for
`conary-local://bootstrap/status`. The next useful read-only surface is the
static suite manifest inventory:

```text
conary-test://suites
```

This gives an assistant a cheap way to orient itself before proposing local
validation work. It answers which suite manifests exist, which phase they
belong to, how many test cases they contain, and whether they need container or
QEMU support. It does not start tests, stream logs, expose run artifacts, or
create per-suite MCP resources.

## External Facts

MCP draft facts refreshed on 2026-05-24:

- Servers that support resources declare the `resources` capability.
- `resources/list` returns resource descriptors, supports pagination, and
  includes cache hints.
- `resources/read` returns one or more resource content blocks and includes
  cache hints.
- Resource descriptors include `uri`, `name`, optional `title`,
  `description`, and `mimeType`.
- Text resource content uses `uri`, `mimeType`, and `text`.
- Cache hints use `ttlMs` and `cacheScope`.
- Current draft resource-not-found errors use JSON-RPC `-32602` Invalid
  Params, with older `-32002` accepted by clients for backwards compatibility.
- `resources/read` requires `Mcp-Name` to match `params.uri` under Conary's
  target stateless transport validation.

Primary references:

- <https://modelcontextprotocol.io/specification/draft/server/resources>
- <https://modelcontextprotocol.io/specification/draft/server/utilities/caching>
- <https://modelcontextprotocol.io/specification/draft/basic/transports>
- <https://modelcontextprotocol.io/specification/draft/server/discover>

## Current Repo Facts

- `apps/conary-test/src/server/stateless_mcp.rs` adapts Axum requests into
  `conary_mcp::stateless_http` and currently provides only the bootstrap-status
  resource.
- `apps/conary-test/src/server/routes.rs` mounts `/mcp/stateless` inside the
  same auth boundary as the legacy `/mcp` route.
- `apps/conary-test/src/server/routes.rs` already has shared `AppState` and
  the `/v1/suites` HTTP route reads suite data from `state.manifest_dir`.
- `apps/conary-test/src/server/service.rs::list_suites` returns a narrow HTTP
  shape: `name`, `phase`, and `test_count`. It ignores invalid manifests.
- `apps/conary-test/src/bootstrap.rs` contains private manifest-inventory
  logic that already computes `id`, `name`, `phase`, `test_count`,
  `requires_container_runtime`, `requires_qemu`, and `qemu_only`.
- `apps/conary-test/src/config/manifest.rs::TestManifest::is_qemu_only`
  identifies QEMU-only suites.
- `crates/conary-agent-contract::resource` already defines per-suite helpers
  such as `test_suite(suite_id)`, but it does not yet define a static suite
  index helper for `conary-test://suites`.
- `crates/conary-agent-contract::catalog::default_read_resources` lists
  transport-neutral read-resource catalog entries and should include every
  durable read surface this work makes live.

## Decision

Add a second live stateless MCP resource to `conary-test`:

```text
conary-test://suites
```

It is a static suite index, not a resource template and not a collection of
per-suite resources. The route remains:

```text
POST /mcp/stateless
```

Successful methods after this slice:

- `server/discover`
- `resources/list`
- `resources/read` for `conary-local://bootstrap/status`
- `resources/read` for `conary-test://suites`

Out of scope:

- No Remi route changes.
- No live MCP tools.
- No live MCP prompts.
- No resource templates.
- No subscriptions.
- No SSE streaming.
- No smoke execution.
- No `conary-test://suites/{suite_id}` resource.
- No mutation behavior.
- No change to the legacy session-based `/mcp` route.

## Resource List Shape

`resources/list` should return two descriptors in deterministic order:

1. `conary-local://bootstrap/status`
2. `conary-test://suites`

The new descriptor should be:

```json
{
  "uri": "conary-test://suites",
  "name": "conary_test_suites",
  "title": "Conary-Test Suites",
  "description": "Read the local conary-test suite manifest inventory",
  "mimeType": "application/json"
}
```

The response keeps the same private short cache policy:

```json
{
  "resultType": "complete",
  "ttlMs": 30000,
  "cacheScope": "private"
}
```

No `nextCursor` is needed because this slice returns a complete two-item list.
If a client sends a cursor, the route returns the same stable list.

## Resource Read Shape

`resources/read` for `conary-test://suites` returns one text content block:

```json
{
  "uri": "conary-test://suites",
  "mimeType": "application/json",
  "text": "{ ... pretty JSON InspectResult ... }"
}
```

The `text` field must contain a valid serialized
`conary_agent_contract::InspectResult` with this envelope:

```json
{
  "operation": "conary-test.suites.inspect",
  "status": "ok",
  "subject": {
    "uri": "conary-test://suites"
  },
  "risk": "read_only",
  "summary": "Known conary-test suite manifests inspected",
  "data": {
    "manifest_dir": "/absolute/or/configured/path",
    "dir_exists": true,
    "toml_files": 3,
    "parsed": 3,
    "failed": 0,
    "suites": [
      {
        "id": "phase1-core",
        "name": "phase1-core",
        "phase": 1,
        "test_count": 12,
        "requires_container_runtime": true,
        "requires_qemu": false,
        "qemu_only": false
      }
    ],
    "errors": []
  }
}
```

Sorting is by `id` ascending. The `id` is the manifest file stem, matching the
bootstrap inventory model.

## Status Semantics

The MCP request succeeds when the resource URI exists and the server can return
an `InspectResult`. Manifest health is reported inside the Conary operation
envelope:

| Condition | MCP HTTP status | Inspect status | Notes |
| --- | --- | --- | --- |
| manifest directory exists, all TOML files parse | `200` | `ok` | `failed = 0`. |
| manifest directory exists, at least one TOML file parses, at least one fails | `200` | `partial` | Include parse errors in `data.errors` and warnings. |
| manifest directory exists, no TOML files parse | `200` | `unavailable` | Include a warning explaining no parseable manifests were found. |
| manifest directory is missing or unreadable | `200` | `unavailable` | The resource exists; local state is unavailable. |
| unknown resource URI | `404` | none | JSON-RPC `-32602` with `error.data.uri`. |

Do not map local manifest parse failures to MCP transport errors.

Unreadable manifest directories must be visible in both the operation envelope
and the data payload: add a warning and an explanatory `data.errors` entry.
The inventory should still report `dir_exists = true` when the path is a
directory but `read_dir` fails.

## Data Source And Ownership

Add a focused suite-inventory module in `apps/conary-test` rather than growing
more manifest parsing inside `stateless_mcp.rs`.

Recommended ownership:

- `apps/conary-test/src/suite_inventory.rs`
  - Reads a manifest directory.
  - Produces typed inventory data.
  - Computes QEMU and container-runtime flags.
  - Builds the `InspectResult` for `conary-test://suites`.
  - Does not depend on Axum or MCP types.

- `apps/conary-test/src/bootstrap.rs`
  - May reuse the new inventory module to keep bootstrap and suites resource
    logic consistent.
  - Must preserve the existing bootstrap JSON shape and tests.

- `apps/conary-test/src/server/stateless_mcp.rs`
  - Owns MCP descriptor registration and resource dispatch.
  - Receives `AppState` through Axum `State`.
  - Reads `state.manifest_dir` for `conary-test://suites`.
  - Keeps `/mcp/stateless` auth and route behavior unchanged.

- `crates/conary-agent-contract/src/resource.rs`
  - Adds `test_suites() -> ResourceRef` returning `conary-test://suites`.

- `crates/conary-agent-contract/src/catalog.rs`
  - Adds a catalog entry for `conary-test.suites`.

The `/v1/suites` HTTP route should keep its current response shape in this
slice. It can share the inventory module later, but this work does not need to
change its API.

## Error Model

Preserve existing stateless HTTP behavior for method, origin, JSON parse,
JSON-RPC envelope, header, protocol-version, and `_meta` errors.

New or newly covered resource-specific behavior:

| Condition | HTTP status | JSON-RPC code | Notes |
| --- | --- | --- | --- |
| valid `resources/list` | `200` | none | Returns bootstrap and suites descriptors with private cache hints. |
| valid suites `resources/read` | `200` | none | Returns one JSON text content block. |
| unknown resource URI | `404` | `-32602` | Include `{ "uri": ... }` in `error.data`. |
| missing `params.uri` for `resources/read` | `400` | `-32001` | Existing required-name validation path. |
| missing or mismatched `Mcp-Name` for `resources/read` | `400` | `-32001` | Existing header mismatch path. |
| valid but unsupported method | `404` | `-32601` | Examples: `resources/templates/list`, `tools/list`, `prompts/list`. |
| manifest inventory unavailable | `200` | none | State appears in the serialized `InspectResult`. |

Do not return an empty `contents` array for an unknown resource.

## Auth, Privacy, And Caching

The route remains inside conary-test's existing auth middleware:

- with no configured token, local non-browser clients can read the resource
- with a configured token, missing or wrong bearer token returns existing HTTP
  `401` plain JSON before the MCP handler runs

Cache scope must be `private` because the resource includes local filesystem
paths and potentially caller-specific manifest directories. The TTL remains
`30000` milliseconds to match existing short-lived local inspection resources.

## Testing Requirements

Required unit coverage:

- inventory reads valid manifests and emits stable `id`, `name`, `phase`,
  `test_count`, `requires_container_runtime`, `requires_qemu`, and `qemu_only`
  fields
- inventory sorts suites by `id`
- invalid TOML produces `status = partial` when at least one manifest parses
- missing or unreadable manifest directory produces `status = unavailable`
- `test_suites()` emits `conary-test://suites`
- catalog includes `conary-test.suites` as a read-only private short resource

Required conary-test route coverage:

- `server/discover` still advertises `resources = {}` only
- `resources/list` returns bootstrap and suites descriptors in deterministic
  order
- `resources/list` includes `ttlMs = 30000` and `cacheScope = "private"`
- `resources/read` for `conary-test://suites` returns JSON text that parses as
  an `InspectResult`
- `resources/read` for `conary-test://suites` includes `ttlMs = 30000` and
  `cacheScope = "private"`
- the suites resource uses `state.manifest_dir`, not hard-coded repo paths
- partial manifest parse state returns HTTP `200` with `status = partial`
  inside the resource text
- missing manifest directory returns HTTP `200` with `status = unavailable`
  inside the resource text
- unreadable manifest directory returns HTTP `200` with
  `status = unavailable` plus an explanatory warning and `data.errors` entry
  inside the resource text
- unknown resource URI still returns HTTP `404` with JSON-RPC `-32602`
- missing or mismatched `Mcp-Name` for `resources/read` still returns
  `-32001`
- legacy `/mcp` does not return stateless suites resource output
- token auth still gates `/mcp/stateless`

Required scope guards:

- Remi files do not mention `conary-test://suites`
- no live MCP tools are added
- no live MCP prompts are added
- no resource templates are added
- no SSE or subscription handling is added

## Documentation And Audit Requirements

Update active docs after implementation:

- `apps/conary-test/README.md`
- `docs/operations/agent-mcp-adapter-decision.md`
- `docs/operations/infrastructure.md`

Refresh and validate:

- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

The docs must keep the route boundary honest: the stateless route exposes
discovery plus two read-only resources, and still exposes no tools, prompts,
templates, subscriptions, SSE streaming, or mutations.

## Acceptance Criteria

- `POST /mcp/stateless` `resources/list` returns exactly the bootstrap-status
  and suites descriptors.
- `POST /mcp/stateless` `resources/read` for `conary-test://suites` returns
  one `application/json` text content block.
- `resources/list` and `resources/read` include private short cache hints:
  `ttlMs = 30000` and `cacheScope = "private"`.
- The resource content parses as an `InspectResult` with
  `operation = "conary-test.suites.inspect"`,
  `subject.uri = "conary-test://suites"`, and `risk = "read_only"`.
- Suite data comes from `AppState.manifest_dir`.
- Manifest parse failures are represented as Conary operation state, not MCP
  transport failures.
- The legacy `/mcp` route remains session-based and does not return stateless
  resource responses.
- Remi remains untouched.
- The implementation adds no live tools, prompts, resource templates,
  subscriptions, SSE streaming, smoke execution, or mutations.

## Verification Commands

The implementation plan should include at least:

```bash
cargo fmt --check
cargo test -p conary-agent-contract
cargo test -p conary-test suite_inventory
cargo test -p conary-test stateless_mcp
cargo test -p conary-test bootstrap
cargo test -p conary-test mcp_endpoint_requires_token
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
rg -n "conary-test://suites|conary_test_suites" docs/operations apps/conary-test/README.md
! rg -n "conary-test://suites|conary_test_suites" apps/remi
! rg -n "tools/call|prompts/get|resources/templates/list|subscriptions/listen|notifications/resources|EventStream|Sse" apps/conary-test/src/server/stateless_mcp.rs
git diff --check
```

The `rg` commands are guardrails. The positive docs check must find updated
documentation, and the negative code checks must find no Remi exposure, live
tools, live prompts, resource templates, subscriptions, or SSE implementation.

## Follow-On Slices

This slice intentionally stops at a static suite index. Good follow-ons are:

- a recent-runs resource that exposes known run IDs and high-level outcomes
- a run-detail resource such as `conary-test://runs/{run_id}`
- artifact resources for logs and result files
- hybrid prompts after bootstrap, suites, and run evidence resources exist

Do not add prompts before there is enough read-only evidence for the assistant
to inspect deterministically.
