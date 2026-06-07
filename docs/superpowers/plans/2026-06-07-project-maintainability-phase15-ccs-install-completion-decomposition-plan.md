# Phase 15 CCS Install Completion Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose the remaining `apps/conary/src/commands/ccs/install.rs` hotspot into focused CCS install child modules while preserving the `conary ccs install` command behavior, the capability-policy enforcement re-export, and existing payload-path module boundaries.

**Architecture:** Keep `apps/conary/src/commands/ccs/install.rs` as the stable hub module and add child files under `apps/conary/src/commands/ccs/install/`. Move dependency validation, capability policy, component selection, command orchestration, and command test families into focused modules. Leave `apps/conary/src/commands/ccs/payload_paths.rs` as the sibling owner for payload path normalization and symlink safety from Phase 9.

**Tech Stack:** Rust 2024 workspace modules, `anyhow`, `rusqlite`, `tokio`, `conary_core::ccs`, Conary DB models, existing Conary command re-export pattern.

---

## Current Repo Facts

- Baseline SHA before this plan draft: verify with `git rev-parse HEAD`.
- Current hotspot ranking from `scripts/line-count-report.sh 30`:
  - `apps/conary/src/commands/ccs/install.rs` - 3118 lines.
  - `apps/conary/src/commands/install/mod.rs` - 2874 lines.
  - `crates/conary-core/src/scriptlet/mod.rs` - 2408 lines.
  - `apps/conaryd/src/daemon/routes.rs` - 2345 lines.
  - `apps/conary/src/commands/model.rs` - 2260 lines.
- Current CCS command hub: `apps/conary/src/commands/ccs/mod.rs`.
- Current CCS install exports:
  - `pub(crate) use install::enforce_ccs_capability_policy;`
  - `pub use install::{cmd_ccs_install, cmd_ccs_install_with_replay_options};`
- Current Phase 9 payload path owner:
  - `apps/conary/src/commands/ccs/payload_paths.rs`
  - re-exported from `apps/conary/src/commands/ccs/mod.rs` for existing callers.
- Current CCS install unit inventory:
  - `cargo test -p conary --lib commands::ccs::install::tests -- --list`
  - Expected: 28 tests, 0 benchmarks.
- Current docs-audit baseline before locking this plan:
  - Inventory: 158 tracked doc-like files.
  - Ledger categories: `corrected 58`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
  - After lock-in of this plan: 159 tracked doc-like files, `corrected 59`.

## Why This Hotspot

`apps/conary/src/commands/ccs/install.rs` is now the largest Rust source file in the workspace and is mostly splitable without behavior changes. Phase 9 already extracted payload path normalization, so the remaining file naturally separates into:

- CCS install command orchestration.
- CCS install dependency policy.
- CCS capability-policy enforcement.
- CCS component selection.
- CCS install command-flow tests.

## Alternatives Considered

### Option A: Whole-File CCS Install Decomposition

Split `apps/conary/src/commands/ccs/install.rs` into a hub plus focused child modules and move all existing tests into colocated test modules.

**Pros:** Removes the current top hotspot in one `/goal`, keeps public routes stable, and gives each concern a durable owner.

**Cons:** Larger implementation than a narrow helper extraction, and test redistribution needs careful import cleanup.

**Recommendation:** Choose this option.

### Option B: Move Only Dependency Helpers

Extract only dependency validation and the seven dependency tests.

**Pros:** Low risk and fast.

**Cons:** Leaves the file above 2400 lines and forces another CCS install plan immediately.

### Option C: Skip to `install/mod.rs` or Core Scriptlet

Start the next hotspot after `ccs/install.rs`.

**Pros:** Install and scriptlet are important subsystems.

**Cons:** Ignores the current top file after Phase 14 and leaves an already prepared CCS boundary unfinished.

## Non-Goals

- Do not change `conary ccs install` CLI flags, output, dependency semantics, component semantics, capability policy, scriptlet behavior, composefs behavior, database writes, or replay options.
- Do not move or rewrite `apps/conary/src/commands/ccs/payload_paths.rs`.
- Do not change `apps/conary/src/commands/ccs/mod.rs` public re-export names.
- Do not change `apps/conary/src/commands/install/ccs_transaction.rs`.
- Do not add schema migrations.
- Do not add new behavior tests except a temporary compile/list gate if implementation needs one.
- Do not archive this plan during implementation.

## File Structure After Implementation

```text
apps/conary/src/commands/ccs/
  install.rs
  install/
    capability_policy.rs
    command.rs
    command_capability_tests.rs
    command_component_tests.rs
    command_hook_tests.rs
    command_metadata_tests.rs
    command_payload_tests.rs
    command_reinstall_tests.rs
    component_selection.rs
    dependency.rs
    test_support.rs
  payload_paths.rs
```

Every new Rust file created by this plan must start with the repository-standard path comment:

```rust
// src/commands/ccs/install/<file>.rs
```

### `apps/conary/src/commands/ccs/install.rs`

Hub only. It owns module declarations and stable re-exports:

```rust
// src/commands/ccs/install.rs

//! CCS package installation
//!
//! Commands for installing CCS packages with signature verification,
//! dependency checking, and hook execution.

mod capability_policy;
mod command;
mod component_selection;
mod dependency;

#[cfg(test)]
mod command_capability_tests;
#[cfg(test)]
mod command_component_tests;
#[cfg(test)]
mod command_hook_tests;
#[cfg(test)]
mod command_metadata_tests;
#[cfg(test)]
mod command_payload_tests;
#[cfg(test)]
mod command_reinstall_tests;
#[cfg(test)]
mod test_support;

pub(crate) use capability_policy::enforce_ccs_capability_policy;
pub use command::{cmd_ccs_install, cmd_ccs_install_with_replay_options};
```

Keep `install.rs` as a file module. Rust can resolve child modules from the sibling directory `apps/conary/src/commands/ccs/install/`, so this does not require converting `install.rs` into `install/mod.rs`.

### `apps/conary/src/commands/ccs/install/capability_policy.rs`

Owns only:

- `pub(crate) fn enforce_ccs_capability_policy(...) -> Result<()>`

Import surface:

```rust
// src/commands/ccs/install/capability_policy.rs

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::PackageFormat;
```

Keep the inner import currently inside the function:

```rust
use conary_core::capability::policy::{
    CapabilityPolicy, PolicyDecision, infer_linux_capabilities,
};
```

Visibility must remain `pub(crate)` because `apps/conary/src/commands/ccs/mod.rs` re-exports it for `apps/conary/src/commands/install/conversion.rs`.

### `apps/conary/src/commands/ccs/install/dependency.rs`

Owns dependency, provide, and version-constraint policy:

- `fn package_provided_names(...)`
- `pub(super) fn package_self_provides(...)`
- `fn installed_versions_satisfying_constraint(...)`
- `pub(super) fn validate_package_dependency(...)`
- `pub(super) fn validate_incoming_version_against_dependents(...)`
- `fn version_satisfies_constraint(...)`
- `fn installed_package_version_scheme(...)`
- `fn repo_constraint_set_satisfied(...)`
- `fn split_constraint_parts(...)`
- `fn repo_constraint_satisfies(...)`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// src/commands/ccs/install/dependency.rs

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::PackageFormat;
use conary_core::repository::versioning::{
    RepoVersionConstraint, VersionScheme, parse_repo_constraint, repo_version_satisfies,
};
```

Keep helper visibility narrow:

- `package_self_provides`, `validate_package_dependency`, and `validate_incoming_version_against_dependents` are `pub(super)` because `command.rs` calls them.
- Everything else stays private.

Move these seven tests into `dependency.rs`:

- `installed_versions_respect_version_constraints`
- `installed_versions_respect_debian_version_constraints`
- `incoming_version_uses_arch_constraints_for_dependents`
- `incoming_version_cannot_break_installed_dependents`
- `package_dependency_rejects_undeclared_capability_guess`
- `package_dependency_accepts_declared_capability_when_no_exact_package_exists`
- `package_dependency_does_not_hide_exact_package_version_mismatch`

### `apps/conary/src/commands/ccs/install/component_selection.rs`

Owns CCS manifest component selection:

- `pub(super) struct SelectedCcsComponents`
- `impl SelectedCcsComponents`
- `pub(super) fn sorted_available_component_names(...)`
- `pub(super) fn select_ccs_components(...)`

Import surface:

```rust
// src/commands/ccs/install/component_selection.rs

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::components::ComponentType;
use conary_core::packages::traits::PackageFormat;

use crate::commands::install::ComponentSelection;
```

Expected code adjustment:

```rust
impl SelectedCcsComponents {
    pub(super) fn to_install_component_selection(
        &self,
        available_names: &[String],
    ) -> ComponentSelection {
        if self.names.len() == available_names.len()
            && available_names
                .iter()
                .all(|available| self.names.iter().any(|name| name == available))
        {
            return ComponentSelection::All;
        }

        if self.recognized_types.is_empty() {
            return ComponentSelection::All;
        }

        ComponentSelection::Specific(self.recognized_types.clone())
    }
}
```

Use this exact visibility shape. The current command body reads `selected_components.names`, while `recognized_types` is only read by `to_install_component_selection` inside this module:

```rust
#[derive(Debug, Clone)]
pub(super) struct SelectedCcsComponents {
    pub(super) names: Vec<String>,
    recognized_types: Vec<ComponentType>,
}
```

### `apps/conary/src/commands/ccs/install/command.rs`

Owns command entrypoints:

- `pub async fn cmd_ccs_install(...) -> Result<()>`
- `pub async fn cmd_ccs_install_with_replay_options(...) -> Result<()>`

Import surface:

```rust
// src/commands/ccs/install/command.rs

use anyhow::{Context, Result};
use conary_core::ccs::{CcsPackage, TrustPolicy, verify};
use conary_core::packages::traits::PackageFormat;
use std::path::Path;

use super::capability_policy::enforce_ccs_capability_policy;
use super::component_selection::{select_ccs_components, sorted_available_component_names};
use super::dependency::{
    package_self_provides, validate_incoming_version_against_dependents,
    validate_package_dependency,
};
use super::super::payload_paths::validate_ccs_payload_paths;
use crate::commands::open_db;
use crate::commands::install::{
    CcsTransactionInstallOptions, LegacyReplayOptions, install_ccs_package_transactionally,
};
```

Expected path adjustments inside the moved body:

- `super::super::install::LegacyReplayOptions::default()` -> `LegacyReplayOptions::default()`.
- `super::super::install::CcsTransactionInstallOptions` -> `CcsTransactionInstallOptions`.
- `super::super::install::install_ccs_package_transactionally(...)` -> `install_ccs_package_transactionally(...)`.
- Keep `crate::commands::SandboxMode` in signatures unless importing `SandboxMode` makes the signatures cleaner.
- Keep `#[allow(clippy::too_many_arguments)]` on both command functions.
- Keep `PackageFormat` import because `CcsPackage::parse(...)`, `ccs_pkg.name()`, `ccs_pkg.version()`, and `ccs_pkg.files()` rely on the package trait surface in the current code.

### `apps/conary/src/commands/ccs/install/test_support.rs`

Move reusable command-test helpers:

- `pub(super) fn stage_test_boot_assets(root: &std::path::Path)`
- `pub(super) fn seed_test_init_trove(db_path: &str, db_dir: &std::path::Path)`
- `pub(super) fn ccs_init_file() -> (conary_core::ccs::FileEntry, Vec<u8>, String)`

Import these helpers from command test modules with:

```rust
use super::test_support::{ccs_init_file, seed_test_init_trove, stage_test_boot_assets};
```

Only import the helpers each module actually uses.

### Command Test Modules

Use small sibling test modules rather than one large `command_tests.rs`. These are unit-test modules under `crate::commands::ccs::install`, not integration tests.

Common imports per module should be copied from the moved tests, not centralized behind a macro. Keep `use std::collections::HashMap;` only where a module builds package fixtures with `HashMap::from`.

#### `command_metadata_tests.rs`

Move:

- `ccs_install_records_payload_without_direct_live_root_write`
- `ccs_install_strips_special_permission_bits_from_db_metadata`
- `ccs_install_persists_manifest_provides`
- `ccs_install_persists_typed_provide_when_name_collides`
- `ccs_install_registers_metadata_only_package_without_files`
- `ccs_install_records_ldconfig_trigger_for_shared_libraries`

Use:

```rust
use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, seed_test_init_trove, stage_test_boot_assets};
```

#### `command_capability_tests.rs`

Move:

- `ccs_install_persists_capability_declarations`
- `ccs_install_rejects_scriptlet_capabilities_without_enforcement_before_mutation`

Use:

```rust
use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, stage_test_boot_assets};
```

#### `command_component_tests.rs`

Move:

- `ccs_install_respects_manifest_component_selection`
- `ccs_install_skips_post_install_hook_for_devel_only_component_selection`

Use:

```rust
use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{seed_test_init_trove, stage_test_boot_assets};
```

#### `command_hook_tests.rs`

Move:

- `ccs_install_persists_pre_remove_hook`
- `ccs_install_marks_changeset_post_hooks_failed_after_post_install_error`
- `ccs_install_reverts_pre_hook_directories_when_deploy_fails`

Use:

```rust
use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, stage_test_boot_assets};
```

#### `command_payload_tests.rs`

Move:

- `ccs_install_rejects_child_write_beneath_package_symlink`
- `ccs_install_rejects_child_before_package_symlink`
- `ccs_install_persists_usrmerge_payload_under_usr_path`
- `ccs_install_allows_identical_existing_symlink_destination`
- `ccs_install_replaces_existing_leaf_symlink_destination`
- `ccs_install_coalesces_identical_usrmerge_duplicate_files`
- `ccs_install_rejects_conflicting_usrmerge_duplicate_files`

Use:

```rust
use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::stage_test_boot_assets;
```

Note: two tests use local `std::path::PathBuf` imports in their function bodies today. Keep those local imports with the moved tests.

#### `command_reinstall_tests.rs`

Move:

- `ccs_install_reinstall_dry_run_does_not_mutate_db`

Use:

```rust
use std::collections::HashMap;

use super::command::cmd_ccs_install;
```

## Visibility Contract

- `ccs/install.rs` remains the parent module for all new child modules.
- `pub(crate)` is used only for `enforce_ccs_capability_policy`, preserving the current crate-visible re-export used by install conversion.
- `pub` remains only on `cmd_ccs_install` and `cmd_ccs_install_with_replay_options`.
- `pub(super)` is enough for sibling child-module calls because each child module is inside `crate::commands::ccs::install`.
- Private helper functions in a child module remain visible to that child module's own `#[cfg(test)] mod tests`.
- Private items in an ancestor module are visible to descendants, but this plan does not rely on that for production logic. Prefer explicit `pub(super)` for sibling calls.

## Implementation Tasks

### Task 0: Lock In This Plan And Docs-Audit Baseline

**Files:**

- Create: `docs/superpowers/plans/2026-06-07-project-maintainability-phase15-ccs-install-completion-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Verify baseline state**

Run:

```bash
git status --short --branch
git rev-parse HEAD origin/main
scripts/line-count-report.sh 30
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected before staging this plan:

- `git status --short --branch` shows only this untracked plan file.
- Inventory count is `158`.
- Ledger categories include `corrected 58`.
- Ledger check passes.

- [ ] **Step 2: Stage this plan before regenerating inventory**

Run:

```bash
git add docs/superpowers/plans/2026-06-07-project-maintainability-phase15-ccs-install-completion-decomposition-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
```

prints `159`.

- [ ] **Step 3: Add the Phase 15 ledger row**

Insert one new row in `docs/superpowers/documentation-accuracy-audit-ledger.tsv` immediately after the Phase 14 plan row. Use literal tab separators and exactly 9 columns:

| Column | Value |
| --- | --- |
| `doc_path` | `docs/superpowers/plans/2026-06-07-project-maintainability-phase15-ccs-install-completion-decomposition-plan.md` |
| `canonical_source` | `docs/superpowers/plans/2026-06-07-project-maintainability-phase15-ccs-install-completion-decomposition-plan.md` |
| `doc_type` | `planning` |
| `audience` | `maintainer` |
| `tags` | `maintainability; phase15; ccs-install; hotspot-decomposition; capability-policy; dependency-validation` |
| `evidence_sources` | `apps/conary/src/commands/ccs/install.rs; apps/conary/src/commands/ccs/payload_paths.rs; apps/conary/src/commands/ccs/mod.rs; apps/conary/src/commands/install/conversion.rs; apps/conary/src/commands/install/ccs_transaction.rs; apps/conary/tests/component.rs; apps/conary/tests/bundle_replay.rs; apps/conary/tests/conversion_integration.rs; scripts/line-count-report.sh; docs/modules/ccs.md; docs/modules/feature-ownership.md` |
| `verification_state` | `verified` |
| `disposition` | `corrected` |
| `notes` | `Added the Phase 15 CCS install completion decomposition plan to split the remaining CCS install hotspot into command orchestration, dependency validation, component selection, capability policy, and focused command test modules while preserving payload path ownership and public command re-exports.` |

Verify formatting:

```bash
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
```

Expected: no output.

- [ ] **Step 4: Update the audit summary active-planning section**

In `docs/superpowers/documentation-accuracy-audit-summary.md`:

- Add a Phase 15 paragraph after the Phase 14 paragraph.
- Update total tracked doc-like files from `158` to `159`.
- Update corrected rows from `58` to `59`.
- Add this plan path to the audit-summary ledger row evidence if the row explicitly lists active planning packets.

Use this summary paragraph:

```markdown
The Phase 15 CCS install completion decomposition plan targets the current
largest Rust hotspot, `apps/conary/src/commands/ccs/install.rs`. It keeps
`install.rs` as the stable command hub, moves dependency validation,
capability-policy enforcement, component selection, command orchestration, and
the command-flow test families into child modules under
`apps/conary/src/commands/ccs/install/`, and preserves the Phase 9
`payload_paths.rs` owner plus existing public command re-exports.
```

- [ ] **Step 5: Verify docs lock-in**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
```

Expected:

- Inventory count is `159`.
- Ledger categories include `corrected 59`.
- Malformed-row check prints no output.
- Docs-audit ledger check passes.
- Git whitespace check passes.

- [ ] **Step 6: Commit and push the docs-only plan lock-in**

Run:

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan ccs install decomposition"
git push
git status --short --branch
git rev-parse HEAD origin/main
git worktree list --porcelain
```

Expected:

- Commit succeeds.
- Push succeeds.
- `HEAD` equals `origin/main`.
- Only one worktree is listed unless the user has explicitly created another.

### Task 1: Extract Dependency Validation

**Files:**

- Create: `apps/conary/src/commands/ccs/install/dependency.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`

- [ ] **Step 1: Create `dependency.rs`**

Create `apps/conary/src/commands/ccs/install/dependency.rs` with the path comment and imports listed in the file-structure section.

- [ ] **Step 2: Move dependency helpers**

Move these helpers from `apps/conary/src/commands/ccs/install.rs` into `dependency.rs`:

- `package_provided_names`
- `package_self_provides`
- `installed_versions_satisfying_constraint`
- `validate_package_dependency`
- `validate_incoming_version_against_dependents`
- `version_satisfies_constraint`
- `installed_package_version_scheme`
- `repo_constraint_set_satisfied`
- `split_constraint_parts`
- `repo_constraint_satisfies`

Change visibility only for command-callable helpers:

```rust
pub(super) fn package_self_provides(...)
pub(super) fn validate_package_dependency(...)
pub(super) fn validate_incoming_version_against_dependents(...)
```

- [ ] **Step 3: Move dependency tests**

Move the seven dependency tests from the parent `tests` module into `dependency.rs` `#[cfg(test)] mod tests`.

Use:

```rust
#[cfg(test)]
mod tests {
    use super::{
        installed_versions_satisfying_constraint, validate_incoming_version_against_dependents,
        validate_package_dependency,
    };

    // moved tests
}
```

Remove these now-stale parent test imports from `apps/conary/src/commands/ccs/install.rs`:

```rust
use super::installed_versions_satisfying_constraint;
use super::validate_incoming_version_against_dependents;
use super::validate_package_dependency;
```

- [ ] **Step 4: Wire the dependency module**

Add to `apps/conary/src/commands/ccs/install.rs`:

```rust
mod dependency;
```

Temporarily import the moved functions in `install.rs` until command extraction:

```rust
use dependency::{
    package_self_provides, validate_incoming_version_against_dependents,
    validate_package_dependency,
};
```

Remove dependency-only imports from `install.rs`:

```rust
use conary_core::repository::versioning::{
    RepoVersionConstraint, VersionScheme, parse_repo_constraint, repo_version_satisfies,
};
```

- [ ] **Step 5: Verify Task 1**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::ccs::install::dependency::tests
cargo test -p conary --lib installed_versions_respect
cargo test -p conary --lib incoming_version
cargo test -p conary --lib package_dependency
```

Expected:

- Formatting passes.
- `cargo check -p conary` passes.
- The dependency test module passes.
- The legacy name filters still find and pass the moved tests.

- [ ] **Step 6: Commit Task 1**

Run:

```bash
git add apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/install/dependency.rs
git commit -m "refactor(ccs): extract install dependency policy"
```

### Task 2: Extract Capability Policy And Component Selection

**Files:**

- Create: `apps/conary/src/commands/ccs/install/capability_policy.rs`
- Create: `apps/conary/src/commands/ccs/install/component_selection.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`

- [ ] **Step 1: Create `capability_policy.rs`**

Move `enforce_ccs_capability_policy` into `capability_policy.rs` and preserve `pub(crate)` visibility.

Add to `install.rs`:

```rust
mod capability_policy;

pub(crate) use capability_policy::enforce_ccs_capability_policy;
```

The `ccs/mod.rs` re-export must remain unchanged:

```rust
pub(crate) use install::enforce_ccs_capability_policy;
```

- [ ] **Step 2: Create `component_selection.rs`**

Move:

- `SelectedCcsComponents`
- its `impl`
- `sorted_available_component_names`
- `select_ccs_components`

Adjust `SelectedCcsComponents::to_install_component_selection` to use the directly imported `ComponentSelection`.

Expose only what `install.rs` or `command.rs` uses:

```rust
pub(super) struct SelectedCcsComponents {
    pub(super) names: Vec<String>,
    recognized_types: Vec<ComponentType>,
}

impl SelectedCcsComponents {
    pub(super) fn to_install_component_selection(...)
}

pub(super) fn sorted_available_component_names(...)
pub(super) fn select_ccs_components(...)
```

- [ ] **Step 3: Wire component selection temporarily through the parent**

Add to `install.rs`:

```rust
mod component_selection;

use component_selection::{select_ccs_components, sorted_available_component_names};
```

Remove component-only import from `install.rs`:

```rust
use conary_core::components::ComponentType;
```

- [ ] **Step 4: Verify Task 2**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib ccs_install_respects_manifest_component_selection
cargo test -p conary --lib ccs_install_skips_post_install_hook_for_devel_only_component_selection
cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload
cargo test -p conary --lib converted_ccs_install_rejects_child_before_package_symlink
```

Expected:

- Formatting passes.
- `cargo check -p conary` passes.
- Component selection tests still pass by their old name filters.
- Converted install tests still compile through the `enforce_ccs_capability_policy` re-export.

- [ ] **Step 5: Commit Task 2**

Run:

```bash
git add apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/install/capability_policy.rs apps/conary/src/commands/ccs/install/component_selection.rs
git commit -m "refactor(ccs): split install policy helpers"
```

### Task 3: Extract Command Orchestration

**Files:**

- Create: `apps/conary/src/commands/ccs/install/command.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`

- [ ] **Step 1: Create `command.rs`**

Move `cmd_ccs_install` and `cmd_ccs_install_with_replay_options` into `command.rs`.

Preserve both `#[allow(clippy::too_many_arguments)]` attributes.

- [ ] **Step 2: Apply import and path updates**

Use the import surface listed in the `command.rs` file-structure section.

Apply these body updates:

```rust
LegacyReplayOptions::default()
CcsTransactionInstallOptions { ... }
install_ccs_package_transactionally(...)?
```

instead of old `super::super::install::...` paths.

Import sibling helpers through `super::...`:

```rust
use super::capability_policy::enforce_ccs_capability_policy;
use super::component_selection::{select_ccs_components, sorted_available_component_names};
use super::dependency::{
    package_self_provides, validate_incoming_version_against_dependents,
    validate_package_dependency,
};
```

- [ ] **Step 3: Re-export commands from the hub**

In `apps/conary/src/commands/ccs/install.rs`, replace the moved command bodies with:

```rust
mod command;

pub use command::{cmd_ccs_install, cmd_ccs_install_with_replay_options};
```

Remove temporary imports from `install.rs` that were only used by command bodies.

- [ ] **Step 4: Verify parent route stability**

Run:

```bash
rg -n "pub use install::\\{cmd_ccs_install|cmd_ccs_install_with_replay_options|enforce_ccs_capability_policy" apps/conary/src/commands/ccs/mod.rs
rg -n "cmd_ccs_install_with_replay_options|cmd_ccs_install\\(" apps/conary/src -g '*.rs'
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib ccs_install_records_payload_without_direct_live_root_write
cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload
```

Expected:

- `ccs/mod.rs` re-exports are unchanged.
- `dispatch.rs` still reaches `commands::cmd_ccs_install_with_replay_options`.
- `cargo check -p conary` passes.
- One direct CCS install test and one converted install caller test pass.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git add apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/install/command.rs
git commit -m "refactor(ccs): extract install command orchestration"
```

### Task 4: Extract Shared Test Support And Metadata/Capability/Component Tests

**Files:**

- Create: `apps/conary/src/commands/ccs/install/test_support.rs`
- Create: `apps/conary/src/commands/ccs/install/command_metadata_tests.rs`
- Create: `apps/conary/src/commands/ccs/install/command_capability_tests.rs`
- Create: `apps/conary/src/commands/ccs/install/command_component_tests.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`

- [ ] **Step 1: Move shared test helpers**

Move these helpers into `test_support.rs` and mark them `pub(super)`:

- `stage_test_boot_assets`
- `seed_test_init_trove`
- `ccs_init_file`

Add to the hub:

```rust
#[cfg(test)]
mod test_support;
```

- [ ] **Step 2: Move metadata tests**

Create `command_metadata_tests.rs` and move:

- `ccs_install_records_payload_without_direct_live_root_write`
- `ccs_install_strips_special_permission_bits_from_db_metadata`
- `ccs_install_persists_manifest_provides`
- `ccs_install_persists_typed_provide_when_name_collides`
- `ccs_install_registers_metadata_only_package_without_files`
- `ccs_install_records_ldconfig_trigger_for_shared_libraries`

Add to the hub:

```rust
#[cfg(test)]
mod command_metadata_tests;
```

- [ ] **Step 3: Move capability tests**

Create `command_capability_tests.rs` and move:

- `ccs_install_persists_capability_declarations`
- `ccs_install_rejects_scriptlet_capabilities_without_enforcement_before_mutation`

Add to the hub:

```rust
#[cfg(test)]
mod command_capability_tests;
```

- [ ] **Step 4: Move component tests**

Create `command_component_tests.rs` and move:

- `ccs_install_respects_manifest_component_selection`
- `ccs_install_skips_post_install_hook_for_devel_only_component_selection`

Add to the hub:

```rust
#[cfg(test)]
mod command_component_tests;
```

- [ ] **Step 5: Verify Task 4**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::ccs::install::command_metadata_tests
cargo test -p conary --lib commands::ccs::install::command_capability_tests
cargo test -p conary --lib commands::ccs::install::command_component_tests
cargo test -p conary --lib ccs_install_persists_capability_declarations
cargo test -p conary --lib ccs_install_records_payload_without_direct_live_root_write
cargo test -p conary --lib ccs_install_respects_manifest_component_selection
```

Expected:

- Formatting passes.
- `cargo check -p conary` passes.
- New command test modules pass.
- Old test-name filters still find and pass moved tests.

- [ ] **Step 6: Commit Task 4**

Run:

```bash
git add apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/install/test_support.rs apps/conary/src/commands/ccs/install/command_metadata_tests.rs apps/conary/src/commands/ccs/install/command_capability_tests.rs apps/conary/src/commands/ccs/install/command_component_tests.rs
git commit -m "refactor(ccs): split install command tests"
```

### Task 5: Extract Payload, Hook, And Reinstall Tests

**Files:**

- Create: `apps/conary/src/commands/ccs/install/command_payload_tests.rs`
- Create: `apps/conary/src/commands/ccs/install/command_hook_tests.rs`
- Create: `apps/conary/src/commands/ccs/install/command_reinstall_tests.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`

- [ ] **Step 1: Move payload path tests**

Create `command_payload_tests.rs` and move:

- `ccs_install_rejects_child_write_beneath_package_symlink`
- `ccs_install_rejects_child_before_package_symlink`
- `ccs_install_persists_usrmerge_payload_under_usr_path`
- `ccs_install_allows_identical_existing_symlink_destination`
- `ccs_install_replaces_existing_leaf_symlink_destination`
- `ccs_install_coalesces_identical_usrmerge_duplicate_files`
- `ccs_install_rejects_conflicting_usrmerge_duplicate_files`

Add to the hub:

```rust
#[cfg(test)]
mod command_payload_tests;
```

Keep local `PathBuf` imports inside the two symlink-destination tests that already use local imports.

- [ ] **Step 2: Move hook tests**

Create `command_hook_tests.rs` and move:

- `ccs_install_persists_pre_remove_hook`
- `ccs_install_marks_changeset_post_hooks_failed_after_post_install_error`
- `ccs_install_reverts_pre_hook_directories_when_deploy_fails`

Add to the hub:

```rust
#[cfg(test)]
mod command_hook_tests;
```

- [ ] **Step 3: Move reinstall test**

Create `command_reinstall_tests.rs` and move:

- `ccs_install_reinstall_dry_run_does_not_mutate_db`

Add to the hub:

```rust
#[cfg(test)]
mod command_reinstall_tests;
```

- [ ] **Step 4: Remove the old parent test module**

After all tests are moved, `apps/conary/src/commands/ccs/install.rs` must not contain:

```rust
#[cfg(test)]
mod tests {
```

Check:

```bash
rg -n "#\\[cfg\\(test\\)\\]\\s*$|mod tests" apps/conary/src/commands/ccs/install.rs
```

Expected: only the explicit `#[cfg(test)] mod command_*;` and `#[cfg(test)] mod test_support;` declarations remain.

- [ ] **Step 5: Verify Task 5**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::ccs::install::command_payload_tests
cargo test -p conary --lib commands::ccs::install::command_hook_tests
cargo test -p conary --lib commands::ccs::install::command_reinstall_tests
cargo test -p conary --lib ccs_install_rejects_child_before_package_symlink
cargo test -p conary --lib ccs_install_marks_changeset_post_hooks_failed_after_post_install_error
cargo test -p conary --lib ccs_install_reinstall_dry_run_does_not_mutate_db
cargo test -p conary --lib commands::ccs::install -- --list
```

Expected:

- Formatting passes.
- `cargo check -p conary` passes.
- New modules pass.
- Name filters still find moved tests.
- `commands::ccs::install -- --list` still lists 28 tests across child modules.

- [ ] **Step 6: Commit Task 5**

Run:

```bash
git add apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/install/command_payload_tests.rs apps/conary/src/commands/ccs/install/command_hook_tests.rs apps/conary/src/commands/ccs/install/command_reinstall_tests.rs
git commit -m "refactor(ccs): finish install test split"
```

### Task 6: Update Active Docs For New CCS Install Owners

**Files:**

- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update `docs/modules/ccs.md` install ownership**

In the `## Install` section, add a short owner note after the existing install command examples:

```markdown
Implementation routing: `apps/conary/src/commands/ccs/install.rs` is the
stable command hub. Command execution lives in
`apps/conary/src/commands/ccs/install/command.rs`; dependency/version policy
lives in `apps/conary/src/commands/ccs/install/dependency.rs`; component
selection lives in `apps/conary/src/commands/ccs/install/component_selection.rs`;
capability-policy enforcement lives in
`apps/conary/src/commands/ccs/install/capability_policy.rs`; and payload path
normalization remains in `apps/conary/src/commands/ccs/payload_paths.rs`.
```

- [ ] **Step 2: Update `docs/modules/feature-ownership.md` CCS card**

In `## CCS Authoring, Conversion, Install, And Legacy Replay`, expand the `Start here` field so it includes:

```markdown
`apps/conary/src/commands/ccs/install.rs`;
`apps/conary/src/commands/ccs/install/command.rs`;
`apps/conary/src/commands/ccs/install/dependency.rs`;
`apps/conary/src/commands/ccs/install/component_selection.rs`;
`apps/conary/src/commands/ccs/install/capability_policy.rs`;
`apps/conary/src/commands/ccs/payload_paths.rs`;
```

Preserve existing `crates/conary-core/src/ccs/legacy_replay.rs`, `apps/conary/src/commands/ccs/`, `docs/modules/ccs.md`, and `docs/modules/test-fixtures.md` entries.

- [ ] **Step 3: Update `docs/llms/subsystem-map.md` CCS routing**

In the CCS package building/conversion bullet, mention the install child owners compactly:

```markdown
- CCS package building, chunking, verification, conversion, install, and
  fixture proof: [`docs/modules/ccs.md`](../modules/ccs.md);
  install command owners include
  `apps/conary/src/commands/ccs/install/command.rs`,
  `apps/conary/src/commands/ccs/install/dependency.rs`, and
  `apps/conary/src/commands/ccs/payload_paths.rs`
```

- [ ] **Step 4: Refresh ledger rows for touched docs**

Update existing ledger rows for:

- `docs/modules/ccs.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`

Each row should add the new CCS install child paths to `evidence_sources` and mention Phase 15 CCS install decomposition in `notes`. Keep exactly 9 tab-separated columns.

Verify:

```bash
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected: no malformed rows, ledger check passes.

- [ ] **Step 5: Verify Task 6**

Run:

```bash
rg -n "install/command.rs|install/dependency.rs|install/component_selection.rs|install/capability_policy.rs|payload_paths.rs" docs/modules/ccs.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
```

Expected:

- New child paths appear in the three active docs and ledger.
- Inventory remains `159`.
- Ledger remains `corrected 59`.

- [ ] **Step 6: Commit Task 6**

Run:

```bash
git add docs/modules/ccs.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: route ccs install owners"
```

### Task 7: Final Verification And Cleanup

**Files:**

- Verify all changed files.

- [ ] **Step 1: Inspect final module surfaces**

Run:

```bash
rg -n "^(pub |pub\\(|fn |async fn|struct |enum |impl |mod |pub use |#\\[cfg\\(test\\)\\])" apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/install -g '*.rs'
rg -n "pub\\(crate\\)|pub fn|pub struct|pub enum" apps/conary/src/commands/ccs/install -g '*.rs'
```

Expected:

- `install.rs` contains only module declarations and the two re-export lines.
- `command.rs` exposes only the two public command functions.
- `capability_policy.rs` exposes only `pub(crate) fn enforce_ccs_capability_policy`.
- Sibling-callable helpers are `pub(super)`, not `pub(crate)`.

- [ ] **Step 2: Verify focused tests**

Run:

```bash
cargo test -p conary --lib commands::ccs::install::dependency::tests
cargo test -p conary --lib commands::ccs::install::command_capability_tests
cargo test -p conary --lib commands::ccs::install::command_component_tests
cargo test -p conary --lib commands::ccs::install::command_hook_tests
cargo test -p conary --lib commands::ccs::install::command_metadata_tests
cargo test -p conary --lib commands::ccs::install::command_payload_tests
cargo test -p conary --lib commands::ccs::install::command_reinstall_tests
cargo test -p conary --lib commands::ccs::install -- --list
```

Expected:

- Every focused test module passes.
- The full install module list still shows 28 tests.

- [ ] **Step 3: Verify caller and conversion gates**

Run:

```bash
cargo test -p conary --lib ccs_install
cargo test -p conary --lib installed_versions_respect
cargo test -p conary --lib incoming_version
cargo test -p conary --lib package_dependency
cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload
cargo test -p conary --lib converted_ccs_install_rejects_child_before_package_symlink
cargo test -p conary --test component
cargo test -p conary --test bundle_replay
cargo test -p conary --test conversion_integration golden_conversion
```

Expected:

- Legacy filters still find moved tests.
- Converted CCS install path-safety tests still pass through preserved public routes.
- Integration gates pass.

- [ ] **Step 4: Run package and workspace verification**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary
cargo clippy -p conary --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected:

- Formatting, check, tests, and clippy pass.
- If workspace clippy finds an unrelated pre-existing warning outside this phase, record the exact output and still ensure `cargo clippy -p conary --all-targets -- -D warnings` passes.

- [ ] **Step 5: Run maintainability and docs verification**

Run:

```bash
scripts/line-count-report.sh 30
scripts/maintainability-drift-report.sh
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected:

- `apps/conary/src/commands/ccs/install.rs` no longer appears as a large hotspot.
- `install.rs` is a small hub.
- Inventory remains `159`.
- Ledger remains `corrected 59`.
- No malformed ledger rows.
- Docs-audit check passes.
- Git whitespace check passes.

- [ ] **Step 6: Final commit**

If Task 7 verification required cleanup edits, commit them:

```bash
git add apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/install docs/modules/ccs.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "refactor(ccs): complete install module split"
```

If all previous task commits already cover the final state, skip this commit and record that no cleanup commit was needed.

## Review Checklist Before Implementation

- [ ] `install.rs` as a file module with `install/*.rs` children is accepted by Rust module resolution.
- [ ] `ccs/mod.rs` re-export surface remains unchanged.
- [ ] `enforce_ccs_capability_policy` remains crate-visible through `ccs/mod.rs`.
- [ ] `payload_paths.rs` remains the owner of payload path normalization and validation.
- [ ] All 28 existing `commands::ccs::install` tests are assigned to exactly one new module.
- [ ] Shared test helpers move before command-flow tests that need them.
- [ ] `PackageFormat` remains in child modules that call `PackageFormat` trait methods, not the hub.
- [ ] `ComponentType` moves to `component_selection.rs`, not the hub.
- [ ] Version constraint imports move to `dependency.rs`, not the hub.
- [ ] Docs-audit counts move from 158/58 to 159/59 at plan lock-in and stay there during implementation.

## Prompt For External Reviewers

Use this prompt with Gemini or DeepSeek before lock-in:

```markdown
Please critically review `docs/superpowers/plans/2026-06-07-project-maintainability-phase15-ccs-install-completion-decomposition-plan.md` against the current Conary repository.

Focus on whether this plan can be executed by an agent in one `/goal` without behavior changes.

Review priorities:

1. Rust module resolution: `apps/conary/src/commands/ccs/install.rs` remains a file module while child files live under `apps/conary/src/commands/ccs/install/`.
2. Public route preservation: `apps/conary/src/commands/ccs/mod.rs` keeps re-exporting `cmd_ccs_install`, `cmd_ccs_install_with_replay_options`, and `enforce_ccs_capability_policy`.
3. Visibility: `enforce_ccs_capability_policy` stays `pub(crate)`, command functions stay `pub`, and sibling helpers use `pub(super)` only where needed.
4. Import surfaces: `open_db`, `CcsTransactionInstallOptions`, `LegacyReplayOptions`, `install_ccs_package_transactionally`, `ComponentSelection`, `PackageFormat`, versioning imports, `ComponentType`, and `payload_paths` paths all resolve from their proposed child modules.
5. Test redistribution: every one of the 28 existing `commands::ccs::install::tests` tests is assigned to exactly one new module, and shared helpers move before dependents.
6. Docs-audit math: current baseline is 158 tracked docs / 58 corrected rows; plan lock-in should become 159 / 59.
7. Verification gates: focused unit filters, converted CCS install callers, integration tests, package clippy, workspace clippy, docs-audit, and maintainability drift checks are sufficient.

Please return:

- Summary verdict: Ready / Ready with fixes / Not ready
- Critical findings
- Important findings
- Minor findings
- Missing concerns
- Suggested exact edits
- Verification commands you ran and results
- Claims verified against code
- Claims not verified
```
