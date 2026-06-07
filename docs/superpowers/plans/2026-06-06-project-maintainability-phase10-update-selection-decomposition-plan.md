# Project Maintainability Phase 10 Update Selection Decomposition Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. This is the Phase 10 child packet
> under
> `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Extract update candidate selection, source-switch previewing, and
security-update eligibility from the current `update.rs` hotspot into a focused
update submodule without changing update behavior.

**Architecture:** Convert `apps/conary/src/commands/update.rs` into the
directory module `apps/conary/src/commands/update/mod.rs`, then add
`apps/conary/src/commands/update/selection.rs` as the owner for repository
candidate selection, source-switch metadata, security-only metadata checks, and
security marker rendering. Keep pin/unpin/list commands, update orchestration,
delta/full package execution, collection update orchestration, replatform
previewing, and adopted-package authority policy in `update/mod.rs` for this
slice.

**Tech Stack:** Rust, existing Conary command modules, existing
`conary_core::repository` selection APIs, existing SQLite-backed unit fixtures,
existing cargo tests, docs-audit scripts.

---

## Status

Draft plan for external review.

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
- `docs/superpowers/plans/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md`
- `docs/superpowers/plans/2026-06-06-project-maintainability-phase9-ccs-payload-paths-decomposition-plan.md`
- `apps/conary/src/commands/mod.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/tests/cli_daily_ux.rs`
- `apps/conary/tests/native_pm_live_root.rs`

## Design Summary

After Phase 9, `apps/conary/src/commands/update.rs` is the largest Rust source
file in the workspace. The first coherent owner inside it is update selection:
how an installed trove maps to a repository candidate, whether selection may
switch source authorities under latest mode, and how security-only updates
refuse unsupported advisory metadata before mutation.

This slice should not split update execution. The delta/full download path,
changeset creation, replay preflight, package installation, pin/unpin/list
commands, collection update orchestration, and adopted-package authority policy
remain in `update/mod.rs`. Adopted update policy has a separate, clean cluster
and should be a later Phase 11 candidate once selection has its own module.

The Rust module conversion is intentional. Rust cannot load both
`apps/conary/src/commands/update.rs` and
`apps/conary/src/commands/update/mod.rs` for `mod update;`, so the first
implementation checkpoint moves the existing file to `update/mod.rs` before
adding `selection.rs`.

## Current Repo-Grounded Inputs

| Signal | Current value | Phase 10 interpretation |
|--------|---------------|-------------------------|
| Largest Rust files | `apps/conary/src/commands/update.rs` 3334 lines; `apps/conary/src/commands/ccs/install.rs` 3118 lines; `apps/remi/src/server/conversion.rs` 2999 lines; `apps/conary/src/commands/install/mod.rs` 2874 lines | `update.rs` is now the top maintainability hotspot |
| Current update module declaration | `apps/conary/src/commands/mod.rs` has `mod update;` and re-exports `cmd_delta_stats`, `cmd_list_pinned`, `cmd_pin`, `cmd_unpin`, `cmd_update`, `cmd_update_group` | Converting `update.rs` to `update/mod.rs` preserves the public command surface |
| Candidate selection cluster | `is_repo_version_newer`, `trove_version_scheme`, source-distro helpers, `select_update_candidate`, source-switch preview helpers, security marker/helpers | Move into `update/selection.rs` |
| Current adopted-update cluster | `AdoptedUpdateDecision`, `AdoptedUpdateSkip`, native-manager fallback, adopted skip text | Keep in `update/mod.rs`; this is a future slice |
| Docs-audit baseline | 153 tracked doc-like files, 53 corrected rows | Lock-in should add this planning file and update counts to 154 total / 54 corrected |

Evidence commands used to shape this packet:

```bash
scripts/line-count-report.sh 15
rg -n "^(pub |pub\\(|async |fn |impl |struct |enum |const |type )" apps/conary/src/commands/update.rs
rg -n "select_update_candidate|UpdateCandidateSelection|SelectedUpdateCandidate|UpdateSourceSwitch|SecurityMetadataUnavailable|requires_source_switch_confirmation|render_source_switch_preview|adopted_update_decision|AdoptedUpdate" apps/conary/src apps/conary/tests -g '*.rs'
cargo test -p conary --lib latest_mode_update -- --list
cargo test -p conary --lib security_update -- --list
cargo test -p conary --lib selects_debian_update_from_generic_metadata_driven_repo -- --list
cargo test -p conary --lib policy_mode_update_prefers_current_source_candidate -- --list
cargo test -p conary --lib adopted_update -- --list
cargo test -p conary --test cli_daily_ux adopted_update -- --list
cargo test -p conary --test native_pm_live_root security -- --list
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
```

Current filter-discovery results:

| Filter | Current matches |
|--------|-----------------|
| `cargo test -p conary --lib latest_mode_update -- --list` | 4 tests |
| `cargo test -p conary --lib security_update -- --list` | 5 tests |
| `cargo test -p conary --lib selects_debian_update_from_generic_metadata_driven_repo -- --list` | 1 test |
| `cargo test -p conary --lib policy_mode_update_prefers_current_source_candidate -- --list` | 1 test |
| `cargo test -p conary --lib adopted_update -- --list` | 6 tests |
| `cargo test -p conary --lib source_policy_update -- --list` | 2 tests |
| `cargo test -p conary --lib partial_update_failure -- --list` | 1 test |
| `cargo test -p conary --test cli_daily_ux adopted_update -- --list` | 1 test |
| `cargo test -p conary --test native_pm_live_root security -- --list` | 2 tests |

## Module Boundary

Create:

- `apps/conary/src/commands/update/mod.rs`
- `apps/conary/src/commands/update/selection.rs`

Delete by move:

- `apps/conary/src/commands/update.rs`

Move these items from `update/mod.rs` into `selection.rs`:

- `UpdateSourceSwitch`
- `SelectedUpdateCandidate`
- `SecurityMetadataUnavailable`
- `UpdateCandidateSelection`
- `UpdateCandidateSelection::expect` test helper
- `is_repo_version_newer`
- `trove_version_scheme`
- `installed_source_distro`
- `candidate_source_distro`
- `candidate_matches_installed_source`
- `candidate_has_positive_latest_signal`
- `source_switch_reason`
- `select_update_candidate`
- `render_source_switch_preview_line`
- `requires_source_switch_confirmation`
- `render_source_switch_preview_lines`
- `print_source_switch_preview`
- `render_security_update_marker`
- `security_advisory_metadata_text`
- `print_security_metadata_unavailable`
- `security_metadata_unavailable_error`
- direct selection/security tests:
  - `test_is_repo_version_newer_uses_debian_scheme`
  - `test_is_repo_version_newer_uses_arch_scheme`
  - `selects_debian_update_from_generic_metadata_driven_repo`
  - `latest_mode_update_can_switch_sources_when_newest_allowed_candidate_differs`
  - `latest_mode_update_previews_source_switches_in_dry_run`
  - `latest_mode_update_requires_confirmation_for_source_switch_without_yes`
  - `policy_mode_update_prefers_current_source_candidate`
  - `security_update_refuses_unknown_source_metadata_before_mutation`
  - `security_update_refuses_unsupported_source_metadata_before_mutation`
  - `security_update_selects_supported_security_candidate`
  - `security_update_marker_includes_trusted_advisory_details`
  - `security_update_ignores_supported_non_security_candidate`
  - `latest_mode_update_respects_strict_mixing_and_stays_on_current_source`

Move these test fixtures only if they are no longer needed by tests remaining
in `update/mod.rs`:

- `seed_latest_mode_update_fixture`
- `seed_security_update_fixture`

Keep these items in `update/mod.rs` for this slice:

- `read_delta_result_from_cas`
- `resolution_options_for_selected_update`
- `mark_pending_changeset_rolled_back`
- `source_policy_update_context`
- `render_replatform_action_preview`
- `AdoptedUpdateDecision`
- `AdoptedUpdateSkipReason`
- `AdoptedUpdateSkip`
- `adopted_update_decision`
- `native_manager_for_trove`
- `render_adopted_skip_sample`
- `no_update_message`
- `UpdatePackageFailure`
- `update_required_failure_message`
- `PreparedFullUpdate`
- `prepare_full_updates_before_changeset`
- `preflight_prepared_full_update_legacy_replay`
- `install_options_for_update`
- `cmd_pin`
- `cmd_unpin`
- `cmd_list_pinned`
- `cmd_update`
- `cmd_delta_stats`
- `installed_troves_for_update`
- `CollectionUpdateTarget`
- `cmd_update_group`
- command-level and execution tests, including replay, collection, replatform,
  delta, changeset rollback, adopted-update, source-policy context, and
  update-failure-message tests.

`selection.rs` should expose only the parent-module surface needed by
`update/mod.rs`:

```rust
pub(super) struct SelectedUpdateCandidate {
    pub(super) package: RepositoryPackage,
    pub(super) repository: Repository,
    source_switch: Option<UpdateSourceSwitch>,
}

pub(super) enum UpdateCandidateSelection {
    Selected(Box<SelectedUpdateCandidate>),
    NoEligibleUpdate,
    SecurityMetadataUnavailable(SecurityMetadataUnavailable),
}

pub(super) fn select_update_candidate(
    conn: &rusqlite::Connection,
    trove: &Trove,
    security_only: bool,
    policy: &ResolutionPolicy,
    primary_flavor: Option<RepositoryDependencyFlavor>,
) -> Result<UpdateCandidateSelection>;
pub(super) fn render_security_update_marker(package: &RepositoryPackage) -> String;
pub(super) fn print_security_metadata_unavailable(unavailable: &[SecurityMetadataUnavailable]);
pub(super) fn security_metadata_unavailable_error(count: usize) -> String;
pub(super) fn print_source_switch_preview(updates: &[(Trove, SelectedUpdateCandidate)]);
pub(super) fn requires_source_switch_confirmation(
    updates: &[SelectedUpdateCandidate],
    yes: bool,
) -> bool;
```

`UpdateSourceSwitch`, `render_source_switch_preview_line`,
`render_source_switch_preview_lines`, and `security_advisory_metadata_text` can
remain private inside `selection.rs` because the parent module does not call
them directly. Keep any field visibility narrower if the compiler proves the
parent does not need it.

## Non-Goals

- Do not change update candidate ranking, repository selection policy, source
  switching rules, security-advisory support behavior, or security marker
  wording.
- Do not change CLI flags, command-risk classification, live-system mutation
  gates, conaryd APIs, or integration manifests.
- Limit docs changes to active file-path guidance for the new `update/mod.rs`
  and `update/selection.rs` owners.
- Do not move delta update execution, full package update execution, changeset
  status handling, replay preflight, pin/unpin/list commands, collection update
  orchestration, or replatform previewing.
- Do not move adopted-package authority policy in this slice.
- Do not add a new public API outside `commands::update`.
- Do not create behavior-only tests whose expected values differ from current
  behavior; this is a behavior-preserving decomposition.

## Review Focus

Reviewers should check:

- whether `selection.rs` owns a coherent update-selection boundary;
- whether file-to-directory conversion avoids Rust module-discovery conflicts;
- whether visibility stays at `pub(super)` or private rather than `pub(crate)`;
- whether `cmd_update` and `cmd_update_group` still have access to the selected
  package/repository fields they consume;
- whether moved tests carry the required SQLite fixture helpers and imports;
- whether adopted-update policy is intentionally kept for a later slice;
- whether verification covers source switching, security-only refusal, command
  UX, live-root security-update behavior, and import hygiene.

## Tasks

### Task 0: Lock Planning Doc Into The Docs Audit

**Files:**

- Create:
  `docs/superpowers/plans/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the new plan before regenerating inventory**

Run:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md
```

Expected: the new plan is staged so the tracked-file inventory script can see
it.

- [ ] **Step 2: Regenerate docs-audit inventory**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: the inventory includes the Phase 10 plan as a planning/maintainer
row. If another docs file lands first, use the regenerated inventory as the
source of truth and update counts accordingly.

- [ ] **Step 3: Add the ledger row**

Add this literal-tab row near the active maintainability plan rows, after the
Phase 9 row in `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```tsv
docs/superpowers/plans/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md	docs/superpowers/plans/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md	planning	maintainer	maintainability; phase10; update; selection; hotspot-decomposition	apps/conary/src/commands/update.rs; apps/conary/src/commands/mod.rs; apps/conary/tests/cli_daily_ux.rs; apps/conary/tests/native_pm_live_root.rs; docs/modules/source-selection.md; docs/modules/feature-ownership.md	verified	corrected	Added Phase 10 plan for extracting update candidate selection, source-switch previewing, and security-update eligibility into a focused update selection module while preserving update behavior.
```

- [ ] **Step 4: Update the audit summary narrative and counts**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph to the existing `### 2026-06-06 Maintainability Planning` section:

```markdown
The Phase 10 update selection decomposition plan now targets the current
largest source file, `apps/conary/src/commands/update.rs`. It converts the
update command into a directory module and extracts repository candidate
selection, latest-mode source-switch previewing, and security-update metadata
eligibility into `apps/conary/src/commands/update/selection.rs`, while keeping
update execution, delta handling, collection orchestration, replatform
previewing, and adopted-package authority policy in `update/mod.rs`.
```

Then update the final counts from:

```markdown
- Total tracked doc-like files audited: 153
- `verified-no-change`: 13
- `corrected`: 53
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

to:

```markdown
- Total tracked doc-like files audited: 154
- `verified-no-change`: 13
- `corrected`: 54
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its
`claim_clusters`, `evidence_sources`, and notes include the Phase 10 planning
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
154
archived 73
corrected 54
retained-historical 14
verified-no-change 13
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 6: Commit the locked plan**

Run:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv \
    docs/superpowers/documentation-accuracy-audit-inventory.tsv \
    docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan update selection decomposition"
```

Expected: docs-only planning commit succeeds.

### Task 1: Convert The Update Command To A Directory Module

**Files:**

- Move: `apps/conary/src/commands/update.rs` to
  `apps/conary/src/commands/update/mod.rs`
- Verify: `apps/conary/src/commands/mod.rs`

- [ ] **Step 1: Move the file into a directory module**

Run:

```bash
mkdir -p apps/conary/src/commands/update
git mv apps/conary/src/commands/update.rs apps/conary/src/commands/update/mod.rs
```

Expected: `apps/conary/src/commands/update.rs` no longer exists, and
`apps/conary/src/commands/update/mod.rs` contains the former command module.

- [ ] **Step 2: Update the path comment**

At the top of `apps/conary/src/commands/update/mod.rs`, replace:

```rust
// src/commands/update.rs
```

with:

```rust
// src/commands/update/mod.rs
```

Expected: the file path comment matches the new owner path.

- [ ] **Step 3: Confirm `commands/mod.rs` needs no public-surface change**

Verify `apps/conary/src/commands/mod.rs` still has:

```rust
mod update;
```

and still re-exports:

```rust
pub use update::{
    cmd_delta_stats, cmd_list_pinned, cmd_pin, cmd_unpin, cmd_update, cmd_update_group,
};
```

Expected: Rust module discovery now resolves `mod update;` through
`commands/update/mod.rs`, and callers continue to import the same command
functions.

- [ ] **Step 4: Run a compile and one update filter**

Run:

```bash
cargo check -p conary
cargo test -p conary --lib source_policy_update -- --list
```

Expected: `conary` compiles, and the two source-policy update tests are still
discoverable under `commands::update::tests::*`.

- [ ] **Step 5: Commit the module conversion**

Run:

```bash
git add apps/conary/src/commands/update.rs apps/conary/src/commands/update/mod.rs
git commit -m "refactor(update): convert update command to module"
```

Expected: the first code commit contains only the file move and path-comment
update.

### Task 2: Extract Candidate Selection Into `selection.rs`

**Files:**

- Create: `apps/conary/src/commands/update/selection.rs`
- Modify: `apps/conary/src/commands/update/mod.rs`

- [ ] **Step 1: Add the selection submodule and import surface**

In `apps/conary/src/commands/update/mod.rs`, add this near the top after the
module doc comment:

```rust
mod selection;
```

Then add this import block after the existing `use` declarations:

```rust
use selection::{
    SelectedUpdateCandidate, SecurityMetadataUnavailable, UpdateCandidateSelection,
    print_security_metadata_unavailable, print_source_switch_preview,
    render_security_update_marker, requires_source_switch_confirmation,
    security_metadata_unavailable_error, select_update_candidate,
};
```

Expected: `cmd_update` and `cmd_update_group` can keep their current call sites
after the moved functions land.

- [ ] **Step 2: Create `selection.rs` with focused imports**

Create `apps/conary/src/commands/update/selection.rs`:

```rust
// src/commands/update/selection.rs

//! Update candidate selection, source-switch previewing, and security metadata checks.

use anyhow::Result;
use chrono::Utc;
use conary_core::db::models::{
    RepologyCacheEntry, Repository, RepositoryPackage, SecurityAdvisorySupport, Trove,
};
use conary_core::repository::{
    LatestSignal, PackageSelector, SelectionOptions,
    dependency_model::RepositoryDependencyFlavor,
    resolution_policy::{ResolutionPolicy, SelectionMode},
    versioning::{VersionScheme, compare_mixed_repo_versions, resolve_package_version_scheme},
};
use std::cmp::Ordering;
use tracing::{debug, warn};
```

Expected: the new module owns the repository-selection, version-comparison,
Repology signal, and security metadata imports.

- [ ] **Step 3: Move selection types and helpers**

Move the following code block from `update/mod.rs` into `selection.rs`, keeping
function bodies unchanged except for visibility:

- `UpdateSourceSwitch`
- `SelectedUpdateCandidate`
- `SecurityMetadataUnavailable`
- `UpdateCandidateSelection`
- `UpdateCandidateSelection::expect`
- `is_repo_version_newer`
- `trove_version_scheme`
- `installed_source_distro`
- `candidate_source_distro`
- `candidate_matches_installed_source`
- `candidate_has_positive_latest_signal`
- `source_switch_reason`
- `select_update_candidate`
- `render_source_switch_preview_line`
- `requires_source_switch_confirmation`
- `render_source_switch_preview_lines`
- `print_source_switch_preview`
- `render_security_update_marker`
- `security_advisory_metadata_text`
- `print_security_metadata_unavailable`
- `security_metadata_unavailable_error`

Use this visibility pattern as the starting point:

```rust
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateSourceSwitch {
    from_distro: String,
    to_distro: String,
    reason: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct SelectedUpdateCandidate {
    pub(super) package: RepositoryPackage,
    pub(super) repository: Repository,
    source_switch: Option<UpdateSourceSwitch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SecurityMetadataUnavailable {
    package: String,
    repository: String,
    support: SecurityAdvisorySupport,
    candidate_version: String,
}

#[derive(Debug, Clone)]
pub(super) enum UpdateCandidateSelection {
    Selected(Box<SelectedUpdateCandidate>),
    NoEligibleUpdate,
    SecurityMetadataUnavailable(SecurityMetadataUnavailable),
}
```

Then mark these parent-called functions as `pub(super)` while keeping their
existing bodies unchanged:

- `select_update_candidate`
- `requires_source_switch_confirmation`
- `print_source_switch_preview`
- `render_security_update_marker`
- `print_security_metadata_unavailable`
- `security_metadata_unavailable_error`

Keep helper functions private unless the compiler proves the parent module
needs them.

- [ ] **Step 4: Clean up `update/mod.rs` imports**

Remove imports from `update/mod.rs` that are no longer needed outside
`selection.rs`. The compiler should confirm the final set, but these are the
expected removals from the parent module:

```rust
use chrono::Utc;
use conary_core::db::models::{RepologyCacheEntry, SecurityAdvisorySupport};
use conary_core::repository::{
    LatestSignal, PackageSelector, SelectionOptions,
    versioning::{VersionScheme, compare_mixed_repo_versions, resolve_package_version_scheme},
};
use std::cmp::Ordering;
use tracing::debug;
```

Keep parent imports that still feed update execution, command tests, or
remaining helpers, including:

```rust
use conary_core::db::models::{
    DeltaStats, DistroPin, PackageDelta, Repository, RepositoryPackage, SystemAffinity, Trove,
    TroveType,
};
use conary_core::repository::{
    self, DownloadOptions, PackageSource, ResolutionOptions,
    dependency_model::RepositoryDependencyFlavor,
    resolution_policy::ResolutionPolicy,
    resolve_package,
};
```

Expected: `update/mod.rs` no longer imports selection-only repository search
or Repology signal types.

- [ ] **Step 5: Move direct selection and security tests**

Move these tests from the `update/mod.rs` test module into a `#[cfg(test)]`
module at the bottom of `selection.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::{
        CanonicalPackage, InstallSource, RepologyCacheEntry, Repository, RepositoryPackage,
        SecurityAdvisorySupport, Trove, TroveType,
    };
    use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
    use conary_core::repository::resolution_policy::{
        DependencyMixingPolicy, ResolutionPolicy, SelectionMode,
    };

}
```

Move these fixture helpers with the tests:

- `seed_latest_mode_update_fixture`
- `seed_security_update_fixture`

Move these tests with unchanged assertions:

- `test_is_repo_version_newer_uses_debian_scheme`
- `test_is_repo_version_newer_uses_arch_scheme`
- `selects_debian_update_from_generic_metadata_driven_repo`
- `latest_mode_update_can_switch_sources_when_newest_allowed_candidate_differs`
- `latest_mode_update_previews_source_switches_in_dry_run`
- `latest_mode_update_requires_confirmation_for_source_switch_without_yes`
- `policy_mode_update_prefers_current_source_candidate`
- `security_update_refuses_unknown_source_metadata_before_mutation`
- `security_update_refuses_unsupported_source_metadata_before_mutation`
- `security_update_selects_supported_security_candidate`
- `security_update_marker_includes_trusted_advisory_details`
- `security_update_ignores_supported_non_security_candidate`
- `latest_mode_update_respects_strict_mixing_and_stays_on_current_source`

Expected: selection/security tests now resolve under
`commands::update::selection::tests::*`. Tests remaining in `update/mod.rs`
should no longer depend on `seed_latest_mode_update_fixture` or
`seed_security_update_fixture`.

- [ ] **Step 6: Run focused selection tests and compile**

Run:

```bash
cargo test -p conary --lib commands::update::selection::tests
cargo check -p conary
```

Expected: all moved direct selection/security tests pass under the new module
path and `conary` compiles.

- [ ] **Step 7: Commit the selection extraction**

Run:

```bash
git add apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/selection.rs
git commit -m "refactor(update): extract candidate selection"
```

Expected: the second code commit contains the selection module, parent import
cleanup, and direct test relocation.

### Task 3: Preserve Command-Level Update Behavior

**Files:**

- Verify: `apps/conary/src/commands/update/mod.rs`
- Verify: `apps/conary/src/commands/update/selection.rs`
- Verify: `apps/conary/tests/cli_daily_ux.rs`
- Verify: `apps/conary/tests/native_pm_live_root.rs`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update active documentation paths**

Update active guidance that currently points only at the flat update module.

In `docs/llms/subsystem-map.md`, replace:

```markdown
`apps/conary/src/commands/update.rs`, and
```

with:

```markdown
`apps/conary/src/commands/update/mod.rs`,
`apps/conary/src/commands/update/selection.rs`, and
```

In `docs/modules/feature-ownership.md`, update the Package Manager Lifecycle
start-here list by replacing:

```markdown
`apps/conary/src/commands/update.rs`; `apps/conary/src/commands/remove.rs`;
```

with:

```markdown
`apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/remove.rs`;
```

In the Adoption, Unadoption, And Native-Authority Handoff neighbor list in
`docs/modules/feature-ownership.md`, replace:

```markdown
**Neighbor systems:** `apps/conary/src/commands/update.rs`;
```

with:

```markdown
**Neighbor systems:** `apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/selection.rs`;
```

In `docs/modules/source-selection.md`, replace:

```markdown
- `apps/conary/src/commands/update.rs` for source-switching update behavior
```

with:

```markdown
- `apps/conary/src/commands/update/mod.rs` for update command orchestration
- `apps/conary/src/commands/update/selection.rs` for source-switching update
  candidate behavior
```

Expected: active assistant/contributor docs point to the new update module
owners and no longer point readers at the deleted flat `update.rs` path.

- [ ] **Step 2: Refresh docs-audit ledger rows for touched docs**

Update the existing ledger rows for these active docs so their evidence or
notes mention the Phase 10 update module path split:

- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`

Expected: the docs-audit ledger remains current for every active doc modified
by this phase, without changing the final row counts.

- [ ] **Step 3: Run parent-module tests that should remain in `update/mod.rs`**

Run:

```bash
cargo test -p conary --lib commands::update::tests
```

Expected:

- source-policy context tests still run under `commands::update::tests::*`;
- partial update failure text still runs under `commands::update::tests::*`;
- adopted-update authority tests still run under `commands::update::tests::adopted_update_tests::*`.

- [ ] **Step 4: Run update command integration surfaces**

Run:

```bash
cargo test -p conary --test cli_daily_ux adopted_update
cargo test -p conary --test native_pm_live_root security
```

Expected: adopted-update UX and security-update live-root integration behavior
remain unchanged.

- [ ] **Step 5: Check for accidental behavior-boundary movement**

Run:

```bash
rg -n "AdoptedUpdateDecision|AdoptedUpdateSkip|adopted_update_decision|native_manager_for_trove|no_update_message" apps/conary/src/commands/update
rg -n "read_delta_result_from_cas|prepare_full_updates_before_changeset|cmd_update_group|cmd_delta_stats|mark_pending_changeset_rolled_back" apps/conary/src/commands/update
```

Expected:

- adopted-update authority policy remains in `apps/conary/src/commands/update/mod.rs`;
- delta/full update execution, changeset rollback, collection update, and delta
  stats remain in `apps/conary/src/commands/update/mod.rs`;
- `apps/conary/src/commands/update/selection.rs` contains only selection,
  source-switch, and security metadata helpers.

- [ ] **Step 6: Check for stale paths and broad visibility**

Run:

```bash
test ! -e apps/conary/src/commands/update.rs
if rg -n "pub\\(crate\\)|pub fn|pub struct|pub enum" apps/conary/src/commands/update/selection.rs; then
    echo "unexpected broad visibility in update/selection.rs" >&2
    exit 1
fi
if rg -n "LatestSignal|PackageSelector|SelectionOptions|RepologyCacheEntry|compare_mixed_repo_versions|resolve_package_version_scheme" apps/conary/src/commands/update/mod.rs; then
    echo "unexpected selection-only import or helper in update/mod.rs" >&2
    exit 1
fi
rg -n "select_update_candidate|render_security_update_marker|requires_source_switch_confirmation|SecurityMetadataUnavailable" apps/conary/src/commands/update
if rg -n "apps/conary/src/commands/update\\.rs" docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/source-selection.md; then
    echo "unexpected stale update.rs guidance path" >&2
    exit 1
fi
```

Expected:

- the old flat `update.rs` path is gone;
- `selection.rs` does not expose `pub(crate)` or public API;
- selection-only imports do not remain in `update/mod.rs`;
- moved functions appear in `selection.rs`, and parent call sites import them
  through the explicit `use selection` import block from Task 2.
- active guidance no longer points readers at the deleted flat `update.rs`
  path.

- [ ] **Step 7: Commit boundary and docs fixes if needed**

If Task 3 steps required import, visibility, or test-location fixes, commit
them:

```bash
git add apps/conary/src/commands/update/mod.rs \
    apps/conary/src/commands/update/selection.rs \
    docs/llms/subsystem-map.md \
    docs/modules/feature-ownership.md \
    docs/modules/source-selection.md \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "refactor(update): preserve selection callers"
```

Expected: if no code changed after Task 2, skip this commit and record the
passing commands in the final implementation notes.

### Task 4: Final Workspace Verification

**Files:**

- Verify: `apps/conary/src/commands/update/mod.rs`
- Verify: `apps/conary/src/commands/update/selection.rs`
- Verify: `apps/conary/src/commands/mod.rs`
- Verify: `docs/llms/subsystem-map.md`
- Verify: `docs/modules/feature-ownership.md`
- Verify: `docs/modules/source-selection.md`

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
cargo test -p conary --lib commands::update::selection::tests
cargo test -p conary --lib commands::update::tests
cargo test -p conary --test cli_daily_ux adopted_update
cargo test -p conary --test native_pm_live_root security
```

Expected: selection, security, adopted authority, source-policy context,
partial failure text, CLI daily UX, and live-root security-update proof all
pass.

- [ ] **Step 4: Run the full `conary` package test suite**

Run:

```bash
cargo test -p conary
```

Expected: the complete `conary` package test suite passes after the module
split, catching any import, module-discovery, or re-export fallout outside the
focused update filters.

- [ ] **Step 5: Run Clippy for the touched package**

Run:

```bash
cargo clippy -p conary --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 6: Verify hotspot reduction**

Run:

```bash
scripts/line-count-report.sh 15
wc -l apps/conary/src/commands/update/mod.rs apps/conary/src/commands/update/selection.rs
```

Expected: `update/mod.rs` drops by roughly the size of the moved selection
cluster, and `selection.rs` is a focused update-selection module.

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
    apps/conary/src/commands/update/selection.rs \
    docs/llms/subsystem-map.md \
    docs/modules/feature-ownership.md \
    docs/modules/source-selection.md
git commit -m "refactor(update): finish selection split"
```

Expected: no uncommitted code changes remain after the final task.

## Final Verification Before Merge

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::update::selection::tests
cargo test -p conary --lib commands::update::tests
cargo test -p conary --test cli_daily_ux adopted_update
cargo test -p conary --test native_pm_live_root security
cargo test -p conary
cargo clippy -p conary --all-targets -- -D warnings
git diff --check
```

Expected:

- docs-audit inventory count is `154` after plan lock-in;
- docs-audit ledger check passes;
- format, check, focused tests, integration tests, and Clippy all pass;
- `git diff --check` reports no whitespace errors.

## Rollback

If a later task exposes an unexpected regression, revert the Phase 10 commits
in reverse order. Because this slice only moves helper ownership and preserves
the `commands::update` command re-export surface, rollback should restore the
flat `apps/conary/src/commands/update.rs` owner without requiring schema, data,
docs, or CLI migration rollback.
