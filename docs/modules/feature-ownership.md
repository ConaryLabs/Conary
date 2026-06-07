---
last_updated: 2026-06-06
revision: 1
summary: Feature ownership cards with focused and cross-system verification gates
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

## Native Package Install, Update, Remove, And Live-Root Mutation

**Capability:** install, update, remove, restore, batch, scriptlet, and live-root
mutation flows for local package operations.

**Start here:** `apps/conary/src/commands/install/mod.rs`;
`apps/conary/src/commands/install/legacy_replay.rs`;
`apps/conary/src/commands/install/inner.rs`;
`apps/conary/src/commands/install/batch.rs`;
`apps/conary/src/commands/install/restore.rs`;
`apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
`apps/conary/src/commands/remove.rs`;
`docs/modules/test-fixtures.md`; `docs/operations/daily-driver-ux-matrix.md`.

**Neighbor systems:** `crates/conary-core/src/transaction/`;
`crates/conary-core/src/db/`; `crates/conary-core/src/scriptlet/`;
`crates/conary-core/src/ccs/legacy_replay.rs`; conaryd package jobs.

**Focused proof:** `cargo test -p conary --test live_host_mutation_safety`;
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

**Start here:** `apps/conary/src/commands/adopt/mod.rs`;
`apps/conary/src/commands/adopt/unadopt.rs`;
`apps/conary/src/commands/adopt/native_handoff.rs`;
`docs/modules/source-selection.md`; `docs/ARCHITECTURE.md`.

**Neighbor systems:** `apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
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

## Generation Build, Switch, Recovery, And Export

**Capability:** build generation artifacts, select complete generations for the
next boot, recover publication debt, and export raw/qcow2/ISO carriers.

**Start here:** `crates/conary-core/src/generation/builder.rs`;
`crates/conary-core/src/generation/builder/runtime_inputs.rs`;
`crates/conary-core/src/generation/export.rs`;
`crates/conary-core/src/generation/artifact.rs`;
`crates/conary-core/src/generation/gc.rs`;
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
`crates/conary-core/src/ccs/legacy_replay.rs`;
`apps/conary/src/commands/ccs/`; `docs/modules/ccs.md`;
`docs/modules/test-fixtures.md`;
`docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`.

**Neighbor systems:** install orchestration, Remi publication, repository
metadata, scriptlet sandboxing, fixture maps.

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

## Remi Publication, Serving, Admin, And Fixture Artifacts

**Capability:** ingest, convert, publish, index, search, and serve CCS artifacts
and static test fixtures through Remi.

**Start here:** `apps/remi/src/server/publication.rs`;
`apps/remi/src/server/conversion.rs`; `apps/remi/src/server/index_gen.rs`;
`apps/remi/src/server/prewarm.rs`; `apps/remi/src/server/handlers/`;
`docs/modules/remi.md`; `docs/modules/test-fixtures.md`.

**Neighbor systems:** CCS conversion metadata, repository client behavior,
federation peer state, admin audit logs, artifact path handling.

**Focused proof:** `cargo test -p remi publication`;
`cargo test -p remi test_upload_fixture`;
`cargo test -p remi test_public_fixture_get_and_head`.

**Interaction gate:** `cargo test -p remi`;
`cargo test -p conary --test conversion_integration golden_conversion` when
serving behavior depends on conversion output.

**Docs to update:** `docs/modules/remi.md`; `docs/modules/test-fixtures.md`;
`docs/llms/subsystem-map.md`; operator docs when deployment behavior changes.

**Safety notes:** do not expose non-public scriptlet rows, private review paths,
or unverified native package signatures through public listings.

## conaryd Package Jobs And Daemon Routes

**Capability:** accept local daemon requests, authenticate socket access, queue
package jobs, expose job state, and stream route lifecycle events.

**Start here:** `apps/conaryd/src/daemon/mod.rs`;
`apps/conaryd/src/daemon/routes.rs`;
`apps/conaryd/src/daemon/routes/`; `apps/conaryd/src/daemon/jobs.rs`;
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

**Start here:** `apps/conary/src/commands/bootstrap/`;
`apps/conary-test/src/bootstrap.rs`;
`docs/modules/bootstrap.md`;
`docs/operations/bootstrap-selfhosting-vm.md`;
`docs/operations/bootstrap-follow-up-investigations.md`.

**Neighbor systems:** recipe versions, image generation, QEMU validation,
container runtime availability, ignored local artifact paths.

**Focused proof:** `cargo run -p conary-test -- bootstrap check --json`;
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
