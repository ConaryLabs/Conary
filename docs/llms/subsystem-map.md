---
last_updated: 2026-07-29
revision: 57
summary: Route exact Remi signing, serialized selected-root mutation, full-adoption captured-root continuity, typed rollback lineage, canonical-map authority, carrier security, typed generation GC, exact release authority, and subsystem proof through current feature owners.
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
- `release`: exact-tag construction, immutable publication, serialized deployment, and independent proof
- `conary-test`: declarative integration suites, HTTP, MCP, and QEMU proof
- `agent-mcp`: transport-neutral operation vocabulary and MCP adapters

## Canonical Detail Pointers

- CLI dispatch and command routing: `docs/modules/feature-ownership.md` slug
  `dispatch`, plus `docs/ARCHITECTURE.md`.
- Install, update, remove, native lifecycle execution, and selected-root
  publication:
  `docs/modules/feature-ownership.md` slugs `install`, `adopt`, and `ccs`, plus
  `docs/modules/test-fixtures.md`. Exact shared config decisions start in
  `crates/conary-core/src/config_transaction.rs` and
  `crates/conary-core/src/config_transaction/`; selected-root config capture
  and publication are owned by
  `apps/conary/src/commands/generation/config_transaction.rs`,
  `apps/conary/src/commands/generation/selected_root.rs`, and
  `apps/conary/src/commands/generation/publication.rs`. Single-package install
  execution starts in
  `apps/conary/src/commands/install/transaction/selected_root.rs`; removal starts
  in `apps/conary/src/commands/remove/native_graph.rs`. The selected-root
  session acquires and owns the canonical runtime mutation lock before
  materialization; the lock implementation starts in
  `crates/conary-core/src/transaction/mod.rs`. Exact rollback execution and
  typed compensating lineage start in
  `apps/conary/src/commands/system/rollback_command.rs` and
  `crates/conary-core/src/db/models/changeset.rs`. Exact rollback capture
  starts in `apps/conary/src/commands/installed_authority_snapshot.rs` and
  `apps/conary/src/commands/installed_authority_snapshot/`; installed-state
  reconstruction starts in
  `apps/conary/src/commands/system/rollback_restore.rs` and
  `apps/conary/src/commands/system/rollback_restore/`. Shared-directory
  materialization starts in
  `apps/conary/src/commands/install/shared_directory.rs`; exact persisted
  claims and package-facing payload ownership start in
  `crates/conary-core/src/db/models/directory_claim.rs` and
  `crates/conary-core/src/db/models/package_payload_ownership.rs`; bounded
  selected-root node inspection starts in
  `crates/conary-core/src/filesystem/selected_root.rs`. Debian dpkg process
  environment, administrative state, trigger/config capture, and
  update-alternatives projection start in
  `apps/conary/src/commands/install/native_events/debian_runtime.rs` and
  `apps/conary/src/commands/install/native_events/debian_runtime/`. Exact RPM
  pre-payload sysusers target-interface execution starts in
  `crates/conary-core/src/scriptlet/sysusers.rs`; generic native argv remains
  selected-root confined in
  `crates/conary-core/src/scriptlet/native_command.rs`. Exact source-independent
  payload nodes start in `crates/conary-core/src/payload.rs`.
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
  `crates/conary-core/src/repository/trust/openpgp.rs`, with ALPM keyring and
  package-signature semantics in
  `crates/conary-core/src/repository/trust/openpgp/arch/`; ecosystem parsers start
  in `crates/conary-core/src/repository/parsers/`.
- Canonical package equivalence and Remi map exchange:
  `docs/modules/feature-ownership.md` slug `canonical-map`, plus
  `docs/modules/source-selection.md`. Start with
  `crates/conary-core/src/canonical/exchange.rs` for the versioned wire and
  atomic Remi snapshot, `canonical/rules.rs` for literal local contracts, and
  `db/models/canonical.rs` for typed persistence authority.
- Generation, bootstrap, and QEMU proof:
  `docs/modules/feature-ownership.md` slugs `generation` and `bootstrap`, plus
  `crates/conary-core/src/generation/root_manifest.rs`,
  `crates/conary-core/src/generation/builder/carrier_capabilities.rs`,
  `crates/conary-core/src/generation/artifact.rs`,
  `crates/conary-core/src/generation/export.rs`,
  `crates/conary-core/src/ccs/hooks/capabilities/filesystem_security.rs`,
  `crates/conary-core/src/activation/systemd.rs`,
  `crates/conary-core/src/activation/systemd/grammar.rs`,
  `crates/conary-core/src/activation/security_policy.rs`,
  `crates/conary-core/src/activation/security_policy/`,
  `crates/conary-core/src/scriptlet/activation_capture.rs`,
  `crates/conary-core/src/db/models/generation_activation.rs`,
  `crates/conary-core/src/generation/gc.rs`,
  `apps/conary/src/commands/generation/activation_intents.rs`,
  `apps/conary/src/commands/generation/gc.rs`,
  `docs/modules/bootstrap.md`, and
  `docs/operations/bootstrap-selfhosting-vm.md`.
- CCS authoring, conversion, native package contracts, and repository feed
  profiles:
  `docs/modules/feature-ownership.md` slugs `ccs`, `packaging`, and `profiles`,
  plus `docs/modules/ccs.md` and `docs/modules/recipe.md`. Native authoring
  content flow starts in `crates/conary-core/src/ccs/builder.rs`,
  `builder/source.rs`, `policy/content.rs`, and `builder/package_writer.rs`.
  Debian lifecycle
  service-helper argv grammar starts in
  `crates/conary-core/src/packages/deb/lifecycle_helpers.rs` and its focused
  child modules. The selected-root namespace, capability, and seccomp contract
  starts in `crates/conary-core/src/scriptlet/boundary.rs`.
- Try-session start, refresh, keep, and rollback orchestration:
  `apps/conary/src/commands/try_session/session.rs`; watch-created identity:
  `apps/conary/src/commands/try_session/session/watch_marker.rs`.
- Remi, federation, publication, and service-owned conversion:
  `docs/modules/feature-ownership.md` slug `remi`, plus `docs/modules/remi.md`
  and `docs/modules/federation.md`. Durable exact-profile CCS/TUF signing
  authority starts in `apps/remi/src/server/signing_authority.rs`; deployment
  wiring and validation start in `apps/remi/src/deployment.rs` and
  `deploy/remi-deploy-helper.sh`.
- conaryd routes and package jobs: `docs/modules/feature-ownership.md` slug
  `conaryd`, plus `docs/modules/conaryd.md`.
- Exact-tag release construction, signing, immutable publication, deployment,
  and independent live proof: `docs/modules/feature-ownership.md` slug
  `release`, plus `.github/workflows/release-artifact-proof.yml`,
  `docs/operations/release-artifact-matrix.md`, and
  `docs/operations/infrastructure.md`.
- `conary-test`, fixtures, and declarative suites:
  `docs/modules/feature-ownership.md` slug `conary-test`, plus
  `docs/INTEGRATION-TESTING.md` and `docs/modules/test-fixtures.md`.
- Agent/MCP operation vocabulary and adapters:
  `docs/modules/feature-ownership.md` slug `agent-mcp`, plus
  `docs/operations/infrastructure.md`.

## Stable Patterns

- Generation-aware mutation authority is the exact cumulative selected root,
  materialized only after the runtime mutation lock is held, captured and
  persisted before its SQLite transaction commits, and published before the
  lock owner is released. Publication never reconstructs filesystem effects
  from a database-only snapshot. Forward changesets and rollback changesets
  carry distinct typed lineage so compensating rows do not become LIFO
  mutation authority.
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
- Complete unfiltered full-system adoption starts in
  `apps/conary/src/commands/adopt/system.rs`; its exact unowned-root partition
  is owned by `adopt/system/captured_root.rs`, the finite scanner and runtime
  exclusions by `crates/conary-core/src/generation/root_manifest/scan.rs`, and
  generation consumption by
  `crates/conary-core/src/generation/builder/runtime_inputs.rs`. `CapturedRoot`
  preserves continuity state only; package anchors and claims remain the
  install/update/remove ownership authority.
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
- [`docs/operations/release-artifact-matrix.md`](../operations/release-artifact-matrix.md) for exact release lineage, artifact, deployment, and independent verification evidence
- [`docs/roadmaps/development-roadmap.md`](../roadmaps/development-roadmap.md) for remaining generation-bundle trust, pristine validation, and platform-projection horizons
- [`docs/modules/recipe.md`](../modules/recipe.md) for recipe/build-system behavior
- [`docs/modules/query.md`](../modules/query.md) for query-oriented CLI flows
- [`docs/modules/source-selection.md`](../modules/source-selection.md) for source-policy, ranking, and replatform behavior

## Drift Rule

If a "look here first" path, owner slug, proof command, or interaction gate
changes, update `docs/modules/feature-ownership.md` first. Then update this file
only when the high-level orientation or canonical pointer changes.
