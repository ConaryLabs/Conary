---
last_updated: 2026-07-25
revision: 46
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
- `resolution`: repository metadata, typed requirements, providers, and SAT
- `generation`: generation build, switch, recovery, GC, and export
- `ccs`: CCS authoring, conversion, install, v2, and lifecycle execution
- `packaging`: explicit recipes, try sessions, static repositories, trust, and sync
- `profiles`: repository feed profiles, parser selection, and Remi route slugs
- `remi`: Remi ingest, conversion, publication, admin, MCP, and fixture serving
- `conaryd`: local daemon auth, package jobs, routes, and lifecycle events
- `bootstrap`: bootstrap prerequisite, image, seed, run, and local QEMU validation
- `conary-test`: declarative integration suites, HTTP, MCP, and QEMU proof
- `agent-mcp`: transport-neutral operation vocabulary and MCP adapters

## Canonical Detail Pointers

- CLI dispatch and command routing: `docs/modules/feature-ownership.md` slug
  `dispatch`, plus `docs/ARCHITECTURE.md`.
- Install, update, remove, native lifecycle execution, and selected-root
  publication:
  `docs/modules/feature-ownership.md` slugs `install`, `adopt`, and `ccs`, plus
  `docs/modules/test-fixtures.md`. Exact shared config decisions start in
  `crates/conary-core/src/config_transaction.rs`; selected-root config capture
  and publication are owned by
  `apps/conary/src/commands/generation/config_transaction.rs`,
  `apps/conary/src/commands/generation/selected_root.rs`, and
  `apps/conary/src/commands/generation/publication.rs`. Single-package install
  execution starts in
  `apps/conary/src/commands/install/transaction/selected_root.rs`; removal starts
  in `apps/conary/src/commands/remove/native_graph.rs`. Debian dpkg process
  environment, administrative state, trigger/config capture, and
  update-alternatives projection start in
  `apps/conary/src/commands/install/native_events/debian_runtime.rs` and
  `apps/conary/src/commands/install/native_events/debian_runtime/`. Exact
  source-independent payload nodes start in `crates/conary-core/src/payload.rs`.
  `apps/conary/src/commands/live_root/recovery.rs` is confined to the
  selected-root session journal implementation.
- Declarative models, source selection, and replatforming:
  `docs/modules/feature-ownership.md` slug `model`, plus
  `docs/modules/source-selection.md`.
- Native repository trust, authenticated metadata/package intake, typed
  package relations, provider matching, and SAT selection:
  `docs/modules/feature-ownership.md` slug `resolution`, plus
  `docs/modules/source-selection.md`. Trust starts in
  `crates/conary-core/src/repository/trust.rs` and
  `crates/conary-core/src/repository/trust/openpgp.rs`; ecosystem parsers start
  in `crates/conary-core/src/repository/parsers/`.
- Generation, bootstrap, and QEMU proof:
  `docs/modules/feature-ownership.md` slugs `generation` and `bootstrap`, plus
  `crates/conary-core/src/generation/root_manifest.rs`,
  `crates/conary-core/src/activation/systemd.rs`,
  `crates/conary-core/src/activation/systemd/grammar.rs`,
  `crates/conary-core/src/activation/security_policy.rs`,
  `crates/conary-core/src/activation/security_policy/`,
  `crates/conary-core/src/scriptlet/activation_capture.rs`,
  `crates/conary-core/src/db/models/generation_activation.rs`,
  `apps/conary/src/commands/generation/activation_intents.rs`,
  `docs/modules/bootstrap.md`, and
  `docs/operations/bootstrap-selfhosting-vm.md`.
- CCS authoring, conversion, native package contracts, and repository feed
  profiles:
  `docs/modules/feature-ownership.md` slugs `ccs`, `packaging`, and `profiles`,
  plus `docs/modules/ccs.md` and `docs/modules/recipe.md`. Debian lifecycle
  service-helper argv grammar starts in
  `crates/conary-core/src/packages/deb/lifecycle_helpers.rs` and its focused
  child modules.
- Try-session start, refresh, keep, and rollback orchestration:
  `apps/conary/src/commands/try_session/session.rs`; watch-created identity:
  `apps/conary/src/commands/try_session/session/watch_marker.rs`.
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

- Generation-aware mutation authority is the exact cumulative selected root,
  captured and persisted before its SQLite transaction commits. Publication
  never reconstructs filesystem effects from a database-only snapshot.
- Resolution is SAT-only; start from the active resolver and install flows, not
  older graph-based assumptions.
- Keep shared operation vocabulary in `conary-core` and daemon-only request or
  execution policy in `conaryd`.
- Remi and `conary-test` share service-layer patterns between HTTP handlers and
  MCP tools; prefer those seams before duplicating handler logic.
- Remi package-source and parser authority is the typed repository manifest;
  repository names, URLs, file extensions, and discovery indexes are not
  semantic authority.
- Transaction and generation work stay coupled: resolve and fetch, mutate an
  isolated selected root, capture exact immutable and mutable-state manifests,
  persist that candidate and its SQLite debt atomically, then build, publish,
  mount, or export it.
- Component and provide authority comes from typed package metadata, never
  payload path classification. Unannotated native payloads remain one lossless
  `runtime` component.
- Adoption preserves native package-manager authority until explicit takeover or
  selected-generation handoff.
- Single-package adoption preview and apply share the planner in
  `apps/conary/src/commands/adopt/packages.rs`; preview stops before every
  SQLite, checkpoint, CAS, native-PM, hook, generation, and live-root write.
- Trust is typed authority, not a default toggle. Native repositories require
  their ecosystem-specific metadata-to-package chain; static repositories
  require TUF. Missing proof fails closed and has no runtime disable command.

## Prefer Existing Deep Dives

- [`docs/modules/federation.md`](../modules/federation.md) for federation background
- [`docs/modules/ccs.md`](../modules/ccs.md) for CCS format and conversion context
- [`docs/specs/foreign-package-lifecycle-contracts.md`](../specs/foreign-package-lifecycle-contracts.md) for authoritative RPM, Debian, and Arch lifecycle ABIs, transaction order, arguments, triggers, and payload visibility
- [`docs/modules/feature-ownership.md`](../modules/feature-ownership.md) for feature ownership cards, neighboring systems, and interaction verification gates
- [`docs/modules/test-fixtures.md`](../modules/test-fixtures.md) for Remi and CCS fixture ownership and proof commands
- [`docs/modules/bootstrap.md`](../modules/bootstrap.md) for bootstrap and stage flows
- [`docs/operations/bootstrap-selfhosting-vm.md`](../operations/bootstrap-selfhosting-vm.md) for the truthful self-hosting VM build and validation path
- [`docs/roadmaps/development-roadmap.md`](../roadmaps/development-roadmap.md) for remaining generation-bundle trust, pristine validation, and platform-projection horizons
- [`docs/modules/recipe.md`](../modules/recipe.md) for recipe/build-system behavior
- [`docs/modules/query.md`](../modules/query.md) for query-oriented CLI flows
- [`docs/modules/source-selection.md`](../modules/source-selection.md) for source-policy, ranking, and replatform behavior

## Drift Rule

If a "look here first" path, owner slug, proof command, or interaction gate
changes, update `docs/modules/feature-ownership.md` first. Then update this file
only when the high-level orientation or canonical pointer changes.
