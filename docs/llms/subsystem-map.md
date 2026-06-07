---
last_updated: 2026-06-06
revision: 11
summary: Stable subsystem pointers with feature ownership and interaction-gate routing
---

# Assistant Subsystem Map

## Workspace Orientation

- `apps/conary/`: user-facing CLI commands, argument parsing, and command dispatch
- `crates/conary-core/`: shared package-management domain, repository sync, resolver, trust, transaction, and CCS logic
- `apps/remi/`: Remi package service, admin surface, MCP server, and federation
- `apps/conaryd/`: local daemon, socket auth, job queue, and REST/SSE routes
- `apps/conary-test/`: declarative integration-test engine, HTTP API, and MCP server
- `crates/conary-bootstrap/`: shared tracing, runtime, and error-exit helpers for workspace apps
- `crates/conary-agent-contract/`: transport-neutral agent operation contract, resource refs, risk labels, and catalogs
- `crates/conary-mcp/`: shared MCP adapter helpers used by workspace apps

## Look Here First

- Repository sync, remote metadata, chunk retrieval, and Remi client behavior:
  `crates/conary-core/src/repository/`
- Source selection, runtime policy mirrors, and replatform convergence:
  `crates/conary-core/src/repository/effective_policy.rs`,
  `crates/conary-core/src/model/parser.rs`,
  `crates/conary-core/src/model/replatform.rs`,
  `apps/conary/src/commands/distro.rs`,
  `apps/conary/src/commands/update/mod.rs`,
  `apps/conary/src/commands/update/selection.rs`,
  `apps/conary/src/commands/update/adopted_authority.rs`,
  `apps/conary/src/commands/update/collection.rs`, and
  `apps/conary/src/commands/model.rs`
- Dependency resolution and package candidate ranking:
  `crates/conary-core/src/resolver/sat.rs`,
  `crates/conary-core/src/resolver/provider/`, and
  `crates/conary-core/src/resolver/provides_index.rs`
- Transaction lifecycle and conflict preflight:
  `crates/conary-core/src/transaction/mod.rs` and
  `crates/conary-core/src/transaction/planner.rs`
- Install orchestration, legacy replay install adapter behavior, and live-root
  preflight:
  `apps/conary/src/commands/install/mod.rs`,
  `apps/conary/src/commands/install/legacy_replay.rs`,
  `apps/conary/src/commands/install/inner.rs`,
  `apps/conary/src/commands/install/batch.rs`,
  `apps/conary/src/commands/install/restore.rs`, and
  `docs/modules/test-fixtures.md`
- Generation building, artifact export, composefs mounting, `/etc` merge, and GC:
  `crates/conary-core/src/generation/builder.rs`,
  `crates/conary-core/src/generation/builder/runtime_inputs.rs`,
  `crates/conary-core/src/generation/artifact.rs`,
  `crates/conary-core/src/generation/export.rs`,
  `crates/conary-core/src/image/repart.rs`,
  `crates/conary-core/src/generation/mount.rs`,
  `crates/conary-core/src/generation/etc_merge.rs`, and
  `crates/conary-core/src/generation/gc.rs`
- Adoption, unadoption, and selected-generation native-authority handoff:
  `apps/conary/src/commands/adopt/mod.rs`,
  `apps/conary/src/commands/adopt/unadopt.rs`,
  `apps/conary/src/commands/adopt/native_handoff.rs`, and
  `apps/conary/tests/integration/remi/manifests/phase3-active-generation-handoff.toml`
- CCS package building, chunking, verification, conversion, and fixture proof:
  `crates/conary-core/src/ccs/builder.rs`,
  `crates/conary-core/src/ccs/binary_manifest.rs`,
  `crates/conary-core/src/ccs/chunking.rs`,
  `crates/conary-core/src/ccs/convert/`, and
  `docs/modules/test-fixtures.md`
- TUF trust and signature verification:
  `crates/conary-core/src/trust/verify.rs`,
  `crates/conary-core/src/trust/client.rs`, and
  `crates/conary-core/src/trust/keys.rs`
- Shared operation vocabulary and daemon-boundary ownership:
  `crates/conary-core/src/operations.rs`,
  `apps/conaryd/src/daemon/mod.rs`, and
  `apps/conaryd/src/daemon/routes/transactions.rs`
- Remi admin, publication, artifact fixture, and MCP flows:
  `apps/remi/src/server/admin_service.rs`,
  `apps/remi/src/server/publication.rs`,
  `apps/remi/src/server/mcp.rs`,
  `apps/remi/src/server/handlers/artifacts.rs`,
  `apps/remi/src/server/handlers/admin/`, and
  `docs/modules/test-fixtures.md`
- conary-test HTTP and MCP service layer:
  `apps/conary-test/src/server/service.rs`,
  `apps/conary-test/src/server/mcp.rs`, and
  `apps/conary-test/src/engine/`
- Agent operation contract and MCP adapter helpers:
  `crates/conary-agent-contract/src/` and `crates/conary-mcp/src/`
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
  commit, build the EROFS generation artifact, then mount or export it.
- Adoption mode preserves native package-manager authority until explicit
  takeover or selected-generation native handoff. Do not silently convert
  adopted RPM/DEB/Arch packages into Conary-owned packages.
- Trust defaults matter. Keep HTTPS peer identity pinning and strict signature
  verification intact unless the task explicitly changes the trust model.

## Prefer Existing Deep Dives

- [`docs/modules/federation.md`](../modules/federation.md) for federation background
- [`docs/modules/ccs.md`](../modules/ccs.md) for CCS format and conversion context
- [`docs/modules/feature-ownership.md`](../modules/feature-ownership.md) for feature ownership cards, neighboring systems, and interaction verification gates
- [`docs/modules/test-fixtures.md`](../modules/test-fixtures.md) for Remi and CCS fixture ownership and proof commands
- [`docs/modules/bootstrap.md`](../modules/bootstrap.md) for bootstrap and stage flows
- [`docs/operations/bootstrap-selfhosting-vm.md`](../operations/bootstrap-selfhosting-vm.md) for the truthful self-hosting VM build and validation path
- [`docs/operations/post-generation-export-follow-up-roadmap.md`](../operations/post-generation-export-follow-up-roadmap.md) for remaining bundle, boot-artifact verification, sandbox, pristine-validation, and image-projection follow-ups
- [`docs/modules/recipe.md`](../modules/recipe.md) for recipe/build-system behavior
- [`docs/modules/query.md`](../modules/query.md) for query-oriented CLI flows
- [`docs/modules/source-selection.md`](../modules/source-selection.md) for source-policy, ranking, and replatform behavior

## Freshness Notes

- Keep this file focused on stable pointers and invariants.
- Do not copy schema versions, table counts, workflow counts, or other
  fast-moving inventories into assistant guidance.
- If a subsystem needs more than these pointers, add or update a narrow
  canonical doc instead of expanding this map into a handbook.
