---
last_updated: 2026-07-24
revision: 35
summary: Compact assistant subsystem orientation index with detailed path and proof routing delegated to feature ownership cards.
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

Use this file for quick subsystem orientation only. For exact owner files,
path matches, focused proof, and interaction gates, use the feature-card bridge:

```bash
bash scripts/agent-context.sh --list
bash scripts/agent-context.sh --feature <slug>
bash scripts/agent-context.sh --path <file>
```

`docs/modules/feature-ownership.md` is the canonical source behind those
commands.

- `dispatch`: CLI command routing, namespace dispatch, and command risk labels
- `install`: install, update, remove, restore, scriptlet, and live-root mutation
- `adopt`: adoption, unadoption, takeover, and native-authority handoff
- `model`: declarative system model diff/apply/check/snapshot/publish/lock
- `generation`: generation build, switch, recovery, GC, and export
- `ccs`: CCS authoring, conversion, install, v2, and legacy replay
- `packaging`: source inference, recipes, try sessions, static repositories, trust, and sync
- `profiles`: supported distro adapter catalog and Remi route slugs
- `remi`: Remi ingest, conversion, publication, admin, MCP, and fixture serving
- `conaryd`: local daemon auth, package jobs, routes, and lifecycle events
- `bootstrap`: bootstrap prerequisite, image, seed, run, and local QEMU validation
- `conary-test`: declarative integration suites, HTTP, MCP, and QEMU proof
- `agent-mcp`: transport-neutral operation vocabulary and MCP adapters

## Canonical Detail Pointers

- CLI dispatch and command routing: `docs/modules/feature-ownership.md` slug
  `dispatch`, plus `docs/ARCHITECTURE.md`.
- Install, update, remove, scriptlet replay, and live-root mutation:
  `docs/modules/feature-ownership.md` slugs `install`, `adopt`, and `ccs`, plus
  `docs/modules/test-fixtures.md`.
- Declarative models, source selection, and replatforming:
  `docs/modules/feature-ownership.md` slug `model`, plus
  `docs/modules/source-selection.md`.
- Generation, bootstrap, and QEMU proof:
  `docs/modules/feature-ownership.md` slugs `generation` and `bootstrap`, plus
  `docs/modules/bootstrap.md` and `docs/operations/bootstrap-selfhosting-vm.md`.
- CCS authoring, conversion, native package contracts, and supported profiles:
  `docs/modules/feature-ownership.md` slugs `ccs`, `packaging`, and `profiles`,
  plus `docs/modules/ccs.md` and `docs/modules/recipe.md`.
- Remi, federation, publication, and service-owned conversion:
  `docs/modules/feature-ownership.md` slug `remi`, plus `docs/modules/remi.md`
  and `docs/modules/federation.md`.
- conaryd routes and package jobs: `docs/modules/feature-ownership.md` slug
  `conaryd`, plus `docs/modules/conaryd.md`.
- `conary-test`, fixtures, and declarative suites:
  `docs/modules/feature-ownership.md` slug `conary-test`, plus
  `docs/INTEGRATION-TESTING.md` and `docs/modules/test-fixtures.md`.
- Agent/MCP operation vocabulary and adapters:
  `docs/modules/feature-ownership.md` slug `agent-mcp`, plus
  `docs/operations/infrastructure.md`.

## Stable Patterns

- Runtime state is database-first; later transaction stages are re-derivable
  from SQLite state.
- Resolution is SAT-only; start from the active resolver and install flows, not
  older graph-based assumptions.
- Keep shared operation vocabulary in `conary-core` and daemon-only request or
  execution policy in `conaryd`.
- Remi and `conary-test` share service-layer patterns between HTTP handlers and
  MCP tools; prefer those seams before duplicating handler logic.
- Transaction and generation work stay coupled: resolve, fetch, commit DB
  state, build the generation artifact, then mount or export it.
- Adoption preserves native package-manager authority until explicit takeover or
  selected-generation handoff.
- Single-package adoption preview and apply share the planner in
  `apps/conary/src/commands/adopt/packages.rs`; preview stops before every
  SQLite, checkpoint, CAS, native-PM, hook, generation, and live-root write.
- Trust defaults matter; keep HTTPS peer pinning and signature verification
  strict unless the task explicitly changes the trust model.

## Prefer Existing Deep Dives

- [`docs/modules/federation.md`](../modules/federation.md) for federation background
- [`docs/modules/ccs.md`](../modules/ccs.md) for CCS format and conversion context
- [`docs/specs/foreign-package-lifecycle-contracts.md`](../specs/foreign-package-lifecycle-contracts.md) for authoritative RPM, Debian, and Arch lifecycle parsing and adapter rules
- [`docs/modules/feature-ownership.md`](../modules/feature-ownership.md) for feature ownership cards, neighboring systems, and interaction verification gates
- [`docs/modules/test-fixtures.md`](../modules/test-fixtures.md) for Remi and CCS fixture ownership and proof commands
- [`docs/modules/bootstrap.md`](../modules/bootstrap.md) for bootstrap and stage flows
- [`docs/operations/bootstrap-selfhosting-vm.md`](../operations/bootstrap-selfhosting-vm.md) for the truthful self-hosting VM build and validation path
- [`docs/operations/post-generation-export-follow-up-roadmap.md`](../operations/post-generation-export-follow-up-roadmap.md) for remaining bundle, boot-artifact verification, sandbox, pristine-validation, and image-projection follow-ups
- [`docs/modules/recipe.md`](../modules/recipe.md) for recipe/build-system behavior
- [`docs/modules/query.md`](../modules/query.md) for query-oriented CLI flows
- [`docs/modules/source-selection.md`](../modules/source-selection.md) for source-policy, ranking, and replatform behavior

## Drift Rule

If a "look here first" path, owner slug, proof command, or interaction gate
changes, update `docs/modules/feature-ownership.md` first. Then update this file
only when the high-level orientation or canonical pointer changes.

## Freshness Notes

- Keep this file focused on stable pointers and invariants.
- Do not copy schema versions, table counts, workflow counts, or other
  fast-moving inventories into assistant guidance.
- If a subsystem needs more than these pointers, add or update a narrow
  canonical doc instead of expanding this map into a handbook.
