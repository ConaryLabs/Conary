# Project Maintainability Phase 5/6 Feature Ownership And Workflow UX Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` or `superpowers:executing-plans`
> to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for
> tracking. This is a combined Phase 5/6 child packet under
> `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Make Conary's major user-visible capabilities easy to work on in
isolation while making their cross-system verification gates obvious to humans
and coding agents.

**Architecture:** Add a canonical feature ownership map rather than expanding
`AGENTS.md` or duplicating subsystem docs. The map names each capability's owner
files, neighboring systems, focused proof, broader interaction gate, and docs
that must move with the feature. Then route contributor and assistant entrypoints
to that map with narrow workflow updates.

**Tech Stack:** Markdown docs, existing docs-audit tooling, Cargo verification
commands, `conary-test` suite discovery, existing module docs.

---

## Status

Draft packet for review.

This packet combines Phase 5 subsystem ownership-map consolidation with Phase 6
contributor workflow UX because the highest-value artifact is the same for both:
a feature-focused map that lets someone choose one Conary capability, start in
the right code and docs, and see when a small edit crosses into broader
verification. It is documentation and workflow routing only. It does not move
Rust code, add scripts, change package behavior, simplify licensing, or alter
live-system mutation acknowledgement behavior.

## Read First

- `AGENTS.md`
- `CONTRIBUTING.md`
- `.github/PULL_REQUEST_TEMPLATE.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/modules/conaryd.md`
- `docs/modules/source-selection.md`
- `docs/modules/test-fixtures.md`
- `docs/operations/bootstrap-selfhosting-vm.md`
- `docs/operations/daily-driver-ux-matrix.md`
- `docs/operations/post-generation-export-follow-up-roadmap.md`
- `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md`

## Design Summary

`docs/llms/subsystem-map.md` is good at stable subsystem pointers, and
`docs/modules/test-fixtures.md` is good at fixture families. Neither answers the
contributor's feature-shaped question directly:

> I want to work on one useful Conary capability. Which code owns it, which
> neighbors can I break, and which command proves the change?

This slice adds `docs/modules/feature-ownership.md` as the missing middle layer.
It should not become an architecture manual. It should be a compact set of
ownership cards for the actual capabilities contributors are likely to pick up:

- Native package install, update, remove, and live-root mutation.
- Adoption, unadoption, and selected-generation native-authority handoff.
- Generation build, switch, recovery, and export.
- CCS authoring, conversion, install, and legacy replay.
- Remi publication, serving, admin, and fixture artifacts.
- conaryd package jobs and daemon routes.
- Bootstrap and self-hosting.
- `conary-test` integration execution.
- Agent/MCP operation surfaces.

Each card uses the same schema:

- **Capability:** short description.
- **Start here:** canonical owner files and docs.
- **Neighbor systems:** code or docs likely affected by behavioral changes.
- **Focused proof:** narrow command for small edits.
- **Interaction gate:** broader command when the change crosses a boundary.
- **Docs to update:** docs that should move with the feature.
- **Safety notes:** persisted state, trust, host mutation, distro scope,
  private-path, or fixture-publication boundaries.

The first implementation should add the map and route existing entrypoints to
it. Later packets can add scripts or richer templates after this map proves
useful in real work.

## Current Repo-Grounded Feature Areas

| Feature area | Current first files or docs | Current proof shape |
|--------------|-----------------------------|---------------------|
| Native install/update/remove | `apps/conary/src/commands/install/`, `apps/conary/src/commands/update.rs`, `apps/conary/src/commands/remove.rs` | `cargo test -p conary --test live_host_mutation_safety`; focused CLI tests |
| Adoption and handoff | `apps/conary/src/commands/adopt/`, `docs/modules/source-selection.md` | `cargo test -p conary --lib adopt::native_handoff`; `conary-test` handoff suites |
| Generation lifecycle | `crates/conary-core/src/generation/`, `docs/operations/post-generation-export-follow-up-roadmap.md` | `cargo test -p conary-core generation::export`; selected `conary-test` generation suites |
| CCS authoring/install/conversion | `crates/conary-core/src/ccs/`, `apps/conary/src/commands/ccs/`, `docs/modules/ccs.md` | `cargo test -p conary-core golden_fixtures`; `cargo test -p conary --test conversion_integration golden_conversion` |
| Remi publication/serving | `apps/remi/src/server/`, `docs/modules/remi.md` | `cargo test -p remi publication`; targeted Remi handler tests |
| conaryd jobs/routes | `apps/conaryd/src/daemon/`, `docs/modules/conaryd.md` | `cargo test -p conaryd daemon::routes`; daemon route/job tests |
| Bootstrap/self-hosting | `apps/conary/src/commands/bootstrap/`, `docs/operations/bootstrap-selfhosting-vm.md` | `cargo run -p conary-test -- bootstrap check --json`; dry-run smoke |
| Integration-test execution | `apps/conary-test/src/`, `docs/INTEGRATION-TESTING.md` | `cargo run -p conary-test -- list`; `cargo test -p conary-test suite_inventory` |
| Agent/MCP operations | `crates/conary-agent-contract/src/`, `crates/conary-mcp/src/` | affected package tests plus service MCP tests |

## Non-Goals

- Do not add or imply public distro support beyond Fedora 44, Ubuntu 26.04, and
  Arch.
- Do not change Rust code, CLI behavior, schemas, package formats,
  integration-test manifest semantics, trust defaults, or live-host mutation
  acknowledgement behavior.
- Do not fold MIT-only license simplification into this packet.
- Do not fold live-system mutation acknowledgement UX redesign into this
  packet.
- Do not create a parallel contributor manual that duplicates existing module
  docs.
- Do not add a new script until the ownership card format has survived at least
  one implementation slice.
- Do not require full workspace verification for docs-only or module-local
  changes.

## Deferred Follow-Ups

These stay recorded for later child packets:

- **MIT-only license simplification:** inventory license statements in tracked
  files and host-service settings, then simplify the repository policy if the
  final decision remains single MIT.
- **Live-system mutation acknowledgement UX:** inventory current flag consumers,
  decide which operations truly need explicit acknowledgement, and design a
  lower-friction safety-equivalent flow if warranted.
- **Changed-path verification helper:** after feature cards prove stable, add a
  warn-only script that maps changed paths to recommended proof commands.
- **Contributor diagnostics:** improve `conary-test` failing-run output once the
  workflow map names the exact diagnostics contributors need.
- **Additional feature cards:** add recipe/build-system, trust/signature
  verification, and Remi federation cards after the first feature map lands and
  reviewers validate the card schema against real contributor workflows.

## Review Focus

Reviewers should check:

- whether the feature cards name real owner files and avoid stale or overly
  broad paths;
- whether each focused proof command is valid and narrow enough;
- whether broader gates are triggered only by meaningful cross-system changes;
- whether the map helps contributors without duplicating `docs/ARCHITECTURE.md`
  or module docs;
- whether `CONTRIBUTING.md` and the PR template remain human-facing while
  `AGENTS.md` and `docs/llms/README.md` remain assistant entrypoints;
- whether the plan respects the CCS native ecosystem roadmap instead of
  turning this docs packet into CCS v2 implementation work.

## Implementation Plan

### Task 0: Lock The Reviewed Phase 5/6 Plan And Docs-Audit Row

**Files:**
- Add: `docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the reviewed plan before regenerating docs inventory**

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md
```

- [ ] **Step 2: Regenerate docs-audit inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected on the current baseline: tracked doc-like files grow from 146 to 147,
with this plan file added as `planning` / `maintainer`. If another docs file
lands first, use the regenerated inventory as source of truth and update counts
accordingly.

- [ ] **Step 3: Add the plan ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the other active
maintainability plan rows:

```text
docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md	docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md	planning	maintainer	maintainability; phase5; phase6; feature-ownership; contributor-ux; workflow	AGENTS.md; CONTRIBUTING.md; .github/PULL_REQUEST_TEMPLATE.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/ARCHITECTURE.md; docs/INTEGRATION-TESTING.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/conaryd.md; docs/modules/source-selection.md; docs/modules/test-fixtures.md; docs/operations/bootstrap-selfhosting-vm.md; docs/operations/daily-driver-ux-matrix.md; docs/operations/post-generation-export-follow-up-roadmap.md; docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md	verified	corrected	Added the reviewed Phase 5/6 plan for feature ownership cards, contributor workflow routing, focused verification recipes, and cross-system interaction gates without changing Rust behavior, schemas, licensing, or live-mutation UX.
```

- [ ] **Step 4: Update the audit summary for the active Phase 5/6 plan**

Append this paragraph to the existing
`### 2026-06-06 Maintainability Planning` section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The Phase 5/6 feature ownership and workflow UX plan now combines subsystem-map
consolidation with contributor workflow routing. It scopes the first slice to a
canonical feature ownership map, human/assistant entrypoint links, focused proof
recipes, and cross-system interaction gates while leaving MIT-only licensing and
live-system mutation UX as separate follow-up designs.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 147
- `verified-no-change`: 13
- `corrected`: 47
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes mention the Phase 5/6 planning update.

- [ ] **Step 5: Verify docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --cached --check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit the reviewed plan lock-in**

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan feature ownership workflow"
```

### Task 1: Add The Feature Ownership Map

**Files:**
- Add: `docs/modules/feature-ownership.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Create the feature ownership document**

Create `docs/modules/feature-ownership.md` with this content:

````markdown
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
`apps/conary/src/commands/update.rs`; `apps/conary/src/commands/remove.rs`;
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

**Docs to update:** `docs/llms/subsystem-map.md`; `docs/modules/feature-ownership.md`;
`docs/modules/test-fixtures.md`; `docs/operations/daily-driver-ux-matrix.md`.

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

**Neighbor systems:** `apps/conary/src/commands/update.rs`;
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
````

- [ ] **Step 2: Stage the feature ownership doc before regenerating inventory**

```bash
git add docs/modules/feature-ownership.md
```

- [ ] **Step 3: Regenerate docs-audit inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected on the current baseline after Task 0: tracked doc-like files grow from
147 to 148, with `docs/modules/feature-ownership.md` classified as
`canonical` / `contributor`.

- [ ] **Step 4: Add the feature ownership ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the module-doc
rows:

```text
docs/modules/feature-ownership.md	docs/modules/feature-ownership.md	canonical	contributor	feature-ownership; contributor-ux; verification; interaction-gates	AGENTS.md; CONTRIBUTING.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/ARCHITECTURE.md; docs/INTEGRATION-TESTING.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/conaryd.md; docs/modules/source-selection.md; docs/modules/test-fixtures.md; docs/operations/bootstrap-selfhosting-vm.md; docs/operations/daily-driver-ux-matrix.md; docs/operations/post-generation-export-follow-up-roadmap.md; apps/conary/src/commands/install; apps/conary/src/commands/adopt; apps/conary/src/commands/ccs; apps/remi/src/server; apps/conaryd/src/daemon; apps/conary-test/src; crates/conary-core/src/ccs; crates/conary-core/src/generation; crates/conary-agent-contract/src; crates/conary-mcp/src	verified	corrected	Added the canonical feature ownership map with start-here paths, neighboring systems, focused proof commands, broader interaction gates, docs routing, and safety notes for major Conary capabilities.
```

- [ ] **Step 5: Update summary counts for the new module doc**

Append this paragraph to the maintainability planning section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The first Phase 5/6 implementation slice adds
`docs/modules/feature-ownership.md` as the canonical feature-card map for major
Conary capabilities. The map records start-here files, neighboring systems,
focused proof commands, broader interaction gates, docs to update, and safety
notes so feature-focused work can stay local without hiding cross-system
coupling.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 148
- `verified-no-change`: 13
- `corrected`: 48
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes mention `docs/modules/feature-ownership.md`.

- [ ] **Step 6: Verify Task 1 docs-audit**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all commands exit 0.

### Task 2: Route Assistant And Contributor Entry Points

**Files:**
- Modify: `docs/llms/README.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `CONTRIBUTING.md`
- Modify: `.github/PULL_REQUEST_TEMPLATE.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Update `docs/llms/README.md` core docs**

Add this bullet to `## Core Docs` after `docs/modules/ccs.md`:

```markdown
- [`docs/modules/feature-ownership.md`](../modules/feature-ownership.md): feature ownership cards and interaction verification gates
```

Add this bullet to `## Working Rules` after the maintainability packet rule:

```markdown
- For feature-scoped work, use `docs/modules/feature-ownership.md` to find the
  start-here files, neighboring systems, focused proof, and broader interaction
  gate before editing.
```

- [ ] **Step 2: Update `docs/llms/subsystem-map.md`**

Bump frontmatter revision by 1 and change the summary to:

```yaml
summary: Stable subsystem pointers with feature ownership and interaction-gate routing
```

Add this bullet to `## Prefer Existing Deep Dives` before the fixture map bullet:

```markdown
- [`docs/modules/feature-ownership.md`](../modules/feature-ownership.md) for feature ownership cards, neighboring systems, and interaction verification gates
```

- [ ] **Step 3: Update contributor workflow docs**

In `CONTRIBUTING.md`, insert `docs/modules/feature-ownership.md` at position 3
in the numbered assistant list under `### Using Coding Assistants`, renumbering
the existing items 3 (`docs/INTEGRATION-TESTING.md`) and 4
(`docs/operations/infrastructure.md`) to positions 4 and 5:

```markdown
3. `docs/modules/feature-ownership.md` when choosing a feature area or
   deciding which cross-system gates apply
4. `docs/INTEGRATION-TESTING.md` when validation spans `conary-test`
5. `docs/operations/infrastructure.md` for MCP, deploy, and host workflow notes
```

Then add this section after `### Maintainability Slices` and before
`### Commit Messages`:

```markdown
### Feature Ownership And Verification

Use `docs/modules/feature-ownership.md` when a change is easier to describe as
a feature than as a crate or file. Each card names the files to read first, the
neighboring systems that can be affected, a focused proof command for small
edits, and a broader interaction gate for behavior that crosses subsystem
boundaries.

Small docs-only or module-local changes do not need full workspace validation by
default. Run the focused proof for the touched feature, then add the broader
gate when the card's neighboring systems are affected.
```

- [ ] **Step 4: Update the PR template**

Add this checkbox under `## Ownership / Boundary`:

```markdown
- [ ] Checked `docs/modules/feature-ownership.md` when this changes a user-visible capability
```

Add this checkbox under `## Verification` after the existing docs/maps checkbox:

```markdown
- [ ] Ran the broader interaction gate when the feature ownership card required it
```

- [ ] **Step 5: Update docs-audit rows**

Update these literal-tab rows in
`docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```text
CONTRIBUTING.md	CONTRIBUTING.md	canonical	contributor	contributing; development; verification; feature-ownership	AGENTS.md; docs/llms/README.md; docs/modules/feature-ownership.md; docs/INTEGRATION-TESTING.md; docs/operations/infrastructure.md; Cargo.toml; scripts/line-count-report.sh	verified	corrected	Refreshed contributor workflow guidance with feature ownership map routing, focused proof expectations, broader interaction gates, and existing maintainability slice discipline.
.github/PULL_REQUEST_TEMPLATE.md	.github/PULL_REQUEST_TEMPLATE.md	canonical	contributor	pull-request-template; verification; ownership; feature-ownership	AGENTS.md; CONTRIBUTING.md; docs/modules/feature-ownership.md; docs/llms/subsystem-map.md	verified	corrected	Added feature ownership and broader interaction-gate checkboxes while keeping the PR template concise and human-facing.
docs/llms/README.md	docs/llms/README.md	canonical	contributor	assistant-guidance; llm-map; feature-ownership	AGENTS.md; docs/ARCHITECTURE.md; docs/INTEGRATION-TESTING.md; docs/modules/feature-ownership.md; docs/modules/test-fixtures.md; docs/operations/infrastructure.md; scripts/docs-audit-inventory.sh; scripts/check-doc-audit-ledger.sh	verified	corrected	Refreshed assistant routing to include feature ownership cards and interaction gates while preserving AGENTS.md as the repo-wide assistant contract.
docs/llms/subsystem-map.md	docs/llms/subsystem-map.md	canonical	contributor	assistant-guidance; subsystem-map; feature-ownership	docs/ARCHITECTURE.md; docs/modules/feature-ownership.md; docs/modules/test-fixtures.md; crates/conary-core/src/generation/builder/runtime_inputs.rs; docs/operations/post-generation-export-follow-up-roadmap.md; apps/conary/src/commands/adopt/native_handoff.rs; crates/conary-core/src/ccs/convert; apps/remi/src/server/publication.rs; apps/conary/src/commands/install/mod.rs; apps/conary/src/commands/install/legacy_replay.rs; apps/conary/src/commands/install/inner.rs; apps/conary/src/commands/install/batch.rs; apps/conary/src/commands/install/restore.rs	verified	corrected	Refreshed subsystem pointers to route feature-scoped work through the feature ownership map while keeping existing selected-generation, fixture, Remi/CCS, and install replay pointers compact.
docs/superpowers/documentation-accuracy-audit-summary.md	docs/superpowers/documentation-accuracy-audit-summary.md	planning	maintainer	audit-summary; verification; release-hardening; active-planning; maintainability; feature-ownership	docs/superpowers/documentation-accuracy-audit-ledger.tsv; docs/superpowers/documentation-accuracy-audit-inventory.tsv; scripts/check-doc-audit-ledger.sh; ROADMAP.md; docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md; docs/superpowers/plans/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md; docs/superpowers/plans/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md; docs/superpowers/plans/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md; docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md; docs/modules/test-fixtures.md; docs/modules/feature-ownership.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/modules/ccs.md; docs/modules/remi.md; CONTRIBUTING.md; .github/PULL_REQUEST_TEMPLATE.md; docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md	verified	corrected	Refreshed the audit summary for the active maintainability planning lane, current docs-audit counts, Phase 1 discipline contract, Phase 2 dead-surface pruning plan and inventory, Phase 3 fixture-discipline plan, Phase 4 install hotspot decomposition, and Phase 5/6 feature ownership workflow routing.
```

Use the summary row above as the final form after Task 2. If Tasks 0 and 1 are
committed separately, the same row may be used early because docs-audit does not
validate evidence-source paths, but the summary prose and final counts must
match the files that have actually landed in that commit.

- [ ] **Step 6: Update audit summary**

Append this paragraph to the maintainability planning section:

```markdown
Assistant and contributor entrypoints now route feature-scoped work through the
feature ownership map. `docs/llms/README.md`, `docs/llms/subsystem-map.md`,
`CONTRIBUTING.md`, and the pull request template point contributors toward
start-here files, focused proof commands, and broader interaction gates without
turning any one entrypoint into a duplicate manual.
```

Counts stay unchanged after this task because it modifies existing tracked
doc-like files.

- [ ] **Step 7: Verify Task 2 docs-audit and stale-term sweep**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
for term in T''BD TO''DO FI''XME Cent''OS RH''EL "Debian sta""ble" open''SUSE Al''pine CLAU''DE Cla''ude "Open Review"" Questions"; do
    if git diff -U0 -- CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md docs/llms/README.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md | grep '^[+][^+]' | grep -n "$term"; then
        exit 1
    fi
done
git diff --check
```

Expected: all commands exit 0 with no stale-term matches in added lines.

### Task 3: Final Verification And Commit The Slice

**Files:**
- Add: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/README.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `CONTRIBUTING.md`
- Modify: `.github/PULL_REQUEST_TEMPLATE.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Run docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
git diff --cached --check
```

Expected: all commands exit 0.

- [ ] **Step 2: Run docs-only search checks**

```bash
rg -n "docs/modules/feature-ownership.md|Feature Ownership|interaction gate|focused proof" AGENTS.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md docs/llms docs/modules docs/superpowers/documentation-accuracy-audit-summary.md
rg -n 'T''BD|TO''DO|FI''XME|Cent''OS|RH''EL|Deb''ian stable|open''SUSE|Al''pine|CLAU''DE|Cla''ude|Open Review'' Questions' docs/modules/feature-ownership.md docs/superpowers/plans/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md
```

Expected: the first command shows intentional routing references. The second
command has no matches.

- [ ] **Step 3: Run lightweight workspace sanity**

```bash
cargo fmt --check
```

Expected: exits 0. No Rust code changes are expected.

- [ ] **Step 4: Review staged diff**

```bash
git status --short --branch
git diff --stat
git diff --cached --stat
```

Expected: only the Phase 5/6 docs, audit metadata, and PR template are staged.

- [ ] **Step 5: Commit the Phase 5/6 implementation slice**

```bash
git add docs/modules/feature-ownership.md docs/llms/README.md docs/llms/subsystem-map.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: add feature ownership workflow map"
```

Expected: commit includes only docs and template changes.

- [ ] **Step 6: Stop for review**

After the implementation slice lands, stop. Do not start MIT-only license
simplification, live-system mutation acknowledgement UX redesign, or changed-path
verification scripting until each has its own reviewed child plan.

## Final Verification For The Whole Packet

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
cargo fmt --check
```

Expected: all commands exit 0.

Then verify:

```bash
git status --short --branch
git rev-list --left-right --count HEAD...origin/main
```

Expected after commit and push: clean `main`, `0 0` ahead/behind.
