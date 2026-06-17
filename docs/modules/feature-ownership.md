---
last_updated: 2026-06-17
revision: 16
summary: Route M3 packaging ownership
---

# Feature Ownership And Interaction Gates

This map helps contributors and agents choose one Conary capability, start in
the right files, and know when a narrow edit needs broader verification. It does
not replace `docs/ARCHITECTURE.md`, subsystem docs, or tests.

## How To Use This Map

- Pick the capability that matches the change.
- Read the `Start here` files before editing.
- Use the focused proof for small local edits.
- Use the interaction gate when behavior crosses a listed neighbor system.
- Update the named docs when a "look here first" path, public behavior, or proof
  command changes.

Public package and repository support claims stay limited to Fedora 44,
Ubuntu 26.04, and Arch.

## Card Schema

Each ownership card uses these fields:

- **Capability:** what user-facing or contributor-facing job this area owns.
- **Start here:** owner files and canonical docs to read first.
- **Neighbor systems:** nearby systems that often need verification when
  behavior changes.
- **Focused proof:** narrow command for small edits.
- **Interaction gate:** broader command when the change crosses a boundary.
- **Docs to update:** docs that should move with the feature.
- **Safety notes:** persisted-state, trust, host mutation, fixture,
  private-path, or distro-scope boundaries.

## CLI Dispatch And Command Routing

**Capability:** route parsed CLI command variants to command implementations
while preserving live-mutation labels, dry-run bypasses, command risk checks,
and top-level command UX.

**Start here:** `apps/conary/src/dispatch.rs`;
`apps/conary/src/dispatch/root.rs`; `apps/conary/src/dispatch/context.rs`;
`apps/conary/src/dispatch/`; `apps/conary/src/cli/`;
`apps/conary/src/command_risk.rs`; `apps/conary/src/live_host_safety.rs`.

**Neighbor systems:** command implementation modules under
`apps/conary/src/commands/`, Clap command definitions under
`apps/conary/src/cli/`, conaryd package-job compatibility, and integration
tests that exercise CLI surfaces.

**Focused proof:** `cargo check -p conary`;
`cargo test -p conary --lib cli::tests`;
`cargo test -p conary --test live_host_mutation_safety`;
`cargo run -p conary -- system completions bash >/dev/null`.

**Interaction gate:** `cargo test -p conary --test query`;
`cargo test -p conary --test query_scripts`;
`cargo test -p conary --test cli_daily_ux`;
`cargo test -p conary --lib commands::model` when routing crosses query,
completion, UX, model, or live-mutation behavior.

**Docs to update:** `docs/ARCHITECTURE.md`;
`docs/llms/subsystem-map.md`; `docs/modules/feature-ownership.md`;
`docs/modules/query.md` when query or SBOM routing paths move.

**Safety notes:** keep `command_risk::enforce_cli_policy` ahead of command
routing, preserve `require_live_mutation` labels/classes/dry-run arguments
exactly, and do not add new command surfaces without matching CLI and dispatch
proof.

## Native Package Install, Update, Remove, And Live-Root Mutation

**Capability:** install, update, remove, restore, batch, scriptlet, and live-root
mutation flows for local package operations.

**Start here:** `apps/conary/src/commands/install/mod.rs`;
`apps/conary/src/commands/install/` for child modules;
`apps/conary/src/commands/install/command.rs`;
`apps/conary/src/commands/install/acquire.rs`;
`apps/conary/src/commands/install/blocklist.rs`;
`apps/conary/src/commands/install/ccs_transaction.rs`;
`apps/conary/src/commands/install/conversion.rs`;
`apps/conary/src/commands/install/dep_mode.rs`;
`apps/conary/src/commands/install/dep_resolution.rs`;
`apps/conary/src/commands/install/validation.rs`;
`apps/conary/src/commands/install/dependencies.rs`;
`apps/conary/src/commands/install/execute.rs`;
`apps/conary/src/commands/install/lifecycle.rs`;
`apps/conary/src/commands/install/transaction.rs`;
`apps/conary/src/commands/install/options.rs`;
`apps/conary/src/commands/install/semantics.rs`;
`apps/conary/src/commands/install/source_policy.rs`;
`apps/conary/src/commands/install/legacy_replay.rs`;
`apps/conary/src/commands/install/inner.rs`;
`apps/conary/src/commands/install/batch.rs`;
`apps/conary/src/commands/install/prepare.rs`;
`apps/conary/src/commands/install/resolve.rs`;
`apps/conary/src/commands/install/restore.rs`;
`apps/conary/src/commands/install/scriptlets.rs`;
`apps/conary/src/commands/install/system_pm.rs`;
`apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/package.rs`;
`apps/conary/src/commands/update/source_policy.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
`apps/conary/src/commands/update/collection.rs`;
`apps/conary/src/commands/update/pinning.rs`;
`apps/conary/src/commands/update/delta_stats.rs`;
`apps/conary/src/commands/remove.rs`;
`apps/conary/src/commands/remove/command.rs`;
`apps/conary/src/commands/remove/autoremove.rs`;
`apps/conary/src/commands/remove/transaction.rs`;
`apps/conary/src/commands/remove/scriptlets.rs`;
`apps/conary/src/commands/remove/legacy_replay.rs`;
`apps/conary/src/commands/remove/execution_path.rs`;
`apps/conary/src/commands/remove/types.rs`;
`apps/conary/src/commands/system.rs`;
`apps/conary/src/commands/live_root.rs`;
`docs/modules/test-fixtures.md`; `docs/operations/daily-driver-ux-matrix.md`.

**Neighbor systems:** `crates/conary-core/src/transaction/`;
`crates/conary-core/src/db/`; `crates/conary-core/src/scriptlet/mod.rs`;
`crates/conary-core/src/scriptlet/executor.rs`;
`crates/conary-core/src/scriptlet/sandbox.rs`;
`crates/conary-core/src/scriptlet/process.rs`;
`crates/conary-core/src/scriptlet/legacy.rs`;
`crates/conary-core/src/ccs/legacy_replay.rs`;
`apps/conary/src/commands/state.rs`;
`apps/conary/src/commands/provenance.rs`; conaryd package jobs.

**Focused proof:** `cargo test -p conary --lib commands::remove`;
`cargo test -p conary --test live_host_mutation_safety`;
`cargo test -p conary --lib legacy_replay`.

**Interaction gate:** `cargo test -p conary --test bundle_replay`;
`cargo test -p conary --test foreign_replay`;
`cargo test -p conary --test query_scripts`; `cargo test -p conaryd daemon::routes`
when daemon package jobs are affected.

**Docs to update:** `docs/llms/subsystem-map.md`;
`docs/modules/feature-ownership.md`; `docs/modules/test-fixtures.md`;
`docs/operations/daily-driver-ux-matrix.md`.

**Safety notes:** do not weaken live-host mutation acknowledgement,
refusal-before-mutation, persisted bundle replay, private-path redaction, or
legacy replay refusal gates.

## Adoption, Unadoption, And Native-Authority Handoff

**Capability:** preserve native package-manager authority, support explicit
takeover, recover selected-generation handoff state, and provide non-destructive
escape hatches.

**Start here:** `apps/conary/src/cli/system.rs` ->
`apps/conary/src/dispatch/system.rs` ->
`apps/conary/src/commands/adopt/`;
`apps/conary/src/commands/adopt/mod.rs`;
`apps/conary/src/commands/adopt/system.rs`;
`apps/conary/src/commands/adopt/packages.rs`;
`apps/conary/src/commands/adopt/refresh.rs`;
`apps/conary/src/commands/adopt/convert.rs`;
`apps/conary/src/commands/adopt/hooks.rs`;
`apps/conary/src/commands/adopt/status.rs`;
`apps/conary/src/commands/adopt/unadopt.rs`;
`apps/conary/src/commands/adopt/native_handoff.rs`;
`docs/modules/source-selection.md`; `docs/ARCHITECTURE.md`.

**Neighbor systems:** `apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/package.rs`;
`apps/conary/src/commands/update/source_policy.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
`apps/conary/src/commands/update/collection.rs`;
`apps/conary/src/commands/install/`; `crates/conary-core/src/repository/`;
`crates/conary-core/src/generation/`; integration manifests under
`apps/conary/tests/integration/remi/manifests/`.

**Focused proof:** `cargo test -p conary --lib adopt::native_handoff`;
`cargo test -p conary --lib adopt::unadopt`.

**Interaction gate:** `cargo run -p conary-test -- list`;
`cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3`
when selected-generation handoff behavior changes.

**Docs to update:** `docs/modules/source-selection.md`;
`docs/llms/subsystem-map.md`; `docs/INTEGRATION-TESTING.md`.

**Safety notes:** do not silently take over adopted packages or erase native
package-manager authority without an explicit takeover path.

## Declarative System Models And Replatform Planning

**Capability:** diff, apply, check, snapshot, publish, lock, update, and
remote-diff declarative system model files while preserving source-policy and
replatform convergence behavior.

**Start here:** `apps/conary/src/commands/model.rs`;
`apps/conary/src/commands/model/context.rs`;
`apps/conary/src/commands/model/presentation.rs`;
`apps/conary/src/commands/model/diff.rs`;
`apps/conary/src/commands/model/apply.rs`;
`apps/conary/src/commands/model/check.rs`;
`apps/conary/src/commands/model/snapshot.rs`;
`apps/conary/src/commands/model/remote_diff.rs`;
`apps/conary/src/commands/model/lock.rs`;
`apps/conary/src/commands/model/publish.rs`;
`crates/conary-core/src/model/parser.rs`;
`crates/conary-core/src/model/replatform.rs`;
`docs/modules/source-selection.md`.

**Neighbor systems:** install/remove execution, update source-policy selection,
repository remote include cache, derived package builds, live-host mutation
acknowledgement, and conaryd package-job request compatibility.

**Focused proof:** `cargo test -p conary --lib commands::model`.

**Interaction gate:** `cargo test -p conary model`;
`cargo test -p conary --test model_apply`;
`cargo test -p conary --test live_host_mutation_safety model` when apply
behavior or live-mutation safety changes.

**Docs to update:** `docs/modules/source-selection.md`;
`docs/llms/subsystem-map.md`; `docs/ARCHITECTURE.md`.

**Safety notes:** preserve `model check` drift exit code 2, source-policy
persistence semantics, executable replatform planning boundaries, lockfile
reproducibility, remote include cache behavior, and refusal-before-live-mutation
gates.

## Generation Build, Switch, Recovery, And Export

**Capability:** build generation artifacts, select complete generations for the
next boot, recover publication debt, and export raw/qcow2/ISO carriers.

**Start here:** `crates/conary-core/src/generation/builder.rs`;
`crates/conary-core/src/generation/builder/create.rs`;
`crates/conary-core/src/generation/builder/rebuild.rs`;
`crates/conary-core/src/generation/builder/boot_assets.rs`;
`crates/conary-core/src/generation/builder/initramfs.rs`;
`crates/conary-core/src/generation/builder/kernel.rs`;
`crates/conary-core/src/generation/builder/root_validation.rs`;
`crates/conary-core/src/generation/builder/runtime_inputs.rs`;
`crates/conary-core/src/generation/builder/erofs.rs`;
`crates/conary-core/src/generation/export.rs`;
`crates/conary-core/src/generation/artifact.rs`;
`crates/conary-core/src/generation/gc.rs`;
`apps/conary/src/commands/system.rs`;
`apps/conary/src/commands/state.rs`;
`apps/conary/src/commands/provenance.rs`;
`docs/operations/post-generation-export-follow-up-roadmap.md`.

**Neighbor systems:** transaction commit, SQLite generation state, image
building, bootstrap validation, conaryd route history.

**Focused proof:** `cargo test -p conary-core generation::export`;
`cargo test -p conary-core generation::builder`.

**Interaction gate:** `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`;
`cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`
for export or boot-carrier behavior.

**Docs to update:** `docs/ARCHITECTURE.md`;
`docs/operations/post-generation-export-follow-up-roadmap.md`;
`docs/INTEGRATION-TESTING.md`; `docs/llms/subsystem-map.md`.

**Safety notes:** generation state and artifact formats are persisted behavior;
schema or format changes require explicit compatibility decisions.

## CCS Authoring, Conversion, Install, And Legacy Replay

**Capability:** build native CCS packages, convert legacy package formats,
install CCS packages, and preserve/replay legacy scriptlet metadata safely.

**Start here:** `crates/conary-core/src/ccs/`;
`crates/conary-core/src/ccs/convert/`;
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`;
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`;
`crates/conary-core/src/ccs/legacy_replay.rs`;
`apps/conary/src/commands/ccs/`;
`apps/conary/src/commands/ccs/install.rs`;
`apps/conary/src/commands/ccs/install/command.rs`;
`apps/conary/src/commands/ccs/install/dependency.rs`;
`apps/conary/src/commands/ccs/install/component_selection.rs`;
`apps/conary/src/commands/ccs/install/capability_policy.rs`;
`apps/conary/src/commands/ccs/payload_paths.rs`;
`docs/modules/ccs.md`;
`docs/modules/test-fixtures.md`;
`docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`.

**Neighbor systems:** install orchestration, Remi publication, repository
metadata, scriptlet sandboxing (`crates/conary-core/src/scriptlet/mod.rs`,
`crates/conary-core/src/scriptlet/executor.rs`,
`crates/conary-core/src/scriptlet/sandbox.rs`,
`crates/conary-core/src/scriptlet/process.rs`,
`crates/conary-core/src/scriptlet/legacy.rs`), fixture maps.

**Focused proof:** `cargo test -p conary-core golden_fixtures`;
`cargo test -p conary-core support_matrix`;
`cargo test -p conary-core legacy_replay`.

**Interaction gate:** `cargo test -p conary --test conversion_integration golden_conversion`;
`cargo test -p conary --test bundle_replay`;
`cargo test -p remi publication` when conversion output affects public serving.

**Docs to update:** `docs/modules/ccs.md`; `docs/modules/test-fixtures.md`;
`docs/llms/subsystem-map.md`; CCS roadmap child specs when active.

**Safety notes:** text-pattern detections are advisory, public-ready serving is
gated by adapter/support-matrix evidence, and raw legacy replay remains local
and fail-closed.

## Packaging, Try Sessions, And Static Repository Publishing

**Capability:** infer and materialize package recipes from source trees,
build recipe or inferred-source CCS packages, try a built artifact with an
explicit keep/rollback decision, publish recipe-built CCS packages to local
static repositories, establish root trust, sync TUF-verified indexes, and
install packages only when their CCS signatures chain to active package keys
pinned by the repository.

**Start here:** `docs/specs/static-repo-format-v1.md`;
`docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`;
`docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`;
`docs/superpowers/specs/2026-06-17-m3d-record-mode-spike-design.md`;
`docs/superpowers/specs/2026-06-13-m2-publish-hardening-remi-design.md`;
`docs/superpowers/specs/2026-06-14-m2a-builder-config-publish-divergence-design.md`;
`docs/superpowers/plans/2026-06-14-m2a-hermetic-publish-foundation-implementation-plan.md`;
`docs/guides/first-package.md`;
`crates/conary-core/src/recipe/inference/`;
`crates/conary-core/src/recipe/hermetic/`;
`crates/conary-core/src/recipe/kitchen/`;
`crates/conary-core/src/diagnostics/`;
`apps/conary/src/commands/packaging_mcp/`;
`crates/conary-core/src/db/models/try_session.rs`;
`apps/conary/src/commands/new.rs`;
`apps/conary/src/commands/publish.rs`;
`apps/conary/src/commands/cook.rs`;
`apps/conary/src/commands/diagnostics.rs`;
`apps/conary/src/commands/operation_records.rs`;
`apps/conary/src/commands/hermetic_config.rs`;
`apps/conary/src/commands/hermetic_state.rs`;
`apps/conary/src/commands/try_session/`;
`apps/conary/src/commands/try_session/watch.rs`;
`apps/conary/src/commands/try_session/watch_source.rs`;
`apps/conary/src/commands/repo_static.rs`;
`apps/conary/tests/packaging_m1b.rs`;
`apps/conary/tests/packaging_m2a.rs`;
`apps/conary/tests/packaging_m3a.rs`;
`apps/conary/tests/packaging_m3c.rs`;
`apps/conary/tests/packaging_m3b.rs`;
`crates/conary-agent-contract/src/{resource,catalog,result}.rs`;
`crates/conary-mcp/src/`;
`crates/conary-core/src/ccs/attestation.rs`;
`crates/conary-core/src/repository/static_repo/`;
`crates/conary-core/src/trust/`;
`crates/conary-core/src/ccs/signing.rs`.

**Neighbor systems:** CLI command routing and command-risk labels, source-target
fetch/materialization, recipe Kitchen source fetching and provenance, try
session SQLite state, generation building/current-generation selection, install
acquisition and static package signature policy, repository sync
orchestration, CCS signing/verification, TUF metadata verification, and
docs-audit truth gates.

**Focused proof:** `cargo test -p conary-core repository::static_repo`;
`cargo test -p conary-core recipe::hermetic`;
`cargo test -p conary-core recipe::kitchen`;
`cargo test -p conary-core trust::client`;
`cargo test -p conary-core trust::verify`;
`cargo test -p conary-core db::models::try_session`;
`cargo test -p conary --test static_repo_m1a`;
`cargo test -p conary --test packaging_m2a`;
`cargo test -p conary --test packaging_m3a`;
`cargo test -p conary --test packaging_m3b`;
`cargo test -p conary commands::diagnostics::tests`;
`cargo test -p conary commands::packaging_mcp`;
`cargo test -p conary --lib commands::try_session`;
`cargo test -p conary --lib dispatch::root`;
`cargo test -p conary --test packaging_m1b`;
`cargo test -p conary --test packaging_m3c`.

**Interaction gate:** `cargo test -p conary-core`;
`cargo test -p conary`;
`cargo run -p conary-test -- list`;
`cargo clippy --workspace --all-targets -- -D warnings` when changes cross
publish, trust establishment, sync, install, or package-signing boundaries.

**Docs to update:** `docs/specs/static-repo-format-v1.md`;
`docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`;
`docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`;
`docs/superpowers/specs/2026-06-13-m2-publish-hardening-remi-design.md`;
`docs/superpowers/specs/2026-06-14-m2a-builder-config-publish-divergence-design.md`;
`docs/guides/first-package.md`;
`docs/ARCHITECTURE.md`; `docs/llms/subsystem-map.md`;
`docs/modules/feature-ownership.md`.

**Safety notes:** keep `conary new`, `conary try`, and `cook --explain`
visibility aligned with the active rollout gate; try rollback/keep decisions
operate on the selected database/runtime and must preserve the one-active-session
invariant. After M2a, `conary cook --isolated` and project-form
`conary publish <target>` must use hermetic Kitchen execution before emitting
`hardening_level = "hermetic"` and a signed build-attestation envelope.
Artifact-form `conary publish <pkg.ccs> <target>` must pass
`publish_gate.rs` checks for package signatures, TOML integrity, attestation
authority, output identity, command-risk evidence, and foreign-boundary hashes
before static publication or Remi release upload. Never parse static `index.json` or
`keys/package-keys.json` before TUF target length/hash verification succeeds;
do not allow `--allow-unsigned` to bypass static repository package signature
checks; keep static repo GPG and TUF trust surfaces separate; retired package
keys are audit/history only unless a later compatibility task explicitly
changes that policy.

### M3a Packaging Diagnostics

Start with `crates/conary-core/src/diagnostics/` for the shared diagnostic,
event, redaction, and JSON schema contract. CLI rendering and operation-record
glue live in `apps/conary/src/commands/diagnostics.rs`; command-specific report
construction stays in `cook.rs` and `publish.rs`.

### M3b Packaging MCP

Start with `apps/conary/src/commands/packaging_mcp/` for local stdio MCP tools,
agent projection, publish plan registry, and read-only operation-record/project
inspection. Transport-neutral resource and catalog vocabulary lives in
`crates/conary-agent-contract/src/{resource,catalog,result}.rs`; generic MCP
helpers live in `crates/conary-mcp/src/`. Publish mutations remain owned by
`apps/conary/src/commands/publish.rs`.

### M3c Try Watch Mode

Start with `apps/conary/src/commands/try_session/watch.rs` for watch lifecycle,
event streaming, refresh retry behavior, and cancellation. Source-set discovery,
identity hashing, and debounce live in
`apps/conary/src/commands/try_session/watch_source.rs`; staged generation
refresh remains behind the try-session API in `session.rs` and namespace
switching helpers in `namespace.rs`.

## Remi Publication, Serving, Admin, And Fixture Artifacts

**Capability:** ingest, convert, publish, index, search, and serve CCS artifacts,
release uploads, and static test fixtures through Remi.

**Start here:** `apps/remi/src/server/release_publish.rs`;
`apps/remi/src/server/publication.rs`;
`apps/remi/src/server/conversion.rs`;
`apps/remi/src/server/conversion/types.rs`;
`apps/remi/src/server/conversion/workflow.rs`;
`apps/remi/src/server/conversion/persistence.rs`;
`apps/remi/src/server/conversion/lookup.rs`;
`apps/remi/src/server/conversion/metadata.rs`;
`apps/remi/src/server/conversion/safety.rs`;
`apps/remi/src/server/conversion/storage.rs`;
`apps/remi/src/server/conversion/recipe.rs`;
`apps/remi/src/server/conversion/benchmark.rs`;
`apps/remi/src/server/index_gen.rs`;
`apps/remi/src/server/prewarm.rs`; `apps/remi/src/server/handlers/`;
`docs/modules/remi.md`; `docs/modules/test-fixtures.md`.

**Neighbor systems:** CCS conversion metadata, repository client behavior,
federation peer state, admin audit logs, artifact path handling.

**Focused proof:** `cargo test -p remi release_upload_`;
`cargo test -p remi remi_release_parity`;
`cargo test -p remi publication`;
`cargo test -p remi test_upload_fixture`;
`cargo test -p remi test_public_fixture_get_and_head`.

**Interaction gate:** `cargo test -p remi`;
`cargo test -p conary --test conversion_integration golden_conversion` when
serving behavior depends on conversion output.

**Docs to update:** `docs/modules/remi.md`; `docs/modules/test-fixtures.md`;
`docs/llms/subsystem-map.md`; operator docs when deployment behavior changes.

**Safety notes:** do not expose non-public scriptlet rows, private review paths,
or unverified native package signatures through public listings. Remi release
uploads must stage privately, enforce trusted build-attestation signer policy,
and publish package rows, chunks, and TUF targets only after the shared gate
passes.

## conaryd Package Jobs And Daemon Routes

**Capability:** accept local daemon requests, authenticate socket access, queue
package jobs, expose job state, and stream route lifecycle events.

**Start here:** `apps/conaryd/src/daemon/mod.rs`;
`apps/conaryd/src/daemon/routes.rs`;
`apps/conaryd/src/daemon/routes/router.rs`;
`apps/conaryd/src/daemon/routes/auth.rs`;
`apps/conaryd/src/daemon/routes/types.rs`;
`apps/conaryd/src/daemon/routes/errors.rs`;
`apps/conaryd/src/daemon/routes/db.rs`;
`apps/conaryd/src/daemon/routes/sse.rs`;
`apps/conaryd/src/daemon/routes/transactions.rs`;
`apps/conaryd/src/daemon/routes/query.rs`;
`apps/conaryd/src/daemon/routes/system.rs`;
`apps/conaryd/src/daemon/routes/events.rs`;
`apps/conaryd/src/daemon/jobs.rs`;
`docs/modules/conaryd.md`.

**Neighbor systems:** Conary CLI package commands, SQLite `daemon_jobs` state,
operation vocabulary in `crates/conary-core/src/operations.rs`, live-host
mutation acknowledgement.

**Focused proof:** `cargo test -p conaryd daemon::routes` for route behavior;
`cargo test -p conaryd daemon` for broader daemon behavior including auth, jobs,
and route lifecycle.

**Interaction gate:** `cargo test -p conary --test cli_daily_ux` when CLI
diagnostics change; `cargo test -p conaryd` for route/job behavior.

**Docs to update:** `docs/modules/conaryd.md`;
`docs/llms/subsystem-map.md`; `docs/operations/infrastructure.md` for host
workflow changes.

**Safety notes:** preserve job idempotency, queued/running restart behavior,
SSE lifecycle, socket auth, and live-host mutation boundaries.

## Bootstrap And Self-Hosting

**Capability:** validate bootstrap prerequisites, build self-hosting images,
run dry-run smoke checks, and support local QEMU validation.

**Start here:** `apps/conary/src/commands/bootstrap/mod.rs`;
`apps/conary/src/commands/bootstrap/setup.rs`;
`apps/conary/src/commands/bootstrap/phases.rs`;
`apps/conary/src/commands/bootstrap/image.rs`;
`apps/conary/src/commands/bootstrap/run.rs`;
`apps/conary/src/commands/bootstrap/run_record.rs`;
`apps/conary/src/commands/bootstrap/run_artifact.rs`;
`apps/conary/src/commands/bootstrap/seed.rs`;
`apps/conary/src/commands/bootstrap/convergence.rs`;
`apps/conary/src/commands/bootstrap/cleanup.rs`;
`apps/conary/src/commands/bootstrap/types.rs`;
`apps/conary/src/commands/bootstrap/state.rs`;
`apps/conary-test/src/bootstrap.rs`;
`docs/modules/bootstrap.md`;
`docs/operations/bootstrap-selfhosting-vm.md`;
`docs/operations/bootstrap-follow-up-investigations.md`.

**Neighbor systems:** recipe versions, image generation, QEMU validation,
container runtime availability, ignored local artifact paths.

**Focused proof:** `cargo test -p conary --lib commands::bootstrap`;
`cargo test -p conary --test bootstrap_workflow`;
`cargo run -p conary-test -- bootstrap check --json`;
`cargo run -p conary-test -- bootstrap smoke --dry-run --json`.

**Interaction gate:** `cargo run -p conary-test -- bootstrap smoke --json` only
when the local environment is intended to build or run the image.

**Docs to update:** `docs/modules/bootstrap.md`;
`docs/operations/bootstrap-selfhosting-vm.md`;
`docs/INTEGRATION-TESTING.md`; `docs/llms/subsystem-map.md`.

**Safety notes:** do not treat ignored local image paths, credentials, or
machine-specific artifacts as tracked repo truth. Non-dry-run bootstrap smoke
can start QEMU-backed validation and depends on local container/runtime
availability; keep dry-run smoke as the routine contributor gate unless the
task explicitly needs live image proof.

## conary-test Integration Execution

**Capability:** list, validate, and execute declarative integration suites,
including slow QEMU/KVM proof when release evidence needs it.

**Start here:** `apps/conary-test/src/`;
`apps/conary-test/src/suite_inventory.rs`;
`apps/conary-test/src/config/`;
`docs/INTEGRATION-TESTING.md`; `docs/modules/test-fixtures.md`.

**Neighbor systems:** package-manager CLI behavior, Remi fixture publication,
QEMU images, integration manifests, result JSON.

**Focused proof:** `cargo run -p conary-test -- list`;
`cargo test -p conary-test suite_inventory`.

**Interaction gate:** the suite command named by the touched manifest or
feature card, such as
`cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`
for native package-manager parity manifests or
`cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3`
for selected-generation handoff manifests.

**Docs to update:** `docs/INTEGRATION-TESTING.md`;
`docs/modules/test-fixtures.md`; affected feature cards.

**Safety notes:** manifest TOML is persisted test configuration; schema changes
need parser proof and migration or defaulting decisions. Suite names in
`--suite` arguments use the manifest filename stem, such as
`phase4-native-pm-parity`, not the human-readable title shown by
`cargo run -p conary-test -- list`.

## Agent/MCP Operation Surfaces

**Capability:** expose transport-neutral operation vocabulary and MCP adapters
for Conary, Remi, and `conary-test` automation.

**Start here:** `crates/conary-agent-contract/src/`;
`crates/conary-mcp/src/`; `apps/remi/src/server/mcp.rs`;
`apps/conary-test/src/server/mcp.rs`; `docs/operations/infrastructure.md`.

**Neighbor systems:** HTTP handlers, service-layer methods, operation risk
labels, resource references, and authentication.

**Focused proof:** `cargo test -p conary-agent-contract`;
`cargo test -p conary-mcp`.

**Interaction gate:** owning service tests such as `cargo test -p remi` or
`cargo test -p conary-test` when adapter changes call service behavior.

**Docs to update:** `docs/operations/infrastructure.md`;
`docs/llms/README.md`; `docs/llms/subsystem-map.md`.

**Safety notes:** keep `crates/conary-agent-contract` transport-neutral; MCP
code should adapt the contract rather than becoming product truth.
