# Project Maintainability Phase 4 Install Hotspot Decomposition Design And Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` or `superpowers:executing-plans`
> to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for
> tracking. This is a Phase 4 child packet under
> `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Ship the first behavior-preserving hotspot decomposition slice by
moving Conary install legacy replay adapter logic out of
`apps/conary/src/commands/install/mod.rs` and into an owning module without
changing install, replay, host-mutation, or CCS behavior.

**Architecture:** Treat `install/mod.rs` as the orchestration entrypoint and
extract only the cohesive legacy replay install adapter. Keep the public
`commands::LegacyReplayOptions` surface and current sibling-module import paths
stable through narrow re-exports. Update assistant and fixture routing so future
workers land in the new owner module instead of re-reading the whole install
orchestrator.

**Tech Stack:** Rust, Cargo unit and integration tests, Conary CLI install
submodules, core CCS legacy replay planner, Markdown docs, existing docs-audit
tooling.

---

## Status

Draft packet for review.

This packet is intentionally scoped to the first Phase 4 CLI hotspot slice. It
does not touch `apps/conary/src/commands/ccs/install.rs`, Remi conversion,
conaryd routes, CCS v2 package-contract work, persisted schema, or runtime
behavior. It should be reviewed and locked in before implementation starts.

## Read First

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/ccs.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/source-selection.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md`
- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/install/batch.rs`
- `apps/conary/src/commands/install/restore.rs`
- `apps/conary/src/commands/install/inner.rs`
- `apps/conary/src/commands/update.rs`
- `crates/conary-core/src/ccs/legacy_replay.rs`

## Design Summary

Phase 4 starts with `apps/conary/src/commands/install/mod.rs` because it is the
largest Rust source file in the workspace:

```text
lines	path
4267	apps/conary/src/commands/install/mod.rs
3441	apps/conary/src/commands/ccs/install.rs
3334	apps/conary/src/commands/update.rs
2999	apps/remi/src/server/conversion.rs
```

The first slice should not try to split the entire install command. The safest
high-leverage target is the legacy replay install adapter that currently lives
at the top of `install/mod.rs`. That block is cohesive, safety-critical, and
already has focused tests.

## Current Evidence

The following commands were used while drafting this packet:

```bash
git status --short --branch
git rev-list --left-right --count HEAD...origin/main
scripts/line-count-report.sh 30
rg -n "struct LegacyReplay|enum LegacyReplay|LEGACY_REPLAY|legacy_replay|run_legacy|execute_legacy|build_legacy|PackageExecutionPath|live_root|preflight" apps/conary/src/commands/install/mod.rs
rg -n "LegacyReplayOptions|LegacyReplayInstallState|AcceptedLegacyBundleInstall|LegacyReplayAuditContext|plan_ccs_fresh_install|run_legacy_replay|build_legacy_replay|PackageExecutionPath" apps/conary/src crates/conary-core/src
cargo test -p conary --lib legacy_replay -- --list
cargo test -p conary --lib preflights_live_root -- --list
cargo test -p conary --test bundle_replay -- --list
cargo test -p conary --test live_host_mutation_safety -- --list
cargo test -p conary-core legacy_replay -- --list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Findings:

- `HEAD` and `origin/main` matched at draft time.
- `install/mod.rs` was the largest Rust source file in the workspace.
- Legacy replay install adapter types and helpers were concentrated in
  `install/mod.rs`, with `LegacyReplayOptions` re-exported through
  `commands::`.
- Install sibling modules and update preflight code already rely on install
  module re-exports for replay options, state, and planner functions.
- Focused replay, bundle, and live-host mutation test filters exist and can be
  used as behavior-preserving gates.
- Docs-audit ledger verification passed on the pre-draft baseline.

Current `install/mod.rs` responsibilities include:

- installing command entrypoint and option types;
- repository source selection and dependency policy overlay;
- direct install and CCS transaction orchestration;
- live-root execution path and ownership preflight helpers;
- scriptlet preflight and execution orchestration;
- legacy replay install planning, replay execution, warning handling, and audit
  assembly;
- tests for legacy replay carriers, replay runner behavior, audit rendering,
  live-root file materialization, and preflight ordering.

Target ownership after this slice:

- `apps/conary/src/commands/install/mod.rs`: remains the install orchestration
  entrypoint and keeps package install option/context types.
- `apps/conary/src/commands/install/legacy_replay.rs`: owns the install-side
  adapter between Conary CLI install flows and
  `crates/conary-core/src/ccs/legacy_replay.rs`.
- `crates/conary-core/src/ccs/legacy_replay.rs`: remains the host-I/O-free core
  replay planner and refusal policy.
- `apps/conary/src/commands/legacy_replay_policy.rs`: remains the host context
  resolver and policy-input builder for CLI command surfaces.
- `apps/conary/tests/bundle_replay.rs`, `foreign_replay.rs`, and
  `query_scripts.rs`: remain the user-flow fixture proof surfaces.

## Module Boundary

Create `apps/conary/src/commands/install/legacy_replay.rs` and move the install
legacy replay adapter there:

- `LegacyReplayOptions`
- `LegacyReplayInstallState`
- `AcceptedLegacyBundleInstall`
- `LegacyReplayAuditContext`
- `LEGACY_REPLAY_POLICY`
- `plan_ccs_fresh_install_legacy_replay`
- `plan_ccs_old_installed_upgrade_legacy_replay`
- `merge_old_upgrade_legacy_replay_state`
- `plan_from_preflight`
- `compatibility_audit_from_plan`
- `legacy_replay_refusal_error`
- `run_legacy_replay_plan_entries_with`
- `LegacyReplayExecutionScope`
- `execute_legacy_replay_plan_entries`
- `require_legacy_replay_success`
- `legacy_post_replay_warnings`
- `build_legacy_replay_audit_for_install`
- replay audit helper functions
- `legacy_source_scriptlet_format`
- `legacy_lifecycle_phase_name`
- the existing legacy replay unit tests from `install/mod.rs`
- the `test_legacy_bundle`, `test_legacy_plan`, `test_legacy_entry`, and
  `accepted_compatibility_decision` test helpers used only by those moved tests

Keep these items in `install/mod.rs` for this slice:

- `PackageExecutionPath`
- live-root preflight functions
- `preflighted_execution_path` helpers
- `CcsTransactionInstallOptions`
- install orchestration functions and context structs

`apps/conary/src/commands/remove.rs` and
`apps/conary/src/commands/system.rs` carry their own replay refusal, lifecycle,
format, or compatibility helpers. Those copies are out of scope; do not
deduplicate them in this slice.

Register the module from `install/mod.rs`:

```rust
mod legacy_replay;
```

Preserve current import paths through explicit re-exports:

```rust
pub use legacy_replay::LegacyReplayOptions;
pub(crate) use legacy_replay::{
    AcceptedLegacyBundleInstall, LegacyReplayAuditContext, LegacyReplayInstallState,
};
pub(super) use legacy_replay::{
    merge_old_upgrade_legacy_replay_state,
    plan_ccs_fresh_install_legacy_replay,
    plan_ccs_old_installed_upgrade_legacy_replay,
};
```

Then import only the helpers that `install/mod.rs` still calls directly:

```rust
use legacy_replay::{
    LegacyReplayExecutionScope, build_legacy_replay_audit_for_install,
    execute_legacy_replay_plan_entries, legacy_post_replay_warnings,
    require_legacy_replay_success,
};
```

Items called from `install/mod.rs` after the move should be `pub(super)` in
`legacy_replay.rs`, including `LegacyReplayExecutionScope` and any fields the
orchestrator must set: `root`, `package_name`, `package_version`, `mode`,
`sandbox_mode`, `old_version`, and `new_version`. Items re-exported for sibling
modules should stay `pub(crate)` or `pub(super)` through `install/mod.rs`; do
not make the `legacy_replay` module itself public.

`LegacyReplayAuditContext` must remain `pub(crate)` because
`LegacyReplayInstallState` exposes it through its `audit` field.

If the exact visibility needs minor adjustment during compilation, keep the
same principle: preserve `commands::LegacyReplayOptions`, preserve existing
`super::LegacyReplayInstallState` sibling imports, and avoid widening the new
module into a general public API.

## Behavior Boundaries

This is a refactor, not a behavior change.

Preserve these invariants:

- Legacy replay defaults fail closed on install, update, remove, restore, and
  CCS install surfaces.
- Fresh install and upgrade replay planning still run before file or DB
  mutation when a bundle is present.
- Old installed bundle pre-remove and post-remove plans still merge into upgrade
  state.
- Review-required, blocked, raw replay, target-compatibility, no-scripts, and
  foreign replay refusal gates remain unchanged.
- Replay audit metadata still excludes private local paths and still records
  compatibility decisions, matrix digests, feature-gate state, and outcomes.
- Live-root mutation acknowledgement and live-root ownership preflight ordering
  remain unchanged.
- `install/mod.rs` remains the install orchestrator; this slice does not move
  dependency resolution, direct install transaction execution, batch install,
  restore preparation, or CCS component selection.

## Non-Goals

- Do not rewrite install behavior to reduce line count.
- Do not touch `apps/conary/src/commands/ccs/install.rs`.
- Do not start CCS v2 native package contract work.
- Do not touch Remi conversion, Remi publication, conaryd route/job code, or
  bootstrap code.
- Do not alter persisted database schemas, bundle schemas, audit JSON shape,
  docs-audit script behavior, or `conary-test` manifests.
- Do not remove live-host mutation acknowledgement behavior in this slice.
- Do not add or imply public distro support beyond Fedora 44, Ubuntu 26.04, and
  Arch.
- Do not extract live-root execution path helpers in this first slice; that is a
  later Phase 4 candidate.

## Review Focus

Before lock-in, reviewers should check:

- the target module boundary is cohesive and does not hide install orchestration
  in another large file;
- the visibility plan preserves current public and sibling imports;
- the verification gate covers scriptlet replay, install audit, and live-host
  mutation safety;
- the packet does not collide with CCS native ecosystem contract work;
- docs-audit ordering stages this new plan before regenerating inventory.

## Implementation Plan

### Task 0: Lock The Reviewed Phase 4 Plan And Docs-Audit Row

**Files:**
- Add: `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the reviewed plan before regenerating docs inventory**

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md
```

- [ ] **Step 2: Regenerate docs-audit inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: inventory grows from the current 145 tracked doc-like files to 146,
with this plan file added as `planning` / `maintainer`. The generated inventory
includes:

```text
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md	planning	maintainer
```

- [ ] **Step 3: Add the plan ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the other active
maintainability plan rows:

```text
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md	planning	maintainer	maintainability; phase4; hotspot-decomposition; conary-install; legacy-replay	AGENTS.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/ARCHITECTURE.md; docs/INTEGRATION-TESTING.md; docs/modules/ccs.md; docs/modules/test-fixtures.md; docs/modules/source-selection.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md; apps/conary/src/commands/install/mod.rs; apps/conary/src/commands/install/batch.rs; apps/conary/src/commands/install/restore.rs; apps/conary/src/commands/install/inner.rs; apps/conary/src/commands/update.rs; crates/conary-core/src/ccs/legacy_replay.rs	verified	corrected	Added the reviewed Phase 4 plan for the first install hotspot decomposition slice: extract the install-side legacy replay adapter, preserve import paths and replay gates, route docs to the new owner module, and avoid CCS native, Remi, conaryd, schema, and behavior changes.
```

- [ ] **Step 4: Update the audit summary for the active Phase 4 plan**

Append this paragraph to the existing
`### 2026-06-06 Maintainability Planning` section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The Phase 4 install hotspot decomposition plan now opens the first code-moving
maintenance slice. It chooses `apps/conary/src/commands/install/mod.rs` over the
CCS install hotspot so CCS native contract work can proceed separately, and it
narrows the first refactor to the install-side legacy replay adapter plus docs
routing.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 146
- `verified-no-change`: 13
- `corrected`: 46
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes mention the Phase 4 install hotspot planning update.

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
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan install hotspot decomposition"
```

### Task 1: Extract The Install Legacy Replay Adapter

**Files:**
- Add: `apps/conary/src/commands/install/legacy_replay.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`

- [ ] **Step 1: Run the focused baseline gates**

These should pass before moving code:

```bash
cargo test -p conary --lib legacy_replay
cargo test -p conary-core legacy_replay
cargo test -p conary --test bundle_replay synthetic_legacy_bundle_fixtures_cover_task5_matrix
cargo test -p conary --test live_host_mutation_safety install_refuses_without_live_mutation_flag
```

- [ ] **Step 2: Create and register the new module**

Create `apps/conary/src/commands/install/legacy_replay.rs` with a path comment:

```rust
// src/commands/install/legacy_replay.rs
//! Install-side adapter for legacy scriptlet replay planning, execution, and audit metadata.
```

Add `mod legacy_replay;` to `apps/conary/src/commands/install/mod.rs`.

- [ ] **Step 3: Move the legacy replay adapter block**

Move the items listed in `## Module Boundary` from `install/mod.rs` into
`legacy_replay.rs`. Keep the existing logic intact. Adjust imports only as
needed for the new module path.

Use `super::CcsTransactionInstallOptions` where the planner needs install
transaction options. Use `super::scriptlets::scriptlet_warning_from_failure`
for warning construction. Keep audit types referenced through
`crate::commands::...` so the metadata boundary remains unchanged.

- [ ] **Step 4: Preserve current import surfaces**

In `install/mod.rs`, re-export the moved public and sibling-facing types and
planner functions as described in `## Module Boundary`.

Expected after this step:

- `crate::commands::LegacyReplayOptions` still compiles.
- `super::LegacyReplayInstallState` still compiles in install sibling modules.
- `super::install::plan_ccs_fresh_install_legacy_replay` still compiles for
  update preflight code.
- `install/mod.rs` calls replay execution/audit helpers through explicit
  imports from `legacy_replay`.

- [ ] **Step 5: Move the local legacy replay unit tests**

Move these tests from `install/mod.rs` into `legacy_replay.rs`:

- `legacy_replay_options_default_disabled_for_install_surfaces`
- `legacy_replay_install_state_defaults_to_empty_carriers`
- `legacy_replay_plan_runner_invokes_selected_legacy_entry_once`
- `legacy_replay_plan_runner_skips_fully_replaced_plan_entries`
- `legacy_replay_audit_records_planned_entry_outcome`

Move their local helper functions too:

- `test_legacy_bundle`
- `test_legacy_plan`
- `test_legacy_entry`
- `accepted_compatibility_decision`

Keep live-root preflight ordering tests in `install/mod.rs`; they are about
install orchestration, not the replay adapter.

- [ ] **Step 6: Verify the extraction**

```bash
cargo check -p conary
cargo test -p conary --lib legacy_replay
cargo test -p conary --lib preflights_live_root
cargo test -p conary-core legacy_replay
cargo test -p conary --test bundle_replay synthetic_legacy_bundle_fixtures_cover_task5_matrix
```

Expected: all commands exit 0.

### Task 2: Route Docs And Fixture Guidance To The New Owner

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Update assistant routing**

In `docs/llms/subsystem-map.md`, add or update a "Look Here First" pointer for
install orchestration, legacy replay install adapter behavior, and live-root
preflight. It should point to:

- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/install/legacy_replay.rs`
- `apps/conary/src/commands/install/inner.rs`
- `apps/conary/src/commands/install/batch.rs`
- `apps/conary/src/commands/install/restore.rs`
- `docs/modules/test-fixtures.md`

Keep this map compact; do not turn it into install documentation.

- [ ] **Step 2: Update CCS module guidance**

In `docs/modules/ccs.md`, update the legacy scriptlet replay section so local
install/update/remove replay behavior points to
`apps/conary/src/commands/install/legacy_replay.rs` for the install-side
adapter and `crates/conary-core/src/ccs/legacy_replay.rs` for the core replay
planner.

- [ ] **Step 3: Update fixture ownership routing**

In `docs/modules/test-fixtures.md`, update the
`legacy-scriptlet-bundle-fixtures` family so future install replay refactors
know the code owner is `apps/conary/src/commands/install/legacy_replay.rs` and
the fixture owner remains `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`.

- [ ] **Step 4: Update docs-audit rows and summary**

Update these literal-tab rows in
`docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```text
docs/llms/subsystem-map.md	docs/llms/subsystem-map.md	canonical	contributor	assistant-guidance; subsystem-map	docs/ARCHITECTURE.md; crates/conary-core/src/generation/builder/runtime_inputs.rs; docs/operations/post-generation-export-follow-up-roadmap.md; apps/conary/src/commands/adopt/native_handoff.rs; docs/modules/test-fixtures.md; crates/conary-core/src/ccs/convert; apps/remi/src/server/publication.rs; apps/conary/src/commands/install/mod.rs; apps/conary/src/commands/install/legacy_replay.rs; apps/conary/src/commands/install/inner.rs; apps/conary/src/commands/install/batch.rs; apps/conary/src/commands/install/restore.rs	verified	corrected	Refreshed subsystem pointers for selected-generation native handoff, adoption authority, current post-ISO/export follow-up routing, Remi/CCS fixture proof ownership, and install legacy replay adapter routing.
docs/modules/ccs.md	docs/modules/ccs.md	canonical	contributor	ccs; cli-surface; fixture-proof	apps/conary/src/cli/ccs.rs; docs/specs/ccs-format-v1.md; crates/conary-core/src/ccs/manifest.rs; crates/conary-core/src/ccs/convert/golden_fixtures.rs; crates/conary-core/src/ccs/convert/support_matrix.rs; docs/modules/test-fixtures.md; apps/conary/src/commands/install/legacy_replay.rs; crates/conary-core/src/ccs/legacy_replay.rs	verified	corrected	Added CCS conversion fixture ownership routing, fast proof commands, and legacy scriptlet install-side adapter routing.
docs/modules/test-fixtures.md	docs/modules/test-fixtures.md	canonical	contributor	test-fixtures; fixture-map; remi; ccs; conary-test	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md; crates/conary-core/src/ccs/convert/golden_fixtures.rs; crates/conary-core/src/ccs/convert/support_matrix.rs; apps/conary/tests/common/legacy_scriptlet_fixtures.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/conversion.rs; apps/conary-test/src/suite_inventory.rs; apps/conary/tests/integration/remi/manifests; apps/conary/src/commands/install/legacy_replay.rs	verified	corrected	Added the first canonical fixture ownership map for Remi and CCS conversion/publication fixtures, and updated code owner routing for legacy scriptlet bundle fixtures.
docs/superpowers/documentation-accuracy-audit-summary.md	docs/superpowers/documentation-accuracy-audit-summary.md	planning	maintainer	audit-summary; verification; release-hardening; active-planning; maintainability	docs/superpowers/documentation-accuracy-audit-ledger.tsv; docs/superpowers/documentation-accuracy-audit-inventory.tsv; scripts/check-doc-audit-ledger.sh; ROADMAP.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md; docs/modules/test-fixtures.md; docs/llms/subsystem-map.md; docs/modules/ccs.md; docs/modules/remi.md; docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md	verified	corrected	Refreshed the audit summary for the active maintainability planning lane, current docs-audit counts, Phase 1 discipline contract, Phase 2 dead-surface pruning plan and inventory, Phase 3 fixture-discipline plan, and Phase 4 install hotspot decomposition plan.
```

Append this paragraph to the maintainability section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The first Phase 4 implementation slice extracted the install legacy replay
adapter into `apps/conary/src/commands/install/legacy_replay.rs` and refreshed
assistant plus fixture routing so replay refactors no longer start in the full
install orchestrator.
```

Counts stay unchanged because this task modifies existing doc-like files.

- [ ] **Step 5: Verify docs-audit**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

Expected: both commands exit 0.

### Task 3: Final Verification And Commit The Slice

**Files:**
- Add: `apps/conary/src/commands/install/legacy_replay.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Run focused code gates**

```bash
cargo test -p conary --lib legacy_replay
cargo test -p conary --lib preflights_live_root
cargo test -p conary-core legacy_replay
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary --test query_scripts
cargo test -p conary --test live_host_mutation_safety
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: all commands exit 0. The `golden_conversion` filter intentionally
matches multiple conversion integration tests that cover public-ready,
replaced, legacy replay, and foreign replay outcomes.

- [ ] **Step 2: Run package and formatting gates**

```bash
cargo check -p conary
cargo fmt --check
cargo clippy -p conary --all-targets -- -D warnings
```

Expected: all commands exit 0.

- [ ] **Step 3: Run docs and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 4: Confirm the intended line-count reduction**

```bash
wc -l apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/legacy_replay.rs
scripts/line-count-report.sh 10
```

Expected: `install/mod.rs` is meaningfully smaller and
`install/legacy_replay.rs` is focused on replay adapter behavior rather than a
new mixed-responsibility module.

- [ ] **Step 5: Commit only the Phase 4 implementation slice**

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/legacy_replay.rs docs/llms/subsystem-map.md docs/modules/ccs.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "refactor(conary): extract install legacy replay adapter"
```

## Optional Escalation Gates

Do not run these by default for this slice. Add them only if the implementation
touches selected-generation handoff, active host flows beyond replay adapter
movement, or direct live-root execution behavior:

```bash
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro ubuntu-26.04 --phase 3
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro arch --phase 3
```

## Stop Point

Stop after Task 3 is committed and verified. Do not start:

- extracting live-root execution path helpers;
- decomposing `apps/conary/src/commands/ccs/install.rs`;
- decomposing `apps/conary/src/commands/update.rs`;
- decomposing Remi conversion or conaryd routes;
- CCS native package ecosystem work.

Report the commit SHA, commands run, `git status --short --branch`, and whether
anything unexpected changed.
