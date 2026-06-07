# Project Maintainability Phase 11 Update Adopted Authority Decomposition Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. This is the Phase 11 child packet
> under
> `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Extract adopted-package update authority policy from
`apps/conary/src/commands/update/mod.rs` into a focused update submodule without
changing update behavior.

**Architecture:** Keep the Phase 10 update module split and add
`apps/conary/src/commands/update/adopted_authority.rs` as the owner for
adopted-update takeover decisions, native package-manager fallback selection,
adopted skip records, and adopted-update user-facing summary text. Keep update
candidate selection in `selection.rs`, and keep update execution, delta/full
package application, collection orchestration, replatform previewing, and
changeset rollback in `update/mod.rs`.

**Tech Stack:** Rust, existing Conary command modules, existing
`conary_core::db::models::Trove` authority metadata, existing
`SystemPackageManager` detection helpers, existing cargo tests, docs-audit
scripts.

---

## Status

Draft plan for local and external review.

## Candidate Choice

After Phase 10, the next update-owned refactor should continue reducing
`apps/conary/src/commands/update/mod.rs` rather than moving to a different
hotspot. The adopted/native-authority cluster is the best next slice because:

- it was explicitly kept as a future slice in the Phase 10 plan;
- it is a coherent policy/rendering boundary used by both `cmd_update` and
  `cmd_update_group`;
- it already has a compact unit-test cluster plus one CLI UX integration proof;
- it does not require changing update candidate selection, download, transaction,
  legacy replay, or collection execution.

Alternatives considered:

| Candidate | Trade-off | Decision |
|-----------|-----------|----------|
| `update/adopted_authority.rs` | Small, cohesive, behavior-preserving, direct continuation of Phase 10 | Choose for Phase 11 |
| `update/collection.rs` | Could reduce more orchestration lines, but collection update mixes selection, adopted authority, member variant targeting, and command execution | Defer until adopted authority is extracted |
| `update/execution.rs` | Larger line reduction, but it touches CAS retrieval, delta/full update preparation, legacy replay preflight, and changeset rollback | Defer; higher blast radius |

## Read First

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/modules/test-fixtures.md`
- `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md`
- `apps/conary/src/commands/mod.rs`
- `apps/conary/src/commands/update/mod.rs`
- `apps/conary/src/commands/update/selection.rs`
- `apps/conary/tests/cli_daily_ux.rs`

## Current Repo-Grounded Inputs

| Signal | Current value | Phase 11 interpretation |
|--------|---------------|-------------------------|
| Current Rust hotspots | `apps/conary/src/commands/ccs/install.rs` 3118 lines; `apps/remi/src/server/conversion.rs` 2999 lines; `apps/conary/src/commands/install/mod.rs` 2874 lines; `apps/conary/src/commands/update/mod.rs` 2471 lines | `update/mod.rs` is still a top CLI hotspot after Phase 10 |
| Existing update submodules | `apps/conary/src/commands/update/mod.rs`; `apps/conary/src/commands/update/selection.rs` | Add one sibling submodule instead of changing public command routing |
| Adopted authority cluster | `AdoptedUpdateDecision`, `AdoptedUpdateSkipReason`, `AdoptedUpdateSkip`, `adopted_update_decision`, `native_manager_for_trove`, `render_adopted_skip_sample`, `no_update_message` | Move into `update/adopted_authority.rs` |
| Parent call sites | `cmd_update` and `cmd_update_group` call adopted decision/fallback helpers and read skip records | Keep a narrow `pub(super)` surface for parent orchestration |
| Current test inventory | `cargo test -p conary --lib adopted_update -- --list` matches 6 tests; `cargo test -p conary --test cli_daily_ux adopted_update -- --list` matches 1 test | Move the 6 direct unit tests into the new module and keep the CLI proof as an integration gate |
| Docs-audit baseline | 154 tracked doc-like files, 54 corrected rows | Lock-in should add one planning file and update counts to 155 total / 55 corrected |

`apps/conary/tests/cli_daily_ux.rs` is the focused integration gate because
`adopted_update_routes_to_native_pm_and_refresh` verifies that parent command
dispatch still wires adopted authority policy into user-facing native package
manager guidance. `apps/conary/tests/native_pm_live_root.rs` exercises
live-root update/security flows but does not seed adopted-package native
authority scenarios, so it stays covered by the full `cargo test -p conary`
gate rather than the focused Phase 11 test list.

Evidence commands used to shape this packet:

```bash
scripts/line-count-report.sh 20
rg -n "AdoptedUpdateDecision|AdoptedUpdateSkipReason|AdoptedUpdateSkip|adopted_update_decision|native_manager_for_trove|render_adopted_skip_sample|no_update_message|adopted_update_tests" apps/conary/src apps/conary/tests docs -g '*.rs' -g '*.md'
cargo test -p conary --lib adopted_update -- --list
cargo test -p conary --test cli_daily_ux adopted_update -- --list
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
```

Current filter-discovery results:

| Filter | Current matches |
|--------|-----------------|
| `cargo test -p conary --lib adopted_update -- --list` | 6 tests |
| `cargo test -p conary --test cli_daily_ux adopted_update -- --list` | 1 test |

## Module Boundary

Create:

- `apps/conary/src/commands/update/adopted_authority.rs`

Modify:

- `apps/conary/src/commands/update/mod.rs`
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

Move these items from `update/mod.rs` into `adopted_authority.rs`:

- `AdoptedUpdateDecision`
- `AdoptedUpdateSkipReason`
- `AdoptedUpdateSkip`
- `adopted_update_decision`
- `native_manager_for_trove`
- `render_adopted_skip_sample`
- `no_update_message`
- direct adopted authority tests:
  - `adopted_updates_do_not_take_over_without_explicit_takeover_mode`
  - `adopted_updates_take_over_only_under_explicit_takeover_mode`
  - `critical_adopted_packages_are_blocked_even_under_takeover_mode`
  - `adopted_updates_are_not_queued_under_satisfy_or_adopt`
  - `adopted_update_guidance_uses_recorded_version_scheme_before_live_detection`
  - `adopted_update_skip_message_is_not_generic_up_to_date_text`

Keep these items in `update/mod.rs` for this slice:

- `cmd_update`
- `cmd_update_group`
- `installed_troves_for_update`
- `CollectionUpdateTarget`
- `select_update_candidate` call sites and selection handling
- `SecurityMetadataUnavailable` aggregation and security metadata refusal
- pinned package handling
- delta/full update preparation and execution
- legacy replay preflight
- changeset rollback and delta stats
- source-policy, replatform, partial-failure, selector, collection, and execution
  tests.

`adopted_authority.rs` should expose only the parent-module surface needed by
`update/mod.rs`:

```rust
pub(super) enum AdoptedUpdateDecision {
    SkipNativeAuthority,
    QueueTakeover,
    BlockCritical,
}

pub(super) enum AdoptedUpdateSkipReason {
    NativeAuthority,
    CriticalBlocked,
}

pub(super) struct AdoptedUpdateSkip {
    pub(super) package: String,
    pub(super) manager: SystemPackageManager,
    pub(super) reason: AdoptedUpdateSkipReason,
}

pub(super) fn adopted_update_decision(
    trove: &Trove,
    dep_mode: DepMode,
    requested_dep_mode: Option<DepMode>,
) -> AdoptedUpdateDecision;

pub(super) fn native_manager_for_trove(
    trove: &Trove,
    fallback_manager: SystemPackageManager,
) -> SystemPackageManager;

pub(super) fn render_adopted_skip_sample(skips: &[&AdoptedUpdateSkip]) -> String;
pub(super) fn no_update_message(
    security_only: bool,
    adopted_updates_skipped: bool,
) -> &'static str;
```

Keep helper fields private unless `update/mod.rs` actually reads them.
Currently, the parent module reads `AdoptedUpdateSkip.reason` when partitioning
native-authority and critical-blocked messages, and passes `package`/`manager`
through `render_adopted_skip_sample`.

## Non-Goals

- Do not change adopted update policy.
- Do not change `DepMode::Takeover` semantics.
- Do not change critical-package blocklist behavior.
- Do not change native package-manager detection or fallback behavior.
- Do not change update candidate selection.
- Do not change update execution, changeset rollback, package installation,
  legacy replay, collection update, or security metadata handling.
- Do not move CLI integration tests.
- Do not add schema migrations, new CLI flags, or daemon API changes.

## Review Focus

Reviewers should check:

- whether `adopted_authority.rs` owns a coherent native-authority update policy
  boundary;
- whether parent visibility stays at `pub(super)` or private rather than
  `pub(crate)`;
- whether `cmd_update` and `cmd_update_group` still preserve adopted-package
  authority before mutation;
- whether security-only update metadata checks still skip enforcement for
  adopted packages that remain under native authority;
- whether the CLI integration proof still covers the user-facing native package
  manager guidance;
- whether active docs point to `update/adopted_authority.rs` where adopted
  update policy is the relevant owner.

## Tasks

### Task 0: Lock Planning Doc Into The Docs Audit

**Files:**

- Create:
  `docs/superpowers/plans/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the new plan before regenerating inventory**

Run:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md
```

Expected: the new plan is staged so the tracked-file inventory script can see
it.

- [ ] **Step 2: Regenerate docs-audit inventory**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: the inventory includes the Phase 11 plan as a planning/maintainer
row. If another docs file lands first, use the regenerated inventory as the
source of truth and update counts accordingly.

- [ ] **Step 3: Add the ledger row**

Locate the exact Phase 10 ledger row by searching for
`phase10-update-selection-decomposition-plan.md`, then insert this literal-tab
Phase 11 row immediately after it. Keep literal tab characters matching the
existing TSV format.

```tsv
docs/superpowers/plans/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md	docs/superpowers/plans/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md	planning	maintainer	maintainability; phase11; update; adopted-authority; hotspot-decomposition	apps/conary/src/commands/update/mod.rs; apps/conary/src/commands/update/selection.rs; apps/conary/src/commands/update/adopted_authority.rs; apps/conary/tests/cli_daily_ux.rs; docs/llms/subsystem-map.md; docs/modules/source-selection.md; docs/modules/feature-ownership.md	verified	corrected	Added Phase 11 plan for extracting adopted-package update authority policy, native package-manager fallback guidance, and adopted update skip text into a focused update submodule while preserving update behavior.
```

- [ ] **Step 4: Update the audit summary narrative and counts**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph to the existing `### 2026-06-06 Maintainability Planning` section:

```markdown
The Phase 11 update adopted authority decomposition plan continues reducing
`apps/conary/src/commands/update/mod.rs` after the selection split. It extracts
adopted-package update takeover decisions, native package-manager fallback
selection, adopted skip records, and native-authority summary text into
`apps/conary/src/commands/update/adopted_authority.rs`, while keeping update
selection, collection orchestration, delta/full update execution, and legacy
replay preflight in their existing owners.
```

Then update the final counts from:

```markdown
- Total tracked doc-like files audited: 154
- `verified-no-change`: 13
- `corrected`: 54
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

to:

```markdown
- Total tracked doc-like files audited: 155
- `verified-no-change`: 13
- `corrected`: 55
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its
`claim_clusters`, `evidence_sources`, and notes include the Phase 11 planning
update.

- [ ] **Step 5: Verify docs-audit lock-in**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected:

```text
155
archived 73
corrected 55
retained-historical 14
verified-no-change 13
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 6: Commit the locked plan**

Run:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv \
    docs/superpowers/documentation-accuracy-audit-inventory.tsv \
    docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan update adopted authority decomposition"
```

Expected: docs-only planning commit succeeds.

### Task 1: Extract Adopted Authority Policy Into `adopted_authority.rs`

**Files:**

- Create: `apps/conary/src/commands/update/adopted_authority.rs`
- Modify: `apps/conary/src/commands/update/mod.rs`

- [ ] **Step 1: Add the adopted authority submodule and import surface**

In `apps/conary/src/commands/update/mod.rs`, add this directly before the
existing `mod selection;` declaration:

```rust
mod adopted_authority;
```

Then add this import block after the existing `use` declarations:

```rust
use adopted_authority::{
    AdoptedUpdateDecision, AdoptedUpdateSkip, AdoptedUpdateSkipReason,
    adopted_update_decision, native_manager_for_trove, no_update_message,
    render_adopted_skip_sample,
};
```

Expected: parent call sites in `cmd_update` and `cmd_update_group` can keep
their existing names after the moved functions land.

- [ ] **Step 2: Create `adopted_authority.rs` with focused imports**

Create `apps/conary/src/commands/update/adopted_authority.rs`:

```rust
// src/commands/update/adopted_authority.rs

//! Adopted-package update authority and native package-manager guidance.

use super::super::install::{self, DepMode};
use conary_core::db::models::Trove;
use conary_core::packages::SystemPackageManager;
```

Expected: the new module depends only on update authority inputs: installed
trove metadata, dependency mode, the install blocklist re-export, and native
package-manager guidance.

- [ ] **Step 3: Move adopted authority types and helpers**

Move the following code block from `update/mod.rs` into
`adopted_authority.rs`, keeping function bodies unchanged except for
visibility:

- `AdoptedUpdateDecision`
- `adopted_update_decision`
- `AdoptedUpdateSkipReason`
- `AdoptedUpdateSkip`
- `native_manager_for_trove`
- `render_adopted_skip_sample`
- `no_update_message`

Use this visibility pattern:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdoptedUpdateDecision {
    SkipNativeAuthority,
    QueueTakeover,
    BlockCritical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdoptedUpdateSkipReason {
    NativeAuthority,
    CriticalBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdoptedUpdateSkip {
    pub(super) package: String,
    pub(super) manager: SystemPackageManager,
    pub(super) reason: AdoptedUpdateSkipReason,
}
```

Then mark these parent-called functions as `pub(super)` while keeping their
existing bodies unchanged:

- `adopted_update_decision`
- `native_manager_for_trove`
- `render_adopted_skip_sample`
- `no_update_message`

Inside `adopted_update_decision`, replace the old sibling-module reference:

```rust
super::install::is_package_blocked(&trove.name)
```

with the new module import:

```rust
install::is_package_blocked(&trove.name)
```

Expected: the new module owns adopted authority policy and exposes only the
surface `update/mod.rs` already uses.

- [ ] **Step 4: Keep required `update/mod.rs` imports**

Keep both `PackageFormat` and `SystemPackageManager` in `update/mod.rs`.
`PackageFormat` is required for the trait-provided `CcsPackage::parse(...)`
call in `preflight_prepared_full_update_legacy_replay`, and
`SystemPackageManager` is still required because `cmd_update` and
`cmd_update_group` call `SystemPackageManager::detect()`:

```rust
use conary_core::packages::{PackageFormat, SystemPackageManager};
```

Expected: `update/mod.rs` keeps the package-format trait import for CCS update
preflight parsing and native package-manager detection for parent
orchestration.

- [ ] **Step 5: Move direct adopted authority tests**

Move the nested `adopted_update_tests` module from `update/mod.rs` into a
`#[cfg(test)]` module at the bottom of `adopted_authority.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, Trove, TroveType};

    fn adopted_trove(name: &str) -> Trove {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
        );
        trove.version_scheme = Some("debian".to_string());
        trove
    }
}
```

Move these tests with unchanged assertions:

- `adopted_updates_do_not_take_over_without_explicit_takeover_mode`
- `adopted_updates_take_over_only_under_explicit_takeover_mode`
- `critical_adopted_packages_are_blocked_even_under_takeover_mode`
- `adopted_updates_are_not_queued_under_satisfy_or_adopt`
- `adopted_update_guidance_uses_recorded_version_scheme_before_live_detection`
- `adopted_update_skip_message_is_not_generic_up_to_date_text`

Expected: adopted authority tests now resolve under
`commands::update::adopted_authority::tests::*`, while parent update tests no
longer contain the adopted authority-only nested module.

- [ ] **Step 6: Run focused adopted authority tests and compile**

Run:

```bash
cargo test -p conary --lib commands::update::adopted_authority::tests
cargo check -p conary
```

Expected: all moved direct adopted authority tests pass under the new module
path and `conary` compiles.

- [ ] **Step 7: Commit the adopted authority extraction**

Run:

```bash
git add apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/adopted_authority.rs
git commit -m "refactor(update): extract adopted authority policy"
```

Expected: the first code commit contains the adopted authority module, parent
import cleanup, and direct test relocation.

### Task 2: Preserve Update And Collection Behavior

**Files:**

- Verify: `apps/conary/src/commands/update/mod.rs`
- Verify: `apps/conary/src/commands/update/adopted_authority.rs`
- Verify: `apps/conary/src/commands/update/selection.rs`
- Verify: `apps/conary/tests/cli_daily_ux.rs`

- [ ] **Step 1: Run update parent-module tests**

Run:

```bash
cargo test -p conary --lib commands::update::tests
```

Expected:

- source-policy context tests remain under `commands::update::tests::*`;
- partial update failure text remains under `commands::update::tests::*`;
- selector, changeset rollback, replatform, collection, and execution tests
  remain under `commands::update::tests::*`;
- adopted authority unit tests no longer run under the parent test module.

- [ ] **Step 2: Run adopted update CLI proof**

Run:

```bash
cargo test -p conary --test cli_daily_ux adopted_update
```

Expected: user-facing adopted update guidance still routes to native package
manager update commands and refresh instructions.

- [ ] **Step 3: Check for accidental behavior-boundary movement**

Run:

```bash
rg -n "select_update_candidate|render_security_update_marker|SecurityMetadataUnavailable" apps/conary/src/commands/update
rg -n "read_delta_result_from_cas|prepare_full_updates_before_changeset|cmd_update_group|cmd_delta_stats|mark_pending_changeset_rolled_back" apps/conary/src/commands/update
rg -n "AdoptedUpdateDecision|AdoptedUpdateSkip|adopted_update_decision|native_manager_for_trove|no_update_message|render_adopted_skip_sample" apps/conary/src/commands/update
```

Expected:

- selection/security helpers remain in `apps/conary/src/commands/update/selection.rs`;
- delta/full update execution, changeset rollback, collection update, and delta
  stats remain in `apps/conary/src/commands/update/mod.rs`;
- adopted authority definitions and direct tests live in
  `apps/conary/src/commands/update/adopted_authority.rs`;
- parent call sites in `update/mod.rs` import the adopted authority surface from
  the new module.

- [ ] **Step 4: Check broad visibility**

Run:

```bash
if rg -n 'pub\(crate\)|pub +(fn|struct|enum|mod|type|use|const|static|trait)' apps/conary/src/commands/update/adopted_authority.rs; then
    echo "unexpected broad visibility in update/adopted_authority.rs" >&2
    exit 1
fi
```

Expected: the new module exposes only `pub(super)` or private items. This
check intentionally flags `pub(crate)` and bare public Rust items while
allowing the planned parent-only `pub(super)` surface.

- [ ] **Step 5: Commit behavior-preservation fixes if needed**

If Task 2 steps required import, visibility, or test-location fixes, commit
them:

```bash
git add apps/conary/src/commands/update/mod.rs \
    apps/conary/src/commands/update/adopted_authority.rs \
    apps/conary/src/commands/update/selection.rs
git commit -m "refactor(update): preserve adopted authority callers"
```

Expected: if no code changed after Task 1, skip this commit and record the
passing commands in the final implementation notes.

### Task 3: Update Active Documentation Paths

**Files:**

- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update active documentation paths**

Update active guidance that currently points to update ownership but does not
name adopted authority as a separate owner.

In `docs/llms/subsystem-map.md`, replace:

```markdown
  `apps/conary/src/commands/update/mod.rs`,
  `apps/conary/src/commands/update/selection.rs`, and
```

with:

```markdown
  `apps/conary/src/commands/update/mod.rs`,
  `apps/conary/src/commands/update/selection.rs`,
  `apps/conary/src/commands/update/adopted_authority.rs`, and
```

In `docs/modules/feature-ownership.md`, update the Package Manager Lifecycle
start-here list by replacing:

```markdown
`apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/remove.rs`;
```

with:

```markdown
`apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
`apps/conary/src/commands/remove.rs`;
```

In the Adoption, Unadoption, And Native-Authority Handoff neighbor list in
`docs/modules/feature-ownership.md`, replace:

```markdown
**Neighbor systems:** `apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
```

with:

```markdown
**Neighbor systems:** `apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
```

In `docs/modules/source-selection.md`, replace:

```markdown
- `apps/conary/src/commands/update/selection.rs` for source-switching update
  candidate behavior
```

with:

```markdown
- `apps/conary/src/commands/update/selection.rs` for source-switching update
  candidate behavior
- `apps/conary/src/commands/update/adopted_authority.rs` for adopted-update
  native-authority policy
```

Expected: active assistant/contributor docs point to the new adopted authority
owner wherever update-native authority policy is relevant.

- [ ] **Step 2: Refresh docs-audit ledger rows for touched docs**

Update the existing ledger rows for these active docs so their evidence or
notes mention the Phase 11 update adopted authority split:

- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`

Use literal tabs between TSV fields. Replace the three existing rows with
these rows:

```text
docs/llms/subsystem-map.md	docs/llms/subsystem-map.md	canonical	contributor	assistant-guidance; subsystem-map; feature-ownership; update-selection; update-adopted-authority	docs/ARCHITECTURE.md; docs/modules/feature-ownership.md; docs/modules/test-fixtures.md; crates/conary-core/src/generation/builder/runtime_inputs.rs; docs/operations/post-generation-export-follow-up-roadmap.md; apps/conary/src/commands/adopt/native_handoff.rs; crates/conary-core/src/ccs/convert; apps/remi/src/server/publication.rs; apps/conary/src/commands/install/mod.rs; apps/conary/src/commands/install/legacy_replay.rs; apps/conary/src/commands/install/inner.rs; apps/conary/src/commands/install/batch.rs; apps/conary/src/commands/install/restore.rs; apps/conary/src/commands/update/mod.rs; apps/conary/src/commands/update/selection.rs; apps/conary/src/commands/update/adopted_authority.rs	verified	corrected	Refreshed subsystem pointers to route feature-scoped work through the feature ownership map while keeping existing selected-generation, fixture, Remi/CCS, install replay, update selection, and update adopted authority pointers compact.
docs/modules/feature-ownership.md	docs/modules/feature-ownership.md	canonical	contributor	feature-ownership; contributor-ux; verification; interaction-gates; update-selection; update-adopted-authority	AGENTS.md; CONTRIBUTING.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/ARCHITECTURE.md; docs/INTEGRATION-TESTING.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/conaryd.md; docs/modules/source-selection.md; docs/modules/test-fixtures.md; docs/operations/bootstrap-selfhosting-vm.md; docs/operations/daily-driver-ux-matrix.md; docs/operations/post-generation-export-follow-up-roadmap.md; apps/conary/src/commands/install; apps/conary/src/commands/adopt; apps/conary/src/commands/ccs; apps/conary/src/commands/update/mod.rs; apps/conary/src/commands/update/selection.rs; apps/conary/src/commands/update/adopted_authority.rs; apps/remi/src/server; apps/conaryd/src/daemon; apps/conary-test/src; crates/conary-core/src/ccs; crates/conary-core/src/generation; crates/conary-agent-contract/src; crates/conary-mcp/src	verified	corrected	Added the canonical feature ownership map with start-here paths, neighboring systems, focused proof commands, broader interaction gates, docs routing, safety notes for major Conary capabilities, Phase 10 update module path ownership, and Phase 11 update adopted authority path ownership.
docs/modules/source-selection.md	docs/modules/source-selection.md	canonical	contributor	source-selection; policy; update-selection; update-adopted-authority	crates/conary-core/src/repository/effective_policy.rs; apps/conary/src/commands/update/mod.rs; apps/conary/src/commands/update/selection.rs; apps/conary/src/commands/update/adopted_authority.rs; apps/conary/src/commands/model.rs	verified	corrected	Updated update-flow notes for no-generation mutable live-root updates, adopted-package native authority, explicit takeover, security-advisory support refusal, the Phase 10 update module path split, and the Phase 11 update adopted authority split.
```

Expected: the docs-audit ledger remains current for every active doc modified
by this phase, without changing the final row counts.

- [ ] **Step 3: Run docs checks**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
if rg -n "adopted authority|native-authority|AdoptedUpdate" docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/source-selection.md; then
    true
else
    echo "expected adopted authority docs routing was not found" >&2
    exit 1
fi
```

Expected:

- docs-audit inventory count remains `155` after plan lock-in;
- docs-audit ledger check passes;
- active docs mention the new adopted authority owner.

- [ ] **Step 4: Commit docs routing**

Run:

```bash
git add docs/llms/subsystem-map.md \
    docs/modules/feature-ownership.md \
    docs/modules/source-selection.md \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: route update adopted authority owner"
```

Expected: docs-only routing commit succeeds.

### Task 4: Final Workspace Verification

**Files:**

- Verify: `apps/conary/src/commands/update/mod.rs`
- Verify: `apps/conary/src/commands/update/adopted_authority.rs`
- Verify: `apps/conary/src/commands/update/selection.rs`
- Verify: `docs/llms/subsystem-map.md`
- Verify: `docs/modules/feature-ownership.md`
- Verify: `docs/modules/source-selection.md`
- Verify: `scripts/maintainability-drift-report.sh`

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --check
```

Expected: formatting passes. If it fails, run `cargo fmt`, inspect the diff,
and rerun `cargo fmt --check`.

- [ ] **Step 2: Compile the owning package**

Run:

```bash
cargo check -p conary
```

Expected: `conary` compiles.

- [ ] **Step 3: Run the focused proof suite**

Run:

```bash
cargo test -p conary --lib commands::update::adopted_authority::tests
cargo test -p conary --lib commands::update::tests
cargo test -p conary --lib commands::update::selection::tests
cargo test -p conary --test cli_daily_ux adopted_update
```

Expected: adopted authority tests, parent update behavior tests, selection tests,
and CLI adopted-update proof all pass.

- [ ] **Step 4: Run the full `conary` package test suite**

Run:

```bash
cargo test -p conary
```

Expected: the complete `conary` package test suite passes after the module
split, catching import, module-discovery, or re-export fallout outside the
focused update filters.

- [ ] **Step 5: Run Clippy for the touched package**

Run:

```bash
cargo clippy -p conary --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 6: Verify hotspot reduction and drift routing**

Run:

```bash
test -x scripts/maintainability-drift-report.sh
scripts/line-count-report.sh 15
wc -l apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/adopted_authority.rs apps/conary/src/commands/update/selection.rs
scripts/maintainability-drift-report.sh --base origin/main | sed -n '1,60p'
```

Expected:

- `scripts/maintainability-drift-report.sh` exists and is executable;
- `update/mod.rs` drops by roughly the size of the moved adopted authority
  cluster;
- `adopted_authority.rs` is a small focused module;
- the drift report maps `apps/conary/src/commands/update/adopted_authority.rs`
  to the native package install/update/remove feature hint.

- [ ] **Step 7: Check diff hygiene**

Run:

```bash
git diff --check
git status --short --branch
```

Expected: no whitespace errors; status shows only intentional working tree
changes if commits are not yet made, or a clean branch if all task commits have
landed.

- [ ] **Step 8: Commit final verification fixes if needed**

If formatting or Clippy required changes after prior commits, commit them:

```bash
git add apps/conary/src/commands/update/mod.rs \
    apps/conary/src/commands/update/adopted_authority.rs \
    docs/llms/subsystem-map.md \
    docs/modules/feature-ownership.md \
    docs/modules/source-selection.md
git commit -m "refactor(update): finish adopted authority split"
```

Expected: no uncommitted code changes remain after the final task.

## Final Verification Before Merge

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::update::adopted_authority::tests
cargo test -p conary --lib commands::update::tests
cargo test -p conary --lib commands::update::selection::tests
cargo test -p conary --test cli_daily_ux adopted_update
cargo test -p conary
cargo clippy -p conary --all-targets -- -D warnings
git diff --check
```

Expected:

- docs-audit inventory count is `155` after plan lock-in;
- docs-audit ledger check passes;
- format, check, focused tests, integration test, full conary suite, and Clippy
  all pass;
- `git diff --check` reports no whitespace errors.

## Rollback

This plan should land as small commits. If a later task exposes a regression,
identify the Phase 11 commits and revert them newest-first:

```bash
git log --oneline --max-count=6
git revert HEAD
```

Because this slice is a behavior-preserving module extraction, rollback should
restore the previous `update/mod.rs` owner without requiring schema, data, CLI,
or daemon compatibility work.

## Completion Notes Template

Use this in the final implementation response:

```markdown
Implemented Phase 11 update adopted authority decomposition.

Changed:
- Added `apps/conary/src/commands/update/adopted_authority.rs`.
- Moved adopted update decision policy, native package-manager fallback
  guidance, adopted skip records, summary text, and direct unit tests out of
  `update/mod.rs`.
- Updated active docs and docs-audit rows for the new owner path.

Verification:
- `cargo fmt --check`
- `cargo check -p conary`
- `cargo test -p conary --lib commands::update::adopted_authority::tests`
- `cargo test -p conary --lib commands::update::tests`
- `cargo test -p conary --lib commands::update::selection::tests`
- `cargo test -p conary --test cli_daily_ux adopted_update`
- `cargo test -p conary`
- `cargo clippy -p conary --all-targets -- -D warnings`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `git diff --check`

Hotspot result:
- Reported by
  `wc -l apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/adopted_authority.rs`.
```
