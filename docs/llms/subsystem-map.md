---
last_updated: 2026-06-17
revision: 27
summary: Route decomposed try-session ownership
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
- Packaging, source inference, try sessions, and static repository publishing:
  `docs/specs/static-repo-format-v1.md`,
  `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`,
  `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`,
  `docs/superpowers/specs/2026-06-13-m2-publish-hardening-remi-design.md`,
  `docs/superpowers/specs/2026-06-14-m2a-builder-config-publish-divergence-design.md`,
  `docs/superpowers/plans/2026-06-14-m2a-hermetic-publish-foundation-implementation-plan.md`,
  `docs/guides/first-package.md`,
  `crates/conary-core/src/recipe/inference/`,
  `crates/conary-core/src/recipe/hermetic/`,
  `crates/conary-core/src/recipe/kitchen/`,
  `crates/conary-core/src/diagnostics/`,
  `apps/conary/src/commands/packaging_mcp/`,
  `crates/conary-core/src/db/models/try_session.rs`,
  `apps/conary/src/commands/new.rs`,
  `apps/conary/src/commands/publish.rs`,
  `apps/conary/src/commands/cook.rs`,
  `apps/conary/src/commands/diagnostics.rs`,
  `apps/conary/src/commands/operation_records.rs`,
  `apps/conary/src/commands/hermetic_config.rs`,
  `apps/conary/src/commands/hermetic_state.rs`,
  `apps/conary/src/commands/try_session/`,
  `apps/conary/src/commands/repo_static.rs`,
  `apps/conary/tests/packaging_m1b.rs`,
  `apps/conary/tests/packaging_m2a.rs`,
  `apps/conary/tests/packaging_m3a.rs`,
  `apps/conary/tests/packaging_m3b.rs`,
  `crates/conary-agent-contract/src/{resource,catalog,result}.rs`,
  `crates/conary-mcp/src/`,
  `crates/conary-core/src/ccs/attestation.rs`,
  `crates/conary-core/src/repository/static_repo/publish.rs`,
  `crates/conary-core/src/repository/static_repo/publish_context.rs`,
  `crates/conary-core/src/repository/static_repo/package_staging.rs`,
  `crates/conary-core/src/repository/static_repo/publish_gate.rs`,
  `crates/conary-core/src/repository/static_repo/`,
  `crates/conary-core/src/trust/`, and
  `crates/conary-core/src/ccs/signing.rs`
- Source selection, runtime policy mirrors, and replatform convergence:
  `crates/conary-core/src/repository/effective_policy.rs`,
  `crates/conary-core/src/model/parser.rs`,
  `crates/conary-core/src/model/replatform.rs`,
  `apps/conary/src/commands/distro.rs`,
  `apps/conary/src/commands/update/mod.rs`,
  `apps/conary/src/commands/update/package.rs`,
  `apps/conary/src/commands/update/source_policy.rs`,
  `apps/conary/src/commands/update/selection.rs`,
  `apps/conary/src/commands/update/adopted_authority.rs`,
  `apps/conary/src/commands/update/collection.rs`,
  `apps/conary/src/commands/update/pinning.rs`,
  `apps/conary/src/commands/update/delta_stats.rs`, and
  `apps/conary/src/commands/model.rs`,
  `apps/conary/src/commands/model/context.rs`,
  `apps/conary/src/commands/model/presentation.rs`,
  `apps/conary/src/commands/model/apply.rs`,
  `apps/conary/src/commands/model/remote_diff.rs`, and
  `apps/conary/src/commands/model/lock.rs`
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
  `apps/conary/src/commands/install/` for child modules,
  `apps/conary/src/commands/install/command.rs`,
  `apps/conary/src/commands/install/acquire.rs`,
  `apps/conary/src/commands/install/blocklist.rs`,
  `apps/conary/src/commands/install/ccs_transaction.rs`,
  `apps/conary/src/commands/install/conversion.rs`,
  `apps/conary/src/commands/install/dep_mode.rs`,
  `apps/conary/src/commands/install/dep_resolution.rs`,
  `apps/conary/src/commands/install/validation.rs`,
  `apps/conary/src/commands/install/dependencies.rs`,
  `apps/conary/src/commands/install/execute.rs`,
  `apps/conary/src/commands/install/lifecycle.rs`,
  `apps/conary/src/commands/install/transaction.rs`,
  `apps/conary/src/commands/install/options.rs`,
  `apps/conary/src/commands/install/semantics.rs`,
  `apps/conary/src/commands/install/source_policy.rs`,
  `apps/conary/src/commands/install/legacy_replay.rs`,
  `apps/conary/src/commands/install/inner.rs`,
  `apps/conary/src/commands/install/batch.rs`,
  `apps/conary/src/commands/install/prepare.rs`,
  `apps/conary/src/commands/install/resolve.rs`,
  `apps/conary/src/commands/install/restore.rs`,
  `apps/conary/src/commands/install/scriptlets.rs`,
  `apps/conary/src/commands/install/system_pm.rs`,
  `apps/conary/src/commands/live_root.rs`,
  `apps/conary/src/commands/remove.rs`,
  `apps/conary/src/commands/remove/command.rs`,
  `apps/conary/src/commands/remove/autoremove.rs`,
  `apps/conary/src/commands/remove/transaction.rs`,
  `apps/conary/src/commands/remove/scriptlets.rs`,
  `apps/conary/src/commands/remove/legacy_replay.rs`,
  `apps/conary/src/commands/remove/execution_path.rs`,
  `apps/conary/src/commands/remove/types.rs`,
  `crates/conary-core/src/scriptlet/mod.rs`,
  `crates/conary-core/src/scriptlet/executor.rs`,
  `crates/conary-core/src/scriptlet/arguments.rs`,
  `crates/conary-core/src/scriptlet/sandbox.rs`,
  `crates/conary-core/src/scriptlet/process.rs`,
  `crates/conary-core/src/scriptlet/legacy.rs`,
  `crates/conary-core/src/scriptlet/runtime.rs`, and
  `docs/modules/test-fixtures.md`
- Remove command direct helper fixtures and tests:
  `apps/conary/src/commands/remove/test_support.rs`
- Generation building, artifact export, composefs mounting, `/etc` merge, and GC:
  `crates/conary-core/src/generation/builder.rs`,
  `crates/conary-core/src/generation/builder/create.rs`,
  `crates/conary-core/src/generation/builder/rebuild.rs`,
  `crates/conary-core/src/generation/builder/boot_assets.rs`,
  `crates/conary-core/src/generation/builder/initramfs.rs`,
  `crates/conary-core/src/generation/builder/kernel.rs`,
  `crates/conary-core/src/generation/builder/root_validation.rs`,
  `crates/conary-core/src/generation/builder/runtime_inputs.rs`,
  `crates/conary-core/src/generation/builder/erofs.rs`,
  `crates/conary-core/src/generation/artifact.rs`,
  `crates/conary-core/src/generation/export.rs`,
  `crates/conary-core/src/image/repart.rs`,
  `crates/conary-core/src/generation/mount.rs`,
  `crates/conary-core/src/generation/etc_merge.rs`, and
  `crates/conary-core/src/generation/gc.rs`
- Adoption, unadoption, and selected-generation native-authority handoff:
  `apps/conary/src/cli/system.rs` ->
  `apps/conary/src/dispatch/system.rs` ->
  `apps/conary/src/commands/adopt/`,
  `apps/conary/src/commands/adopt/mod.rs`,
  `apps/conary/src/commands/adopt/system.rs`,
  `apps/conary/src/commands/adopt/packages.rs`,
  `apps/conary/src/commands/adopt/refresh.rs`,
  `apps/conary/src/commands/adopt/convert.rs`,
  `apps/conary/src/commands/adopt/hooks.rs`,
  `apps/conary/src/commands/adopt/status.rs`,
  `apps/conary/src/commands/adopt/unadopt.rs`,
  `apps/conary/src/commands/adopt/native_handoff.rs`, and
  `apps/conary/tests/integration/remi/manifests/phase3-active-generation-handoff.toml`
- System state, rollback, verify, GC, provenance, and live-root helpers:
  `apps/conary/src/commands/system.rs`,
  `apps/conary/src/commands/state.rs`,
  `apps/conary/src/commands/provenance.rs`, and
  `apps/conary/src/commands/live_root.rs`
- CCS package building, chunking, verification, conversion, install, and
  fixture proof:
  `crates/conary-core/src/ccs/builder.rs`,
  `crates/conary-core/src/ccs/manifest.rs`,
  `crates/conary-core/src/ccs/manifest_provenance.rs`,
  `crates/conary-core/src/ccs/binary_manifest.rs`,
  `crates/conary-core/src/ccs/chunking.rs`,
  `crates/conary-core/src/ccs/convert/`,
  `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`,
  `crates/conary-core/src/ccs/convert/scriptlet_bundle/`,
  `apps/conary/src/commands/ccs/install/command.rs`,
  `apps/conary/src/commands/ccs/install/dependency.rs`,
  `apps/conary/src/commands/ccs/payload_paths.rs`, and
  `docs/modules/test-fixtures.md`
- TUF trust and signature verification:
  `crates/conary-core/src/trust/verify.rs`,
  `crates/conary-core/src/trust/client.rs`, and
  `crates/conary-core/src/trust/keys.rs`
- System bootstrap from scratch, prerequisite validation, seed generation,
  image creation, and run orchestration:
  `apps/conary/src/commands/bootstrap/mod.rs`,
  `apps/conary/src/commands/bootstrap/setup.rs`,
  `apps/conary/src/commands/bootstrap/phases.rs`,
  `apps/conary/src/commands/bootstrap/image.rs`,
  `apps/conary/src/commands/bootstrap/run.rs`,
  `apps/conary/src/commands/bootstrap/run_record.rs`,
  `apps/conary/src/commands/bootstrap/run_artifact.rs`,
  `apps/conary/src/commands/bootstrap/seed.rs`,
  `apps/conary/src/commands/bootstrap/convergence.rs`,
  `apps/conary/src/commands/bootstrap/cleanup.rs`,
  `apps/conary/src/commands/bootstrap/types.rs`, and
  `apps/conary/src/commands/bootstrap/state.rs`
- Shared operation vocabulary and daemon-boundary ownership:
  `crates/conary-core/src/operations.rs`,
  `apps/conaryd/src/daemon/mod.rs`,
  `apps/conaryd/src/daemon/package_ops.rs`, and
  `apps/conaryd/src/daemon/routes/transactions.rs`
- Remi admin, conversion, publication, artifact fixture, and MCP flows:
  `apps/remi/src/server/admin_service.rs`,
  `apps/remi/src/server/release_publish.rs`,
  `apps/remi/src/server/conversion.rs`,
  `apps/remi/src/server/conversion/workflow.rs`,
  `apps/remi/src/server/conversion/persistence.rs`,
  `apps/remi/src/server/conversion/lookup.rs`,
  `apps/remi/src/server/conversion/metadata.rs`,
  `apps/remi/src/server/publication.rs`,
  `apps/remi/src/server/mcp.rs`,
  `apps/remi/src/server/handlers/artifacts.rs`,
  `apps/remi/src/server/handlers/admin/`, and
  `docs/modules/test-fixtures.md`
- conary-test HTTP and MCP service layer:
  `apps/conary-test/src/server/service.rs`,
  `apps/conary-test/src/server/mcp.rs`, and
  `apps/conary-test/src/engine/`
- CLI command routing, live-mutation command labels, and namespace dispatch:
  `apps/conary/src/dispatch.rs`,
  `apps/conary/src/dispatch/root.rs`,
  `apps/conary/src/dispatch/context.rs`,
  `apps/conary/src/dispatch/system.rs`,
  `apps/conary/src/dispatch/system_state.rs`,
  `apps/conary/src/dispatch/system_generation.rs`,
  `apps/conary/src/dispatch/ccs.rs`,
  `apps/conary/src/dispatch/model.rs`,
  `apps/conary/src/dispatch/automation.rs`, and
  `apps/conary/src/dispatch/`
- Agent operation contract and MCP adapter helpers:
  `crates/conary-agent-contract/src/` and `crates/conary-mcp/src/`
- conaryd daemon routes and auth boundaries:
  `apps/conaryd/src/daemon/mod.rs`,
  `apps/conaryd/src/daemon/package_ops.rs`,
  `apps/conaryd/src/daemon/routes.rs`,
  `apps/conaryd/src/daemon/routes/router.rs`,
  `apps/conaryd/src/daemon/routes/auth.rs`,
  `apps/conaryd/src/daemon/routes/types.rs`,
  `apps/conaryd/src/daemon/routes/errors.rs`,
  `apps/conaryd/src/daemon/routes/db.rs`,
  `apps/conaryd/src/daemon/routes/sse.rs`,
  `apps/conaryd/src/daemon/routes/{system,transactions,query,events}.rs`,
  `apps/conaryd/src/daemon/auth.rs`, and
  `apps/conaryd/src/daemon/jobs.rs`

  Package install/remove/update jobs currently adapt daemon requests into the
  CLI command functions from the `conary` crate. When changing package-job
  behavior, inspect both `apps/conaryd/src/daemon/package_ops.rs` and the
  relevant CLI command owner under `apps/conary/src/commands/`.

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
