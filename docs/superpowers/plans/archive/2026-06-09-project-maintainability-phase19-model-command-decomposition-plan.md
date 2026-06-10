# Phase 19 Model Command Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `apps/conary/src/commands/model.rs` from a 2,260-line hotspot into a focused model command hub plus command-owned child modules, without changing CLI behavior, public command exports, model diff/apply/check semantics, source-policy replatform behavior, lockfile behavior, or test coverage.

**Architecture:** Keep `model.rs` as the stable `commands::model` module and public re-export point for the current model command API. Move shared model loading/diff enrichment, presentation helpers, command bodies, remote include drift logic, lock/update logic, snapshot logic, apply orchestration, and model-only tests into focused child modules under `apps/conary/src/commands/model/`. Preserve the existing `apply.rs` and `publish.rs` module pattern and extend it rather than introducing a different command architecture.

**Tech Stack:** Rust 2024, Tokio, rusqlite-backed Conary DB models, `conary_core::model` diff/replatform APIs, existing CLI dispatch in `apps/conary/src/dispatch.rs`, existing command re-exports in `apps/conary/src/commands/mod.rs`.

## Current Repo Facts To Preserve

- `apps/conary/src/commands/model.rs` is 2,260 lines and is currently the top Rust hotspot after Phase 18.
- Existing model child modules:
  - `apps/conary/src/commands/model/apply.rs` (828 lines)
  - `apps/conary/src/commands/model/publish.rs` (285 lines)
- Existing public command surface re-exported by `apps/conary/src/commands/mod.rs`:

```rust
pub use model::{
    ApplyOptions, cmd_model_apply, cmd_model_check, cmd_model_diff, cmd_model_lock,
    cmd_model_publish, cmd_model_remote_diff, cmd_model_snapshot, cmd_model_update,
};
```

- Existing CLI dispatch calls in `apps/conary/src/dispatch.rs`:
  - `commands::cmd_model_diff(&model, &db.db_path, offline).await`
  - the `ModelApply` arm constructs `commands::ApplyOptions` and calls `commands::cmd_model_apply`
  - `commands::cmd_model_check(&model, &db.db_path, verbose, offline).await`
  - `commands::cmd_model_remote_diff(&model, &db.db_path, refresh).await`
  - `commands::cmd_model_snapshot(&output, &db.db_path, description.as_deref()).await`
  - `commands::cmd_model_lock(&model, output.as_deref(), &db.db_path).await`
  - `commands::cmd_model_update(&model, &db.db_path).await`
  - the `ModelPublish` arm calls `commands::cmd_model_publish`
- Baseline model test inventory:
  - `cargo test -p conary --lib commands::model::tests -- --list` lists exactly 26 tests.
  - `cargo test -p conary --lib commands::model -- --list` lists exactly 26 tests before decomposition.
  - `cargo test -p conary model -- --list` lists 30 unit/integration tests plus the model CLI integration filters:
    - 26 direct model unit tests
    - `cli::tests::{update_dep_mode_help_is_model_derived, update_dep_mode_omission_is_model_derived}`
    - `commands::automation::tests::automation_install_leaves_dependency_mode_model_derived`
    - `commands::install::dependencies::tests::missing_model_uses_preview_convergence_dep_mode`
    - 5 `apps/conary/tests/features.rs` model tests
    - 1 `apps/conary/tests/live_host_mutation_safety.rs` model apply safety test
    - 2 `apps/conary/tests/model_apply.rs` tests
- Baseline docs-audit inventory before locking this plan:
  - `LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l` returns `162`.
  - Ledger counts are `archived 73`, `corrected 62`, `retained-historical 14`, `verified-no-change 13`.
- After locking in this plan file, the docs-audit inventory must be `163` tracked doc-like files and the ledger must have `63` `corrected` rows.

## Desired End State

```text
apps/conary/src/commands/model.rs
apps/conary/src/commands/model/apply.rs
apps/conary/src/commands/model/check.rs
apps/conary/src/commands/model/context.rs
apps/conary/src/commands/model/diff.rs
apps/conary/src/commands/model/lock.rs
apps/conary/src/commands/model/presentation.rs
apps/conary/src/commands/model/publish.rs
apps/conary/src/commands/model/remote_diff.rs
apps/conary/src/commands/model/snapshot.rs
apps/conary/src/commands/model/test_support.rs
```

Final `model.rs` should contain only:

- the path comment and module docs,
- child module declarations,
- `#[cfg(test)] mod test_support;`,
- public re-exports for `ApplyOptions` and all current `cmd_model_*` functions,
- no command bodies,
- no presentation helpers,
- no DB/loading helpers,
- no parent `#[cfg(test)] mod tests`.

Sketch:

```rust
// apps/conary/src/commands/model.rs

//! System Model Commands
//!
//! Command hub for declarative system state management using model files.

mod apply;
mod check;
mod context;
mod diff;
mod lock;
mod presentation;
mod publish;
mod remote_diff;
mod snapshot;
#[cfg(test)]
mod test_support;

pub use apply::{ApplyOptions, cmd_model_apply};
pub use check::cmd_model_check;
pub use diff::cmd_model_diff;
pub use lock::{cmd_model_lock, cmd_model_update};
pub use publish::cmd_model_publish;
pub use remote_diff::cmd_model_remote_diff;
pub use snapshot::cmd_model_snapshot;
```

## Design Choice

Three decomposition paths were considered:

1. **Command-owner split with shared `context` and `presentation` modules.** This is the recommended path. It keeps every CLI command body in an owner module, keeps shared diff enrichment/presentation out of the hub, and keeps public exports stable.
2. **Domain-owner split by source policy, remote includes, lockfiles, and replatforming.** This would produce useful domain modules but would spread command bodies across multiple files and make dispatch-oriented review harder.
3. **Minimal split that moves only tests and large helpers.** This lowers line count but leaves `model.rs` as a command-body hotspot and does not set up the next maintainability phases well.

Use option 1.

## Visibility Contract

- `ApplyOptions` remains public through `crate::commands::ApplyOptions`.
- All current `cmd_model_*` functions remain public through `crate::commands::*` re-exports:
  - `cmd_model_apply`
  - `cmd_model_check`
  - `cmd_model_diff`
  - `cmd_model_lock`
  - `cmd_model_publish`
  - `cmd_model_remote_diff`
  - `cmd_model_snapshot`
  - `cmd_model_update`
- `context::load_model` must be `pub(super)` because `publish.rs`, `remote_diff.rs`, and `lock.rs` load model files.
- `context::load_model_and_diff` must be `pub(super)` because `diff.rs`, `apply.rs`, and `check.rs` share the current load-open-capture-diff path.
- `context::compute_model_diff` must be `pub(super)` because `apply.rs` tests currently exercise replatform execution planning through the lower-level diff path. If those tests are rewritten to use only public command entry points, this can be private, but do not rewrite behavior in this phase.
- `presentation` helpers used by command modules must be `pub(super)`:
  - `is_source_policy_action`
  - `is_replatform_action`
  - `source_policy_summary`
  - `source_policy_replatform_note`
  - `model_check_drift_headline`
  - `render_replatform_summary`
  - `print_source_policy_and_replatform`
- `presentation::render_realignment_proposal_preview` can stay private because only `print_source_policy_and_replatform` calls it. Its tests live in `presentation.rs` and can access private items through the child test module.
- `remote_diff::version_matches_constraint` and `remote_diff::format_version_info` can stay private because only `remote_diff.rs` and its tests need them.
- `lock::collect_lock_data` and `lock::build_lock_from_data` can stay private because both `cmd_model_lock` and `cmd_model_update` live in `lock.rs`.
- `test_support.rs` stays behind `#[cfg(test)] mod test_support;`. Helper functions and fixture types inside it should be `pub(super)` so sibling child test modules can import them through `super::super::test_support`.
- Rust privacy reminder: child test modules can access private items in their own module, but sibling modules cannot access each other's private items. Use explicit `pub(super)` for helpers shared across model child modules.

## Non-Goals

- Do not change CLI command names, flags, exit codes, apply-intent gating, or dispatch behavior.
- Do not change model file parsing, source-policy semantics, replatform planning, install/remove execution, lockfile format, remote include cache behavior, or snapshot output.
- Do not add new model commands.
- Do not rewrite `conary_core::model` behavior in this phase.
- Do not move `cmd_model_publish` out of `publish.rs`; only update its import path for `load_model`.
- Do not change docs-audit counts except by adding and later updating this plan row.
- Do not remove or weaken model apply live-host mutation tests.

## Task 0: Lock In This Plan

**Files:**

- `docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase19-model-command-decomposition-plan.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

**Steps:**

- [ ] Stage this plan file.
- [ ] Add a `corrected` ledger row for this plan file with exactly 9 tab-separated columns.
- [ ] The plan ledger row must use:

```text
origin_path = docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase19-model-command-decomposition-plan.md
path = docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase19-model-command-decomposition-plan.md
family = planning
audience = maintainer
status = verified
disposition = corrected
```

- [ ] Stage the ledger update after adding the row.
- [ ] Regenerate the tracked docs-audit inventory:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] Update `docs/superpowers/documentation-accuracy-audit-summary.md` so the latest maintainability planning note includes Phase 19 and the counts move to `163` tracked files / `63` corrected rows.
- [ ] Stage the inventory and summary updates.
- [ ] Use this evidence source set in the ledger row:

```text
apps/conary/src/commands/model.rs; apps/conary/src/commands/model/apply.rs; apps/conary/src/commands/model/publish.rs; apps/conary/src/commands/mod.rs; apps/conary/src/dispatch.rs; docs/modules/feature-ownership.md; docs/modules/source-selection.md; docs/llms/subsystem-map.md; docs/ARCHITECTURE.md
```

- [ ] Suggested ledger tags:

```text
maintainability; phase19; conary-model; model-commands; hotspot-decomposition
```

- [ ] Suggested ledger note:

```text
Added the Phase 19 model command decomposition plan for turning apps/conary/src/commands/model.rs into a focused command hub while extracting model loading/diff context, presentation helpers, command bodies, remote include drift handling, lock/update logic, snapshot handling, apply orchestration, and model-only tests into child modules without changing CLI behavior.
```

- [ ] Run:

```bash
git diff --check
git diff --cached --check
bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected after staging this plan: inventory `163`; corrected count `63`; no malformed TSV rows.

- [ ] Commit with:

```bash
git commit -m "docs: plan model command decomposition"
git push
git status --short --branch
git rev-parse HEAD origin/main
```

## Task 1: Extract Shared Model Context And Presentation Helpers

**Files:**

- Create `apps/conary/src/commands/model/context.rs`
- Create `apps/conary/src/commands/model/presentation.rs`
- Update `apps/conary/src/commands/model.rs`
- Update `apps/conary/src/commands/model/publish.rs`

**Move from `model.rs` to `context.rs`:**

- `load_model`
- `load_model_and_diff`
- `compute_model_diff`
- `compute_replatform_estimate`

**`context.rs` import surface:**

```rust
// apps/conary/src/commands/model/context.rs

use std::path::Path;

use super::super::open_db;
use anyhow::{Result, anyhow};
use conary_core::db::models::SystemAffinity;
use conary_core::model::parser::SystemModel;
use conary_core::model::{
    DiffAction, ModelDiff, ReplatformEstimate, SystemState, capture_current_state, compute_diff,
    compute_diff_with_includes_offline, parse_model_file, planned_replatform_actions,
    replatform_estimate_from_affinities, source_policy_replatform_snapshot,
};
use rusqlite::Connection;
```

The body of `compute_model_diff` constructs
`VisibleRealignmentCandidates` and `SourcePolicyReplatformSnapshot` using
fully qualified `conary_core::model` paths; no explicit import is required
for those types unless the moved body is also shortened.

**Visibility requirements:**

- `load_model`, `load_model_and_diff`, and `compute_model_diff` must be `pub(super)`.
- `compute_replatform_estimate` can stay private because it is only called by `compute_model_diff` and `context.rs` tests.

**Move from `model.rs` to `presentation.rs`:**

- `is_source_policy_action`
- `is_replatform_action`
- `source_policy_summary`
- `source_policy_replatform_estimate`
- `source_policy_replatform_note`
- `model_check_drift_headline`
- `render_replatform_summary`
- `render_realignment_proposal_preview`
- `print_source_policy_and_replatform`

**`presentation.rs` import surface:**

```rust
// apps/conary/src/commands/model/presentation.rs

use super::super::replatform_rendering::render_replatform_execution_plan;
use anyhow::Result;
use conary_core::model::{
    DiffAction, ModelDiff, ModelDiffSummary, ReplatformEstimate, ReplatformStatus,
    VisibleRealignmentProposal, replatform_execution_plan,
};
use rusqlite::Connection;
```

**Visibility requirements:**

- Make these `pub(super)`: `is_source_policy_action`, `is_replatform_action`, `source_policy_summary`, `source_policy_replatform_note`, `model_check_drift_headline`, `render_replatform_summary`, and `print_source_policy_and_replatform`.
- Keep `source_policy_replatform_estimate` private unless a sibling module needs it directly.
- Keep `render_realignment_proposal_preview` private.

**Path updates:**

- In `model.rs`, add:

```rust
mod context;
mod presentation;

use context::{compute_model_diff, load_model, load_model_and_diff};
use presentation::{
    is_replatform_action, is_source_policy_action, model_check_drift_headline,
    print_source_policy_and_replatform, render_replatform_summary, source_policy_replatform_note,
    source_policy_summary,
};
```

These `use` items are temporary while command bodies remain in the hub.

- In `publish.rs`, replace:

```rust
use super::load_model;
```

with:

```rust
use super::context::load_model;
```

**Move these tests to `context.rs` `#[cfg(test)] mod tests`:**

- `test_source_policy_replatform_estimate_uses_affinity_counts`
- `test_source_policy_replatform_estimate_handles_missing_affinity_data`
- `test_compute_model_diff_surfaces_mixed_replatform_execution_states`
- `test_planned_replatform_actions_promote_proposals_into_actions`

**`context.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::{create_test_db, seed_mixed_replatform_fixture};
    use conary_core::model::{ReplatformBlockedReason, replatform_execution_plan};
}
```

Use fully qualified `conary_core::model::VisibleRealignmentCandidates`, `SourcePolicyReplatformSnapshot`, `VisibleRealignmentProposal`, and `InstalledPackage` in the moved bodies unless you add local imports and use them consistently.

**Move these tests to `presentation.rs` `#[cfg(test)] mod tests`:**

- `test_source_policy_summary_for_policy_only_transition`
- `test_source_policy_summary_for_transition_with_package_changes`
- `test_source_policy_summary_policy_only_stays_conservative`
- `test_source_policy_replatform_note_falls_back_when_affinity_missing`
- `test_model_check_drift_headline_for_pending_estimate`
- `test_model_check_drift_headline_for_policy_only_pending`
- `test_model_check_drift_headline_mentions_visible_candidates_when_estimate_missing`
- `test_model_check_drift_headline_for_package_convergence`
- `test_render_replatform_summary_for_pending_estimate`
- `test_render_replatform_summary_for_visible_candidates`
- `test_render_realignment_proposal_preview_lists_examples`

**`presentation.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::model::{DiffAction, ModelDiff, ModelDiffSummary, ReplatformEstimate};
}
```

Use fully qualified `conary_core::model::VisibleRealignmentCandidates` and `VisibleRealignmentProposal` in the moved bodies unless you add local imports and use them consistently.

**Verification:**

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::model::context::tests -- --list
cargo test -p conary --lib commands::model::presentation::tests -- --list
cargo test -p conary --lib commands::model::context::tests
cargo test -p conary --lib commands::model::presentation::tests
cargo test -p conary --lib commands::model -- --list
```

Expected after Task 1: `context.rs` lists 4 tests, `presentation.rs` lists 11
tests, the remaining parent `commands::model::tests` module lists 11 tests,
and the direct `commands::model` inventory still lists 26 tests.

Commit:

```bash
git add apps/conary/src/commands/model.rs apps/conary/src/commands/model/context.rs apps/conary/src/commands/model/presentation.rs apps/conary/src/commands/model/publish.rs
git commit -m "refactor(conary): extract model context and presentation"
```

## Task 2: Extract Read-Only And Remote Model Command Owners

**Files:**

- Create `apps/conary/src/commands/model/diff.rs`
- Create `apps/conary/src/commands/model/check.rs`
- Create `apps/conary/src/commands/model/snapshot.rs`
- Create `apps/conary/src/commands/model/remote_diff.rs`
- Create `apps/conary/src/commands/model/lock.rs`
- Update `apps/conary/src/commands/model.rs`

**Move from `model.rs` to `diff.rs`:**

- `cmd_model_diff`

**`diff.rs` import surface:**

```rust
// apps/conary/src/commands/model/diff.rs

use std::path::Path;

use super::context::load_model_and_diff;
use super::presentation::{
    is_replatform_action, is_source_policy_action, print_source_policy_and_replatform,
    render_replatform_summary,
};
use anyhow::Result;
use conary_core::model::DiffAction;
```

**Move from `model.rs` to `check.rs`:**

- `cmd_model_check`

**`check.rs` import surface:**

```rust
// apps/conary/src/commands/model/check.rs

use std::path::Path;
use std::process;

use super::context::load_model_and_diff;
use super::presentation::{model_check_drift_headline, source_policy_replatform_note};
use anyhow::Result;
```

Preserve `process::exit(2)` behavior for drift.

**Move from `model.rs` to `snapshot.rs`:**

- `cmd_model_snapshot`

**`snapshot.rs` import surface:**

```rust
// apps/conary/src/commands/model/snapshot.rs

use super::super::open_db;
use anyhow::Result;
use conary_core::model::{capture_current_state, snapshot_to_model};
```

**Move from `model.rs` to `remote_diff.rs`:**

- `cmd_model_remote_diff`
- `version_matches_constraint`
- `format_version_info`

**`remote_diff.rs` import surface:**

```rust
// apps/conary/src/commands/model/remote_diff.rs

use std::path::Path;

use super::context::load_model;
use super::super::open_db;
use anyhow::{Result, anyhow};
use conary_core::db::models::RemoteCollection;
use conary_core::model::remote::fetch_remote_collection;
use conary_core::model::{capture_current_state, parse_trove_spec};
use rusqlite::Connection;
use tracing::debug;
```

Keep `version_matches_constraint` and `format_version_info` private.

**Move from `model.rs` to `lock.rs`:**

- `collect_lock_data`
- `build_lock_from_data`
- `cmd_model_lock`
- `cmd_model_update`

**`lock.rs` import surface:**

```rust
// apps/conary/src/commands/model/lock.rs

use std::path::{Path, PathBuf};

use super::context::load_model;
use super::super::open_db;
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::RemoteCollection;
use conary_core::model::lockfile::ModelLock;
use conary_core::model::parser::SystemModel;
use conary_core::model::remote::CollectionData;
use conary_core::model::{parse_trove_spec, resolve_includes};
use rusqlite::Connection;
```

Shorten fully qualified `conary_core::model::remote::CollectionData`, `conary_core::model::lockfile::ModelLock`, and `conary_core::model::resolve_includes` only where the listed imports are used. Do not change lockfile format or printed output. If `PathBuf` is imported, change the current `std::path::PathBuf::from(out)` call to `PathBuf::from(out)` so the import is not unused.

**Path updates in `model.rs`:**

```rust
mod check;
mod diff;
mod lock;
mod remote_diff;
mod snapshot;

pub use check::cmd_model_check;
pub use diff::cmd_model_diff;
pub use lock::{cmd_model_lock, cmd_model_update};
pub use remote_diff::cmd_model_remote_diff;
pub use snapshot::cmd_model_snapshot;
```

After Task 2, remove only temporary hub imports that are unused by both
production code and the still-parented apply tests. `cmd_model_apply` and its
six tests remain in `model.rs` until Task 3, so keep `load_model_and_diff`, the
presentation helpers used by apply, `replatform_execution_plan`, and
`render_replatform_execution_plan` available to the hub. If the remaining
parent apply tests no longer get these through `use super::*`, add explicit
temporary test imports inside the parent `#[cfg(test)] mod tests`:

```rust
use super::context::compute_model_diff;
use conary_core::model::capture_current_state;
```

After Task 2, these apply-sensitive hub imports must remain while
`cmd_model_apply` and its tests are still parented in `model.rs`:

- `use std::path::Path;`
- `use anyhow::{Result, anyhow};`
- `use conary_core::filesystem::CasStore;`
- `use conary_core::model::{DiffAction, replatform_execution_plan};`
- `use super::replatform_rendering::render_replatform_execution_plan;`
- temporary access to `context::load_model_and_diff`
- temporary access to `apply::{apply_source_policy_changes, apply_replatform_changes, apply_package_changes, apply_derived_packages, apply_metadata_changes}`
- temporary access to `presentation::{is_replatform_action, is_source_policy_action, source_policy_summary, source_policy_replatform_note, print_source_policy_and_replatform, render_replatform_summary}`
- test access to `context::compute_model_diff`, `conary_core::model::capture_current_state`, and `conary_core::model::parser::SystemModel` until the remaining apply tests move in Task 3

Do not add a short `rusqlite::Connection` import solely for the remaining apply
tests; they use `rusqlite::Connection::open` fully qualified.

**Move these tests to `remote_diff.rs` `#[cfg(test)] mod tests`:**

- `test_version_matches_constraint_exact`
- `test_version_matches_constraint_glob`
- `test_version_matches_constraint_prefix`
- `test_remote_diff_detects_missing`

**`remote_diff.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::RemoteCollection;
    use conary_core::model::SystemState;
    use std::collections::{HashMap, HashSet};
}
```

The existing `test_remote_diff_detects_missing` body contains local imports for
`RemoteCollection`, `SystemState`, `HashMap`, and `HashSet` that duplicate these
module-level test imports. Remove those local imports after adding the
module-level imports.

**Move this test to `snapshot.rs` `#[cfg(test)] mod tests`:**

- `test_model_snapshot_writes_effective_source_policy`

**`snapshot.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::DistroPin;
    use tempfile::tempdir;
}
```

**Verification:**

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::model::remote_diff::tests -- --list
cargo test -p conary --lib commands::model::snapshot::tests -- --list
cargo test -p conary --lib commands::model::remote_diff::tests
cargo test -p conary --lib commands::model::snapshot::tests
cargo test -p conary --lib commands::model::tests -- --list
cargo test -p conary --lib commands::model -- --list
```

Expected after Task 2: `remote_diff.rs` lists 4 tests, `snapshot.rs` lists 1
test, the remaining parent `commands::model::tests` module lists 6 apply tests,
and the direct `commands::model` inventory still lists 26 tests.

Commit:

```bash
git add apps/conary/src/commands/model.rs apps/conary/src/commands/model/diff.rs apps/conary/src/commands/model/check.rs apps/conary/src/commands/model/snapshot.rs apps/conary/src/commands/model/remote_diff.rs apps/conary/src/commands/model/lock.rs
git commit -m "refactor(conary): extract model command owners"
```

## Task 3: Move Model Apply Orchestration Into `apply.rs`

**Files:**

- Update `apps/conary/src/commands/model/apply.rs`
- Update `apps/conary/src/commands/model.rs`
- Create `apps/conary/src/commands/model/test_support.rs`

**Move from `model.rs` to `apply.rs`:**

- `cmd_model_apply`

`ApplyOptions` already lives in `apply.rs`; moving `cmd_model_apply` into the same module makes the public apply entry point and its helper orchestration co-owned.

**Update `apply.rs` import surface:**

Add these imports to the existing `apply.rs` imports:

```rust
use super::context::load_model_and_diff;
use super::presentation::{
    is_replatform_action, is_source_policy_action, print_source_policy_and_replatform,
    render_replatform_summary, source_policy_replatform_note, source_policy_summary,
};
use crate::commands::replatform_rendering::render_replatform_execution_plan;
```

`apply.rs` already imports `std::path::Path`, `anyhow::{Context, Result, anyhow}`, `conary_core::filesystem::CasStore`, `conary_core::model::{DiffAction, ModelDerivedPackage, replatform_execution_plan}`, `crate::commands::{InstallOptions, LegacyReplayOptions, SandboxMode, cmd_install, cmd_remove}`, and the DB/derived types needed by the moved body. Preserve those imports and prune any unused items after `cargo check`.

**Visibility cleanup inside `apply.rs`:**

- Keep `cmd_model_apply` `pub`.
- `apply_source_policy_changes`, `apply_replatform_changes`, `apply_package_changes`, `apply_derived_packages`, and `apply_metadata_changes` can become private once all apply tests live in `apply.rs`. If a sibling module still calls one after the move, keep only that function `pub(super)`.
- Preserve `#[cfg(test)] pub(super) fn set_replatform_metadata_failpoint_for_test(enabled: bool)`.
- Preserve `#[cfg(test)]` failpoint behavior exactly.

**Create `test_support.rs` and move these helper items from the parent test module:**

- `build_test_ccs_package`
- `build_test_ccs_package_with_bundle`
- `legacy_replatform_upgrade_bundle`
- `legacy_replatform_upgrade_entry`
- `serve_test_file`
- `ReplatformMetadataFailpointReset`
- `impl Drop for ReplatformMetadataFailpointReset`

**`test_support.rs` import surface:**

```rust
// apps/conary/src/commands/model/test_support.rs

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use conary_core::ccs::legacy_scriptlets::{
    DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
    LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
    PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
    TransactionOrder, VersionScheme,
};
```

Helper visibility:

```rust
pub(super) fn build_test_ccs_package(dir: &Path, name: &str, version: &str) -> PathBuf
pub(super) fn build_test_ccs_package_with_bundle(
    dir: &Path,
    name: &str,
    version: &str,
    legacy_scriptlets: Option<LegacyScriptletBundle>,
) -> PathBuf
pub(super) fn legacy_replatform_upgrade_bundle(
    package: &str,
    version: &str,
) -> LegacyScriptletBundle
pub(super) fn serve_test_file(file_path: PathBuf) -> (String, std::thread::JoinHandle<()>)
pub(super) struct ReplatformMetadataFailpointReset;
```

`legacy_replatform_upgrade_entry` can remain private because only `legacy_replatform_upgrade_bundle` calls it.

`ReplatformMetadataFailpointReset::drop` should call:

```rust
super::apply::set_replatform_metadata_failpoint_for_test(false);
```

**Path updates in `model.rs`:**

```rust
#[cfg(test)]
mod test_support;

pub use apply::{ApplyOptions, cmd_model_apply};
```

Remove `cmd_model_apply` from the hub.

**Move these tests to `apply.rs` `#[cfg(test)] mod tests`:**

- `test_model_apply_updates_source_policy_without_package_changes`
- `test_model_apply_updates_selection_mode_without_package_changes`
- `test_model_apply_updates_allowed_distros_without_package_changes`
- `test_model_apply_executes_replatform_replacement_when_route_is_executable`
- `test_model_apply_replatform_legacy_replay_failure_names_safe_choices`
- `test_model_apply_rolls_back_or_reports_partial_failure_during_replatform`

**`apply.rs` test imports:**

Task 3 creates the first `#[cfg(test)] mod tests` in `apply.rs`; the existing
file currently only has cfg-test failpoint support.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::context::compute_model_diff;
    use super::super::test_support::{
        ReplatformMetadataFailpointReset, build_test_ccs_package,
        build_test_ccs_package_with_bundle, legacy_replatform_upgrade_bundle, serve_test_file,
    };
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::DistroPin;
    use conary_core::model::parser::SystemModel;
    use conary_core::model::capture_current_state;
    use conary_core::repository::{SETTINGS_KEY_ALLOWED_DISTROS, SETTINGS_KEY_SELECTION_MODE};
    use conary_core::db::models::settings;
    use tempfile::tempdir;
}
```

The three replatform tests contain local `conary_core::db::models` imports for repository/package fixtures such as `InstallSource`, `LabelEntry`, `PackageResolution`, `PrimaryStrategy`, `Repository`, `RepositoryPackage`, `ResolutionStrategy`, `Trove`, and `TroveType` (plus `DistroPin` in the executable-route test). Preserve those local imports in the moved test bodies to avoid a giant module-level import surface.
The test `test_model_apply_replatform_legacy_replay_failure_names_safe_choices`
uses `toml::from_str`, which resolves through the dependency crate path without
an explicit `use toml;` import.

**Verification:**

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::model::apply::tests -- --list
cargo test -p conary --lib commands::model::apply::tests
cargo test -p conary --lib commands::model::tests -- --list
cargo test -p conary --lib commands::model -- --list
```

Expected after Task 3: `apply.rs` lists 6 tests, the parent
`commands::model::tests` module lists 0 tests or no longer exists, and the
direct `commands::model` inventory still lists 26 tests.

Commit:

```bash
git add apps/conary/src/commands/model.rs apps/conary/src/commands/model/apply.rs apps/conary/src/commands/model/test_support.rs
git commit -m "refactor(conary): move model apply orchestration"
```

## Task 4: Clean The Model Hub And Validate Test Distribution

**Files:**

- `apps/conary/src/commands/model.rs`
- all `apps/conary/src/commands/model/*.rs`

**Steps:**

- [ ] Delete the now-empty parent `#[cfg(test)] mod tests` from `model.rs`.
- [ ] Delete unused imports from `model.rs`.
- [ ] Confirm `model.rs` only contains module declarations and public re-exports.
- [ ] Confirm every touched Rust file starts with a path comment, including `model.rs`, `apply.rs`, `publish.rs`, and all new child modules.
- [ ] Confirm no production child model module uses broad `use super::*;`; test-module `use super::*;` imports are allowed when they stay inside `#[cfg(test)] mod tests`.
- [ ] Confirm `publish.rs` imports `load_model` from `super::context::load_model`.
- [ ] Confirm `apps/conary/src/commands/mod.rs` re-exports still compile without changes.
- [ ] Confirm `apps/conary/src/dispatch.rs` still calls the same `commands::cmd_model_*` paths.

**Suggested boundary checks:**

```bash
rg -n "^\s*use super::\*;" apps/conary/src/commands/model.rs apps/conary/src/commands/model
rg -n "^(pub |pub\(|fn |async fn|struct |enum |type |const |impl |#\[cfg\(test\)\])" apps/conary/src/commands/model.rs apps/conary/src/commands/model/*.rs
rg -n "^\s*(pub\s+)?(async\s+)?fn " apps/conary/src/commands/model.rs
cargo test -p conary --lib commands::model -- --list
cargo test -p conary --lib commands::model::tests -- --list
```

Expected:

- `commands::model -- --list` still lists 26 direct model tests.
- `commands::model::tests -- --list` should list 0 tests because the parent model test module is gone.
- Child owner modules should contain the 26 direct tests.
- The `use super::*` check should have no production-module hits; hits inside
  child test modules are acceptable.
- The `model.rs` function-body check should have no output.

**Verification:**

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::model
cargo test -p conary model -- --list
```

Commit:

```bash
git add apps/conary/src/commands/model.rs apps/conary/src/commands/model/*.rs
git commit -m "refactor(conary): slim model command hub"
```

## Task 5: Update Documentation Routing

**Files:**

- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

**Docs updates:**

- In `docs/modules/feature-ownership.md`, add a dedicated ownership card named `Declarative System Models And Replatform Planning` with:

```markdown
## Declarative System Models And Replatform Planning

**Capability:** diff, apply, check, snapshot, publish, lock, update, and remote-diff declarative system model files while preserving source-policy and replatform convergence behavior.

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

**Neighbor systems:** install/remove execution, update source-policy selection, repository remote include cache, derived package builds, live-host mutation acknowledgement, and conaryd package-job request compatibility.

**Focused proof:** `cargo test -p conary --lib commands::model`.

**Interaction gate:** `cargo test -p conary model`; `cargo test -p conary --test model_apply`; `cargo test -p conary --test live_host_mutation_safety model` when apply behavior or live-mutation safety changes.

**Docs to update:** `docs/modules/source-selection.md`; `docs/llms/subsystem-map.md`; `docs/ARCHITECTURE.md`.

**Safety notes:** preserve `model check` drift exit code 2, source-policy persistence semantics, executable replatform planning boundaries, lockfile reproducibility, remote include cache behavior, and refusal-before-live-mutation gates.
```

- In `docs/modules/source-selection.md`, update "Where To Read Next" so the model command owner paths include:

```markdown
- `apps/conary/src/commands/model.rs` for the model command hub
- `apps/conary/src/commands/model/context.rs` for model loading and diff enrichment
- `apps/conary/src/commands/model/presentation.rs` for source-policy and replatform summaries
- `apps/conary/src/commands/model/apply.rs` for model apply execution and replatform install dispatch
- `apps/conary/src/commands/model/remote_diff.rs` and `apps/conary/src/commands/model/lock.rs` for remote include drift and lockfile behavior
```

- In `docs/llms/subsystem-map.md`, update the source selection / runtime policy / replatform convergence entry to replace the single `apps/conary/src/commands/model.rs` pointer with:

```markdown
`apps/conary/src/commands/model.rs`,
`apps/conary/src/commands/model/context.rs`,
`apps/conary/src/commands/model/presentation.rs`,
`apps/conary/src/commands/model/apply.rs`,
`apps/conary/src/commands/model/remote_diff.rs`,
`apps/conary/src/commands/model/lock.rs`
```

- In `docs/ARCHITECTURE.md`, update the `apps/conary/` CLI tree so the `commands/` entry mentions focused model command child modules rather than implying model command implementation is a single flat file.
- Update YAML frontmatter `last_updated`, `revision`, and `summary` in every changed frontmatter doc.
- Update existing ledger rows for changed docs and update the Phase 19 plan row evidence after the new split files exist. Do not add new rows for existing docs.
- Sweep active docs and audit files for stale `Phase 18`, `162`, `corrected 62`, and single-file `apps/conary/src/commands/model.rs` ownership wording introduced or made stale by this phase.
- `docs/modules/test-fixtures.md` is intentionally unchanged unless Phase 19 introduces a reusable fixture family rather than unit-test-local helpers.

**Verification:**

```bash
git diff --check
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected inventory remains `163` and corrected count remains `63` after Task 0 lock-in. Implementation tasks should not add new doc-like files.

Commit:

```bash
git add docs/modules/feature-ownership.md docs/modules/source-selection.md docs/llms/subsystem-map.md docs/ARCHITECTURE.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: update model command ownership"
```

## Task 6: Final Verification

Run all final gates from a clean working tree except for intentional staged changes.

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::model -- --list
cargo test -p conary --lib commands::model::tests -- --list
cargo test -p conary --lib commands::model
cargo test -p conary model -- --list
cargo test -p conary model
cargo test -p conary --test features model
cargo test -p conary --test model_apply
cargo test -p conary --test live_host_mutation_safety model
cargo test -p conary
cargo clippy -p conary --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/maintainability-drift-report.sh
scripts/line-count-report.sh 30
git diff --check
git status --short --branch
```

Expected outcomes:

- Formatting passes.
- `cargo check -p conary` passes.
- Direct model test inventory remains 26 tests.
- Parent `commands::model::tests` inventory is 0 tests after cleanup.
- `cargo test -p conary model` still lists and runs the broader model-filtered unit and integration tests.
- `cargo test -p conary` passes.
- Conary clippy and workspace clippy pass with `-D warnings`.
- Docs-audit inventory is `163`.
- Docs-audit corrected rows are `63`.
- `model.rs` is no longer a large hotspot; model command logic is distributed across focused child modules.

If workspace-wide clippy finds unrelated pre-existing warnings, stop and record the exact output before deciding whether to fix or report the unrelated blocker.

## Test Mapping Checklist

Move each current parent model test exactly once:

**context.rs**

- [ ] `test_source_policy_replatform_estimate_uses_affinity_counts`
- [ ] `test_source_policy_replatform_estimate_handles_missing_affinity_data`
- [ ] `test_compute_model_diff_surfaces_mixed_replatform_execution_states`
- [ ] `test_planned_replatform_actions_promote_proposals_into_actions`

**presentation.rs**

- [ ] `test_source_policy_summary_for_policy_only_transition`
- [ ] `test_source_policy_summary_for_transition_with_package_changes`
- [ ] `test_source_policy_summary_policy_only_stays_conservative`
- [ ] `test_source_policy_replatform_note_falls_back_when_affinity_missing`
- [ ] `test_model_check_drift_headline_for_pending_estimate`
- [ ] `test_model_check_drift_headline_for_policy_only_pending`
- [ ] `test_model_check_drift_headline_mentions_visible_candidates_when_estimate_missing`
- [ ] `test_model_check_drift_headline_for_package_convergence`
- [ ] `test_render_replatform_summary_for_pending_estimate`
- [ ] `test_render_replatform_summary_for_visible_candidates`
- [ ] `test_render_realignment_proposal_preview_lists_examples`

**remote_diff.rs**

- [ ] `test_version_matches_constraint_exact`
- [ ] `test_version_matches_constraint_glob`
- [ ] `test_version_matches_constraint_prefix`
- [ ] `test_remote_diff_detects_missing`

**snapshot.rs**

- [ ] `test_model_snapshot_writes_effective_source_policy`

**apply.rs**

- [ ] `test_model_apply_updates_source_policy_without_package_changes`
- [ ] `test_model_apply_updates_selection_mode_without_package_changes`
- [ ] `test_model_apply_updates_allowed_distros_without_package_changes`
- [ ] `test_model_apply_executes_replatform_replacement_when_route_is_executable`
- [ ] `test_model_apply_replatform_legacy_replay_failure_names_safe_choices`
- [ ] `test_model_apply_rolls_back_or_reports_partial_failure_during_replatform`

Total: 26 tests.

## Review Prompts

Use this prompt for Gemini/DeepSeek/local agentic review before lock-in:

```text
You are reviewing a repository-grounded Rust maintainability plan for Conary.

Repo: /home/peter/Conary
Plan file: docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase19-model-command-decomposition-plan.md
Target file: apps/conary/src/commands/model.rs

Please perform a critical review against the actual filesystem and code. Do not assume the plan is correct. Check Rust module resolution, visibility, import surfaces, test relocation, command dispatch stability, public command re-exports, docs-audit math, and verification gates.

Important context:
- model.rs is currently 2,260 lines and the top Rust hotspot after Phase 18.
- Existing child modules are apps/conary/src/commands/model/{apply,publish}.rs.
- apps/conary/src/commands/mod.rs re-exports ApplyOptions and all cmd_model_* functions from model.
- apps/conary/src/dispatch.rs calls those commands through commands::cmd_model_* paths.
- Baseline direct model tests: cargo test -p conary --lib commands::model::tests -- --list shows 26 tests.
- Baseline docs-audit count is 162 tracked files / 62 corrected rows before locking the plan; after adding the plan row it should become 163 / 63.

Please return:
1. Summary verdict: Ready, Ready with fixes, or Not ready.
2. Critical findings: compile failures, behavior regressions, broken public command exports, dispatch regressions, model apply/check semantic regressions, or docs-audit failures.
3. Important findings: likely clippy/test/import/visibility issues or sequencing hazards.
4. Minor findings: clarity or polish.
5. Missing concerns the plan should cover.
6. Suggested exact edits to the plan.
7. Verification commands you ran and results.
8. Claims verified against code.
9. Claims not verified and why.

Focus especially on:
- Whether moving load_model/load_model_and_diff/compute_model_diff into context.rs keeps publish.rs, diff.rs, apply.rs, check.rs, remote_diff.rs, and lock.rs compiling.
- Whether presentation helper visibility is sufficient for diff/apply/check command modules.
- Whether moving cmd_model_apply into apply.rs has the right imports and keeps ApplyOptions re-exported.
- Whether cmd_model_lock and cmd_model_update can share private lock helpers in lock.rs.
- Whether all 26 parent tests are assigned exactly once and receive the right imports after moving.
- Whether model::test_support can be accessed from apply tests without duplicating fixtures.
- Whether dispatch.rs and commands/mod.rs public paths remain unchanged.
```

## Self-Review Checklist

- [ ] `model.rs` stays a file module and keeps submodules under `model/`; do not rename it to `model/mod.rs`.
- [ ] `pub use apply::{ApplyOptions, cmd_model_apply};` preserves the public apply API.
- [ ] All current `cmd_model_*` functions remain reachable through `crate::commands::*`.
- [ ] `dispatch.rs` needs no behavior changes.
- [ ] `publish.rs` imports `load_model` from `context.rs`.
- [ ] `model check` still exits with code 2 for drift.
- [ ] Source-policy summaries, replatform estimates, and replatform execution-plan previews are unchanged.
- [ ] Lock/update commands preserve lockfile paths, hashes, and output wording.
- [ ] Remote diff preserves refresh/purge, fetch, missing, and version-drift behavior.
- [ ] No production child module keeps broad `use super::*;`.
- [ ] All 26 direct model tests move exactly once.
- [ ] No parent `commands::model::tests::*` tests remain after cleanup.
- [ ] Docs-audit counts move from 162/62 to 163/63 when the plan is committed and remain 163/63 through implementation.
