# Project Maintainability Phase 8 Install CCS Transaction Decomposition Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. This is the Phase 8 child packet
> under
> `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Extract the direct CCS transaction install path from
`apps/conary/src/commands/install/mod.rs` into a focused install submodule
without changing install behavior, package formats, live-root safety, or CCS
scriptlet policy.

**Architecture:** Add `apps/conary/src/commands/install/ccs_transaction.rs`
as the owner for `CcsTransactionInstallOptions`,
`install_ccs_package_transactionally`, and the CCS manifest-selection,
hook-status, and capability-gate helpers that only serve direct CCS
transaction installs. Keep the shared install engine, manifest-provides
persistence inside the DB transaction, live-root execution path selection,
scriptlet phases, and DB transaction finalization in `install/mod.rs`, and
preserve the current public and sibling-module import paths through re-exports
from `install/mod.rs`.

**Tech Stack:** Rust, existing Conary install modules, existing CCS package
manifest APIs, existing SQLite transaction models, existing cargo tests,
docs-audit scripts.

---

## Status

Draft plan for review.

## Read First

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/ccs.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/feature-ownership.md`
- `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md`
- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/install/legacy_replay.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/src/commands/install/restore.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/tests/bundle_replay.rs`
- `apps/conary/tests/component.rs`

## Design Summary

Phase 4 moved legacy replay ownership out of the install hotspot. Phase 8
should take the next install-side slice, but it should not chase every large
helper in `install/mod.rs`. The coherent next owner is direct CCS transaction
install: it has a single exported entry point, a narrow option/result pair,
and helper functions for CCS manifest file classification, CCS hook status,
and scriptlet capability gating.

The extraction should be behavior-preserving. `install/mod.rs` should remain
the owner of the shared engine that installs files, journals live-root writes,
runs package-format scriptlets, persists manifest-provides inside the active DB
transaction, finalizes changesets, and coordinates generation snapshots.
`ccs_transaction.rs` should call those shared helpers through `super::` until a
later live-root transaction slice deliberately moves that engine.

## Current Repo-Grounded Inputs

| Signal | Current value | Phase 8 interpretation |
|--------|---------------|------------------------|
| Largest Rust files | `apps/conary/src/commands/ccs/install.rs` 3441 lines; `apps/conary/src/commands/install/mod.rs` 3398 lines; `apps/conary/src/commands/update.rs` 3334 lines | `install/mod.rs` remains a top hotspot after Phase 4 and live-mutation UX work |
| Existing install submodules | `batch.rs`, `conversion.rs`, `dependencies.rs`, `execute.rs`, `inner.rs`, `legacy_replay.rs`, `restore.rs`, `scriptlets.rs`, `system_pm.rs` | A focused install submodule is already the local pattern |
| CCS transaction exported entry | `install_ccs_package_transactionally` | Move this entry with its direct CCS helpers |
| Stable sibling consumers | `commands/ccs/install.rs`, `commands/update.rs`, `install/conversion.rs`, `install/restore.rs`, `install/legacy_replay.rs` | Preserve imports through `install/mod.rs` re-exports |
| Docs-audit baseline | 151 tracked doc-like files, 51 corrected rows | Lock-in should add one planning file and update counts to 152 total / 52 corrected |

Evidence commands used to shape this packet:

```bash
scripts/line-count-report.sh 30
find apps/conary/src/commands/install -maxdepth 2 -type f -print | sort
rg -n "CcsTransactionInstallOptions|install_ccs_package_transactionally|extract_and_classify_ccs_manifest_files|persist_ccs_manifest_provides|mark_ccs_changeset_post_hooks_failed|enforce_ccs_scriptlet_capability_gate" apps/conary/src/commands
cargo test -p conary --lib ccs -- --list
cargo test -p conary --test component -- --list
cargo test -p conary --test bundle_replay -- --list
```

Current filter-discovery results:

| Filter | Current matches |
|--------|-----------------|
| `cargo test -p conary --lib ccs_install_persists_manifest_provides -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_persists_capability_declarations -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_rejects_scriptlet_capabilities_without_enforcement_before_mutation -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_marks_changeset_post_hooks_failed_after_post_install_error -- --list` | 1 test |
| `cargo test -p conary --lib converted_ccs_install -- --list` | 5 tests |
| `cargo test -p conary --lib try_convert_to_ccs_rejects_inferred_prompted_capabilities_before_db_mutation -- --list` | 1 test |
| `cargo test -p conary --test component -- --list` | 8 tests |
| `cargo test -p conary --test bundle_replay -- --list` | 26 tests |
| `cargo test -p conary --test workflow -- --list` | 8 tests |
| `cargo test -p conary --test conversion_integration golden_conversion -- --list` | 4 tests |
| `cargo test -p conary --test native_pm_live_root -- --list` | 6 tests |

## Module Boundary

Create:

- `apps/conary/src/commands/install/ccs_transaction.rs`

Move these items from `apps/conary/src/commands/install/mod.rs` into
`ccs_transaction.rs`:

- `CcsTransactionInstallOptions`
- `CcsTransactionInstallResult`
- `extract_and_classify_ccs_manifest_files`
- `check_ccs_upgrade_status`
- `mark_ccs_changeset_post_hooks_failed`
- `ccs_has_pre_hooks`
- `ccs_has_post_hooks`
- `enforce_ccs_scriptlet_capability_gate`
- `install_ccs_package_transactionally`
- `ccs_transaction_install_preflights_live_root_ownership_before_hooks_and_scriptlets`

Keep these items in `install/mod.rs` for this slice:

- `cmd_install`
- `CcsInstallParams`
- `PackageExecutionPath`
- live-root preflight and execution-path helpers
- `run_pre_install_phase`
- `execute_install_transaction`
- `finalize_install_without_snapshot`
- `TransactionContext`
- `InstallTransactionResult`
- `ExtractionResult`
- `ScriptletContext`
- `PreScriptletState`
- `InstallSemantics`
- `persist_ccs_manifest_provides`
- `insert_ccs_manifest_typed_provide`
- dependency, adoption, and package suggestion helpers

Note: `persist_ccs_manifest_provides` is named for the CCS manifest data
format, but it is called from the shared `execute_install_transaction` engine
at the points where `TransactionContext::ccs_manifest_provides` is persisted.
It stays with the shared engine for this slice.

Add this module declaration and re-export block in `install/mod.rs`:

```rust
mod ccs_transaction;

pub(crate) use ccs_transaction::{
    CcsTransactionInstallOptions, CcsTransactionInstallResult,
    install_ccs_package_transactionally,
};
```

If compilation proves that `ccs_transaction.rs` must name a shared install
type, prefer `pub(in crate::commands::install)` or `pub(super)` on the shared
item rather than broadening it to `pub(crate)`. The caller-visible API should
remain the same: external and sibling modules should continue to import through
`crate::commands::install` or `super::`.

## Dependency Direction

`ccs_transaction.rs` may depend on `super::` for shared install mechanics:

- `execute_install_transaction`
- `execute_legacy_replay_plan_entries`
- `finalize_install_without_snapshot`
- `legacy_post_replay_warnings`
- `merge_old_upgrade_legacy_replay_state`
- `plan_ccs_fresh_install_legacy_replay`
- `plan_ccs_old_installed_upgrade_legacy_replay`
- `preflight_extracted_live_root_file_ownership`
- `prepare_install_environment_before_scriptlets`
- `require_legacy_replay_success`
- `run_pre_install_phase`
- `show_dry_run_summary`

It should import non-`mod.rs` helpers from their owner modules instead of
re-exporting them through `install/mod.rs`:

- `build_execution_mode` from `super::scriptlets`
- `should_run_scriptlets` from `conary_core::components`

`install/mod.rs` should not depend on implementation details inside
`ccs_transaction.rs` after the re-export. That keeps the new module as a leaf
adapter over the shared install engine rather than a second engine.

## Non-Goals

- Do not change CCS manifest schema, package format, scriptlet metadata, or
  persisted DB layout.
- Do not change live-system mutation UX, command flags, conaryd API fields, or
  integration manifest syntax.
- Do not change CCS v2/native-package contract work.
- Do not move `execute_install_transaction` or live-root journal ownership in
  this slice.
- Do not decompose `apps/conary/src/commands/ccs/install.rs` in this slice.
- Do not deduplicate legacy replay, conversion, restore, or remove-side helper
  code unless compilation requires a visibility shim for this exact extraction.
- Do not weaken capability enforcement, post-hook failure marking, or
  refusal-before-mutation ordering.

## Review Focus

Reviewers should check:

- whether the moved item list is complete but not too broad;
- whether re-exports preserve all existing caller paths;
- whether the plan avoids circular imports between `install/mod.rs`,
  `commands/update.rs`, `install/legacy_replay.rs`, `install/conversion.rs`,
  and `install/restore.rs`;
- whether the shared transaction engine stays in `install/mod.rs`;
- whether CCS manifest-provides persistence stays with
  `execute_install_transaction` in `install/mod.rs`;
- whether the CCS scriptlet capability gate still runs before extraction,
  hooks, scriptlets, DB mutation, or live-root writes;
- whether post-install hook failures still mark the applied changeset as
  `post_hooks_failed`;
- whether test filters prove direct CCS installs, converted CCS installs,
  component selection, bundle replay, and live-root preflight ordering.

## Implementation Plan

If a later task reveals a regression after earlier extraction commits have
landed, revert the Phase 8 implementation commits in reverse order before
trying a different boundary. The task commits are intentionally small so the
work can be backed out without disturbing unrelated main-branch changes.

### Task 0: Lock The Reviewed Phase 8 Plan And Docs-Audit Row

**Files:**
- Add: `docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the reviewed plan before regenerating docs inventory**

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md
```

- [ ] **Step 2: Regenerate docs-audit inventory**

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected on the current baseline: tracked doc-like files grow from 151 to 152
data rows, excluding the inventory header, with this plan file added as
`planning` / `maintainer`. If another docs file lands first, use the
regenerated inventory as source of truth and update counts accordingly.

- [ ] **Step 3: Add the plan ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the active
maintainability plan rows:

```text
docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md	docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md	planning	maintainer	maintainability; phase8; install-hotspot; ccs-transaction; refactor-plan	apps/conary/src/commands/install/mod.rs; apps/conary/src/commands/install/legacy_replay.rs; apps/conary/src/commands/install/conversion.rs; apps/conary/src/commands/install/restore.rs; apps/conary/src/commands/ccs/install.rs; apps/conary/src/commands/update.rs; apps/conary/tests/bundle_replay.rs; apps/conary/tests/component.rs; scripts/line-count-report.sh; docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md	verified	corrected	Added the reviewed Phase 8 plan for extracting direct CCS transaction install ownership from install/mod.rs into install/ccs_transaction.rs while preserving caller re-exports, live-root safety ordering, scriptlet capability gates, and CCS behavior.
```

- [ ] **Step 4: Update the audit summary**

Append this paragraph to the existing
`### 2026-06-06 Maintainability Planning` section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The Phase 8 install CCS transaction decomposition plan now continues the
install-hotspot lane after the legacy replay extraction. It scopes the next
behavior-preserving slice to `apps/conary/src/commands/install/ccs_transaction.rs`,
keeping shared live-root transaction machinery and manifest-provides
persistence in `install/mod.rs` while moving the direct CCS transaction entry
point, option/result pair, manifest selection helper, hook-status helpers, and
scriptlet capability gate into a focused install submodule.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 152
- `verified-no-change`: 13
- `corrected`: 52
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes include the Phase 8 planning update.

- [ ] **Step 5: Verify docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --cached --check
git diff --check
for term in T''BD TO''DO FI''XME Cent''OS RH''EL "Debian sta""ble" open''SUSE Al''pine CLAU''DE Cla''ude "Open Review"" Questions"; do
    if grep -n "$term" docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md; then
        exit 1
    fi
done
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit the reviewed plan lock-in**

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan install ccs transaction split"
```

### Task 1: Introduce The CCS Transaction Module Shell

**Files:**
- Add: `apps/conary/src/commands/install/ccs_transaction.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`

- [ ] **Step 1: Create the module with a path comment**

Create `apps/conary/src/commands/install/ccs_transaction.rs`:

```rust
// apps/conary/src/commands/install/ccs_transaction.rs
//! Direct CCS package transaction install adapter.
//!
//! This module owns the direct CCS transaction entry point and CCS-specific
//! manifest selection, hook-status, and capability-gate helpers. Shared install
//! transaction mechanics stay in `install/mod.rs`.
```

- [ ] **Step 2: Register the module**

In `apps/conary/src/commands/install/mod.rs`, add the module declaration beside
the other install submodules:

```rust
mod ccs_transaction;
```

- [ ] **Step 3: Run the narrow compile check**

```bash
cargo check -p conary
```

Expected: compilation succeeds. No behavior has moved yet.

- [ ] **Step 4: Commit the module shell**

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "refactor(install): add ccs transaction module"
```

### Task 2: Move The CCS Transaction Types And Helpers

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`

- [ ] **Step 1: Move the option and result types**

Move `CcsTransactionInstallOptions` and `CcsTransactionInstallResult` to
`ccs_transaction.rs`. Keep the field list unchanged:

```rust
pub(crate) struct CcsTransactionInstallOptions<'a> {
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub defer_generation: bool,
    pub no_scripts: bool,
    pub sandbox_mode: SandboxMode,
    pub allow_downgrade: bool,
    pub reinstall: bool,
    pub selection_reason: Option<&'a str>,
    pub component_selection: ComponentSelection,
    pub selected_manifest_components: Option<Vec<String>>,
    pub repository_provenance: Option<RepositoryInstallProvenance>,
    pub legacy_replay: LegacyReplayOptions,
}

pub(crate) struct CcsTransactionInstallResult {
    pub changeset_id: i64,
    pub post_commit_warnings: Vec<String>,
}
```

Import the referenced types from the existing owners:

```rust
use super::{ComponentSelection, LegacyReplayOptions, RepositoryInstallProvenance};
use conary_core::scriptlet::SandboxMode;
```

- [ ] **Step 2: Re-export the moved types from `install/mod.rs`**

Add this block in `install/mod.rs` near the legacy replay re-export block:

```rust
pub(crate) use ccs_transaction::{CcsTransactionInstallOptions, CcsTransactionInstallResult};
```

Expected: existing imports in `install/legacy_replay.rs`,
`install/conversion.rs`, `install/restore.rs`, and
`commands/ccs/install.rs`, plus the `commands/update.rs` import through
`super::install`, remain valid.

- [ ] **Step 3: Move the CCS-only helper functions**

Move these helper functions into `ccs_transaction.rs` without changing their
bodies except for import qualification:

```text
extract_and_classify_ccs_manifest_files
check_ccs_upgrade_status
mark_ccs_changeset_post_hooks_failed
ccs_has_pre_hooks
ccs_has_post_hooks
enforce_ccs_scriptlet_capability_gate
```

Use imports from existing owners rather than changing behavior. Typical
imports will include:

```rust
use super::{
    ComponentSelection, ExtractionResult, InstallProgress, InstallPhase,
    InstallSemantics, UpgradeCheck, check_upgrade_status,
};
use anyhow::Result;
use crate::commands::ccs::{normalize_ccs_extracted_files, normalize_ccs_package_path};
use conary_core::components::{ComponentClassifier, ComponentType};
use conary_core::db::models::{Changeset, ChangesetStatus};
use conary_core::dependencies::LanguageDepDetector;
use conary_core::packages::PackageFormat;
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};
```

- [ ] **Step 4: Keep helper visibility narrow**

The moved helpers should stay private to `ccs_transaction.rs` unless a compiler
error proves a test or sibling module must name one. Do not re-export the
helper functions from `install/mod.rs`.

- [ ] **Step 5: Run the narrow compile check**

```bash
cargo check -p conary
```

Expected: compilation succeeds. If a shared item in `install/mod.rs` is private
to that module, make it `pub(in crate::commands::install)` or `pub(super)`,
whichever is narrower and still compiles.

- [ ] **Step 6: Commit the type and helper move**

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "refactor(install): move ccs transaction helpers"
```

### Task 3: Move The Direct CCS Transaction Entry Point

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`

- [ ] **Step 1: Move `install_ccs_package_transactionally`**

Move `install_ccs_package_transactionally` into `ccs_transaction.rs`. Keep the
signature unchanged:

```rust
pub(crate) fn install_ccs_package_transactionally(
    conn: &mut rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    opts: CcsTransactionInstallOptions<'_>,
) -> Result<CcsTransactionInstallResult>
```

- [ ] **Step 2: Re-export the entry point from `install/mod.rs`**

Extend the existing re-export block:

```rust
pub(crate) use ccs_transaction::{
    CcsTransactionInstallOptions, CcsTransactionInstallResult,
    install_ccs_package_transactionally,
};
```

- [ ] **Step 3: Import shared install engine helpers from their owners**

In `ccs_transaction.rs`, import the shared helpers and types used by the moved
entry point:

```rust
use super::{
    ScriptletContext, TransactionContext, execute_install_transaction,
    execute_legacy_replay_plan_entries,
    finalize_install_without_snapshot, legacy_post_replay_warnings,
    merge_old_upgrade_legacy_replay_state, plan_ccs_fresh_install_legacy_replay,
    plan_ccs_old_installed_upgrade_legacy_replay,
    preflight_extracted_live_root_file_ownership,
    prepare_install_environment_before_scriptlets, require_legacy_replay_success,
    run_pre_install_phase, show_dry_run_summary,
};
use super::scriptlets::build_execution_mode;
use crate::commands::ccs::validate_ccs_payload_paths;
use conary_core::components::should_run_scriptlets;
```

These shared helpers are currently private to `install/mod.rs`, and that is
expected to remain valid because `ccs_transaction.rs` is a child submodule of
`install`. Existing child modules, including `restore.rs`, already call private
parent helpers through `super::`.

Use this visibility audit during compilation:

| Item | Proposed visibility |
|------|---------------------|
| `prepare_install_environment_before_scriptlets` | Keep private if compilation allows; otherwise `pub(in crate::commands::install)` |
| `preflight_extracted_live_root_file_ownership` | Keep private if compilation allows; otherwise `pub(in crate::commands::install)` |
| `show_dry_run_summary` | Keep private if compilation allows; otherwise `pub(in crate::commands::install)` |
| `run_pre_install_phase` | Keep private if compilation allows; otherwise `pub(in crate::commands::install)` |
| `execute_install_transaction` | Keep private if compilation allows; otherwise `pub(in crate::commands::install)` |
| `finalize_install_without_snapshot` | Keep private if compilation allows; otherwise `pub(in crate::commands::install)` |

Keep any additional visibility changes as narrow as the compiler allows.
Prefer `pub(in crate::commands::install)` when only install submodules need
access, and avoid `pub(crate)` for these shared-engine helpers. Do not
re-export `build_execution_mode` from `install/mod.rs`; import it from
`super::scriptlets`.

- [ ] **Step 4: Keep caller imports stable**

Do not rewrite the callers in these files unless compilation requires it:

- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/src/commands/install/restore.rs`
- `apps/conary/src/commands/install/legacy_replay.rs`

The expected stable paths are still:

```rust
use super::super::install::install_ccs_package_transactionally;
use super::{CcsTransactionInstallOptions, ...};
```

- [ ] **Step 5: Run the narrow compile check**

```bash
cargo check -p conary
```

Expected: compilation succeeds with no behavior changes.

- [ ] **Step 6: Commit the entry-point move**

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "refactor(install): move ccs transaction entry point"
```

### Task 4: Move And Repair The CCS Transaction Ordering Test

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`

- [ ] **Step 1: Move the ordering test into `ccs_transaction.rs`**

Move `ccs_transaction_install_preflights_live_root_ownership_before_hooks_and_scriptlets`
from the `tests` module in `install/mod.rs` into a `#[cfg(test)]` module in
`ccs_transaction.rs`.

- [ ] **Step 2: Update the source include path**

Change the test source include from:

```rust
let source = include_str!("mod.rs");
```

to:

```rust
let source = include_str!("ccs_transaction.rs");
```

- [ ] **Step 3: Update the helper boundary assertion**

The moved file no longer contains `finalize_install_without_snapshot`. Use the
end of the file or the test module marker as the boundary:

```rust
let install_start = source
    .find("pub(crate) fn install_ccs_package_transactionally")
    .expect("install_ccs_package_transactionally should exist");
let test_module_start = source[install_start..]
    .find("#[cfg(test)]")
    .unwrap_or(source[install_start..].len());
let install_source = &source[install_start..install_start + test_module_start];
```

Keep the relative-order checks unchanged:

```rust
let extraction_pos = install_source
    .find("extract_and_classify_ccs_manifest_files")
    .expect("CCS transaction install should extract files");
let preflight_pos = install_source
    .find("preflight_extracted_live_root_file_ownership(")
    .expect("CCS transaction install should preflight live-root ownership");
let ccs_hook_pos = install_source
    .find("hook_executor.execute_pre_hooks")
    .expect("CCS transaction install should run pre-hooks");
let scriptlet_pos = install_source
    .find("run_pre_install_phase(")
    .expect("CCS transaction install should run pre-install scriptlets");

assert!(
    extraction_pos < preflight_pos
        && preflight_pos < ccs_hook_pos
        && preflight_pos < scriptlet_pos,
    "CCS transaction installs must preflight live-root ownership before hooks and scriptlets"
);
```

Place the moved helper functions
`extract_and_classify_ccs_manifest_files`, `check_ccs_upgrade_status`,
`mark_ccs_changeset_post_hooks_failed`, `ccs_has_pre_hooks`,
`ccs_has_post_hooks`, and `enforce_ccs_scriptlet_capability_gate` before
`install_ccs_package_transactionally` in `ccs_transaction.rs`, with the
`#[cfg(test)]` module immediately after the entry point. That layout makes the
`#[cfg(test)]` boundary delimit the entry point body rather than an accidental
whole-file span.

- [ ] **Step 4: Run the moved-test filter**

```bash
cargo test -p conary --lib ccs_transaction_install_preflights_live_root_ownership_before_hooks_and_scriptlets
```

Expected: the moved ordering test passes.

- [ ] **Step 5: Commit the test move**

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "test(install): move ccs transaction ordering proof"
```

### Task 5: Run Focused CCS Install Proof

**Files:**
- Verify: `apps/conary/src/commands/install/ccs_transaction.rs`
- Verify: `apps/conary/src/commands/ccs/install.rs`
- Verify: `apps/conary/src/commands/install/conversion.rs`

- [ ] **Step 1: Run direct CCS transaction and CCS install unit filters**

```bash
cargo test -p conary --lib ccs_transaction
cargo test -p conary --lib ccs_install_persists_manifest_provides
cargo test -p conary --lib ccs_install_persists_capability_declarations
cargo test -p conary --lib ccs_install_rejects_scriptlet_capabilities_without_enforcement_before_mutation
cargo test -p conary --lib ccs_install_marks_changeset_post_hooks_failed_after_post_install_error
```

Expected: all commands pass. After extraction, the `ccs_transaction` filter is
expected to match only the moved ordering proof unless more local
`ccs_transaction` tests are added during implementation. The individually
listed `ccs_install_*` filters provide the direct CCS install behavior
coverage: manifest-provides persistence, capability declarations,
capability-gate refusal, and post-hook failure status.

- [ ] **Step 2: Run converted CCS install filters**

```bash
cargo test -p conary --lib converted_ccs_install
cargo test -p conary --lib try_convert_to_ccs_rejects_inferred_prompted_capabilities_before_db_mutation
```

Expected: all commands pass. Conversion flows should still reach
`install_ccs_package_transactionally` through the re-exported install API.

- [ ] **Step 3: Commit any compile-driven import fixes**

Only if Steps 1 or 2 required import/visibility fixes:

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/ccs_transaction.rs apps/conary/src/commands/install/conversion.rs apps/conary/src/commands/install/restore.rs apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/update.rs
git commit -m "fix(install): preserve ccs transaction callers"
```

If no fixes were required, leave this step unchecked until the task runner
marks it as skipped in their execution notes.

### Task 6: Run Medium Integration Proof

**Files:**
- Verify: `apps/conary/tests/component.rs`
- Verify: `apps/conary/tests/bundle_replay.rs`
- Verify: `apps/conary/tests/workflow.rs`
- Verify: `apps/conary/tests/conversion_integration.rs`
- Verify: `apps/conary/tests/native_pm_live_root.rs`

- [ ] **Step 1: Run component and bundle replay coverage**

```bash
cargo test -p conary --test component
cargo test -p conary --test bundle_replay
```

Expected: all commands pass. These cover CCS component selection, bundle
replay behavior, and direct CCS install flows that sit above the moved adapter.

- [ ] **Step 2: Run workflow and conversion coverage**

```bash
cargo test -p conary --test workflow
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: all commands pass. The `golden_conversion` filter is a substring
filter that intentionally runs the golden conversion safety cases, including
the same-target legacy replay case.

- [ ] **Step 3: Run live-root safety coverage**

```bash
cargo test -p conary --test native_pm_live_root
```

Expected: the live-root integration test passes. This gives one more check
that the shared transaction engine and live-root preflight machinery were not
changed while extracting the CCS adapter.

### Task 7: Final Hygiene And Hotspot Evidence

**Files:**
- Verify: workspace formatting and plan hygiene

- [ ] **Step 1: Format and lint the owning package**

```bash
cargo fmt --check
cargo clippy -p conary --all-targets -- -D warnings
```

Expected: both commands exit 0.

- [ ] **Step 2: Re-run hotspot report**

```bash
scripts/line-count-report.sh 30
```

Expected: `apps/conary/src/commands/install/mod.rs` line count decreases from
the Phase 8 baseline of 3398 lines. Do not set a hard target count; the success
criterion is coherent ownership and behavior preservation, not maximum line
removal.

- [ ] **Step 3: Inspect final ownership references**

```bash
rg -n "CcsTransactionInstallOptions|CcsTransactionInstallResult|install_ccs_package_transactionally|enforce_ccs_scriptlet_capability_gate|mark_ccs_changeset_post_hooks_failed|extract_and_classify_ccs_manifest_files" apps/conary/src/commands/install apps/conary/src/commands/ccs/install.rs
```

Expected: option/result/entry references are routed through
`install/ccs_transaction.rs` or the `install/mod.rs` re-export; CCS-only helper
implementations live in `install/ccs_transaction.rs`; caller paths remain
stable.

- [ ] **Step 4: Run diff hygiene and stale-surface sweep**

```bash
git diff --check
for term in T''BD TO''DO FI''XME Cent''OS RH''EL "Debian sta""ble" open''SUSE Al''pine CLAU''DE Cla''ude "Open Review"" Questions"; do
    if grep -n "$term" docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md; then
        exit 1
    fi
done
```

Expected: both checks exit 0. The stale-surface sweep should print no matches.

- [ ] **Step 5: Commit the implementation**

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "refactor(install): extract ccs transaction adapter"
```

If earlier tasks already created implementation commits, use this final commit
only for remaining formatting or import cleanup.

## Success Criteria

- `apps/conary/src/commands/install/ccs_transaction.rs` owns direct CCS
  transaction install options, result data, CCS manifest selection helpers,
  hook-status helpers, capability gating, and
  `install_ccs_package_transactionally`.
- `apps/conary/src/commands/install/mod.rs` keeps shared transaction,
  manifest-provides persistence, live-root, dependency, and scriptlet-engine
  ownership.
- Existing caller paths compile through `install/mod.rs` re-exports.
- CCS capability-gate refusal still happens before live-root writes, hooks,
  scriptlets, and DB mutation.
- CCS post-install hook failures still mark the applied changeset as
  `post_hooks_failed` and surface warnings.
- Focused unit filters and medium integration tests pass.
- `scripts/line-count-report.sh 30` shows a reduced `install/mod.rs` line
  count without creating a new large, mixed-responsibility module.
