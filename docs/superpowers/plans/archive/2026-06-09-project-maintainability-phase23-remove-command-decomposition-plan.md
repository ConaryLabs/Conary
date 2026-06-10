# Phase 23 Remove Command Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `apps/conary/src/commands/remove.rs`, the current largest Rust hotspot, into focused remove-command child modules while preserving CLI behavior, state-restore integration, scriptlet replay safety, and public command exports.

**Architecture:** Keep the existing Rust file-module layout: `apps/conary/src/commands/remove.rs` remains the command hub and new sibling implementation files live under `apps/conary/src/commands/remove/`. Move shared remove types, execution-path selection, legacy replay planning/execution, native scriptlet handling, transactional DB removal, command orchestration, autoremove, and test fixtures into focused children with explicit imports and crate-local re-exports.

**Tech Stack:** Rust 2024, Cargo workspace, `apps/conary`, `conary-core`, `rusqlite`, package scriptlet execution, legacy replay audit metadata, live-root mutation journal, generation publication, docs-audit ledger tooling.

---

## Current Repository Facts

- Repository root: `/home/peter/Conary`.
- Current `HEAD` and `origin/main`: `c237cfd20e00d3dab700b10b5da144e6e4f721a4`.
- Current top hotspots after Phase 22:
  - `apps/conary/src/commands/remove.rs`: 1,990 lines.
  - `apps/conary/src/commands/bootstrap/mod.rs`: 1,946 lines.
  - `crates/conary-core/src/model/replatform.rs`: 1,927 lines.
  - `crates/conary-core/src/resolver/provider/mod.rs`: 1,881 lines.
  - `apps/conary-test/src/engine/runner.rs`: 1,875 lines.
- `apps/conary/src/commands/remove.rs` currently has no child module directory.
- Current public command exports:
  - `apps/conary/src/commands/mod.rs` has `pub use remove::{cmd_autoremove, cmd_remove};`.
  - `apps/conary/src/dispatch/root.rs` dispatches CLI `remove` to `commands::cmd_remove`.
  - `apps/conary/src/dispatch/root.rs` dispatches CLI `autoremove` to `commands::cmd_autoremove`.
- Current crate-local remove integration:
  - `apps/conary/src/commands/state.rs` imports `super::remove::{RemoveScriptletOptions, remove_inner};`.
  - State restore uses `remove_inner(...)` and reads `remove_result.snapshot`.
- Other callers verified by `rg`:
  - `apps/conary/src/commands/automation.rs` calls `super::cmd_remove`.
  - `apps/conary/src/commands/model/apply.rs` imports/calls `cmd_remove` and calls `crate::commands::cmd_autoremove`.
  - `apps/conary/src/command_risk.rs` owns the CLI risk policy for `Commands::Autoremove`.
- Baseline focused tests:
  - `cargo test -p conary --lib commands::remove -- --list` lists exactly 10 tests.
  - `cargo test -p conary --lib commands::remove` passes: 10 passed, 0 failed.
  - `cargo test -p conary --lib commands::state -- --list` lists 7 state tests that must continue compiling against `remove_inner` and `RemoveScriptletOptions`.
- Baseline docs-audit state:
  - Inventory: 166 tracked files.
  - Ledger counts: `archived 73`, `corrected 67`, `retained-historical 14`, `verified-no-change 12`.
  - `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete` passes.

## Non-Goals

- Do not change `conary remove` or `conary autoremove` CLI behavior, output, risk labels, live-host mutation acknowledgement, or dispatch routes.
- Do not change legacy replay policy decisions, target compatibility handling, audit metadata shape, or scriptlet warning metadata shape.
- Do not change generation-aware removal, mutable live-root removal, live-root journal recovery, or generation publication behavior.
- Do not move remove logic into `conary-core`.
- Do not add new DB schema migrations.
- Do not add `apps/conary/src/commands/remove/mod.rs`; that would conflict with the existing file-module layout.

## Public And Crate-Local Contract To Preserve

These public command paths must remain usable:

```rust
crate::commands::cmd_remove
crate::commands::cmd_autoremove
crate::commands::remove::cmd_remove
crate::commands::remove::cmd_autoremove
```

These crate-local restore paths must remain usable:

```rust
crate::commands::remove::RemoveScriptletOptions
crate::commands::remove::remove_inner
```

`remove_inner` must continue returning a `RemoveInnerResult` whose `snapshot` field is readable from `commands::state`.

## Final File Responsibility Map

| File | Responsibility |
| --- | --- |
| `apps/conary/src/commands/remove.rs` | Hub module only: path comment, module declarations, public/crate-local re-exports, high-level docs. |
| `apps/conary/src/commands/remove/types.rs` | Shared remove result/options/context structs used by sibling modules and state restore. |
| `apps/conary/src/commands/remove/execution_path.rs` | Generation-aware vs mutable-live-root removal path detection. |
| `apps/conary/src/commands/remove/legacy_replay.rs` | Installed legacy bundle loading, remove replay planning, replay entry execution, replay success checks, and audit construction. |
| `apps/conary/src/commands/remove/scriptlets.rs` | Post-remove native scriptlet handling, legacy post-remove warning conversion, and warning metadata append calls. |
| `apps/conary/src/commands/remove/transaction.rs` | `remove_inner`, remove preparation, pre-remove native scriptlet handling, dependency recheck, file history writes, and trove deletion. |
| `apps/conary/src/commands/remove/autoremove.rs` | `cmd_autoremove`, fixed-point orphan-removal loop, orphan skip planning, round preflight, output helpers, and autoremove tests. |
| `apps/conary/src/commands/remove/command.rs` | `cmd_remove` orchestration, live-root branch, generation-aware branch, summary output, and command-level tests. |
| `apps/conary/src/commands/remove/test_support.rs` | Test-only fixtures, direct live-root removal helper, and direct helper tests. |

## Visibility Contract

- `types::RemoveInnerResult` remains `pub(crate)`.
- `RemoveInnerResult::snapshot` remains `pub(crate)` because `commands::state` reads it after calling `remove_inner`.
- `RemoveInnerResult` fields used only by remove siblings should be `pub(super)`:
  - `changeset_id`
  - `trove`
  - `stored_scriptlets`
  - `scriptlet_format`
  - `removed_count`
  - `dirs_removed`
  - `planned_pre_remove`
  - `legacy_bundle`
  - `legacy_pre_outcomes`
  - `legacy_audit_context`
  - `planned_post_remove`
- Preserve the existing `#[allow(dead_code)]` attribute on `RemoveInnerResult::planned_pre_remove`.
- `types::RemoveScriptletOptions` remains `pub(crate)`.
- `RemoveScriptletOptions::new(...)` remains `pub(crate)`.
- `RemoveScriptletOptions` fields should be `pub(super)` so `transaction.rs`, `legacy_replay.rs`, and `autoremove.rs` can read them without getters.
- `types::LegacyRemoveReplayAuditContext` should be `pub(super)` and its fields should be `pub(super)`.
- `execution_path::RemoveExecutionPath` and `execution_path::remove_execution_path` should be `pub(super)`.
- `legacy_replay::PreparedLegacyRemoveReplay` should be `pub(super)` because `transaction.rs` receives it from `load_installed_legacy_remove_plan`.
- `PreparedLegacyRemoveReplay` fields should be `pub(super)` because `transaction.rs` reads `bundle`, `planned_pre_remove`, `planned_post_remove`, and `audit_context`.
- `legacy_replay::load_installed_legacy_remove_plan` should be `pub(super)` because `transaction.rs` and `autoremove.rs` use it.
- `legacy_replay::execute_legacy_remove_replay_plan_entries`, `legacy_replay::require_legacy_replay_success`, and `legacy_replay::build_legacy_replay_audit_for_remove` should be `pub(super)`.
- `scriptlets::run_post_remove_scriptlet` should be `pub(super)`.
- `transaction::remove_inner` should be `pub(crate)` and re-exported from `remove.rs`.
- `transaction::prepare_remove` and `transaction::commit_remove_db` should be `pub(super)` because `command.rs` uses them for the mutable live-root branch.
- `autoremove::cmd_autoremove` and `command::cmd_remove` should be `pub`.
- `test_support.rs` should be declared as `#[cfg(test)] pub(super) mod test_support;`.

## Test Migration Map

Move each existing `commands::remove::tests::*` test exactly once:

| Test | New owner |
| --- | --- |
| `autoremove_plan_classifies_authority_and_safety_skips` | `remove/autoremove.rs` |
| `autoremove_refuses_legacy_candidate_before_removing_any_package` | `remove/autoremove.rs` |
| `autoremove_with_legacy_replay_flag_removes_all_candidates` | `remove/autoremove.rs` |
| `commit_remove_db_carries_planned_post_remove_after_trove_delete` | `remove/transaction.rs` |
| `direct_live_root_removal_deletes_files_symlinks_and_empty_dirs` | `remove/test_support.rs` |
| `direct_live_root_removal_ignores_already_missing_paths` | `remove/test_support.rs` |
| `no_generation_remove_deletes_files_and_db_rows` | `remove/command.rs` |
| `no_generation_remove_fails_closed_on_dangling_current_without_mutation` | `remove/command.rs` |
| `no_generation_remove_live_root_failure_leaves_no_pending_changeset` | `remove/command.rs` |
| `remove_refuses_critical_package_before_file_mutation` | `remove/command.rs` |

---

### Task 0: Lock In The Plan Packet

**Files:**
- Create: `docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase23-remove-command-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

- [ ] **Step 1: Stage the new plan before inventory checks**

Run:

```bash
git status --short --branch
git add docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase23-remove-command-decomposition-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected after staging the plan:

```text
docs-audit inventory includes 167 tracked files
```

- [ ] **Step 2: Add the ledger row**

Append a row for the plan file:

```text
docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase23-remove-command-decomposition-plan.md	docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase23-remove-command-decomposition-plan.md	planning	maintainer	maintainability; phase23; conary-remove; autoremove; hotspot-decomposition	apps/conary/src/commands/remove.rs; apps/conary/src/commands/remove/; apps/conary/src/commands/state.rs; apps/conary/src/commands/model/apply.rs; apps/conary/src/dispatch/root.rs; apps/conary/src/commands/mod.rs; crates/conary-core/src/ccs/legacy_replay.rs; crates/conary-core/src/scriptlet/mod.rs; docs/llms/subsystem-map.md; docs/modules/feature-ownership.md; docs/ARCHITECTURE.md	verified	corrected	Added the Phase 23 remove command decomposition plan for turning apps/conary/src/commands/remove.rs into a focused command hub plus child modules for remove command orchestration, autoremove, transactional DB removal, execution-path detection, legacy replay, scriptlet warning handling, and test fixtures without changing CLI behavior or state-restore integration.
```

- [ ] **Step 3: Refresh the docs-audit summary**

Update `docs/superpowers/documentation-accuracy-audit-summary.md` so it reports:

```text
Total tracked doc-like files audited: 167
corrected: 68
archived: 73
retained-historical: 14
verified-no-change: 12
```

- [ ] **Step 4: Verify docs-audit consistency**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
git diff --check
```

Expected:

```text
167
archived 73
corrected 68
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
No diff from the regenerated docs-audit inventory.
check-doc-truth passes.
```

- [ ] **Step 5: Commit the lock-in**

Run:

```bash
git add docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase23-remove-command-decomposition-plan.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: plan remove command decomposition"
git push
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected: clean synced `main`, `0	0` ahead/behind, and one worktree rooted at `/home/peter/Conary`.

---

### Task 1: Extract Shared Types, Execution Path Detection, And Test Support

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Create: `apps/conary/src/commands/remove/types.rs`
- Create: `apps/conary/src/commands/remove/execution_path.rs`
- Create: `apps/conary/src/commands/remove/test_support.rs`

- [ ] **Step 1: Create the child module directory**

Run:

```bash
mkdir -p apps/conary/src/commands/remove
```

- [ ] **Step 2: Add hub module declarations and temporary imports**

At the top of `apps/conary/src/commands/remove.rs`, keep the existing path comment and module doc, then add:

```rust
mod execution_path;
#[cfg(test)]
pub(super) mod test_support;
mod types;

use execution_path::{RemoveExecutionPath, remove_execution_path};
use types::{LegacyRemoveReplayAuditContext, RemoveInnerResult};
```

Also add the stable re-export that `commands::state` already depends on:

```rust
pub(crate) use types::RemoveScriptletOptions;
```

Do not add a separate private `use types::RemoveScriptletOptions;`; use the re-export for local references too.

- [ ] **Step 3: Create `types.rs`**

Move these items from `remove.rs` into `types.rs`:

```rust
RemoveInnerResult
LegacyRemoveReplayAuditContext
RemoveScriptletOptions
impl RemoveScriptletOptions
```

Use this import surface:

```rust
// apps/conary/src/commands/remove/types.rs

use conary_core::ccs::legacy_replay::{HostForeignReplayPolicy, LegacyReplayPlan};
use conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle;
use conary_core::db::models::{ScriptletEntry, Trove};
use conary_core::scriptlet::{PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletOutcome};

use crate::commands::{LegacyReplayCompatibilityAudit, LegacyReplayOptions, TroveSnapshot};
```

Required shape:

```rust
pub(crate) struct RemoveInnerResult {
    pub(super) changeset_id: i64,
    pub(crate) snapshot: TroveSnapshot,
    pub(super) trove: Trove,
    pub(super) stored_scriptlets: Vec<ScriptletEntry>,
    pub(super) scriptlet_format: ScriptletPackageFormat,
    pub(super) removed_count: usize,
    pub(super) dirs_removed: usize,
    #[allow(dead_code)]
    pub(super) planned_pre_remove: Option<LegacyReplayPlan>,
    pub(super) legacy_bundle: Option<LegacyScriptletBundle>,
    pub(super) legacy_pre_outcomes: Vec<ScriptletOutcome>,
    pub(super) legacy_audit_context: Option<LegacyRemoveReplayAuditContext>,
    pub(super) planned_post_remove: Option<LegacyReplayPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LegacyRemoveReplayAuditContext {
    pub(super) target_id: String,
    pub(super) source_target_id: String,
    pub(super) target_compatibility: String,
    pub(super) foreign_replay_policy: String,
    pub(super) host_policy: HostForeignReplayPolicy,
    pub(super) feature_gate_enabled: bool,
    pub(super) foreign_override: bool,
    pub(super) evidence_digest: Option<String>,
    pub(super) compatibility: LegacyReplayCompatibilityAudit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoveScriptletOptions {
    pub(super) no_scripts: bool,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) legacy_replay: LegacyReplayOptions,
}

impl RemoveScriptletOptions {
    pub(crate) fn new(
        no_scripts: bool,
        sandbox_mode: SandboxMode,
        legacy_replay: LegacyReplayOptions,
    ) -> Self {
        Self {
            no_scripts,
            sandbox_mode,
            legacy_replay,
        }
    }
}
```

- [ ] **Step 4: Create `execution_path.rs`**

Move `RemoveExecutionPath` and `remove_execution_path` from `remove.rs` into `execution_path.rs`.

Use this import surface:

```rust
// apps/conary/src/commands/remove/execution_path.rs

use std::path::PathBuf;

use anyhow::{Context, Result};
```

Required visibility:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoveExecutionPath {
    GenerationAware,
    MutableLiveRoot,
}

pub(super) fn remove_execution_path(db_path: &str) -> Result<RemoveExecutionPath> {
    // moved body unchanged
}
```

- [ ] **Step 5: Create `test_support.rs`**

Move these test-only helpers from `remove.rs` into `test_support.rs`:

```rust
DirectRemovalStats
file_snapshot
remove_snapshot
snapshot_path_under_root
snapshot_entry_is_dir
remove_files_from_live_root
```

Move these two tests into `test_support.rs`:

```text
direct_live_root_removal_deletes_files_symlinks_and_empty_dirs
direct_live_root_removal_ignores_already_missing_paths
```

Use this import surface:

```rust
// apps/conary/src/commands/remove/test_support.rs

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;
use tracing::warn;

use crate::commands::{FileSnapshot, TroveSnapshot};
```

Make shared test helpers `pub(super)` so sibling test modules can import them:

```rust
pub(super) fn file_snapshot(path: &str, permissions: i32) -> FileSnapshot
pub(super) fn remove_snapshot(files: Vec<FileSnapshot>) -> TroveSnapshot
```

`remove_files_from_live_root` can remain private if the direct tests stay in `test_support.rs`.

- [ ] **Step 6: Update still-parented tests**

Until later tasks move tests out of the parent, the remaining parent `#[cfg(test)] mod tests` needs:

```rust
use super::test_support::remove_snapshot;
```

Remove direct definitions of `file_snapshot`, `remove_snapshot`, `snapshot_path_under_root`, `snapshot_entry_is_dir`, and `remove_files_from_live_root` from the parent.

- [ ] **Step 7: Verify Task 1**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::remove -- --list
cargo test -p conary --lib commands::remove
```

Expected:

```text
10 tests listed
10 passed
```

- [ ] **Step 8: Commit Task 1**

```bash
git add apps/conary/src/commands/remove.rs \
  apps/conary/src/commands/remove/types.rs \
  apps/conary/src/commands/remove/execution_path.rs \
  apps/conary/src/commands/remove/test_support.rs
git commit -m "refactor(conary): extract remove shared primitives"
```

---

### Task 2: Extract Legacy Replay And Post-Remove Scriptlet Handling

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Create: `apps/conary/src/commands/remove/legacy_replay.rs`
- Create: `apps/conary/src/commands/remove/scriptlets.rs`

- [ ] **Step 1: Add child declarations and temporary parent imports**

Add to `remove.rs`:

```rust
mod legacy_replay;
mod scriptlets;

use legacy_replay::{
    execute_legacy_remove_replay_plan_entries, load_installed_legacy_remove_plan,
    require_legacy_replay_success,
};
use scriptlets::run_post_remove_scriptlet;
```

- [ ] **Step 2: Create `legacy_replay.rs`**

Move these items from `remove.rs` into `legacy_replay.rs`:

```rust
PreparedLegacyRemoveReplay
load_installed_legacy_remove_plan
plan_installed_legacy_remove_replay
compatibility_audit_from_plan
remove_plan_from_preflight
legacy_replay_refusal_error
execute_legacy_remove_replay_plan_entries
require_legacy_replay_success
build_legacy_replay_audit_for_remove
legacy_replay_planned_entries_for_audit
legacy_replay_outcome_audit
legacy_source_scriptlet_format
legacy_lifecycle_phase_name
host_foreign_replay_policy_name
```

Use this import surface:

```rust
// apps/conary/src/commands/remove/legacy_replay.rs

use std::path::Path;

use anyhow::{Context, Result};
use conary_core::ccs::legacy_replay::{
    HostForeignReplayPolicy, LegacyReplayLifecycle, LegacyReplayPlan, LegacyReplayPreflight,
    LegacyReplayRefusal, plan_legacy_replay,
};
use conary_core::ccs::legacy_scriptlets::{LegacyScriptletBundle, LifecyclePath, SourceFormat};
use conary_core::db::models::InstalledLegacyScriptletBundle;
use conary_core::repository::distro::source_target_from_bundle;
use conary_core::scriptlet::{
    ExecutionMode, LegacyInvocationRuntime, LegacyScriptletExecution,
    PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletExecutor, ScriptletOutcome,
};

use super::types::{LegacyRemoveReplayAuditContext, RemoveInnerResult, RemoveScriptletOptions};
use crate::commands::{
    LegacyReplayAudit, LegacyReplayCompatibilityAudit, LegacyReplayOutcomeAudit,
    LegacyReplayPlannedEntryAudit, LegacyReplayPreflightCheckAudit,
};
```

When moving the audit helper bodies, shorten the current fully qualified
`crate::commands::LegacyReplay*` references to the imported audit names above,
or remove any unused imports. The plan assumes the de-qualified names so
`clippy -D warnings` does not trip on unused imports.

Required visibility:

```rust
#[derive(Debug, Default)]
pub(super) struct PreparedLegacyRemoveReplay {
    pub(super) bundle: Option<LegacyScriptletBundle>,
    pub(super) planned_pre_remove: Option<LegacyReplayPlan>,
    pub(super) planned_post_remove: Option<LegacyReplayPlan>,
    pub(super) audit_context: Option<LegacyRemoveReplayAuditContext>,
}
```

Set `load_installed_legacy_remove_plan`, `execute_legacy_remove_replay_plan_entries`, `require_legacy_replay_success`, and `build_legacy_replay_audit_for_remove` to `pub(super)` while preserving their existing parameters and return types.

Keep the rest private unless a sibling import requires it.

- [ ] **Step 3: Create `scriptlets.rs`**

Move these items from `remove.rs` into `scriptlets.rs`:

```rust
run_post_remove_scriptlet
scriptlet_warning_from_failure
legacy_post_replay_warnings
```

`legacy_post_replay_warnings` intentionally lives in `scriptlets.rs`, not `legacy_replay.rs`, so `legacy_replay.rs` does not need to import from `scriptlets.rs`.

Use this import surface:

```rust
// apps/conary/src/commands/remove/scriptlets.rs

use std::path::Path;

use anyhow::Result;
use conary_core::scriptlet::{
    ExecutionMode, SandboxMode, ScriptletExecutor, ScriptletFailureKind, ScriptletFailureOutcome,
    ScriptletOutcome,
};
use tracing::{info, warn};

use super::legacy_replay::{
    build_legacy_replay_audit_for_remove, execute_legacy_remove_replay_plan_entries,
};
use super::types::RemoveInnerResult;
use crate::commands::ScriptletWarning;
use crate::commands::progress::{RemovePhase, RemoveProgress};
```

Update `scriptlet_warning_from_failure` and `legacy_post_replay_warnings` to use
the imported `ScriptletFailureOutcome` and `ScriptletWarning` names, or remove
those imports if the fully qualified paths are kept.

Required visibility:

```rust
pub(super) fn run_post_remove_scriptlet(
    conn: &rusqlite::Connection,
    remove_result: &RemoveInnerResult,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    progress: &RemoveProgress,
) -> Result<()>
```

`scriptlet_warning_from_failure` and `legacy_post_replay_warnings` can stay private to `scriptlets.rs`.

- [ ] **Step 4: Verify Task 2**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::remove
cargo test -p conary --lib commands::state
```

Expected:

```text
commands::remove: 10 passed
commands::state: 7 passed
```

- [ ] **Step 5: Commit Task 2**

```bash
git add apps/conary/src/commands/remove.rs \
  apps/conary/src/commands/remove/legacy_replay.rs \
  apps/conary/src/commands/remove/scriptlets.rs
git commit -m "refactor(conary): extract remove scriptlet replay"
```

---

### Task 3: Extract Transactional Remove Preparation And Commit Logic

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Create: `apps/conary/src/commands/remove/transaction.rs`

- [ ] **Step 1: Add child declaration and re-export**

Add to `remove.rs`:

```rust
mod transaction;

pub(crate) use transaction::remove_inner;

use transaction::{commit_remove_db, prepare_remove};
```

Do not keep a separate private import of `remove_inner`; the `pub(crate) use` re-export is enough for local use and for `commands::state`.

- [ ] **Step 2: Create `transaction.rs`**

Move these items from `remove.rs` into `transaction.rs`:

```rust
PreparedRemove
remove_inner
prepare_remove
commit_remove_db
```

Preserve the existing `#[allow(dead_code)]` attribute on
`PreparedRemove::planned_pre_remove`.

Move this test into `transaction.rs`:

```text
commit_remove_db_carries_planned_post_remove_after_trove_delete
```

Use this production import surface:

```rust
// apps/conary/src/commands/remove/transaction.rs

use std::path::Path;

use anyhow::Result;
use conary_core::ccs::legacy_replay::LegacyReplayPlan;
use conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle;
use conary_core::db::models::{FileEntry, ScriptletEntry, Trove};
use conary_core::scriptlet::{
    ExecutionMode, PackageFormat as ScriptletPackageFormat, ScriptletExecutor,
};
use tracing::info;

use super::legacy_replay::{
    execute_legacy_remove_replay_plan_entries, load_installed_legacy_remove_plan,
    require_legacy_replay_success,
};
use super::types::{LegacyRemoveReplayAuditContext, RemoveInnerResult, RemoveScriptletOptions};
use crate::commands::{FileSnapshot, TroveSnapshot};
use crate::commands::progress::{RemovePhase, RemoveProgress};
```

Required visibility:

Set `remove_inner` to `pub(crate)`, and set `prepare_remove` plus `commit_remove_db` to `pub(super)` while preserving their existing parameters and return types.

`PreparedRemove` can stay private to `transaction.rs`.

- [ ] **Step 3: Add transaction test imports**

Use this test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::remove_snapshot;
    use conary_core::ccs::legacy_replay::{LegacyReplayCompatibilityDecision, LegacyReplayPlan, PlannedLegacyEntry};
    use conary_core::ccs::legacy_scriptlets::LifecyclePath;
    use conary_core::db::models::{InstallSource, Trove, TroveType};
    use conary_core::scriptlet::SandboxMode;
    use tempfile::TempDir;
}
```

Keep `accepted_compatibility_decision()` inside this test module.

After adding the module-level test imports, shorten the moved test body to use
the imported `Trove`, `TroveType`, `InstallSource`, `LegacyReplayPlan`,
`PlannedLegacyEntry`, and `LifecyclePath` names instead of their existing
fully qualified paths.

- [ ] **Step 4: Verify Task 3**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::remove
cargo test -p conary --lib commands::state
```

Expected:

```text
commands::remove: 10 passed
commands::state: 7 passed
```

- [ ] **Step 5: Commit Task 3**

```bash
git add apps/conary/src/commands/remove.rs \
  apps/conary/src/commands/remove/transaction.rs
git commit -m "refactor(conary): extract remove transaction core"
```

---

### Task 4: Extract Autoremove Planning And Loop

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Create: `apps/conary/src/commands/remove/autoremove.rs`

- [ ] **Step 1: Add child declaration and re-export**

Add to `remove.rs`:

```rust
mod autoremove;

pub use autoremove::cmd_autoremove;
```

- [ ] **Step 2: Create `autoremove.rs`**

Move these items from `remove.rs` into `autoremove.rs`:

```rust
AutoremoveSkipReason
AutoremovePlan
cmd_autoremove
preflight_autoremove_round
plan_autoremove
print_autoremove_candidates
print_autoremove_skips
print_autoremove_trove
autoremove_identity
```

Move these tests and their local helper fixtures into `autoremove.rs`:

```text
autoremove_plan_classifies_authority_and_safety_skips
autoremove_refuses_legacy_candidate_before_removing_any_package
autoremove_with_legacy_replay_flag_removes_all_candidates
seed_dependency_trove
seed_installed_legacy_bundle
legacy_post_remove_bundle
legacy_post_remove_entry
table_count
changeset_metadata_by_description
```

Use this production import surface:

```rust
// apps/conary/src/commands/remove/autoremove.rs

use std::collections::HashSet;

use anyhow::Result;
use conary_core::db::models::Trove;
use tracing::info;

use super::legacy_replay::load_installed_legacy_remove_plan;
use super::types::RemoveScriptletOptions;
use crate::commands::{LegacyReplayOptions, SandboxMode, open_db};
```

Inside `cmd_autoremove`, keep the recursive package removal call as:

```rust
match super::cmd_remove(
    &trove.name,
    db_path,
    root,
    Some(trove.version.clone()),
    trove.architecture.clone(),
    no_scripts,
    sandbox_mode,
    false,
    legacy_replay,
)
.await
```

This works while `cmd_remove` is still parent-owned and continues working after Task 5 because the hub will re-export `command::cmd_remove`.

- [ ] **Step 3: Add autoremove test imports**

Use this test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
        LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
        PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
        TransactionOrder, VersionScheme,
    };
    use conary_core::db::models::{InstallSource, InstalledLegacyScriptletBundle, TroveType};
    use std::collections::BTreeMap;
    use tempfile::TempDir;
}
```

- [ ] **Step 4: Remove old parent imports made obsolete by autoremove**

After autoremove moves, remove parent imports that were only needed by autoremove/tests:

```rust
use std::collections::HashSet;
```

Also remove any parent test imports for `BTreeMap`, `InstalledLegacyScriptletBundle`, and legacy scriptlet bundle fixture types if no still-parented test needs them.

- [ ] **Step 5: Verify Task 4**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::remove -- --list
cargo test -p conary --lib commands::remove
cargo test -p conary --lib commands::model
```

Expected:

```text
10 remove tests listed
10 remove tests passed
commands::model passes
```

- [ ] **Step 6: Commit Task 4**

```bash
git add apps/conary/src/commands/remove.rs \
  apps/conary/src/commands/remove/autoremove.rs
git commit -m "refactor(conary): extract autoremove planning"
```

---

### Task 5: Extract Remove Command Orchestration And Finalize The Hub

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Create: `apps/conary/src/commands/remove/command.rs`

- [ ] **Step 1: Add child declaration and re-export**

Add to `remove.rs`:

```rust
mod command;

pub use command::cmd_remove;
```

- [ ] **Step 2: Create `command.rs`**

Move `cmd_remove` and `print_remove_summary` from `remove.rs` into `command.rs`.
Preserve the existing `#[allow(clippy::too_many_arguments)]` attribute on
`cmd_remove`.

Move these tests into `command.rs`:

```text
no_generation_remove_deletes_files_and_db_rows
no_generation_remove_fails_closed_on_dangling_current_without_mutation
no_generation_remove_live_root_failure_leaves_no_pending_changeset
remove_refuses_critical_package_before_file_mutation
```

Use this production import surface:

```rust
// apps/conary/src/commands/remove/command.rs

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use conary_core::db::models::Changeset;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use tracing::info;

use super::execution_path::{RemoveExecutionPath, remove_execution_path};
use super::scriptlets::run_post_remove_scriptlet;
use super::transaction::{commit_remove_db, prepare_remove, remove_inner};
use super::types::{RemoveInnerResult, RemoveScriptletOptions};
use crate::commands::{
    InstalledPackageSelector, LegacyReplayOptions, SandboxMode, open_db, resolve_installed_package,
};
use crate::commands::progress::{RemovePhase, RemoveProgress};
```

Apply these path updates while moving `cmd_remove`:

- Change `super::live_root::recover_pending_journals_with_changesets(...)` to
  `crate::commands::live_root::recover_pending_journals_with_changesets(...)`;
  once `cmd_remove` lives in `remove/command.rs`, `super` resolves to
  `commands::remove`, not `commands`.
- Change `conary_core::db::models::Changeset::with_tx_uuid(...)` and
  `conary_core::db::models::Changeset::new(...)` to `Changeset::with_tx_uuid(...)`
  and `Changeset::new(...)`, or remove the `Changeset` import.

- [ ] **Step 3: Add command test imports**

Use this test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{FileEntry, InstallSource, Trove, TroveType};
    use tempfile::TempDir;
}
```

After adding the module-level test imports, shorten the moved test bodies to use
the imported `Trove`, `FileEntry`, `InstallSource`, and `TroveType` names
instead of their existing fully qualified paths.

Keep the `std::os::unix::fs::symlink(...)` call in `no_generation_remove_fails_closed_on_dangling_current_without_mutation` unchanged.

- [ ] **Step 4: Reduce `remove.rs` to the final hub**

After moving `cmd_remove`, the final `apps/conary/src/commands/remove.rs` should contain only:

```rust
// src/commands/remove.rs
//! Package removal commands

mod autoremove;
mod command;
mod execution_path;
mod legacy_replay;
mod scriptlets;
#[cfg(test)]
pub(super) mod test_support;
mod transaction;
mod types;

pub use autoremove::cmd_autoremove;
pub use command::cmd_remove;

pub(crate) use transaction::remove_inner;
#[allow(unused_imports)]
pub(crate) use types::RemoveInnerResult;
pub(crate) use types::RemoveScriptletOptions;
```

The `#[allow(unused_imports)]` on `RemoveInnerResult` is intentional: it keeps the return type of the re-exported `remove_inner` nameable outside the private `types` child while avoiding `unused_imports` under `-D warnings`.

- [ ] **Step 5: Verify hub boundaries**

Run:

```bash
rg -n "^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn " apps/conary/src/commands/remove.rs
rg -n -U "#\[cfg\(test\)\]\s*\n\s*mod tests" apps/conary/src/commands/remove.rs
rg -n "use super::\*|use crate::\*" apps/conary/src/commands/remove.rs apps/conary/src/commands/remove
```

Expected:

```text
No function bodies in remove.rs.
No parent #[cfg(test)] mod tests in remove.rs.
No wildcard imports in production modules; wildcard imports are acceptable only inside child #[cfg(test)] modules if retained.
```

- [ ] **Step 6: Verify Task 5**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::remove -- --list
cargo test -p conary --lib commands::remove
cargo test -p conary --lib commands::state
cargo test -p conary --lib commands::model
```

Expected:

```text
10 remove tests listed
10 remove tests passed
7 state tests passed
commands::model passes
```

- [ ] **Step 7: Commit Task 5**

```bash
git add apps/conary/src/commands/remove.rs \
  apps/conary/src/commands/remove/command.rs
git commit -m "refactor(conary): extract remove command orchestration"
```

---

### Task 6: Update Documentation Routing And Docs-Audit Metadata

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

- [ ] **Step 1: Update assistant subsystem routing**

In `docs/llms/subsystem-map.md`, update front matter:

```yaml
last_updated: 2026-06-09
revision: 15
summary: Add remove command child-module routing
```

In the install/update/remove routing list, replace the single remove path with:

```text
`apps/conary/src/commands/remove.rs`,
`apps/conary/src/commands/remove/command.rs`,
`apps/conary/src/commands/remove/autoremove.rs`,
`apps/conary/src/commands/remove/transaction.rs`,
`apps/conary/src/commands/remove/scriptlets.rs`,
`apps/conary/src/commands/remove/legacy_replay.rs`,
`apps/conary/src/commands/remove/execution_path.rs`,
`apps/conary/src/commands/remove/types.rs`,
```

Include `apps/conary/src/commands/remove/test_support.rs` only in a test-fixture or test-support note, not as a production start path.

- [ ] **Step 2: Update feature ownership**

In `docs/modules/feature-ownership.md`, update front matter:

```yaml
last_updated: 2026-06-09
revision: 5
summary: Add remove command child-module ownership
```

In the "Native Package Install, Update, Remove, And Live-Root Mutation" start paths, replace the single remove path with:

```text
`apps/conary/src/commands/remove.rs`;
`apps/conary/src/commands/remove/command.rs`;
`apps/conary/src/commands/remove/autoremove.rs`;
`apps/conary/src/commands/remove/transaction.rs`;
`apps/conary/src/commands/remove/scriptlets.rs`;
`apps/conary/src/commands/remove/legacy_replay.rs`;
`apps/conary/src/commands/remove/execution_path.rs`;
`apps/conary/src/commands/remove/types.rs`;
```

- [ ] **Step 3: Update architecture module map**

`docs/ARCHITECTURE.md` currently describes `commands/` as "install, repo, query, model hub + child modules, ccs, bootstrap, system". Update that wording so remove is also clearly represented as a hub with child modules.

Update front matter:

```yaml
last_updated: 2026-06-09
revision: 20
summary: Note remove command child modules
```

Keep this change small. Do not rewrite architecture content unrelated to command module routing.

- [ ] **Step 4: Refresh docs-audit metadata in place**

Update existing ledger rows for touched docs rather than adding duplicate rows:

```text
docs/llms/subsystem-map.md
docs/modules/feature-ownership.md
docs/ARCHITECTURE.md
docs/superpowers/documentation-accuracy-audit-summary.md
```

The disposition counts should remain unchanged from Task 0:

```text
archived 73
corrected 68
retained-historical 14
verified-no-change 12
```

- [ ] **Step 5: Regenerate inventory and verify docs-audit**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
git diff --check
```

Expected:

```text
167
archived 73
corrected 68
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
No diff from the regenerated docs-audit inventory.
check-doc-truth passes.
```

- [ ] **Step 6: Commit Task 6**

```bash
git add docs/llms/subsystem-map.md \
  docs/modules/feature-ownership.md \
  docs/ARCHITECTURE.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: record remove command ownership"
```

---

### Task 7: Final Verification And Push

**Files:**
- Verify all files touched by Tasks 1-6.

- [ ] **Step 1: Formatting and compile gates**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo check --workspace --all-targets
```

Expected: all commands pass.

- [ ] **Step 2: Focused remove and restore tests**

Run:

```bash
cargo test -p conary --lib commands::remove -- --list
cargo test -p conary --lib commands::remove
cargo test -p conary --lib commands::state
cargo test -p conary --lib commands::model
cargo test -p conary --lib commands::automation
```

Expected:

```text
10 remove tests listed
commands::remove: 10 passed
commands::state: 7 passed
commands::model passes
commands::automation passes
```

- [ ] **Step 3: Integration tests covering remove/autoremove/scriptlet surfaces**

Run:

```bash
cargo test -p conary --test live_host_mutation_safety
cargo test -p conary --test native_pm_live_root
cargo test -p conary --test native_pm_daily_driver
cargo test -p conary --test model_apply
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary --test query_scripts
cargo run -p conary-test -- list
```

Expected: all selected integration tests pass, and the `conary-test` manifest
inventory lists successfully.

- [ ] **Step 4: Broad lint gate**

Run:

```bash
cargo clippy -p conary --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both clippy commands pass with zero warnings.

- [ ] **Step 5: Boundary checks**

Run:

```bash
scripts/line-count-report.sh 20
rg -n "^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn " apps/conary/src/commands/remove.rs
rg -n -U "#\[cfg\(test\)\]\s*\n\s*mod tests" apps/conary/src/commands/remove.rs
rg -n "use super::\*|use crate::\*" apps/conary/src/commands/remove.rs apps/conary/src/commands/remove
```

Expected:

```text
remove.rs is no longer a top hotspot.
No function bodies in remove.rs.
No parent test module in remove.rs.
Wildcard imports, if any, are confined to child test modules.
```

- [ ] **Step 6: Docs-audit final check**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
git diff --check
```

Expected:

```text
167
archived 73
corrected 68
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
No diff from the regenerated docs-audit inventory.
check-doc-truth passes.
```

- [ ] **Step 7: Push and verify synced main**

Run:

```bash
git status --short --branch
git log --oneline --decorate --max-count=8
git push
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected:

```text
working tree clean
HEAD and origin/main match
0	0
one worktree at /home/peter/Conary
```

---

## Self-Review Checklist For Implementers

- `apps/conary/src/commands/remove.rs` remains a file module, not a directory `mod.rs`.
- `commands/mod.rs` still re-exports `cmd_remove` and `cmd_autoremove` through `remove`.
- `commands/state.rs` still imports `RemoveScriptletOptions` and `remove_inner` from `super::remove`.
- `RemoveInnerResult::snapshot` stays readable from `commands::state`.
- `cmd_autoremove` still removes candidates by calling the hub-level `super::cmd_remove(...)`.
- Legacy remove replay refusal still preflights an autoremove round before removing any earlier candidates.
- `#[allow(dead_code)]` on planned pre-remove replay state is preserved where the field moves.
- No behavior-changing cleanup is bundled into this phase.
