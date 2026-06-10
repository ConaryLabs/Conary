# Phase 24 Bootstrap Command Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `apps/conary/src/commands/bootstrap/mod.rs`, the current largest Rust hotspot, into focused bootstrap command child modules while preserving every public bootstrap command export, dispatch route, bootstrap-run artifact behavior, and workflow test surface.

**Architecture:** Keep the existing Rust directory-module layout: `apps/conary/src/commands/bootstrap/mod.rs` remains the bootstrap hub, `apps/conary/src/commands/bootstrap/state.rs` stays as the public run-record state owner, and new sibling files under `apps/conary/src/commands/bootstrap/` own setup/status, phase-build commands, image generation, bootstrap-run orchestration, run-record helpers, generation artifact writing, seed commands, convergence commands, and cleanup. The parent hub re-exports all command entry points and `BootstrapRunOptions` so `crate::commands::*` and `crate::commands::bootstrap::*` paths remain stable.

**Tech Stack:** Rust 2024, Cargo workspace, `apps/conary`, `conary-core::bootstrap`, `conary-core::derivation`, `conary-core::generation::artifact`, `rusqlite`, bootstrap workflow integration tests, docs-audit ledger tooling.

---

## Current Repository Facts

- Repository root: `/home/peter/Conary`.
- Current `HEAD` and `origin/main`: `de6b41f6f3e5e5aadea971e87e81afe7b62770d1`.
- Current top hotspots after Phase 23:
  - `apps/conary/src/commands/bootstrap/mod.rs`: 1,946 lines.
  - `crates/conary-core/src/model/replatform.rs`: 1,927 lines.
  - `crates/conary-core/src/resolver/provider/mod.rs`: 1,881 lines.
  - `apps/conary-test/src/engine/runner.rs`: 1,875 lines.
  - `crates/conary-core/src/model/parser.rs`: 1,872 lines.
- Existing bootstrap command layout:
  - `apps/conary/src/commands/bootstrap/mod.rs`: command implementations and bootstrap-run helper functions.
  - `apps/conary/src/commands/bootstrap/state.rs`: `BootstrapRunRecord`, `BootstrapLatestPointer`, and JSON record tests.
- Current public command exports in `apps/conary/src/commands/mod.rs`:
  - `BootstrapRunOptions`
  - `cmd_bootstrap_check`
  - `cmd_bootstrap_clean`
  - `cmd_bootstrap_config`
  - `cmd_bootstrap_cross_tools`
  - `cmd_bootstrap_diff_seeds`
  - `cmd_bootstrap_dry_run`
  - `cmd_bootstrap_guest_profile`
  - `cmd_bootstrap_image`
  - `cmd_bootstrap_init`
  - `cmd_bootstrap_resume`
  - `cmd_bootstrap_run`
  - `cmd_bootstrap_seed`
  - `cmd_bootstrap_seed_adopted`
  - `cmd_bootstrap_status`
  - `cmd_bootstrap_system`
  - `cmd_bootstrap_temp_tools`
  - `cmd_bootstrap_tier2`
  - `cmd_bootstrap_verify_convergence`
- `apps/conary/src/dispatch/bootstrap.rs` calls the command functions through `commands::cmd_bootstrap_*` and constructs `commands::BootstrapRunOptions`.
- Baseline focused tests:
  - `cargo test -p conary --lib commands::bootstrap -- --list` lists exactly 4 tests.
  - `cargo test -p conary --lib commands::bootstrap` passes: 4 passed, 0 failed.
  - `cargo test -p conary --test bootstrap_workflow -- --list` lists exactly 3 tests.
  - `cargo test -p conary --test bootstrap_workflow` passes: 3 passed, 0 failed.
- Baseline docs-audit state:
  - Inventory: 167 tracked files.
  - Ledger counts: `archived 73`, `corrected 68`, `retained-historical 14`, `verified-no-change 12`.
  - `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete` passes.

## Non-Goals

- Do not change `conary bootstrap ...` CLI behavior, output text, argument parsing, dispatch routes, or public command export paths.
- Do not change bootstrap trust posture, `--skip-verify` warning text, source checksum policy, or TOFU documentation.
- Do not change bootstrap-run pipeline semantics, substituter behavior, publish endpoint selection, operation record layout, output symlink layout, generation artifact contract, CAS verification mode, or boot asset staging behavior.
- Do not change convergence comparison semantics or seed diff output.
- Do not change `conary-core::bootstrap` internals, derivation pipeline internals, generation artifact internals, DB schema, or integration manifest behavior.
- Do not add `apps/conary/src/commands/bootstrap.rs`; this command already uses the directory-module layout through `apps/conary/src/commands/bootstrap/mod.rs`.
- Do not remove or privatize `apps/conary/src/commands/bootstrap/state.rs`; it remains a public child module.

## Public Contract To Preserve

These `crate::commands::*` paths must remain usable:

```rust
crate::commands::BootstrapRunOptions
crate::commands::cmd_bootstrap_check
crate::commands::cmd_bootstrap_clean
crate::commands::cmd_bootstrap_config
crate::commands::cmd_bootstrap_cross_tools
crate::commands::cmd_bootstrap_diff_seeds
crate::commands::cmd_bootstrap_dry_run
crate::commands::cmd_bootstrap_guest_profile
crate::commands::cmd_bootstrap_image
crate::commands::cmd_bootstrap_init
crate::commands::cmd_bootstrap_resume
crate::commands::cmd_bootstrap_run
crate::commands::cmd_bootstrap_seed
crate::commands::cmd_bootstrap_seed_adopted
crate::commands::cmd_bootstrap_status
crate::commands::cmd_bootstrap_system
crate::commands::cmd_bootstrap_temp_tools
crate::commands::cmd_bootstrap_tier2
crate::commands::cmd_bootstrap_verify_convergence
```

These module paths must remain usable:

```rust
crate::commands::bootstrap::BootstrapRunOptions
crate::commands::bootstrap::cmd_bootstrap_run
crate::commands::bootstrap::cmd_bootstrap_verify_convergence
crate::commands::bootstrap::state::BootstrapRunRecord
crate::commands::bootstrap::state::BootstrapLatestPointer
```

## Final File Responsibility Map

| File | Responsibility |
| --- | --- |
| `apps/conary/src/commands/bootstrap/mod.rs` | Hub module only: path comment, module docs, `pub mod state`, private child modules, and public re-exports. |
| `apps/conary/src/commands/bootstrap/state.rs` | Existing public bootstrap-run record and latest-pointer JSON state. |
| `apps/conary/src/commands/bootstrap/types.rs` | Public command option types, currently `BootstrapRunOptions`. |
| `apps/conary/src/commands/bootstrap/phases.rs` | Phase-build command entry points: cross-tools, temp-tools, system, config, guest profile, tier2, and skip-verify warning helpers/tests. |
| `apps/conary/src/commands/bootstrap/setup.rs` | Init/check/status/resume/dry-run command entry points and resume routing to phase/image commands. |
| `apps/conary/src/commands/bootstrap/image.rs` | `cmd_bootstrap_image` and image-format output guidance. |
| `apps/conary/src/commands/bootstrap/cleanup.rs` | `cmd_bootstrap_clean` and stage-name path traversal guard. |
| `apps/conary/src/commands/bootstrap/run_record.rs` | Bootstrap-run record creation, success/failure completion, latest-pointer loading, and output symlink management. |
| `apps/conary/src/commands/bootstrap/run_artifact.rs` | Bootstrap-run generation artifact writing, CAS manifest loading, boot asset staging, and initramfs materialization helpers. |
| `apps/conary/src/commands/bootstrap/run.rs` | `cmd_bootstrap_run` derivation pipeline orchestration and generation output publication. |
| `apps/conary/src/commands/bootstrap/seed.rs` | `cmd_bootstrap_seed` and `cmd_bootstrap_seed_adopted`. |
| `apps/conary/src/commands/bootstrap/convergence.rs` | `cmd_bootstrap_verify_convergence` and `cmd_bootstrap_diff_seeds`. |

## Visibility Contract

- All public command entry points remain `pub async fn` inside their child modules and are re-exported from `bootstrap/mod.rs`.
- `types::BootstrapRunOptions` remains `pub` and is re-exported from `bootstrap/mod.rs`.
- `state.rs` remains `pub mod state;`.
- `phases::skip_verify_warning_message` and `phases::print_skip_verify_warning` stay private to `phases.rs`.
- `run_record` helpers should be `pub(super)`:
  - `start_bootstrap_run_record`
  - `finish_bootstrap_run_success`
  - `finish_bootstrap_run_failure`
  - `load_completed_bootstrap_run_record`
- `run_record::link_bootstrap_run_outputs` stays private to `run_record.rs`.
- `run_artifact::write_bootstrap_run_generation_artifact` should be `pub(super)` because `run.rs` calls it.
- `run_artifact` low-level helpers stay private:
  - `write_bootstrap_run_initramfs_source`
  - `materialize_bootstrap_run_initramfs_path`
  - `resolve_bootstrap_run_symlink_target`
  - `normalize_bootstrap_run_relative_path`
  - `architecture_from_target_triple`
  - `load_bootstrap_run_output_manifests`
  - `write_bootstrap_run_boot_asset_source`
- `setup.rs` can route resume through sibling modules using `super::phases::{...}` and `super::image::cmd_bootstrap_image`.
- `convergence.rs` can load latest run records through `super::run_record::load_completed_bootstrap_run_record`.
- Avoid `pub(crate)` unless a current caller outside `commands::bootstrap` needs the item. The only crate-public surfaces should be the existing command exports and `state` module types.

## Test Migration Map

Move each existing `commands::bootstrap::tests::*` test exactly once:

| Test | New owner |
| --- | --- |
| `skip_verify_warning_message_is_prominent` | `bootstrap/phases.rs` |
| `test_bootstrap_run_writes_success_record_with_output_paths` | `bootstrap/run_record.rs` |
| `bootstrap_run_artifact_writer_creates_loadable_generation` | `bootstrap/run_artifact.rs` |

Leave the existing `commands::bootstrap::state::tests::test_bootstrap_run_record_round_trips_json` in `bootstrap/state.rs`.

The integration tests in `apps/conary/tests/bootstrap_workflow.rs` do not move.

---

### Task 0: Lock In The Plan Packet

**Files:**
- Create: `docs/superpowers/plans/2026-06-09-project-maintainability-phase24-bootstrap-command-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

- [ ] **Step 1: Stage the plan before regenerating inventory**

`scripts/docs-audit-inventory.sh` uses `git ls-files`, so stage the new plan file before checking the inventory count.

Run:

```bash
git status --short --branch
git add docs/superpowers/plans/2026-06-09-project-maintainability-phase24-bootstrap-command-decomposition-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected before the ledger row is added:

```text
168
archived 73
corrected 68
retained-historical 14
verified-no-change 12
```

The ledger check is expected to fail until Step 2 adds the row for the staged plan file.

- [ ] **Step 2: Add the docs-audit ledger row**

Append this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```tsv
docs/superpowers/plans/2026-06-09-project-maintainability-phase24-bootstrap-command-decomposition-plan.md	docs/superpowers/plans/2026-06-09-project-maintainability-phase24-bootstrap-command-decomposition-plan.md	planning	maintainer	maintainability; phase24; conary-bootstrap; bootstrap-command; hotspot-decomposition	apps/conary/src/commands/bootstrap/mod.rs; apps/conary/src/commands/bootstrap/; apps/conary/src/dispatch/bootstrap.rs; apps/conary/src/commands/mod.rs; apps/conary/tests/bootstrap_workflow.rs; conary_core::bootstrap; conary_core::derivation; conary_core::generation::artifact; docs/llms/subsystem-map.md; docs/modules/feature-ownership.md; docs/modules/bootstrap.md; docs/ARCHITECTURE.md	verified	corrected	Added the Phase 24 bootstrap command decomposition plan for turning apps/conary/src/commands/bootstrap/mod.rs into a focused command hub plus child modules for setup/status, phase-build commands, image generation, bootstrap-run orchestration, run-record helpers, generation artifact writing, seed commands, convergence commands, and cleanup without changing CLI behavior or public command exports.
```

- [ ] **Step 3: Update the docs-audit summary counts**

Update `docs/superpowers/documentation-accuracy-audit-summary.md` so the final counts report:

```text
- Total tracked doc-like files audited: 168
- `verified-no-change`: 12
- `corrected`: 69
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Add one short summary paragraph near the existing maintainability phase paragraphs:

```markdown
The Phase 24 bootstrap command decomposition plan targets the current largest
Rust hotspot, `apps/conary/src/commands/bootstrap/mod.rs`. It keeps
`bootstrap/mod.rs` as the command hub while planning focused owners for setup
and status commands, phase-build commands, image generation, bootstrap-run
record handling, generation artifact writing, seed commands, convergence
commands, and cleanup.
```

- [ ] **Step 4: Regenerate inventory and verify docs-audit health**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

Expected:

```text
168
archived 73
corrected 69
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
```

The final `diff` command should have no output.

- [ ] **Step 5: Commit plan lock-in**

Run:

```bash
git add docs/superpowers/plans/2026-06-09-project-maintainability-phase24-bootstrap-command-decomposition-plan.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: plan bootstrap command decomposition"
```

---

### Task 1: Extract Public Run Options And Phase Build Commands

**Files:**
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Create: `apps/conary/src/commands/bootstrap/types.rs`
- Create: `apps/conary/src/commands/bootstrap/phases.rs`

- [ ] **Step 1: Add module declarations and re-exports**

At the top of `apps/conary/src/commands/bootstrap/mod.rs`, keep the path comment and module docs, keep `pub mod state;`, then add:

```rust
mod phases;
mod types;

pub use phases::{
    cmd_bootstrap_config, cmd_bootstrap_cross_tools, cmd_bootstrap_guest_profile,
    cmd_bootstrap_system, cmd_bootstrap_temp_tools, cmd_bootstrap_tier2,
};
pub use types::BootstrapRunOptions;
```

- [ ] **Step 2: Create `types.rs`**

Move `BootstrapRunOptions` from `bootstrap/mod.rs` into `types.rs`.

Use this import-free file shape:

```rust
// apps/conary/src/commands/bootstrap/types.rs

/// Options for the `bootstrap run` command.
pub struct BootstrapRunOptions<'a> {
    /// Path to system manifest TOML.
    pub manifest: &'a str,
    /// Working directory for build artifacts.
    pub work_dir: &'a str,
    /// Path to seed directory.
    pub seed: &'a str,
    /// Recipe directory.
    pub recipe_dir: &'a str,
    /// Stop after completing this stage.
    pub up_to: Option<&'a str>,
    /// Only build these packages.
    pub only: Option<&'a [String]>,
    /// Also rebuild reverse dependents of `only` targets.
    pub cascade: bool,
    /// Preserve build logs for successful builds.
    pub keep_logs: bool,
    /// Spawn interactive shell on build failure.
    pub shell_on_failure: bool,
    /// Show verbose build output.
    pub verbose: bool,
    /// Skip remote substituters.
    pub no_substituters: bool,
    /// Auto-publish successful builds.
    pub publish: bool,
}
```

- [ ] **Step 3: Create `phases.rs`**

Move these items into `phases.rs`:

```text
skip_verify_warning_message
print_skip_verify_warning
cmd_bootstrap_cross_tools
cmd_bootstrap_temp_tools
cmd_bootstrap_system
cmd_bootstrap_config
cmd_bootstrap_guest_profile
cmd_bootstrap_tier2
skip_verify_warning_message_is_prominent
```

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/phases.rs

use std::path::Path;

use anyhow::Result;
use conary_core::bootstrap::{Bootstrap, BootstrapConfig};
```

Keep `skip_verify_warning_message` and `print_skip_verify_warning` private. Keep all moved `cmd_bootstrap_*` functions `pub async fn`.

Move the test into `phases.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::skip_verify_warning_message;

    #[test]
    fn skip_verify_warning_message_is_prominent() {
        let warning = skip_verify_warning_message();
        assert!(warning.contains("UNSAFE"));
        assert!(warning.contains("--skip-verify"));
        assert!(warning.contains("placeholder"));
    }
}
```

- [ ] **Step 4: Clean temporary parent imports**

After the move, `bootstrap/mod.rs` should no longer need the phase-only helper functions, but it still needs broad imports for the remaining parent-owned functions. Remove only imports made unused by this task.

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::bootstrap -- --list
cargo test -p conary --lib commands::bootstrap
```

Expected:

```text
4 tests listed
commands::bootstrap: 4 passed
```

- [ ] **Step 5: Commit Task 1**

Run:

```bash
git add apps/conary/src/commands/bootstrap/mod.rs \
  apps/conary/src/commands/bootstrap/types.rs \
  apps/conary/src/commands/bootstrap/phases.rs
git commit -m "refactor(conary): extract bootstrap phase commands"
```

---

### Task 2: Extract Setup, Image, And Cleanup Commands

**Files:**
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Create: `apps/conary/src/commands/bootstrap/setup.rs`
- Create: `apps/conary/src/commands/bootstrap/image.rs`
- Create: `apps/conary/src/commands/bootstrap/cleanup.rs`

- [ ] **Step 1: Add module declarations and re-exports**

Add to `bootstrap/mod.rs`:

```rust
mod cleanup;
mod image;
mod setup;

pub use cleanup::cmd_bootstrap_clean;
pub use image::cmd_bootstrap_image;
pub use setup::{
    cmd_bootstrap_check, cmd_bootstrap_dry_run, cmd_bootstrap_init, cmd_bootstrap_resume,
    cmd_bootstrap_status,
};
```

- [ ] **Step 2: Create `setup.rs`**

Move these items into `setup.rs`:

```text
cmd_bootstrap_init
cmd_bootstrap_check
cmd_bootstrap_status
cmd_bootstrap_resume
cmd_bootstrap_dry_run
```

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/setup.rs

use std::path::PathBuf;

use anyhow::{Context, Result};
use conary_core::bootstrap::{Bootstrap, BootstrapConfig, BootstrapStage, Prerequisites, TargetArch};

use super::image::cmd_bootstrap_image;
use super::phases::{
    cmd_bootstrap_config, cmd_bootstrap_cross_tools, cmd_bootstrap_system,
    cmd_bootstrap_temp_tools, cmd_bootstrap_tier2,
};
```

Keep the `cmd_bootstrap_resume` routing behavior byte-for-byte except for sibling paths. It should call the imported sibling command functions directly.

- [ ] **Step 3: Create `image.rs`**

Move `cmd_bootstrap_image` into `image.rs`.

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/image.rs

use std::str::FromStr;

use anyhow::{Context, Result};
use conary_core::bootstrap::{
    Bootstrap, BootstrapConfig, ImageBuilder, ImageFormat, ImageSize, ImageTools,
};
```

- [ ] **Step 4: Create `cleanup.rs`**

Move `cmd_bootstrap_clean` into `cleanup.rs`.

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/cleanup.rs

use std::path::PathBuf;

use anyhow::Result;
```

Preserve the `ALLOWED_STAGES` guard inside `cmd_bootstrap_clean`.

- [ ] **Step 5: Remove old parent imports made obsolete by setup/image/cleanup**

After this task, `bootstrap/mod.rs` should no longer import:

```rust
Bootstrap
BootstrapConfig
BootstrapStage
ImageBuilder
ImageFormat
ImageSize
ImageTools
Prerequisites
TargetArch
std::str::FromStr
```

unless a still-parented item genuinely uses one of them.

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::bootstrap
cargo test -p conary --test bootstrap_workflow
```

Expected:

```text
commands::bootstrap: 4 passed
bootstrap_workflow: 3 passed
```

- [ ] **Step 6: Commit Task 2**

Run:

```bash
git add apps/conary/src/commands/bootstrap/mod.rs \
  apps/conary/src/commands/bootstrap/setup.rs \
  apps/conary/src/commands/bootstrap/image.rs \
  apps/conary/src/commands/bootstrap/cleanup.rs
git commit -m "refactor(conary): extract bootstrap setup commands"
```

---

### Task 3: Extract Bootstrap-Run Record And Artifact Helpers

**Files:**
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Create: `apps/conary/src/commands/bootstrap/run_record.rs`
- Create: `apps/conary/src/commands/bootstrap/run_artifact.rs`

- [ ] **Step 1: Add module declarations and temporary parent imports**

Add to `bootstrap/mod.rs`:

```rust
mod run_artifact;
mod run_record;

use run_artifact::write_bootstrap_run_generation_artifact;
use run_record::{
    finish_bootstrap_run_failure, finish_bootstrap_run_success,
    load_completed_bootstrap_run_record, start_bootstrap_run_record,
};
```

These imports are temporary while `cmd_bootstrap_run` and `cmd_bootstrap_verify_convergence` still live in the parent.

- [ ] **Step 2: Create `run_record.rs`**

Move these items into `run_record.rs`:

```text
start_bootstrap_run_record
link_bootstrap_run_outputs
finish_bootstrap_run_success
finish_bootstrap_run_failure
load_completed_bootstrap_run_record
test_bootstrap_run_writes_success_record_with_output_paths
```

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/run_record.rs

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::state::{BootstrapLatestPointer, BootstrapRunRecord};
use super::types::BootstrapRunOptions;
use crate::commands::operation_records::new_operation_id;
```

Set `start_bootstrap_run_record`, `finish_bootstrap_run_success`, `finish_bootstrap_run_failure`, and `load_completed_bootstrap_run_record` to `pub(super)`. Keep `link_bootstrap_run_outputs` private.

Move the record test into `run_record.rs` with this test import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::state::{BootstrapLatestPointer, BootstrapRunRecord};
    use super::super::types::BootstrapRunOptions;
    use std::path::PathBuf;
}
```

- [ ] **Step 3: Create `run_artifact.rs`**

Move these items into `run_artifact.rs`:

```text
write_bootstrap_run_generation_artifact
write_bootstrap_run_initramfs_source
materialize_bootstrap_run_initramfs_path
resolve_bootstrap_run_symlink_target
normalize_bootstrap_run_relative_path
architecture_from_target_triple
load_bootstrap_run_output_manifests
write_bootstrap_run_boot_asset_source
bootstrap_run_artifact_writer_creates_loadable_generation
```

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/run_artifact.rs

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::generation::builder::{FileEntryRef, SymlinkEntryRef};
use rusqlite::Connection;
```

Set `write_bootstrap_run_generation_artifact` to `pub(super)`. Keep all other moved helpers private.

Keep the existing function-local imports inside
`write_bootstrap_run_generation_artifact` as function-local imports in
`run_artifact.rs`; do not pull them to module scope unless clippy requires it.

`load_bootstrap_run_output_manifests` uses `toml::from_str` as a fully
qualified crate path. Do not add `use toml;` unless clippy or local style
cleanup requires it.

The file contains two `write_bootstrap_run_initramfs_source` definitions:

```rust
#[cfg(unix)]
fn write_bootstrap_run_initramfs_source(...)

#[cfg(not(unix))]
fn write_bootstrap_run_initramfs_source(...)
```

Preserve both `#[cfg]` attributes exactly.

`materialize_bootstrap_run_initramfs_path`,
`resolve_bootstrap_run_symlink_target`, and
`normalize_bootstrap_run_relative_path` are also `#[cfg(unix)]`. Preserve those
attributes exactly. Keep the `std::os::unix::fs::{PermissionsExt, symlink}`
import inside `materialize_bootstrap_run_initramfs_path`.

Move the artifact test into `run_artifact.rs` with this test import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
}
```

The test body already imports its own `conary_core::db::schema::migrate`, derivation types, and `CasStore`; keep those local imports in the test body unless clippy asks for cleanup.

- [ ] **Step 4: Clean parent imports after helper extraction**

After the helper move, `bootstrap/mod.rs` should no longer import:

```rust
conary_core::generation::builder::{FileEntryRef, SymlinkEntryRef}
rusqlite::Connection
std::collections::{HashMap, HashSet}
crate::commands::operation_records::new_operation_id
self::state::{BootstrapLatestPointer, BootstrapRunRecord}
```

unless a still-parented item genuinely uses one of them.

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::bootstrap -- --list
cargo test -p conary --lib commands::bootstrap
```

Expected:

```text
4 tests listed
commands::bootstrap: 4 passed
```

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git add apps/conary/src/commands/bootstrap/mod.rs \
  apps/conary/src/commands/bootstrap/run_record.rs \
  apps/conary/src/commands/bootstrap/run_artifact.rs
git commit -m "refactor(conary): extract bootstrap run helpers"
```

---

### Task 4: Extract Bootstrap Run Orchestration

**Files:**
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Create: `apps/conary/src/commands/bootstrap/run.rs`

- [ ] **Step 1: Add module declaration and re-export**

Add to `bootstrap/mod.rs`:

```rust
mod run;

pub use run::cmd_bootstrap_run;
```

- [ ] **Step 2: Create `run.rs`**

Move `cmd_bootstrap_run` into `run.rs`.

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/run.rs

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing::info;

use super::run_artifact::write_bootstrap_run_generation_artifact;
use super::run_record::{
    finish_bootstrap_run_failure, finish_bootstrap_run_success, start_bootstrap_run_record,
};
use super::types::BootstrapRunOptions;
```

Keep the existing local imports inside `cmd_bootstrap_run` for derivation, DB migration, pipeline, seed, and CAS types:

```rust
use conary_core::db::schema::migrate;
use conary_core::derivation::build_order::Stage;
use conary_core::derivation::build_order::compute_build_order;
use conary_core::derivation::executor::{DerivationExecutor, ExecutorConfig};
use conary_core::derivation::manifest::SystemManifest;
use conary_core::derivation::pipeline::{Pipeline, PipelineConfig, PipelineEvent};
use conary_core::derivation::seed::Seed;
use conary_core::filesystem::CasStore;
```

Remove the local `use rusqlite::Connection;` and `use std::collections::HashSet;` from inside the function if the module-level imports above are used.

- [ ] **Step 3: Remove temporary parent imports**

After `cmd_bootstrap_run` moves, remove from `bootstrap/mod.rs`:

```rust
use run_artifact::write_bootstrap_run_generation_artifact;
use run_record::{
    finish_bootstrap_run_failure, finish_bootstrap_run_success, start_bootstrap_run_record,
};
```

Keep `load_completed_bootstrap_run_record` temporarily if `cmd_bootstrap_verify_convergence` is still parent-owned.

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::bootstrap
cargo test -p conary --test bootstrap_workflow
```

Expected:

```text
commands::bootstrap: 4 passed
bootstrap_workflow: 3 passed
```

- [ ] **Step 4: Commit Task 4**

Run:

```bash
git add apps/conary/src/commands/bootstrap/mod.rs \
  apps/conary/src/commands/bootstrap/run.rs
git commit -m "refactor(conary): extract bootstrap run orchestration"
```

---

### Task 5: Extract Seed And Convergence Commands

**Files:**
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Create: `apps/conary/src/commands/bootstrap/seed.rs`
- Create: `apps/conary/src/commands/bootstrap/convergence.rs`

- [ ] **Step 1: Add module declarations and re-exports**

Add to `bootstrap/mod.rs`:

```rust
mod convergence;
mod seed;

pub use convergence::{cmd_bootstrap_diff_seeds, cmd_bootstrap_verify_convergence};
pub use seed::{cmd_bootstrap_seed, cmd_bootstrap_seed_adopted};
```

- [ ] **Step 2: Create `seed.rs`**

Move these items into `seed.rs`:

```text
cmd_bootstrap_seed
cmd_bootstrap_seed_adopted
```

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/seed.rs

use std::path::PathBuf;

use anyhow::{Context, Result};
```

Keep the existing local imports inside `cmd_bootstrap_seed`:

```rust
use conary_core::derivation::compose::erofs_image_hash;
use conary_core::derivation::seed::{SeedMetadata, SeedSource};
use conary_core::filesystem::CasStore;
use conary_core::generation::builder::{FileEntryRef, SymlinkEntryRef, build_erofs_image};
use std::os::unix::fs::MetadataExt;
use walkdir::WalkDir;
```

Keep the existing local import inside `cmd_bootstrap_seed_adopted`:

```rust
use conary_core::bootstrap::adopt_seed;
```

- [ ] **Step 3: Create `convergence.rs`**

Move these items into `convergence.rs`:

```text
cmd_bootstrap_verify_convergence
cmd_bootstrap_diff_seeds
```

Use this import surface:

```rust
// apps/conary/src/commands/bootstrap/convergence.rs

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::run_record::load_completed_bootstrap_run_record;
```

Keep the existing local import inside `cmd_bootstrap_verify_convergence`:

```rust
use conary_core::derivation::{Seed, compare_build_sets, load_build_set};
```

Remove the local `use rusqlite::Connection;` inside the function if the module-level import above is used.

- [ ] **Step 4: Remove parent imports made obsolete by seed/convergence**

After this task, `bootstrap/mod.rs` should no longer need `Path`, `PathBuf`, `Context`, `Result`, or `load_completed_bootstrap_run_record` unless a still-parented item uses them.

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::bootstrap
cargo test -p conary --test bootstrap_workflow
```

Expected:

```text
commands::bootstrap: 4 passed
bootstrap_workflow: 3 passed
```

- [ ] **Step 5: Commit Task 5**

Run:

```bash
git add apps/conary/src/commands/bootstrap/mod.rs \
  apps/conary/src/commands/bootstrap/seed.rs \
  apps/conary/src/commands/bootstrap/convergence.rs
git commit -m "refactor(conary): extract bootstrap seed commands"
```

---

### Task 6: Reduce `bootstrap/mod.rs` To The Final Hub

**Files:**
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`

- [ ] **Step 1: Confirm final hub contents**

After Tasks 1 through 5, `apps/conary/src/commands/bootstrap/mod.rs` should contain only the path comment, module docs, module declarations, and re-exports:

```rust
// src/commands/bootstrap/mod.rs

//! Bootstrap command implementations
//!
//! Commands for bootstrapping a complete Conary system from scratch.

mod cleanup;
mod convergence;
mod image;
mod phases;
mod run;
mod run_artifact;
mod run_record;
mod seed;
mod setup;
pub mod state;
mod types;

pub use cleanup::cmd_bootstrap_clean;
pub use convergence::{cmd_bootstrap_diff_seeds, cmd_bootstrap_verify_convergence};
pub use image::cmd_bootstrap_image;
pub use phases::{
    cmd_bootstrap_config, cmd_bootstrap_cross_tools, cmd_bootstrap_guest_profile,
    cmd_bootstrap_system, cmd_bootstrap_temp_tools, cmd_bootstrap_tier2,
};
pub use run::cmd_bootstrap_run;
pub use seed::{cmd_bootstrap_seed, cmd_bootstrap_seed_adopted};
pub use setup::{
    cmd_bootstrap_check, cmd_bootstrap_dry_run, cmd_bootstrap_init, cmd_bootstrap_resume,
    cmd_bootstrap_status,
};
pub use types::BootstrapRunOptions;
```

- [ ] **Step 2: Run structural boundary checks**

Run:

```bash
rg -n "^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn " apps/conary/src/commands/bootstrap/mod.rs
rg -n -U "#\[cfg\(test\)\]\s*\n\s*mod tests" apps/conary/src/commands/bootstrap/mod.rs
rg -n "use super::\*|use crate::\*" apps/conary/src/commands/bootstrap/mod.rs apps/conary/src/commands/bootstrap
```

Expected:

```text
No function bodies in bootstrap/mod.rs.
No parent #[cfg(test)] mod tests in bootstrap/mod.rs.
No wildcard imports in production modules.
```

Wildcard imports inside `#[cfg(test)] mod tests` are acceptable if they stay local to the test module.

- [ ] **Step 3: Run focused verification**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib commands::bootstrap -- --list
cargo test -p conary --lib commands::bootstrap
cargo test -p conary --test bootstrap_workflow
```

Expected:

```text
4 bootstrap unit tests listed
commands::bootstrap: 4 passed
bootstrap_workflow: 3 passed
```

- [ ] **Step 4: Commit Task 6**

Run:

```bash
git add apps/conary/src/commands/bootstrap/mod.rs
git commit -m "refactor(conary): reduce bootstrap command hub"
```

---

### Task 7: Update Documentation Routing And Docs-Audit Metadata

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/operations/bootstrap-follow-up-investigations.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

- [ ] **Step 1: Update `docs/llms/subsystem-map.md`**

Update front matter:

```yaml
last_updated: 2026-06-09
revision: 16
summary: Add bootstrap command child-module routing
```

In the "Look Here First" section, add a new bootstrap command-routing bullet
after the "TUF trust and signature verification" bullet:

```markdown
- System bootstrap from scratch, prerequisite validation, seed generation,
  image creation, and run orchestration:
  `apps/conary/src/commands/bootstrap/mod.rs`,
  `apps/conary/src/commands/bootstrap/setup.rs`,
  `apps/conary/src/commands/bootstrap/phases.rs`,
  `apps/conary/src/commands/bootstrap/image.rs`,
  `apps/conary/src/commands/bootstrap/run.rs`,
  `apps/conary/src/commands/bootstrap/run_record.rs`,
  `apps/conary/src/commands/bootstrap/run_artifact.rs`,
  `apps/conary/src/commands/bootstrap/seed.rs`,
  `apps/conary/src/commands/bootstrap/convergence.rs`,
  `apps/conary/src/commands/bootstrap/cleanup.rs`,
  `apps/conary/src/commands/bootstrap/types.rs`, and
  `apps/conary/src/commands/bootstrap/state.rs`
```

- [ ] **Step 2: Update `docs/modules/feature-ownership.md`**

Update front matter:

```yaml
last_updated: 2026-06-09
revision: 6
summary: Add bootstrap command child-module ownership
```

In the "Bootstrap And Self-Hosting" card, replace the single `apps/conary/src/commands/bootstrap/` start path with the specific command owners listed in Step 1.

Keep the existing focused proof and interaction gate. Add `cargo test -p conary --lib commands::bootstrap` and `cargo test -p conary --test bootstrap_workflow` as focused proof commands.

- [ ] **Step 3: Update `docs/modules/bootstrap.md`**

Update front matter:

```yaml
last_updated: 2026-06-09
revision: 11
summary: Add CLI bootstrap command child-module ownership
```

Add a short section after the opening overview:

```markdown
## CLI Command Owners

The CLI-facing bootstrap commands live under `apps/conary/src/commands/bootstrap/`.
`mod.rs` is a command hub; focused child modules own setup/status commands,
phase-build commands, image generation, bootstrap-run orchestration,
run-record state transitions, generation artifact writing, seed commands,
convergence checks, and cleanup.
```

- [ ] **Step 4: Update `docs/ARCHITECTURE.md`**

Update front matter:

```yaml
last_updated: 2026-06-09
revision: 21
summary: Note bootstrap command child modules
```

Update the `apps/conary/src/commands/` module map wording so bootstrap is represented as a hub with child modules alongside model/remove.

- [ ] **Step 5: Update `docs/operations/bootstrap-follow-up-investigations.md`**

Update front matter:

```yaml
last_updated: 2026-06-09
revision: 4
summary: Route bootstrap artifact follow-ups to command child modules
```

In the bootstrap artifact provenance follow-up section, replace the broad
`apps/conary/src/commands/bootstrap/mod.rs` relevant-file bullet with the
focused child owners:

```text
apps/conary/src/commands/bootstrap/image.rs
apps/conary/src/commands/bootstrap/run.rs
apps/conary/src/commands/bootstrap/run_artifact.rs
apps/conary/src/commands/bootstrap/seed.rs
```

- [ ] **Step 6: Update docs-audit ledger rows in place**

Update the existing rows for:

```text
docs/llms/subsystem-map.md
docs/modules/feature-ownership.md
docs/modules/bootstrap.md
docs/operations/bootstrap-follow-up-investigations.md
docs/ARCHITECTURE.md
docs/superpowers/documentation-accuracy-audit-summary.md
```

Do not add new rows for these existing docs. Add tags like:

```text
phase24; conary-bootstrap; bootstrap-command; bootstrap-child-modules
```

Add references to the new bootstrap child module paths and note that Phase 24 implementation landed.

The docs-audit disposition counts should remain:

```text
archived 73
corrected 69
retained-historical 14
verified-no-change 12
```

- [ ] **Step 7: Regenerate inventory and run docs gates**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
```

Expected:

```text
168
archived 73
corrected 69
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
Documentation truth checks passed.
```

The inventory `diff` command should have no output.

- [ ] **Step 8: Commit Task 7**

Run:

```bash
git add docs/llms/subsystem-map.md \
  docs/modules/feature-ownership.md \
  docs/modules/bootstrap.md \
  docs/operations/bootstrap-follow-up-investigations.md \
  docs/ARCHITECTURE.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: record bootstrap command ownership"
```

---

### Task 8: Final Verification

**Files:**
- Verify the full implementation and docs state.

- [ ] **Step 1: Workspace compile and formatting**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo check --workspace --all-targets
```

Expected: all pass with zero errors.

- [ ] **Step 2: Focused bootstrap tests**

Run:

```bash
cargo test -p conary --lib commands::bootstrap -- --list
cargo test -p conary --lib commands::bootstrap
cargo test -p conary --test bootstrap_workflow -- --list
cargo test -p conary --test bootstrap_workflow
cargo test -p conary --lib
```

Expected:

```text
4 bootstrap unit tests listed
commands::bootstrap: 4 passed
3 bootstrap_workflow tests listed
bootstrap_workflow: 3 passed
all conary lib tests pass
```

The `commands::bootstrap` filter covers tests in bootstrap child modules after
the split, not only tests directly under `bootstrap/mod.rs`.

- [ ] **Step 3: Bootstrap-adjacent integration inventory**

Run:

```bash
cargo run -p conary-test -- list
cargo run -p conary-test -- bootstrap check --json
cargo run -p conary-test -- bootstrap smoke --dry-run --json
```

Expected:

```text
conary-test list succeeds and lists the integration manifests.
bootstrap check returns JSON.
bootstrap smoke --dry-run returns JSON without launching live image/QEMU work.
```

If `bootstrap check --json` reports missing local host tools, that is acceptable only if the command exits successfully and reports the missing tools in JSON. If it exits non-zero, capture the output and decide whether the local environment lacks prerequisites outside this refactor's scope.

- [ ] **Step 4: Broader CLI routing smoke**

Run:

```bash
cargo test -p conary --test cli_daily_ux
cargo run -p conary -- bootstrap --help >/dev/null
cargo run -p conary -- bootstrap run --help >/dev/null
cargo run -p conary -- bootstrap verify-convergence --help >/dev/null
cargo run -p conary -- bootstrap diff-seeds --help >/dev/null
```

Expected: all pass with zero errors.

- [ ] **Step 5: Clippy**

Run:

```bash
cargo clippy -p conary --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both pass with zero warnings.

- [ ] **Step 6: Hotspot and boundary checks**

Run:

```bash
scripts/line-count-report.sh 20
rg -n "^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn " apps/conary/src/commands/bootstrap/mod.rs
rg -n -U "#\[cfg\(test\)\]\s*\n\s*mod tests" apps/conary/src/commands/bootstrap/mod.rs
rg -n "use super::\*|use crate::\*" apps/conary/src/commands/bootstrap/ 2>/dev/null || true
```

Expected:

```text
bootstrap/mod.rs is no longer a top hotspot.
No function bodies in bootstrap/mod.rs.
No parent test module in bootstrap/mod.rs.
No wildcard imports in production bootstrap modules.
```

Wildcard imports inside `#[cfg(test)] mod tests` are acceptable.

- [ ] **Step 7: Docs-audit and whitespace gates**

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
168
archived 73
corrected 69
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
Documentation truth checks passed.
```

The inventory `diff` and `git diff --check` commands should have no output.

- [ ] **Step 8: Push and synced-main verification**

Run:

```bash
git status --short --branch
git push
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected:

```text
Working tree clean.
HEAD and origin/main match.
Ahead/behind count is 0 0.
Only the /home/peter/Conary worktree is listed unless the user intentionally created another one.
```

---

## Implementation Notes And Pitfalls

- Keep `state.rs` public. It is already a child module and should not be folded into the new private helper modules.
- `cmd_bootstrap_resume` crosses module boundaries after Task 2. Import phase/image functions explicitly in `setup.rs` rather than relying on wildcard imports.
- `BootstrapRunOptions` is constructed in `apps/conary/src/dispatch/bootstrap.rs` via `commands::BootstrapRunOptions`; the hub and `apps/conary/src/commands/mod.rs` re-export chain must stay intact.
- `write_bootstrap_run_initramfs_source` has Unix and non-Unix definitions with the same name. Move both definitions together and preserve their `#[cfg]` attributes.
- The bootstrap-run artifact test synthesizes initramfs inputs from `conary_core::bootstrap::bootstrap_initramfs_input_paths()`. Keep it in the artifact owner so future artifact/input changes fail near the owning helper.
- `cmd_bootstrap_seed` has several function-local imports. Preserve them unless clippy requires cleanup; moving them to module scope can create unused-import churn.
- `cmd_bootstrap_verify_convergence` and `cmd_bootstrap_diff_seeds` are covered by `apps/conary/tests/bootstrap_workflow.rs`, not only unit tests. Run that integration test after convergence extraction.
- Do not run the non-dry-run `conary-test bootstrap smoke --json` unless the local environment is intended to perform live bootstrap/image validation.

## Completion Criteria

- `apps/conary/src/commands/bootstrap/mod.rs` is a hub with no function bodies or parent test module.
- All public command exports from `apps/conary/src/commands/mod.rs` still compile.
- `apps/conary/src/dispatch/bootstrap.rs` still routes through `commands::cmd_bootstrap_*` and `commands::BootstrapRunOptions`.
- `commands::bootstrap` still lists exactly 4 focused unit tests.
- `bootstrap_workflow` still lists exactly 3 integration tests.
- Docs-audit inventory and ledger are complete with 168 tracked files and 69 corrected rows after the plan is locked in.
