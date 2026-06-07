# Project Maintainability Phase 12 Update Collection Decomposition Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. This is the Phase 12 child packet
> under
> `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Extract collection update orchestration from
`apps/conary/src/commands/update/mod.rs` into a focused update submodule without
changing update behavior or public command routing.

**Architecture:** Keep the Phase 10/11 update module split and add
`apps/conary/src/commands/update/collection.rs` as the owner for `update
@collection` target modeling, member scanning, adopted-authority handling,
security metadata aggregation, and per-member `cmd_update` dispatch. Keep
single-package update execution, candidate selection, adopted-package authority
policy, pin/list commands, delta/full update preparation, replatform previewing,
and collection management commands in their existing owners.

**Tech Stack:** Rust, existing Conary command modules, existing
`conary_core::db::models::{CollectionMember, Trove}` collection metadata,
existing update `selection` and `adopted_authority` submodules, existing cargo
tests, docs-audit scripts.

---

## Status

Draft plan for local and external review.

## Candidate Choice

Phase 11 deliberately deferred `update/collection.rs` until adopted-update
authority policy had its own module. That precondition is now true, so the next
update-owned refactor should move collection update orchestration before
attempting a larger update execution split.

Alternatives considered:

| Candidate | Trade-off | Decision |
|-----------|-----------|----------|
| `update/collection.rs` | Small, cohesive, continues the Phase 10/11 update lane, and preserves behavior through one focused unit test plus dispatch coverage | Choose for Phase 12 |
| `ccs/install.rs` capability/dependency split | Targets the largest file, but opens a fresh CCS design surface instead of finishing the update slice already prepared by Phase 11 | Defer until this update lane is tidier |
| `update/execution.rs` | Larger line reduction, but touches CAS retrieval, delta/full update preparation, legacy replay preflight, changeset rollback, and transaction admission | Defer; higher blast radius |
| `apps/remi/src/server/conversion.rs` | Important global hotspot, but belongs to a separate Remi-focused maintainability packet | Defer; different subsystem and proof matrix |

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
- `docs/superpowers/plans/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md`
- `apps/conary/src/commands/mod.rs`
- `apps/conary/src/commands/update/mod.rs`
- `apps/conary/src/commands/update/selection.rs`
- `apps/conary/src/commands/update/adopted_authority.rs`
- `apps/conary/src/commands/collection.rs`
- `apps/conary/src/dispatch.rs`
- `apps/conary/tests/query.rs`

## Current Repo-Grounded Inputs

| Signal | Current value | Phase 12 interpretation |
|--------|---------------|-------------------------|
| Current Rust hotspots | `apps/conary/src/commands/ccs/install.rs` 3118 lines; `apps/remi/src/server/conversion.rs` 2999 lines; `apps/conary/src/commands/install/mod.rs` 2874 lines; `crates/conary-core/src/scriptlet/mod.rs` 2408 lines; `apps/conaryd/src/daemon/routes.rs` 2345 lines; `apps/conary/src/commands/update/mod.rs` 2320 lines | `update/mod.rs` is still a top CLI hotspot and now has clearer child-module seams |
| Existing update submodules | `apps/conary/src/commands/update/mod.rs`; `apps/conary/src/commands/update/selection.rs`; `apps/conary/src/commands/update/adopted_authority.rs` | Add one sibling submodule instead of changing global command routing |
| Collection update cluster | `CollectionUpdateTarget` and `cmd_update_group` in `update/mod.rs` | Move collection-specific target modeling and `update @collection` orchestration together |
| Public command route | `apps/conary/src/commands/mod.rs` re-exports `cmd_update_group`; `apps/conary/src/dispatch.rs` calls `commands::cmd_update_group` for `update @collection` | Preserve the public route by re-exporting `collection::cmd_update_group` from `update/mod.rs` |
| Existing collection management owner | `apps/conary/src/commands/collection.rs` owns `conary collection create/list/show/add/remove/delete/install` | Do not move `update @collection` there; it remains update behavior, not collection CRUD |
| Current focused tests | `cargo test -p conary --lib collection_update -- --list` matches 1 unit test; `cargo test -p conary --test query update_collection -- --list` matches 1 dispatch test | Move the unit test into `update::collection::tests` and keep the dispatch test as an integration gate |
| Docs-audit baseline | 155 tracked doc-like files, 55 corrected rows | Lock-in should add one planning file and update counts to 156 total / 56 corrected |

Evidence commands used to shape this packet:

```bash
git status --short --branch
git rev-parse HEAD origin/main
scripts/line-count-report.sh 30
find apps/conary/src/commands/update -maxdepth 1 -type f | sort
rg -n "CollectionUpdateTarget|cmd_update_group|collection_update|update_group|member|updates_to_apply|not_installed|adopted_updates_skipped|security_metadata_unavailable" apps/conary/src/commands/update/mod.rs apps/conary/tests -g '*.rs'
rg -n "cmd_update_group|update_group|collection update|collection_update|Collection" apps/conary/src apps/conary/tests docs -g '*.rs' -g '*.md'
cargo test -p conary --lib collection_update -- --list
cargo test -p conary --test query update_collection -- --list
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
```

Current filter-discovery results:

| Filter | Current matches |
|--------|-----------------|
| `cargo test -p conary --lib collection_update -- --list` | 1 test: `commands::update::tests::collection_update_preserves_member_variant_selector` |
| `cargo test -p conary --test query update_collection -- --list` | 1 test: `update_collection_refuses_installed_variant_selectors` |

## Module Boundary

Create:

- `apps/conary/src/commands/update/collection.rs`

Modify:

- `apps/conary/src/commands/update/mod.rs`
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

Move these items from `update/mod.rs` into `update/collection.rs`:

- `CollectionUpdateTarget`
- `impl CollectionUpdateTarget`
- `cmd_update_group`
- direct collection update test:
  - `collection_update_preserves_member_variant_selector`

Keep these items in `update/mod.rs` for this slice:

- `cmd_update`
- `installed_troves_for_update`
- `UpdatePackageFailure`
- `PreparedFullUpdate`
- `read_delta_result_from_cas`
- `resolution_options_for_selected_update`
- `mark_pending_changeset_rolled_back`
- `update_required_failure_message`
- `preflight_prepared_full_update_legacy_replay`
- `install_options_for_update`
- `cmd_delta_stats`
- `cmd_pin`
- `cmd_unpin`
- `cmd_list_pinned`
- source-policy and replatform preview helpers
- delta/full update preparation and execution
- legacy replay preflight and rollback handling
- selector, delta, replay, replatform, pin/list, and source-policy tests.

Keep these items in other owners:

- Update candidate selection stays in
  `apps/conary/src/commands/update/selection.rs`.
- Adopted-package native-authority policy stays in
  `apps/conary/src/commands/update/adopted_authority.rs`.
- `conary collection ...` management commands stay in
  `apps/conary/src/commands/collection.rs`.

`collection.rs` should expose only the public command entrypoint needed by
`commands/mod.rs` through `update/mod.rs`:

```rust
pub async fn cmd_update_group(
    name: &str,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    dep_mode: Option<DepMode>,
    yes: bool,
    legacy_replay: LegacyReplayOptions,
) -> Result<()>;
```

`CollectionUpdateTarget` should stay private to `collection.rs`.

## Non-Goals

- Do not change `update @collection` behavior, output text, dry-run behavior,
  security-only behavior, adopted-package skip behavior, or per-member update
  ordering.
- Do not change single-package `cmd_update` execution.
- Do not move `cmd_update` into `collection.rs`.
- Do not move collection CRUD/install commands from
  `apps/conary/src/commands/collection.rs`.
- Do not change CLI parsing or dispatch beyond preserving the existing
  `commands::cmd_update_group` route.
- Do not change database schema, repository selection rules, live-system
  mutation gates, legacy replay policy, or package install behavior.

## Risks And Checks

| Risk | Mitigation |
|------|------------|
| Public re-export breakage for `commands::cmd_update_group` | Add `pub use collection::cmd_update_group;` in `update/mod.rs` and keep the child function `pub` |
| Child module accidentally reaching through parent imports | Import direct owners in `collection.rs`: `super::super::install`, `super::super::{open_db, SandboxMode, LegacyReplayOptions}`, `super::selection`, and `super::adopted_authority` |
| `collection.rs` conceptual confusion with `commands/collection.rs` | Document that `commands/collection.rs` owns collection CRUD/install, while `update/collection.rs` owns `update @collection` orchestration |
| Security metadata or adopted authority drift | Keep using the Phase 10/11 child module APIs without changing their signatures |
| Per-member variant selector regression | Move and run `collection_update_preserves_member_variant_selector` under the new module |
| Dispatch selector regression | Keep `apps/conary/tests/query.rs::update_collection_refuses_installed_variant_selectors` in the focused gate |

---

## Task 0: Register The Phase 12 Plan In Docs Audit

**Files:**
- Create: `docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Confirm clean synced baseline**

Run:

```bash
git status --short --branch
git rev-parse HEAD origin/main
```

Expected:

- `git status --short --branch` shows `## main...origin/main` and no tracked
  worktree changes before the plan lock-in starts.
- Both `git rev-parse` lines print the same SHA.

- [ ] **Step 2: Stage the new plan before regenerating inventory**

Run:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected:

- The inventory includes the new Phase 12 plan file.
- Tracked doc-like files grow from 155 to 156 after the new plan is staged.

- [ ] **Step 3: Add the Phase 12 ledger row**

In `docs/superpowers/documentation-accuracy-audit-ledger.tsv`, locate the exact
Phase 11 row by searching for
`phase11-update-adopted-authority-decomposition-plan.md` and insert the Phase 12
row immediately after it. Use literal tab characters:

```tsv
docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md	docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md	planning	maintainer	maintainability; phase12; update; collection-update; hotspot-decomposition	apps/conary/src/commands/update/mod.rs; apps/conary/src/commands/update/collection.rs; apps/conary/src/commands/update/selection.rs; apps/conary/src/commands/update/adopted_authority.rs; apps/conary/src/commands/mod.rs; apps/conary/src/dispatch.rs; apps/conary/tests/query.rs; docs/modules/source-selection.md; docs/modules/feature-ownership.md	verified	corrected	Added Phase 12 plan for extracting update collection target modeling and update @collection orchestration into a focused update submodule while preserving public command re-exports, adopted-authority handling, security metadata refusal, and per-member update behavior.
```

- [ ] **Step 4: Update the audit summary and counts**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph to the existing `### 2026-06-06 Maintainability Planning` section:

```markdown
The Phase 12 update collection decomposition plan continues reducing
`apps/conary/src/commands/update/mod.rs` after the selection and adopted
authority splits. It extracts collection update target modeling and
`update @collection` orchestration into
`apps/conary/src/commands/update/collection.rs`, while keeping single-package
update execution, candidate selection, adopted-package authority policy,
collection management commands, delta/full update preparation, and legacy
replay preflight in their existing owners.
```

Then update the final counts from:

```markdown
- Total tracked doc-like files audited: 155
- `verified-no-change`: 13
- `corrected`: 55
- `archived`: 73
- `retained-historical`: 14
```

to:

```markdown
- Total tracked doc-like files audited: 156
- `verified-no-change`: 13
- `corrected`: 56
- `archived`: 73
- `retained-historical`: 14
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes include the Phase 12 planning update.

- [ ] **Step 5: Verify docs-audit health**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected:

- Inventory count is `156`.
- Ledger distribution is:

```text
archived 73
corrected 56
retained-historical 14
verified-no-change 13
```

- Ledger check passes.

- [ ] **Step 6: Commit the reviewed plan lock-in**

Run:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md
git diff --cached --check
git commit -m "docs: plan update collection decomposition"
```

Expected:

- Diff hygiene passes.
- Commit records only the docs-audit lock-in and new Phase 12 plan.

---

## Task 1: Extract Collection Update Orchestration

**Files:**
- Create: `apps/conary/src/commands/update/collection.rs`
- Modify: `apps/conary/src/commands/update/mod.rs`

- [ ] **Step 1: Add the collection submodule and public re-export**

In `apps/conary/src/commands/update/mod.rs`, add `mod collection;` next to the
existing update child modules, then publicly re-export the command entrypoint:

```rust
mod adopted_authority;
mod collection;
mod selection;

pub use collection::cmd_update_group;
```

Expected:

- `apps/conary/src/commands/mod.rs` can keep this existing re-export
  unchanged:

```rust
pub use update::{
    cmd_delta_stats, cmd_list_pinned, cmd_pin, cmd_unpin, cmd_update, cmd_update_group,
};
```

- `apps/conary/src/dispatch.rs` can keep calling `commands::cmd_update_group`
  unchanged.

- [ ] **Step 2: Create `update/collection.rs` with the direct import surface**

Create `apps/conary/src/commands/update/collection.rs` with this path comment,
module doc, and import block:

```rust
// src/commands/update/collection.rs

//! Collection update orchestration for `conary update @collection`.

use super::adopted_authority::{
    AdoptedUpdateDecision, adopted_update_decision, native_manager_for_trove,
};
use super::selection::{
    SecurityMetadataUnavailable, UpdateCandidateSelection, print_security_metadata_unavailable,
    security_metadata_unavailable_error, select_update_candidate,
};
use super::super::install::{DepMode, resolve_default_dep_mode_from_model};
use super::super::{LegacyReplayOptions, SandboxMode, open_db};
use super::cmd_update;
use anyhow::Result;
use conary_core::db::models::{CollectionMember, Trove, TroveType};
use conary_core::packages::SystemPackageManager;
use conary_core::repository::resolution_policy::RequestScope;
use tracing::info;
```

If `cargo fmt` reorders these imports, keep the formatted ordering.

- [ ] **Step 3: Move the private collection target type**

Move `CollectionUpdateTarget` and its `impl` from `update/mod.rs` into
`update/collection.rs`. Keep it private:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct CollectionUpdateTarget {
    name: String,
    version: String,
    architecture: Option<String>,
}

impl CollectionUpdateTarget {
    fn from_trove(trove: &Trove) -> Self {
        Self {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
        }
    }

    fn display(&self) -> String {
        match self.architecture.as_deref() {
            Some(arch) => format!("{} {} [{}]", self.name, self.version, arch),
            None => format!("{} {}", self.name, self.version),
        }
    }
}
```

Expected:

- `CollectionUpdateTarget` has no import or re-export from `update/mod.rs`.
- `rg -n "CollectionUpdateTarget" apps/conary/src/commands/update/mod.rs`
  returns no matches after the move.

- [ ] **Step 4: Move `cmd_update_group` into `collection.rs`**

Move the full `cmd_update_group` function body from `update/mod.rs` into
`update/collection.rs`.

Use this signature in the new module:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn cmd_update_group(
    name: &str,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    dep_mode: Option<DepMode>,
    yes: bool,
    legacy_replay: LegacyReplayOptions,
) -> Result<()>
```

The moved body should begin with this consolidated opening block, adjusted for
direct imports:

```rust
info!("Updating collection: {}", name);
let requested_dep_mode = dep_mode;
let effective_dep_mode = requested_dep_mode.unwrap_or_else(resolve_default_dep_mode_from_model);
let conn = open_db(db_path)?;
let effective_source_policy =
    conary_core::repository::load_effective_policy(&conn, RequestScope::Any)?;
let policy = effective_source_policy.resolution;
let primary_flavor = effective_source_policy.primary_flavor;

let troves = Trove::find_by_name(&conn, name)?;
let collection = troves
    .iter()
    .find(|t| t.trove_type == TroveType::Collection)
    .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

let collection_id = collection
    .id
    .ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;
let members = CollectionMember::find_by_collection(&conn, collection_id)?;
```

After this block, keep the existing `if members.is_empty()` branch and
subsequent member-scan/update body unchanged except for direct import paths.
Shorten fully qualified paths that now match direct imports:

- `conary_core::db::models::Trove::find_by_name` to `Trove::find_by_name`
- `conary_core::db::models::TroveType::Collection` to `TroveType::Collection`
- `conary_core::db::models::TroveType::Package` to `TroveType::Package`
- `conary_core::db::models::CollectionMember::find_by_collection` to
  `CollectionMember::find_by_collection`
- `conary_core::repository::resolution_policy::RequestScope::Any` to
  `RequestScope::Any`

Keep `conary_core::repository::load_effective_policy` fully qualified because it
is used once.

Keep these behavior-sensitive details unchanged:

- `requested_dep_mode` and `effective_dep_mode` handling;
- `SystemPackageManager::detect()` fallback detection;
- pinned package skip output;
- adopted package skip and critical-block output;
- `security_only` metadata enforcement;
- `drop(conn)` before calling `cmd_update` per target;
- per-member `cmd_update(...).await` invocation;
- final updated/failed summary and error.

- [ ] **Step 5: Clean up `update/mod.rs` after the move**

Remove the moved `CollectionUpdateTarget` and `cmd_update_group` definitions
from `apps/conary/src/commands/update/mod.rs`.

Then clean up parent imports:

```diff
-use conary_core::db::models::{
-    DeltaStats, DistroPin, PackageDelta, Repository, RepositoryPackage, SystemAffinity, Trove,
-    TroveType,
-};
+use conary_core::db::models::{
+    DeltaStats, DistroPin, PackageDelta, Repository, RepositoryPackage, SystemAffinity, Trove,
+};
```

Keep these parent imports because `cmd_update` still uses them:

```rust
use conary_core::packages::{PackageFormat, SystemPackageManager};
use selection::{
    SecurityMetadataUnavailable, SelectedUpdateCandidate, UpdateCandidateSelection,
    print_security_metadata_unavailable, print_source_switch_preview,
    render_security_update_marker, requires_source_switch_confirmation,
    security_metadata_unavailable_error, select_update_candidate,
};
```

Do not remove `SystemPackageManager`; `cmd_update` still calls
`SystemPackageManager::detect()`.

Do not remove `PackageFormat`; `cmd_update` still needs the trait for
`CcsPackage::parse(...)`.

- [ ] **Step 6: Move the direct collection update unit test**

Move `collection_update_preserves_member_variant_selector` out of the parent
`#[cfg(test)] mod tests` in `update/mod.rs` and into a `#[cfg(test)]` module at
the bottom of `update/collection.rs`.

Use this test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use crate::commands::{LegacyReplayOptions, SandboxMode};
    use conary_core::db::models::{
        CollectionMember, InstallSource, Repository, RepositoryPackage, Trove, TroveType,
    };
    use rusqlite::Connection;

    #[tokio::test]
    async fn collection_update_preserves_member_variant_selector() {
        let (_temp, db_path) = create_test_db();
        let conn = Connection::open(&db_path).unwrap();

        let mut repo = Repository::new(
            "variant-repo".to_string(),
            "https://example.test/variant".to_string(),
        );
        repo.gpg_check = false;
        repo.gpg_strict = false;
        repo.default_strategy_distro = Some("fedora-44".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut collection = Trove::new(
            "base".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
        );
        let collection_id = collection.insert(&conn).unwrap();
        CollectionMember::new(collection_id, "demo".to_string())
            .insert(&conn)
            .unwrap();

        for arch in ["x86_64", "aarch64"] {
            let mut installed = Trove::new_with_source(
                "demo".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::Repository,
            );
            installed.architecture = Some(arch.to_string());
            installed.source_distro = Some("fedora-44".to_string());
            installed.version_scheme = Some("rpm".to_string());
            installed.installed_from_repository_id = Some(repo_id);
            installed.insert(&conn).unwrap();

            let mut candidate = RepositoryPackage::new(
                repo_id,
                "demo".to_string(),
                "1.0.1".to_string(),
                format!("sha256:demo-{arch}"),
                123,
                format!("https://example.test/variant/demo-1.0.1-{arch}.ccs"),
            );
            candidate.architecture = Some(arch.to_string());
            candidate.distro = Some("fedora-44".to_string());
            candidate.version_scheme = Some("rpm".to_string());
            candidate.insert(&conn).unwrap();
        }
        drop(conn);

        let result = cmd_update_group(
            "base",
            &db_path,
            "/",
            false,
            true,
            false,
            SandboxMode::None,
            None,
            true,
            LegacyReplayOptions::default(),
        )
        .await;

        assert!(
            result.is_ok(),
            "collection update should preserve member variant selectors: {:?}",
            result
        );
    }
}
```

After removing the test from `update/mod.rs`, remove `CollectionMember` from the
parent test module import list if it is no longer used there:

```diff
-        Changeset, ChangesetStatus, CollectionMember, DistroPin, InstallSource, PackageDelta,
+        Changeset, ChangesetStatus, DistroPin, InstallSource, PackageDelta,
```

- [ ] **Step 7: Run focused compile and collection tests**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::update::collection::tests
cargo test -p conary --lib collection_update
cargo test -p conary --test query update_collection
```

Expected:

- Formatting check passes.
- `cargo check -p conary` passes.
- `commands::update::collection::tests` runs 1 test.
- `collection_update` runs 1 test under the new module path.
- `query update_collection` runs 1 integration test.

- [ ] **Step 8: Commit the code extraction**

Run:

```bash
git add apps/conary/src/commands/update/mod.rs \
  apps/conary/src/commands/update/collection.rs
git diff --cached --check
git commit -m "refactor(update): extract collection update orchestration"
```

Expected:

- Commit contains only the Rust extraction.
- No docs-audit files are included in this code commit.

---

## Task 2: Route Documentation To The New Collection Update Owner

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Update assistant and feature-owner path lists**

In `docs/llms/subsystem-map.md`, update the source-selection/update bullet so
the update path list includes the new collection owner:

```markdown
  `apps/conary/src/commands/update/mod.rs`,
  `apps/conary/src/commands/update/selection.rs`,
  `apps/conary/src/commands/update/adopted_authority.rs`,
  `apps/conary/src/commands/update/collection.rs`, and
  `apps/conary/src/commands/model.rs`
```

In `docs/modules/feature-ownership.md`, update both update path lists so they
include:

```markdown
`apps/conary/src/commands/update/collection.rs`;
```

Keep the existing `update/mod.rs`, `update/selection.rs`, and
`update/adopted_authority.rs` entries.

- [ ] **Step 2: Update source-selection reading guidance**

In `docs/modules/source-selection.md`, add the new owner to "Where To Read
Next":

```markdown
- `apps/conary/src/commands/update/collection.rs` for `update @collection`
  orchestration, member filtering, and per-member update dispatch
```

Keep:

```markdown
- `apps/conary/src/commands/update/mod.rs` for update command orchestration
- `apps/conary/src/commands/update/selection.rs` for source-switching update
  candidate behavior
- `apps/conary/src/commands/update/adopted_authority.rs` for adopted-update
  native-authority policy
```

- [ ] **Step 3: Refresh docs-audit rows for touched docs**

Update the existing rows for these active docs in
`docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

Required row changes:

- add `update-collection` to `claim_clusters` where the row already has
  `update-selection` or `update-adopted-authority`;
- add `apps/conary/src/commands/update/collection.rs` to `evidence_sources`;
- mention Phase 12 update collection ownership in `notes`.

- [ ] **Step 4: Refresh inventory after docs updates**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected:

- Inventory count remains `156`.
- Ledger distribution remains:

```text
archived 73
corrected 56
retained-historical 14
verified-no-change 13
```

- Ledger check passes.

- [ ] **Step 5: Commit docs routing**

Run:

```bash
git add docs/llms/subsystem-map.md \
  docs/modules/feature-ownership.md \
  docs/modules/source-selection.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md
git diff --cached --check
git commit -m "docs: route update collection owner"
```

Expected:

- Commit contains only documentation and docs-audit updates.

---

## Task 3: Final Verification And Cleanup

**Files:**
- Verify: `apps/conary/src/commands/update/mod.rs`
- Verify: `apps/conary/src/commands/update/collection.rs`
- Verify: docs touched in Task 2

- [ ] **Step 1: Run focused update and collection tests**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::update::collection::tests
cargo test -p conary --lib commands::update::tests
cargo test -p conary --lib commands::update::selection::tests
cargo test -p conary --lib commands::update::adopted_authority::tests
cargo test -p conary --test query update_collection
```

Expected:

- All commands pass.
- `commands::update::tests` no longer owns the moved collection update test.
- `commands::update::collection::tests` owns the moved collection update test.

- [ ] **Step 2: Run broader CLI package verification**

Run:

```bash
cargo test -p conary
cargo test -p conary --test cli_daily_ux
cargo clippy -p conary --all-targets -- -D warnings
```

Expected:

- Full `conary` test suite passes.
- `cli_daily_ux` tests that involve update or collection behavior pass
  unchanged.
- Clippy passes with zero warnings.

- [ ] **Step 3: Verify public route and moved symbol placement**

Run:

```bash
rg -n "pub use collection::cmd_update_group|mod collection" apps/conary/src/commands/update/mod.rs
rg -n "pub async fn cmd_update_group|struct CollectionUpdateTarget" apps/conary/src/commands/update/collection.rs
rg -n "CollectionUpdateTarget" apps/conary/src/commands/update/mod.rs
rg -n "cmd_update_group" apps/conary/src/commands/mod.rs apps/conary/src/dispatch.rs apps/conary/src/commands/update
rg -n "cmd_update_group|update_group" apps/conary/src/command_risk.rs || echo "no command-risk collection update route -- expected"
rg -n "cmd_update_group|CollectionUpdateTarget" apps/conary/src/commands/update/selection.rs apps/conary/src/commands/update/adopted_authority.rs || echo "selection/adopted authority have no collection orchestration -- expected"
```

Expected:

- `update/mod.rs` declares `mod collection;` and re-exports
  `cmd_update_group`.
- `update/collection.rs` owns `cmd_update_group` and
  `CollectionUpdateTarget`.
- `CollectionUpdateTarget` has no matches in `update/mod.rs`.
- `commands/mod.rs` and `dispatch.rs` keep their existing public route through
  `commands::cmd_update_group`.
- `command_risk.rs` has no `cmd_update_group` route because risk is classified
  from `Commands::Update` before dispatch routes `@collection`.
- `selection.rs` and `adopted_authority.rs` do not own collection orchestration
  symbols.

- [ ] **Step 4: Verify docs routing and docs-audit health**

Run:

```bash
rg -n "update/collection.rs|update-selection|update-adopted-authority|update-collection" docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/source-selection.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected:

- Active docs route update collection work to
  `apps/conary/src/commands/update/collection.rs`.
- Inventory count is `156`.
- Ledger distribution is `73 archived`, `56 corrected`, `14 retained-historical`,
  and `13 verified-no-change`.
- Ledger check passes.

- [ ] **Step 5: Run maintainability and diff hygiene checks**

Run:

```bash
scripts/line-count-report.sh 30
bash scripts/maintainability-drift-report.sh || true
git diff --check
git status --short --branch
```

Expected:

- `apps/conary/src/commands/update/mod.rs` line count drops by roughly the
  moved `CollectionUpdateTarget`, `cmd_update_group`, and direct test span.
- Drift report is advisory only; inspect any hints before pushing.
- Diff hygiene passes.
- Status shows only intended local commits before push.

- [ ] **Step 6: Push and verify synced main**

Run:

```bash
git push
git status --short --branch
git rev-parse HEAD origin/main
git worktree list --porcelain
```

Expected:

- Push succeeds.
- Status shows `## main...origin/main` with no local changes.
- `HEAD` and `origin/main` print the same SHA.
- Worktree list shows the expected `/home/peter/Conary` worktree.

---

## Review Prompt For DeepSeek/Gemini

Use this prompt for an external critical review before lock-in:

```markdown
You are reviewing a repository-grounded refactor implementation plan for Conary.

Repository root: /home/peter/Conary
Plan under review:
docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md

Please perform a critical review against the actual repository code, not just
the prose. Focus on compile hazards, module-boundary mistakes, stale public
routes, missing tests, docs-audit math, and sequencing issues that would make an
agent fail during implementation.

Read first:
- AGENTS.md
- docs/llms/README.md
- docs/llms/subsystem-map.md
- docs/modules/feature-ownership.md
- docs/modules/source-selection.md
- docs/superpowers/plans/2026-06-06-project-maintainability-phase10-update-selection-decomposition-plan.md
- docs/superpowers/plans/2026-06-06-project-maintainability-phase11-update-adopted-authority-decomposition-plan.md
- apps/conary/src/commands/mod.rs
- apps/conary/src/commands/update/mod.rs
- apps/conary/src/commands/update/selection.rs
- apps/conary/src/commands/update/adopted_authority.rs
- apps/conary/src/commands/collection.rs
- apps/conary/src/dispatch.rs
- apps/conary/tests/query.rs

Verify these plan claims:
- current hotspot ranking and the current `update/mod.rs` line count;
- `CollectionUpdateTarget` and `cmd_update_group` are the only direct collection
  update items being moved;
- `cmd_update_group` can move into `apps/conary/src/commands/update/collection.rs`
  while remaining public through `pub use collection::cmd_update_group;` in
  `update/mod.rs`;
- `collection.rs` can call back into `super::cmd_update` without changing
  `cmd_update` visibility;
- import paths in the proposed child module are correct, especially
  `LegacyReplayOptions`, `SandboxMode`, `DepMode`, `open_db`,
  `resolve_default_dep_mode_from_model`, selection helpers, and adopted
  authority helpers;
- `commands/mod.rs` and `dispatch.rs` keep the existing public
  `commands::cmd_update_group` route;
- `apps/conary/src/commands/collection.rs` should remain separate from
  `update @collection` orchestration;
- the focused tests listed by the plan exist and are sufficient for this
  behavior-preserving move;
- docs routes should include `apps/conary/src/commands/update/collection.rs`;
- docs-audit baseline is currently 155 tracked doc-like files and 55 corrected
  rows, and lock-in should become 156 / 56.

Please return:
1. Summary Verdict: Ready / Ready with fixes / Not ready.
2. Critical Findings: issues that would break compile, tests, behavior, or
   docs-audit lock-in.
3. Important Findings: issues that should be fixed before implementation.
4. Minor Findings: polish or clarity improvements.
5. Missing Concerns: relevant files/tests/docs the plan omitted.
6. Suggested Exact Edits: concrete patch-style text for the plan.
7. Verification Commands Run And Results.
8. Claims Verified Against Code.
9. Claims Not Verified.
```

## Local Self-Review Checklist

Before asking for external review, run:

```bash
rg -n "TBD|TODO|fill in|implement later|appropriate|similar to|placeholder" docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md | rg -v "Local Self-Review|rg -n" || true
rg -n "update/collection.rs|cmd_update_group|CollectionUpdateTarget|docs-audit|156|56" docs/superpowers/plans/2026-06-06-project-maintainability-phase12-update-collection-decomposition-plan.md
git diff --check
```

Expected:

- No incomplete planning language.
- The plan consistently uses `update/collection.rs`, `cmd_update_group`,
  `CollectionUpdateTarget`, and 156 / 56 docs-audit math.
- Diff hygiene passes.
