---
last_updated: 2026-04-07
revision: 3
summary: Stable subsystem pointers and durable assistant-facing guidance for the Conary workspace after the source-selection and replatform execution refresh
---

# Assistant Subsystem Map

## Workspace Orientation

- `apps/conary/`: user-facing CLI commands, argument parsing, and command dispatch
- `crates/conary-core/`: shared package-management domain, repository sync, resolver, trust, transaction, and CCS logic
- `apps/remi/`: Remi package service, admin surface, MCP server, and federation
- `apps/conaryd/`: local daemon, socket auth, job queue, and REST/SSE routes
- `apps/conary-test/`: declarative integration-test engine, HTTP API, and MCP server
- `crates/conary-bootstrap/`: shared tracing, runtime, and error-exit helpers for workspace apps
- `crates/conary-mcp/`: shared transport-agnostic MCP helpers used by workspace apps

## Look Here First

- Repository sync, remote metadata, chunk retrieval, and Remi client behavior:
  `crates/conary-core/src/repository/`
- Source selection, runtime policy mirrors, and replatform convergence:
  `crates/conary-core/src/repository/effective_policy.rs`,
  `crates/conary-core/src/model/parser.rs`,
  `crates/conary-core/src/model/replatform.rs`,
  `apps/conary/src/commands/distro.rs`,
  `apps/conary/src/commands/update.rs`, and
  `apps/conary/src/commands/model.rs`
- Dependency resolution and package candidate ranking:
  `crates/conary-core/src/resolver/sat.rs`,
  `crates/conary-core/src/resolver/provider/`, and
  `crates/conary-core/src/resolver/provides_index.rs`
- Transaction lifecycle and conflict preflight:
  `crates/conary-core/src/transaction/mod.rs` and
  `crates/conary-core/src/transaction/planner.rs`
- Generation building, composefs mounting, `/etc` merge, and GC:
  `crates/conary-core/src/generation/builder.rs`,
  `crates/conary-core/src/generation/mount.rs`,
  `crates/conary-core/src/generation/etc_merge.rs`, and
  `crates/conary-core/src/generation/gc.rs`
- CCS package building, chunking, verification, and conversion:
  `crates/conary-core/src/ccs/builder.rs`,
  `crates/conary-core/src/ccs/binary_manifest.rs`,
  `crates/conary-core/src/ccs/chunking.rs`, and
  `crates/conary-core/src/ccs/convert/`
- TUF trust and signature verification:
  `crates/conary-core/src/trust/verify.rs`,
  `crates/conary-core/src/trust/client.rs`, and
  `crates/conary-core/src/trust/keys.rs`
- Shared operation vocabulary and daemon-boundary ownership:
  `crates/conary-core/src/operations.rs`,
  `apps/conaryd/src/daemon/mod.rs`, and
  `apps/conaryd/src/daemon/routes/transactions.rs`
- Remi admin and MCP flows:
  `apps/remi/src/server/admin_service.rs`,
  `apps/remi/src/server/mcp.rs`, and
  `apps/remi/src/server/handlers/admin/`
- conary-test HTTP and MCP service layer:
  `apps/conary-test/src/server/service.rs`,
  `apps/conary-test/src/server/mcp.rs`, and
  `apps/conary-test/src/engine/`
- conaryd daemon routes and auth boundaries:
  `apps/conaryd/src/daemon/mod.rs`,
  `apps/conaryd/src/daemon/routes/`,
  `apps/conaryd/src/daemon/auth.rs`, and
  `apps/conaryd/src/daemon/jobs.rs`

## Stable Patterns

- Runtime state is database-first. SQLite is the source of truth for package
  state, and later transaction stages are re-derivable from DB state.
- Resolution is SAT-only. The active install/remove entry points live in
  `resolver/sat.rs`; do not assume an older graph-based resolver still owns the
  workflow.
- Keep transport-agnostic naming in `conary-core` and daemon-only execution or
  request policy in `conaryd`; the shared `OperationKind` / daemon `JobKind`
  split is intentional.
- Remi and `conary-test` both share service-layer patterns between HTTP
  handlers and MCP tools. Look for `admin_service.rs` and `server/service.rs`
  before duplicating business logic in handlers.
- Transaction and generation work are tightly coupled: resolve, fetch, DB
  commit, build the EROFS generation, then mount it.
- Trust defaults matter. Keep HTTPS peer identity pinning and strict signature
  verification intact unless the task explicitly changes the trust model.

## Prefer Existing Deep Dives

- [`docs/modules/federation.md`](../modules/federation.md) for federation background
- [`docs/modules/ccs.md`](../modules/ccs.md) for CCS format and conversion context
- [`docs/modules/bootstrap.md`](../modules/bootstrap.md) for bootstrap and stage flows
- [`docs/modules/recipe.md`](../modules/recipe.md) for recipe/build-system behavior
- [`docs/modules/query.md`](../modules/query.md) for query-oriented CLI flows
- [`docs/modules/source-selection.md`](../modules/source-selection.md) for source-policy, ranking, and replatform behavior

## Freshness Notes

- Keep this file focused on stable pointers and invariants.
- Do not copy schema versions, table counts, workflow counts, or other
  fast-moving inventories into assistant guidance.
- If a subsystem needs more than these pointers, add or update a narrow
  canonical doc instead of expanding this map into a handbook.
