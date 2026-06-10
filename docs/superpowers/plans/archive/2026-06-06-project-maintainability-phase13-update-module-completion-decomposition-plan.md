# Project Maintainability Phase 13 Update Module Completion Decomposition Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. This is the Phase 13 child packet
> under
> `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Finish decomposing `apps/conary/src/commands/update/mod.rs` into
focused update submodules so `update/mod.rs` becomes a routing hub while all
public command routes and update behavior remain unchanged.

**Architecture:** Keep the Phase 10-12 update submodules and add
`package.rs`, `source_policy.rs`, `pinning.rs`, and `delta_stats.rs` as focused
owners for the remaining update command responsibilities. Preserve the
`commands::cmd_update`, `commands::cmd_update_group`, `commands::cmd_pin`,
`commands::cmd_unpin`, `commands::cmd_list_pinned`, and
`commands::cmd_delta_stats` public surface by re-exporting each command through
`update/mod.rs`. Do not change selection, adopted-authority, collection update,
delta/full update execution, security update, source-policy preview, pinning,
or dispatch behavior.

**Tech Stack:** Rust, existing Conary command modules, existing
`conary_core` repository/model/database APIs, existing update `selection`,
`adopted_authority`, and `collection` submodules, cargo unit/integration tests,
docs-audit scripts.

---

## Status

Draft plan for local and external review.

## Candidate Choice

The Phase 12 implementation reduced `apps/conary/src/commands/update/mod.rs`
from 2320 lines to 2002 lines, but the parent module still owns five command
surfaces plus the single-package update execution engine. Instead of another
small extraction, Phase 13 should complete the update-module decomposition in a
single larger plan with internal checkpoints.

Alternatives considered:

| Candidate | Trade-off | Decision |
|-----------|-----------|----------|
| Finish `update/mod.rs` decomposition | Larger than the prior three update phases, but still bounded to one command subtree and preserves all public re-exports | Choose for Phase 13 |
| Extract only `update/source_policy.rs` | Lowest-risk next step, but leaves most of the parent hotspot untouched | Fold into the larger Phase 13 |
| Extract only pinning and delta stats | Very safe, but line-count impact is too small and leaves the real execution seam for another cycle | Fold into the larger Phase 13 |
| Move to another hotspot such as `ccs/install.rs` or Remi conversion | Targets larger files, but abandons the update lane just before it can become a clean hub | Defer until update is tidy |

## Read First

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/modules/test-fixtures.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md`
- `apps/conary/src/commands/mod.rs`
- `apps/conary/src/commands/update/mod.rs`
- `apps/conary/src/commands/update/selection.rs`
- `apps/conary/src/commands/update/adopted_authority.rs`
- `apps/conary/src/commands/update/collection.rs`
- `apps/conary/src/commands/replatform_rendering.rs`
- `apps/conary/src/dispatch.rs`
- `apps/conary/tests/query.rs`
- `apps/conary/tests/cli_daily_ux.rs`
- `apps/conary/tests/native_pm_live_root.rs`

## Current Repo-Grounded Inputs

| Signal | Current value | Phase 13 interpretation |
|--------|---------------|-------------------------|
| Current Rust hotspots | `apps/conary/src/commands/ccs/install.rs` 3118 lines; `apps/remi/src/server/conversion.rs` 2999 lines; `apps/conary/src/commands/install/mod.rs` 2874 lines; `crates/conary-core/src/scriptlet/mod.rs` 2408 lines; `apps/conaryd/src/daemon/routes.rs` 2345 lines; `apps/conary/src/commands/update/mod.rs` 2002 lines | `update/mod.rs` is still a top CLI hotspot but is now fully prepared for a hub split |
| Existing update submodules | `adopted_authority.rs`, `collection.rs`, `selection.rs`, and `mod.rs` | Add command-owner siblings instead of changing external routing |
| Current public update commands | `cmd_update`, `cmd_update_group`, `cmd_pin`, `cmd_unpin`, `cmd_list_pinned`, `cmd_delta_stats` are re-exported from `commands/mod.rs` through `update` | Preserve this public surface exactly |
| Current parent-only tests | `cargo test -p conary --lib commands::update::tests -- --list` finds 14 tests | Move tests to `package.rs` and `source_policy.rs`; `update/mod.rs` should have no test module after the split |
| Source-policy preview cluster | `source_policy_update_context`, `render_replatform_action_preview`, and the package-none preview block in `cmd_update` | Move into `update/source_policy.rs` behind one parent-callable preview function |
| Pinning cluster | `cmd_pin`, `cmd_unpin`, `cmd_list_pinned` | Move into `update/pinning.rs` |
| Delta stats cluster | `cmd_delta_stats` | Move into `update/delta_stats.rs` |
| Single-package update execution cluster | `cmd_update` plus helper types/functions for installed target selection, delta/full update preparation, legacy replay preflight, CAS delta retrieval, rollback marking, and summary failures | Move into `update/package.rs` as the remaining package-update owner |
| Docs-audit baseline | 156 tracked doc-like files, 56 corrected rows | Lock-in should add one planning file and update counts to 157 total / 57 corrected |

Evidence commands used to shape this packet:

```bash
git status --short --branch
scripts/line-count-report.sh 30
find apps/conary/src/commands/update -maxdepth 1 -type f | sort
rg -n "^(pub |async |fn |struct |enum |impl |mod |use )" apps/conary/src/commands/update/mod.rs
rg -n "#\[cfg\(test\)\]|mod tests|fn .*test|test_" apps/conary/src/commands/update/mod.rs
rg -n "cmd_update\(|cmd_update_group\(|cmd_pin\(|cmd_unpin\(|cmd_list_pinned\(|cmd_delta_stats\(" apps/conary/src apps/conary/tests docs -g '*.rs' -g '*.md'
rg -n "UpdatePackageFailure|PreparedFullUpdate|read_delta_result_from_cas|resolution_options_for_selected_update|mark_pending_changeset_rolled_back|update_required_failure_message|prepare_full_updates_before_changeset|preflight_prepared_full_update_legacy_replay|install_options_for_update|installed_troves_for_update" apps/conary/src/commands/update apps/conary/src apps/conary/tests -g '*.rs'
cargo test -p conary --lib commands::update::tests -- --list
cargo test -p conary --lib source_policy_update_context -- --list
cargo test -p conary --lib replatform -- --list
cargo test -p conary --lib delta -- --list
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
```

Current filter-discovery results:

| Filter | Current matches |
|--------|-----------------|
| `cargo test -p conary --lib commands::update::tests -- --list` | 14 tests in the parent module |
| `cargo test -p conary --lib source_policy_update_context -- --list` | 2 source-policy context tests |
| `cargo test -p conary --lib replatform -- --list` | 17 tests, including 2 update replatform preview/planning tests |
| `cargo test -p conary --lib delta -- --list` | 2 update-owned delta execution tests |

## Target Module Boundary

Create:

- `apps/conary/src/commands/update/package.rs`
- `apps/conary/src/commands/update/source_policy.rs`
- `apps/conary/src/commands/update/pinning.rs`
- `apps/conary/src/commands/update/delta_stats.rs`

Modify:

- `apps/conary/src/commands/update/mod.rs`
- `apps/conary/src/commands/update/collection.rs`
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

Keep existing public command routes:

```rust
pub use collection::cmd_update_group;
pub use delta_stats::cmd_delta_stats;
pub use package::cmd_update;
pub use pinning::{cmd_list_pinned, cmd_pin, cmd_unpin};
```

Final `apps/conary/src/commands/update/mod.rs` should be a hub:

```rust
// src/commands/update/mod.rs
//! Update command module routing.

mod adopted_authority;
mod collection;
mod delta_stats;
mod package;
mod pinning;
mod selection;
mod source_policy;

pub use collection::cmd_update_group;
pub use delta_stats::cmd_delta_stats;
pub use package::cmd_update;
pub use pinning::{cmd_list_pinned, cmd_pin, cmd_unpin};
```

Move these items into `update/package.rs`:

- `read_delta_result_from_cas`
- `resolution_options_for_selected_update`
- `mark_pending_changeset_rolled_back`
- `UpdatePackageFailure`
- `PreparedFullUpdate`
- `update_required_failure_message`
- `prepare_full_updates_before_changeset`
- `preflight_prepared_full_update_legacy_replay`
- `install_options_for_update`
- `cmd_update`
- `installed_troves_for_update`
- package update tests:
  - `package_specific_update_requires_selector_for_ambiguous_variants`
  - `update_selector_without_package_refuses`
  - `update_refuses_legacy_replay_before_creating_changeset`
  - `update_delta_candidate_refuses_legacy_replay_before_creating_changeset`
  - `update_repository_install_provenance_uses_selected_package_metadata`
  - `selected_update_resolution_bypasses_local_cas_shortcut`
  - `partial_update_failure_message_is_not_clean_success`
  - `delta_result_uses_verified_cas_retrieval`
  - `mark_pending_changeset_rolled_back_updates_pending_rows`
  - `mark_pending_changeset_rolled_back_leaves_applied_rows_alone`

Move these items into `update/source_policy.rs`:

- new parent-callable `print_source_policy_update_preview`
- `source_policy_update_context`
- `render_replatform_action_preview`
- source-policy/replatform tests:
  - `test_source_policy_update_context_with_affinity`
  - `test_source_policy_update_context_without_affinity_data`
  - `test_update_replatform_planning_surfaces_mixed_execution_states`
  - `test_render_replatform_action_preview_lists_examples`

Move these items into `update/pinning.rs`:

- `cmd_pin`
- `cmd_unpin`
- `cmd_list_pinned`

Move these items into `update/delta_stats.rs`:

- `cmd_delta_stats`

Keep these existing modules and responsibilities unchanged:

- `update/selection.rs`: candidate selection, latest-mode source switching,
  security metadata eligibility, and source-switch preview text.
- `update/adopted_authority.rs`: adopted-package update authority decisions and
  native package-manager fallback text.
- `update/collection.rs`: `update @collection` orchestration and per-member
  dispatch.

## Non-Goals

- Do not change update behavior, output text, dry-run behavior, security-only
  behavior, adopted-package skip behavior, source-switch confirmation, delta
  fallback behavior, legacy replay preflight, changeset rollback marking, or
  pin/list behavior.
- Do not change CLI parsing, `commands/mod.rs` exports, or `dispatch.rs`
  routing.
- Do not change database schema, repository selection rules, security advisory
  support requirements, live-system mutation gates, package install behavior,
  collection update behavior, or model replatform planning.
- Do not move `selection.rs`, `adopted_authority.rs`, or `collection.rs` again
  in this phase.
- Do not refactor `cmd_update` internally beyond path/import adjustments needed
  to move it into `package.rs`.

## Risks And Checks

| Risk | Mitigation |
|------|------------|
| Public command re-export breakage | Keep `commands/mod.rs` unchanged and re-export all update command entrypoints from `update/mod.rs` |
| `collection.rs` losing access to `cmd_update` | Re-export `package::cmd_update` from `update/mod.rs`; keep `collection.rs` importing `super::cmd_update` |
| Source-policy preview import drift | Move the whole preview block behind `source_policy::print_source_policy_update_preview(&conn)?` and keep model/replatform imports local to `source_policy.rs` |
| Package execution import drift | Move `cmd_update` and all private execution helpers together into `package.rs`; do not split execution internals across modules in this phase |
| Test filters going stale | Replace parent-module test filters with `commands::update::package::tests` and `commands::update::source_policy::tests` |
| Docs path drift | Update subsystem map, feature ownership, source selection docs, and docs-audit ledger rows during implementation |
| Plan size hides a compile hazard | Commit after each module extraction and run focused tests after each structural checkpoint |

---

## Task 0: Register The Phase 13 Plan In Docs Audit

**Files:**
- Create: `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase13-update-module-completion-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Confirm clean synced baseline**

Run:

```bash
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
```

Expected:

- branch is `main...origin/main`;
- no uncommitted changes other than this draft if lock-in edits are already in
  progress;
- `HEAD` and `origin/main` match;
- left/right count is `0	0`.

- [ ] **Step 2: Stage the new plan before regenerating docs inventory**

Run:

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase13-update-module-completion-decomposition-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
```

Expected: tracked doc-like files grow from 156 to 157 because the inventory
script reads the staged index.

- [ ] **Step 3: Add the Phase 13 ledger row**

In `docs/superpowers/documentation-accuracy-audit-ledger.tsv`, locate the Phase
12 row by searching for:

```text
phase12-update-collection-decomposition-plan.md
```

Insert this new row immediately after it. The row uses literal tab characters:

```tsv
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase13-update-module-completion-decomposition-plan.md	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase13-update-module-completion-decomposition-plan.md	planning	maintainer	maintainability; phase13; update; module-hub; hotspot-decomposition	apps/conary/src/commands/update/mod.rs; apps/conary/src/commands/update/package.rs; apps/conary/src/commands/update/source_policy.rs; apps/conary/src/commands/update/pinning.rs; apps/conary/src/commands/update/delta_stats.rs; apps/conary/src/commands/update/selection.rs; apps/conary/src/commands/update/adopted_authority.rs; apps/conary/src/commands/update/collection.rs; apps/conary/src/commands/mod.rs; apps/conary/src/dispatch.rs; apps/conary/tests/query.rs; apps/conary/tests/cli_daily_ux.rs; apps/conary/tests/native_pm_live_root.rs; docs/modules/source-selection.md; docs/modules/feature-ownership.md	verified	corrected	Added Phase 13 plan for completing the update module decomposition by turning update/mod.rs into a routing hub while moving package update execution, source-policy previewing, pinning commands, and delta statistics into focused update submodules.
```

- [ ] **Step 4: Update the audit summary and counts**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph to the existing `### 2026-06-06 Maintainability Planning` section:

```markdown
The Phase 13 update module completion decomposition plan finishes the update
hotspot lane by targeting the remaining responsibilities in
`apps/conary/src/commands/update/mod.rs`. It plans to turn `update/mod.rs` into
a routing hub while extracting single-package update execution into
`apps/conary/src/commands/update/package.rs`, source-policy/replatform update
previewing into `apps/conary/src/commands/update/source_policy.rs`, pinning
commands into `apps/conary/src/commands/update/pinning.rs`, and delta statistics
into `apps/conary/src/commands/update/delta_stats.rs`.
```

Then update the final counts from:

```markdown
- Total tracked doc-like files audited: 156
- `verified-no-change`: 13
- `corrected`: 56
- `archived`: 73
- `retained-historical`: 14
```

to:

```markdown
- Total tracked doc-like files audited: 157
- `verified-no-change`: 13
- `corrected`: 57
- `archived`: 73
- `retained-historical`: 14
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its
`evidence_sources`, categories, and notes mention Phase 13.

- [ ] **Step 5: Verify docs-audit lock-in**

Stage the plan and refreshed audit files before checking the cached diff:

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase13-update-module-completion-decomposition-plan.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md
```

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
```

Expected:

- inventory count is `157`;
- ledger distribution includes `corrected 57`;
- malformed-row check prints nothing;
- ledger checker passes;
- diff check passes.

- [ ] **Step 6: Commit plan lock-in**

Run:

```bash
git status --short
git commit -m "docs: plan update module completion"
```

Expected: docs-only commit. Do not implement code in this task.

---

## Task 1: Extract Source-Policy Update Preview

**Files:**
- Create: `apps/conary/src/commands/update/source_policy.rs`
- Modify: `apps/conary/src/commands/update/mod.rs`

- [ ] **Step 1: Add the source policy submodule**

In `apps/conary/src/commands/update/mod.rs`, add:

```rust
mod source_policy;
```

Keep it private. The parent/package command will call a `pub(super)` helper;
there is no public command export from this module.

- [ ] **Step 2: Create `source_policy.rs` with local imports**

Create `apps/conary/src/commands/update/source_policy.rs`:

```rust
// src/commands/update/source_policy.rs

//! Source-policy and replatform preview helpers for update commands.

use super::super::replatform_rendering::render_replatform_execution_plan;
use anyhow::Result;
use conary_core::db::models::{DistroPin, SystemAffinity};
use conary_core::model::{
    DiffAction, capture_current_state, planned_replatform_actions, replatform_execution_plan,
    source_policy_replatform_snapshot,
};
use rusqlite::Connection;
```

- [ ] **Step 3: Move the preview helper functions**

Move `source_policy_update_context` and `render_replatform_action_preview` from
`update/mod.rs` into `source_policy.rs` without behavior changes.

Add this parent-callable wrapper above the private helpers. `pub(super)` is the
correct visibility: it makes the function visible to the parent `update` module
and the parent module's descendants, so `update/mod.rs` can call it before Task
4 and `update/package.rs` can call it afterward through
`super::source_policy::print_source_policy_update_preview` without widening it
to `pub(crate)`.

```rust
pub(super) fn print_source_policy_update_preview(conn: &Connection) -> Result<()> {
    let current_pin = DistroPin::get_current(conn)?;
    let affinities = SystemAffinity::list(conn)?;
    let realignment_snapshot = current_pin
        .as_ref()
        .map(|pin| source_policy_replatform_snapshot(conn, &pin.distro))
        .transpose()?;
    let realignment_candidates = realignment_snapshot
        .as_ref()
        .map(|snapshot| snapshot.visible_realignment_candidates);
    if let Some(context) =
        source_policy_update_context(current_pin.as_ref(), &affinities, realignment_candidates)
    {
        println!("{}", context);
    }
    if let Some(snapshot) = realignment_snapshot.as_ref() {
        let state = capture_current_state(conn)?;
        let actions = planned_replatform_actions(snapshot, &state);
        if let Some(plan) = replatform_execution_plan(conn, &actions)? {
            println!("{}", render_replatform_execution_plan(&plan));
        } else if let Some(preview) = render_replatform_action_preview(&actions) {
            println!("{}", preview);
        }
    }

    Ok(())
}
```

Then keep the moved helpers private with these exact bodies:

```rust
fn source_policy_update_context(
    pin: Option<&DistroPin>,
    affinities: &[SystemAffinity],
    realignment_candidates: Option<usize>,
) -> Option<String> {
    let pin = pin?;
    let strength = pin.mixing_policy.as_str();

    if affinities.is_empty() {
        return Some(format!(
            "Active source policy pin: {} ({}). Replatform estimate unavailable: no source affinity data yet.{}",
            pin.distro,
            strength,
            match realignment_candidates {
                Some(count) => format!(
                    " Package-level realignment candidates currently visible: {}.",
                    count
                ),
                None => String::new(),
            }
        ));
    }

    let total_packages: i64 = affinities
        .iter()
        .map(|affinity| affinity.package_count)
        .sum();
    if total_packages == 0 {
        return Some(format!(
            "Active source policy pin: {} ({}). Replatform estimate unavailable: no installed packages are represented in current affinity data.{}",
            pin.distro,
            strength,
            match realignment_candidates {
                Some(count) => format!(
                    " Package-level realignment candidates currently visible: {}.",
                    count
                ),
                None => String::new(),
            }
        ));
    }

    let aligned_packages = affinities
        .iter()
        .find(|affinity| affinity.distro == pin.distro)
        .map(|affinity| affinity.package_count)
        .unwrap_or(0);
    let packages_to_realign = total_packages.saturating_sub(aligned_packages);

    Some(format!(
        "Active source policy pin: {} ({}). About {} installed package(s) already align, and about {} may need source realignment during future convergence.{}",
        pin.distro,
        strength,
        aligned_packages,
        packages_to_realign,
        match realignment_candidates {
            Some(count) => format!(
                " Package-level realignment candidates currently visible: {}.",
                count
            ),
            None => String::new(),
        }
    ))
}

fn render_replatform_action_preview(actions: &[DiffAction]) -> Option<String> {
    let replatforms: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            DiffAction::ReplatformReplace { .. } => Some(action.description()),
            _ => None,
        })
        .collect();

    if replatforms.is_empty() {
        return None;
    }

    let preview: Vec<String> = replatforms.iter().take(3).cloned().collect();

    let mut line = format!("Planned replatform replacements: {}", preview.join(", "));
    if replatforms.len() > preview.len() {
        line.push_str(&format!(", +{} more", replatforms.len() - preview.len()));
    }
    Some(line)
}
```

- [ ] **Step 4: Replace the package-none preview block**

In `cmd_update`, replace the current `if package.is_none()` block that loads
`DistroPin`, `SystemAffinity`, and replatform snapshots with:

```rust
if package.is_none() {
    source_policy::print_source_policy_update_preview(&conn)?;
}
```

When Task 4 later moves `cmd_update` into `package.rs`, this call becomes:

```rust
if package.is_none() {
    print_source_policy_update_preview(&conn)?;
}
```

with `use super::source_policy::print_source_policy_update_preview;` in
`package.rs`.

- [ ] **Step 5: Move source-policy tests**

Move these tests from the parent test module into
`source_policy.rs` under `#[cfg(test)] mod tests`:

- `test_source_policy_update_context_with_affinity`
- `test_source_policy_update_context_without_affinity_data`
- `test_update_replatform_planning_surfaces_mixed_execution_states`
- `test_render_replatform_action_preview_lists_examples`

Use this test-module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::{create_test_db, seed_mixed_replatform_fixture};
    use conary_core::db::models::DistroPin;
    use conary_core::model::ReplatformBlockedReason;
}
```

Remove `seed_mixed_replatform_fixture`, `DiffAction`,
`ReplatformBlockedReason`, and model replatform imports from the parent test
module if no remaining parent tests need them.

- [ ] **Step 6: Clean up parent imports**

After the move, remove these parent imports from `update/mod.rs` if the
compiler confirms they are unused there:

```rust
use conary_core::model::{
    DiffAction, capture_current_state, planned_replatform_actions, replatform_execution_plan,
    source_policy_replatform_snapshot,
};
use super::replatform_rendering::render_replatform_execution_plan;
```

Also remove `DistroPin` and `SystemAffinity` from parent runtime imports if
they are no longer needed outside tests.

- [ ] **Step 7: Verify source-policy extraction**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib --no-run
cargo test -p conary --lib commands::update::source_policy::tests
cargo test -p conary --lib source_policy_update_context
cargo test -p conary --lib replatform
```

Expected:

- all commands pass;
- source-policy tests now live under `commands::update::source_policy::tests`;
- no behavior changes.

- [ ] **Step 8: Commit source-policy extraction**

Run:

```bash
git add apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/source_policy.rs
git diff --cached --check
git commit -m "refactor(update): extract source policy preview"
```

---

## Task 2: Extract Pinning Commands

**Files:**
- Create: `apps/conary/src/commands/update/pinning.rs`
- Modify: `apps/conary/src/commands/update/mod.rs`

- [ ] **Step 1: Add the pinning submodule and re-export**

In `apps/conary/src/commands/update/mod.rs`, add:

```rust
mod pinning;

pub use pinning::{cmd_list_pinned, cmd_pin, cmd_unpin};
```

- [ ] **Step 2: Create `pinning.rs`**

Create `apps/conary/src/commands/update/pinning.rs`:

```rust
// src/commands/update/pinning.rs

//! Update pinning command handlers.

use super::super::{InstalledPackageSelector, open_db, resolve_installed_package};
use anyhow::Result;
use conary_core::db::models::Trove;
use tracing::info;
```

- [ ] **Step 3: Move pinning command bodies**

Move these functions from `update/mod.rs` into `pinning.rs` unchanged:

- `cmd_pin`
- `cmd_unpin`
- `cmd_list_pinned`

The signatures must remain:

```rust
pub async fn cmd_pin(selector: InstalledPackageSelector, db_path: &str) -> Result<()>;
pub async fn cmd_unpin(selector: InstalledPackageSelector, db_path: &str) -> Result<()>;
pub async fn cmd_list_pinned(db_path: &str) -> Result<()>;
```

- [ ] **Step 4: Verify pinning routes**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib --no-run
rg -n "cmd_pin|cmd_unpin|cmd_list_pinned" apps/conary/src/commands/mod.rs apps/conary/src/dispatch.rs apps/conary/src/commands/update
```

Expected:

- `commands/mod.rs` still re-exports pin/list commands through `update`;
- `dispatch.rs` still calls `commands::cmd_pin`, `commands::cmd_unpin`, and
  `commands::cmd_list_pinned`;
- implementations live in `update/pinning.rs`.

- [ ] **Step 5: Commit pinning extraction**

Run:

```bash
git add apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/pinning.rs
git diff --cached --check
git commit -m "refactor(update): extract pinning commands"
```

---

## Task 3: Extract Delta Statistics Command

**Files:**
- Create: `apps/conary/src/commands/update/delta_stats.rs`
- Modify: `apps/conary/src/commands/update/mod.rs`

- [ ] **Step 1: Add the delta stats submodule and re-export**

In `apps/conary/src/commands/update/mod.rs`, add:

```rust
mod delta_stats;

pub use delta_stats::cmd_delta_stats;
```

- [ ] **Step 2: Create `delta_stats.rs`**

Create `apps/conary/src/commands/update/delta_stats.rs`:

```rust
// src/commands/update/delta_stats.rs

//! Delta update statistics command handler.

use super::super::open_db;
use anyhow::Result;
use conary_core::db::models::DeltaStats;
use tracing::info;
```

- [ ] **Step 3: Move `cmd_delta_stats`**

Move the full `cmd_delta_stats` function from `update/mod.rs` into
`delta_stats.rs` unchanged.

Keep the signature:

```rust
pub async fn cmd_delta_stats(db_path: &str) -> Result<()>;
```

- [ ] **Step 4: Verify delta stats route**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib --no-run
rg -n "cmd_delta_stats" apps/conary/src/commands/mod.rs apps/conary/src/dispatch.rs apps/conary/src/commands/update
```

Expected:

- `commands/mod.rs` still re-exports `cmd_delta_stats` through `update`;
- `dispatch.rs` still calls `commands::cmd_delta_stats`;
- implementation lives in `update/delta_stats.rs`.

- [ ] **Step 5: Commit delta stats extraction**

Run:

```bash
git add apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/delta_stats.rs
git diff --cached --check
git commit -m "refactor(update): extract delta stats command"
```

---

## Task 4: Extract Single-Package Update Execution

**Files:**
- Create: `apps/conary/src/commands/update/package.rs`
- Modify: `apps/conary/src/commands/update/mod.rs`
- Modify: `apps/conary/src/commands/update/collection.rs`

- [ ] **Step 1: Add the package update submodule and re-export**

In `apps/conary/src/commands/update/mod.rs`, add:

```rust
mod package;

pub use package::cmd_update;
```

- [ ] **Step 2: Create `package.rs` with the execution import surface**

Create `apps/conary/src/commands/update/package.rs`:

```rust
// src/commands/update/package.rs

//! Single-package update command execution.

use super::adopted_authority::{
    AdoptedUpdateDecision, AdoptedUpdateSkip, AdoptedUpdateSkipReason, adopted_update_decision,
    native_manager_for_trove, no_update_message, render_adopted_skip_sample,
};
use super::selection::{
    SecurityMetadataUnavailable, SelectedUpdateCandidate, UpdateCandidateSelection,
    print_security_metadata_unavailable, print_source_switch_preview,
    render_security_update_marker, requires_source_switch_confirmation,
    security_metadata_unavailable_error, select_update_candidate,
};
use super::source_policy::print_source_policy_update_preview;
use super::super::install::{
    CcsTransactionInstallOptions, ComponentSelection, DepMode,
    repository_install_provenance_from_package, resolve_default_dep_mode_from_model,
};
use super::super::progress::{UpdatePhase, UpdateProgress};
use super::super::{
    InstallOptions, InstalledPackageSelector, LegacyReplayOptions, SandboxMode, cmd_install,
    open_db, resolve_installed_package,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::db::models::{
    DeltaStats, PackageDelta, Repository, RepositoryPackage, Trove,
};
use conary_core::db::paths::objects_dir;
use conary_core::delta::DeltaApplier;
use conary_core::packages::{PackageFormat, SystemPackageManager};
use conary_core::repository::{
    self, DownloadOptions, PackageSource, ResolutionOptions,
    dependency_model::RepositoryDependencyFlavor, resolution_policy::ResolutionPolicy,
    resolve_package,
};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
```

If the compiler reports an unused import after the move, remove only that
confirmed-unused import. Keep `PackageFormat` unless the compiler proves it is
unused; `CcsPackage::parse` may require the trait in scope.

- [ ] **Step 3: Move package update helpers**

Move these private items from `update/mod.rs` into `package.rs` without
behavior changes:

- `read_delta_result_from_cas`
- `resolution_options_for_selected_update`
- `mark_pending_changeset_rolled_back`
- `UpdatePackageFailure`
- `PreparedFullUpdate`
- `update_required_failure_message`
- `prepare_full_updates_before_changeset`
- `preflight_prepared_full_update_legacy_replay`
- `install_options_for_update`
- `installed_troves_for_update`

Preserve all existing attributes on moved helpers, including
`#[allow(clippy::too_many_arguments)]` on
`prepare_full_updates_before_changeset`,
`preflight_prepared_full_update_legacy_replay`, and
`install_options_for_update`.

After the move, adjust `super::` paths for child-module depth:

```rust
// Old in update/mod.rs:
legacy_replay: super::LegacyReplayOptions,
) -> Result<super::InstallOptions<'a>> {
    Ok(super::InstallOptions {

// New in update/package.rs:
legacy_replay: LegacyReplayOptions,
) -> Result<InstallOptions<'a>> {
    Ok(InstallOptions {
```

Also update the inline delta install options inside the transferred
`cmd_update` body:

```rust
// Old in update/mod.rs:
super::InstallOptions {

// New in update/package.rs:
InstallOptions {
```

Similarly, change install helper paths from:

```rust
super::install::plan_ccs_fresh_install_legacy_replay
super::install::plan_ccs_old_installed_upgrade_legacy_replay
super::install::merge_old_upgrade_legacy_replay_state
```

to:

```rust
super::super::install::plan_ccs_fresh_install_legacy_replay
super::super::install::plan_ccs_old_installed_upgrade_legacy_replay
super::super::install::merge_old_upgrade_legacy_replay_state
```

- [ ] **Step 4: Move `cmd_update`**

Move the full `cmd_update` function from `update/mod.rs` into `package.rs`.

Keep the public signature stable:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn cmd_update(
    package: Option<String>,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    dep_mode: Option<DepMode>,
    yes: bool,
    package_version: Option<String>,
    architecture: Option<String>,
    legacy_replay: LegacyReplayOptions,
) -> Result<()>;
```

Preserve the existing `#[allow(clippy::too_many_arguments)]` attribute on
`cmd_update`.

Inside the transferred `cmd_update` function, call the source-policy helper
through the local import:

```rust
if package.is_none() {
    print_source_policy_update_preview(&conn)?;
}
```

Do not change the rest of the body beyond path/import fixes.

- [ ] **Step 5: Keep collection dispatch wired to `cmd_update`**

In `apps/conary/src/commands/update/collection.rs`, keep:

```rust
use super::cmd_update;
```

Expected: it resolves to `pub use package::cmd_update;` in `update/mod.rs`.
Do not import `super::package::cmd_update` directly; preserving the parent
re-export is the regression guard for public routing.

- [ ] **Step 6: Move package update tests**

Move these tests from `update/mod.rs` into `package.rs` under
`#[cfg(test)] mod tests`:

- `package_specific_update_requires_selector_for_ambiguous_variants`
- `update_selector_without_package_refuses`
- `update_refuses_legacy_replay_before_creating_changeset`
- `update_delta_candidate_refuses_legacy_replay_before_creating_changeset`
- `update_repository_install_provenance_uses_selected_package_metadata`
- `selected_update_resolution_bypasses_local_cas_shortcut`
- `partial_update_failure_message_is_not_clean_success`
- `delta_result_uses_verified_cas_retrieval`
- `mark_pending_changeset_rolled_back_updates_pending_rows`
- `mark_pending_changeset_rolled_back_leaves_applied_rows_alone`

Also move these private test helpers from the parent test module into
`package.rs` `#[cfg(test)] mod tests`; they are only used by the moving package
tests and will cause compile failures if left behind after `update/mod.rs`
becomes a hub:

- `build_test_ccs_package_with_bundle`
- `legacy_upgrade_bundle`
- `legacy_upgrade_entry`
- `serve_test_file`
- `table_count`

Use this test-module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
        LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
        PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
        TransactionOrder, VersionScheme,
    };
    use conary_core::ccs::manifest::{CcsManifest, Platform};
    use conary_core::db::models::{
        Changeset, ChangesetStatus, DistroPin, InstallSource, PackageDelta, PackageResolution,
        PrimaryStrategy, Repository, ResolutionStrategy, Trove, TroveType,
    };
    use conary_core::filesystem::{CasStore, object_path};
    use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
    use conary_core::repository::resolution_policy::ResolutionPolicy;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
}
```

If the compiler reports unused imports after moving the exact tests, remove
only those confirmed-unused imports.

- [ ] **Step 7: Reduce `update/mod.rs` to the hub**

After package tests and command bodies move, replace the remaining
`update/mod.rs` content with the target hub:

```rust
// src/commands/update/mod.rs
//! Update command module routing.

mod adopted_authority;
mod collection;
mod delta_stats;
mod package;
mod pinning;
mod selection;
mod source_policy;

pub use collection::cmd_update_group;
pub use delta_stats::cmd_delta_stats;
pub use package::cmd_update;
pub use pinning::{cmd_list_pinned, cmd_pin, cmd_unpin};
```

- [ ] **Step 8: Verify package extraction**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib --no-run
cargo test -p conary --lib commands::update::package::tests
cargo test -p conary --lib update_refuses_legacy_replay_before_creating_changeset
cargo test -p conary --lib update_delta_candidate_refuses_legacy_replay_before_creating_changeset
cargo test -p conary --lib selected_update_resolution_bypasses_local_cas_shortcut
cargo test -p conary --lib delta_result_uses_verified_cas_retrieval
cargo test -p conary --lib mark_pending_changeset_rolled_back
cargo test -p conary --lib commands::update::collection::tests
```

Expected:

- all commands pass;
- package tests now live under `commands::update::package::tests`;
- collection tests still pass through the parent `cmd_update` re-export.

- [ ] **Step 9: Verify routing and ownership**

Run:

```bash
rg -n "pub use collection::cmd_update_group|pub use delta_stats::cmd_delta_stats|pub use package::cmd_update|pub use pinning" apps/conary/src/commands/update/mod.rs
rg -n "pub async fn cmd_update|fn installed_troves_for_update|fn update_required_failure_message" apps/conary/src/commands/update/package.rs
rg -n "pub async fn cmd_pin|pub async fn cmd_unpin|pub async fn cmd_list_pinned" apps/conary/src/commands/update/pinning.rs
rg -n "pub async fn cmd_delta_stats" apps/conary/src/commands/update/delta_stats.rs
rg -n "pub\\(super\\) fn print_source_policy_update_preview" apps/conary/src/commands/update/source_policy.rs
rg -n "cmd_update_group|cmd_update|cmd_pin|cmd_unpin|cmd_list_pinned|cmd_delta_stats" apps/conary/src/commands/mod.rs apps/conary/src/dispatch.rs apps/conary/src/commands/update
rg -n "fn cmd_update|fn cmd_pin|fn cmd_unpin|fn cmd_list_pinned|fn cmd_delta_stats|mod tests" apps/conary/src/commands/update/mod.rs || echo "update/mod.rs is a routing hub -- expected"
```

Expected:

- all command bodies live in focused child modules;
- public routes still flow through `commands/mod.rs`;
- `update/mod.rs` has no command bodies and no test module.

- [ ] **Step 10: Commit package extraction and hub conversion**

Run:

```bash
git add apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/package.rs apps/conary/src/commands/update/collection.rs
git diff --cached --check
git commit -m "refactor(update): extract package update execution"
```

---

## Task 5: Update Docs Routing For The Completed Update Split

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Refresh assistant subsystem map paths**

In `docs/llms/subsystem-map.md`, update the source selection/update path list
to include the new owner files:

```markdown
  `apps/conary/src/commands/update/mod.rs`,
  `apps/conary/src/commands/update/package.rs`,
  `apps/conary/src/commands/update/source_policy.rs`,
  `apps/conary/src/commands/update/selection.rs`,
  `apps/conary/src/commands/update/adopted_authority.rs`,
  `apps/conary/src/commands/update/collection.rs`,
  `apps/conary/src/commands/update/pinning.rs`,
  `apps/conary/src/commands/update/delta_stats.rs`, and
```

Keep `apps/conary/src/commands/model.rs` in the same list after the update
module paths.

- [ ] **Step 2: Refresh feature ownership paths**

In `docs/modules/feature-ownership.md`, update both update path lists under:

- `Native Package Install, Update, Remove, And Live-Root Mutation`
- `Adoption, Unadoption, And Native-Authority Handoff`

Use:

```markdown
`apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/package.rs`;
`apps/conary/src/commands/update/source_policy.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
`apps/conary/src/commands/update/collection.rs`;
`apps/conary/src/commands/update/pinning.rs`;
`apps/conary/src/commands/update/delta_stats.rs`;
```

If the adoption card feels too broad for pinning and delta stats, keep those
two files only in the native package card and include
`package.rs`, `source_policy.rs`, `selection.rs`, `adopted_authority.rs`, and
`collection.rs` in the adoption neighbor list.

- [ ] **Step 3: Refresh source selection read-next paths**

In `docs/modules/source-selection.md`, update the `Where To Read Next` update
bullets:

```markdown
- `apps/conary/src/commands/update/mod.rs` for update module routing
- `apps/conary/src/commands/update/package.rs` for single-package update
  execution, delta/full update handling, and legacy replay preflight
- `apps/conary/src/commands/update/source_policy.rs` for source-policy update
  preview and replatform update context
- `apps/conary/src/commands/update/selection.rs` for source-switching update
  candidate behavior
- `apps/conary/src/commands/update/adopted_authority.rs` for adopted-update
  native-authority policy
- `apps/conary/src/commands/update/collection.rs` for `update @collection`
  orchestration, member filtering, and per-member update dispatch
```

- [ ] **Step 4: Refresh docs-audit files**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: inventory remains at 157 tracked doc-like files after Phase 13 plan
lock-in; implementation edits do not add new doc-like files.

Refresh existing ledger rows for:

- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

Their `evidence_sources` and notes should mention:

- `apps/conary/src/commands/update/package.rs`
- `apps/conary/src/commands/update/source_policy.rs`
- `apps/conary/src/commands/update/pinning.rs`
- `apps/conary/src/commands/update/delta_stats.rs`
- Phase 13 update module completion decomposition

Do not add new ledger rows for Rust files.

- [ ] **Step 5: Verify docs routing**

Run:

```bash
rg -n "update/package.rs|update/source_policy.rs|update/pinning.rs|update/delta_stats.rs|update-module|Phase 13" docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/source-selection.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected:

- new paths appear in the canonical routing docs and audit ledger;
- inventory remains `157`;
- ledger distribution remains `corrected 57`;
- malformed-row check prints nothing;
- ledger checker passes.

- [ ] **Step 6: Commit docs routing**

Run:

```bash
git add docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/source-selection.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git diff --cached --check
git commit -m "docs: route completed update module split"
```

---

## Task 6: Final Verification And Push

**Files:**
- Verify the full Phase 13 implementation and docs lock-in.

- [ ] **Step 1: Run focused update tests**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::update::package::tests
cargo test -p conary --lib commands::update::source_policy::tests
cargo test -p conary --lib commands::update::selection::tests
cargo test -p conary --lib commands::update::adopted_authority::tests
cargo test -p conary --lib commands::update::collection::tests
```

Expected: all focused module tests pass.

- [ ] **Step 2: Run interaction gates**

Run:

```bash
cargo test -p conary --test query update
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test native_pm_live_root
```

Expected:

- query update selector and collection update tests pass;
- daily UX update/adoption guidance tests pass;
- live-root update/security tests pass.

- [ ] **Step 3: Run broad conary gates**

Run:

```bash
cargo test -p conary
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both pass.

- [ ] **Step 4: Run ownership and docs checks**

Run:

```bash
rg -n "fn cmd_update|fn cmd_pin|fn cmd_unpin|fn cmd_list_pinned|fn cmd_delta_stats|mod tests" apps/conary/src/commands/update/mod.rs || echo "update/mod.rs is a routing hub -- expected"
rg -n "pub async fn cmd_update|fn installed_troves_for_update|fn mark_pending_changeset_rolled_back" apps/conary/src/commands/update/package.rs
rg -n "pub\\(super\\) fn print_source_policy_update_preview|fn source_policy_update_context|fn render_replatform_action_preview" apps/conary/src/commands/update/source_policy.rs
rg -n "pub async fn cmd_pin|pub async fn cmd_unpin|pub async fn cmd_list_pinned" apps/conary/src/commands/update/pinning.rs
rg -n "pub async fn cmd_delta_stats" apps/conary/src/commands/update/delta_stats.rs
rg -n "cmd_update_group|cmd_update|cmd_pin|cmd_unpin|cmd_list_pinned|cmd_delta_stats" apps/conary/src/commands/mod.rs apps/conary/src/dispatch.rs apps/conary/src/commands/update
rg -n "update/package.rs|update/source_policy.rs|update/pinning.rs|update/delta_stats.rs|Phase 13" docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/source-selection.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
scripts/line-count-report.sh 30
bash scripts/maintainability-drift-report.sh --base origin/main --limit 30
git diff --check
git status --short --branch
```

Expected:

- `update/mod.rs` is only a routing hub;
- command bodies are in the new owner files;
- docs paths are refreshed;
- docs-audit is complete with `157` files and `corrected 57`;
- line-count report shows `update/mod.rs` far below the pre-Phase 13 2002-line
  hotspot;
- drift report is healthy;
- worktree is clean except any intentional staged/uncommitted commit boundary.

- [ ] **Step 5: Push and verify synced main**

Run:

```bash
git status --short --branch
git log --oneline -6
git pull --ff-only
git push
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected:

- push succeeds;
- `HEAD` and `origin/main` match;
- left/right count is `0	0`;
- only the main worktree is listed unless the user intentionally created extra
  worktrees.

---

## Rollback Plan

This phase is intentionally larger than Phases 10-12, so keep commits small.
If a later task reveals a subtle regression, revert the preceding implementation
commits in reverse order:

1. docs routing commit;
2. package extraction/hub conversion commit;
3. delta stats extraction commit;
4. pinning extraction commit;
5. source-policy extraction commit.

The plan lock-in commit can remain unless the plan itself is being withdrawn.

## Self-Review Checklist

- [ ] No behavior changes are requested.
- [ ] Every public command route remains re-exported through `update/mod.rs`.
- [ ] Every remaining parent helper has a target owner.
- [ ] Existing `selection`, `adopted_authority`, and `collection` modules stay in
      place.
- [ ] Test filters use module-scoped names after parent tests move.
- [ ] Docs-audit counts move from 156/56 to 157/57 when the plan is locked in
      during Task 0 and remain at 157/57 through implementation Tasks 1-5.
- [ ] Final verification includes focused tests, interaction gates, broad conary
      tests, clippy, docs-audit, drift report, push, and synced-main proof.
