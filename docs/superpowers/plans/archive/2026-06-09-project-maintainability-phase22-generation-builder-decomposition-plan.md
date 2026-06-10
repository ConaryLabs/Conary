# Phase 22 Generation Builder Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `crates/conary-core/src/generation/builder.rs` from the new largest Rust hotspot into focused generation-builder child modules while preserving the public `conary_core::generation::builder::*` API and generation artifact behavior.

**Architecture:** Keep the existing Rust file-module layout: `crates/conary-core/src/generation/builder.rs` remains the public hub and continues to own `builder/erofs.rs` plus `builder/runtime_inputs.rs`. Move parent-owned responsibilities into sibling files under `crates/conary-core/src/generation/builder/`: activation policy, build orchestration, rebuild orchestration, root self-containment validation, CAS/artifact input plumbing, temporary runtime sysroot materialization, kernel release discovery, initramfs tool execution, and boot asset resolution.

**Tech Stack:** Rust 2024, Cargo workspace, `conary-core`, `rusqlite`, composefs/EROFS generation, CAS artifact manifests, dracut/depmod/cpio runtime boot asset handling, docs-audit ledger tooling.

---

## Current Repository Facts

- Repository root: `/home/peter/Conary`.
- Current `HEAD` and `origin/main`: `279caea5ff4014688b5ff5a2b5d52d4be6f86f17`.
- Current hotspot: `crates/conary-core/src/generation/builder.rs` at 2,147 lines.
- Existing child files:
  - `crates/conary-core/src/generation/builder/erofs.rs` at 613 lines.
  - `crates/conary-core/src/generation/builder/runtime_inputs.rs` at 513 lines.
- Existing module layout is already valid Rust file-module layout:
  - `crates/conary-core/src/generation/mod.rs` declares `pub mod builder;`.
  - `crates/conary-core/src/generation/builder.rs` declares child modules with `mod erofs;` and `mod runtime_inputs;`.
  - New children should be added under `crates/conary-core/src/generation/builder/`.
- Baseline focused tests:
  - `cargo test -p conary-core --lib generation::builder -- --list` lists 48 tests.
  - `cargo test -p conary-core --lib generation::builder` passes: 48 passed, 0 failed.
- Baseline docs-audit state:
  - Inventory: 165 tracked files.
  - Ledger counts: `archived 73`, `corrected 66`, `retained-historical 14`, `verified-no-change 12`.
  - `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete` passes.

## Non-Goals

- Do not change generation artifact layout, metadata schema, CAS manifest contents, boot asset filenames, EROFS build behavior, or activation semantics.
- Do not rename public APIs under `conary_core::generation::builder`.
- Do not change `crate::generation::builder::rebuild_generation_image` visibility beyond its current crate-local reach.
- Do not alter dracut arguments, initramfs generation policy, kernel release matching, or self-contained runtime validation behavior.
- Do not add a `crates/conary-core/src/generation/builder/mod.rs`; that would conflict with the existing file-module layout.

## Public API Contract To Preserve

These paths must remain usable after decomposition:

```rust
conary_core::generation::builder::BuildResult
conary_core::generation::builder::FileEntryRef
conary_core::generation::builder::SymlinkEntryRef
conary_core::generation::builder::build_erofs_image
conary_core::generation::builder::hex_to_digest
conary_core::generation::builder::GenerationActivation
conary_core::generation::builder::build_generation_from_db
conary_core::generation::builder::build_generation_from_db_with_activation
conary_core::generation::builder::build_generation_from_db_with_boot_root
conary_core::generation::builder::build_generation_from_db_with_boot_root_and_activation
conary_core::generation::builder::detect_kernel_version_from_troves
```

This crate-local path must remain usable:

```rust
crate::generation::builder::rebuild_generation_image
```

Current downstream callers verified by `rg`:

- `crates/conary-core/src/transaction/recovery.rs` calls `crate::generation::builder::rebuild_generation_image`.
- `crates/conary-core/src/transaction/mod.rs` uses `build_generation_from_db_with_boot_root`.
- `apps/conary/src/commands/composefs_ops.rs` uses `build_generation_from_db_with_boot_root_and_activation` and `GenerationActivation::Inactive`.
- `apps/conary/src/commands/generation/builder.rs` uses `build_generation_from_db_with_activation` and `GenerationActivation::Inactive`.
- `apps/conary/src/commands/bootstrap/mod.rs`, `crates/conary-core/src/bootstrap/image.rs`, `crates/conary-core/src/derivation/compose.rs`, `crates/conary-core/src/generation/delta.rs`, and `crates/conary-core/benches/erofs_build.rs` use the existing EROFS exports.
- Test helpers in `apps/conary/src/commands/**`, `apps/conary/tests/common/mod.rs`, and `apps/conary/src/commands/install/conversion.rs` use `detect_kernel_version_from_troves`.
- `crates/conary-core/tests/generation_composefs_runtime_contract.rs` contains a source-text assertion that `apps/conary/src/commands/generation/builder.rs` still uses `GenerationActivation::Inactive`.

## Final File Responsibility Map

| File | Responsibility |
| --- | --- |
| `crates/conary-core/src/generation/builder.rs` | Hub module only: path comment, module declarations, public/crate-local re-exports, high-level docs. |
| `crates/conary-core/src/generation/builder/activation.rs` | `GenerationActivation` and its state-activation helper. |
| `crates/conary-core/src/generation/builder/create.rs` | Public generation creation entrypoints and pending-generation cleanup guard. |
| `crates/conary-core/src/generation/builder/rebuild.rs` | Crate-local in-place recovery rebuild entrypoints. |
| `crates/conary-core/src/generation/builder/root_validation.rs` | Self-contained runtime root validation and virtual symlink path resolution. |
| `crates/conary-core/src/generation/builder/cas.rs` | CAS object list conversion, artifact-root discovery, and CAS object presence verification. |
| `crates/conary-core/src/generation/builder/sysroot.rs` | Temporary runtime sysroot materialization from CAS-backed file and symlink refs. |
| `crates/conary-core/src/generation/builder/kernel.rs` | Kernel version detection, kernel release candidate discovery, module path discovery, and boot-root/system-root path helpers. |
| `crates/conary-core/src/generation/builder/initramfs.rs` | Conary dracut module constants, initramfs generation, depmod/cpio validation, and dracut workspace setup. |
| `crates/conary-core/src/generation/builder/boot_assets.rs` | Runtime/generation boot asset source resolution and staging calls into `generation::artifact`. |
| `crates/conary-core/src/generation/builder/test_support.rs` | Test-only fixtures shared by sibling tests migrated out of the parent module. |
| `crates/conary-core/src/generation/builder/erofs.rs` | Existing low-level EROFS image builder and public EROFS entry types. |
| `crates/conary-core/src/generation/builder/runtime_inputs.rs` | Existing runtime DB/CAS input classification and validation. |

## Visibility Contract

- `activation::GenerationActivation` remains `pub`.
- `GenerationActivation::activates_state` must become `pub(super)` because `create.rs` will call it after the enum moves into `activation.rs`.
- `create::{build_generation_from_db, build_generation_from_db_with_activation, build_generation_from_db_with_boot_root, build_generation_from_db_with_boot_root_and_activation}` must be `pub`.
- `rebuild::{rebuild_generation_image, rebuild_generation_image_with_boot_root}` must be `pub(crate)` because recovery code calls the parent re-export inside `conary-core`.
- `root_validation::validate_runtime_generation_root_is_self_contained` must be `pub(super)`.
- `cas::{artifact_root_for_generations_root, cas_objects_from_file_refs, verify_runtime_generation_cas_object_presence}` must be `pub(super)`.
- `sysroot::materialize_runtime_generation_sysroot` must be `pub(super)`.
- `kernel::detect_kernel_version_from_troves` must be `pub`.
- Kernel helper functions used by sibling modules must be `pub(super)`:
  - `collect_boot_kernel_releases`
  - `collect_module_kernel_releases`
  - `kernel_module_dir`
  - `module_kernel_path`
  - `push_unique_release`
  - `regular_file_exists`
  - `system_root_for_boot_root`
- `initramfs::generate_runtime_initramfs` must be `pub(super)`.
- `boot_assets::RuntimeBootAssetSources` and all fields read by `create.rs`, `rebuild.rs`, or tests must be `pub(super)`.
- `boot_assets::stage_runtime_boot_assets_from_sources` and `boot_assets::resolve_generation_boot_asset_sources` must be `pub(super)`.
- Test support helpers in `test_support.rs` should be `pub(super)` and the module should be declared as `#[cfg(test)] pub(super) mod test_support;`.

---

### Task 0: Lock In The Plan Packet

**Files:**
- Create: `docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase22-generation-builder-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

- [ ] **Step 1: Stage the new plan before inventory checks**

Run:

```bash
git status --short --branch
git add docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase22-generation-builder-decomposition-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected after staging the plan:

```text
docs-audit inventory includes 166 tracked files
```

- [ ] **Step 2: Add the ledger row**

Append a row for the plan file:

```text
docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase22-generation-builder-decomposition-plan.md	docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase22-generation-builder-decomposition-plan.md	planning	maintainer	phase22; generation-builder; maintainability; decomposition; plan	crates/conary-core/src/generation/builder.rs; crates/conary-core/src/generation/builder/erofs.rs; crates/conary-core/src/generation/builder/runtime_inputs.rs; crates/conary-core/src/generation/artifact.rs; crates/conary-core/src/generation/metadata.rs; crates/conary-core/src/transaction/recovery.rs; apps/conary/src/commands/composefs_ops.rs; apps/conary/src/commands/generation/builder.rs; docs/ARCHITECTURE.md; docs/llms/subsystem-map.md; docs/modules/feature-ownership.md; docs/operations/post-generation-export-follow-up-roadmap.md	verified	corrected	Planned the Phase 22 decomposition of the generation builder hotspot into focused child modules while preserving public builder API paths, crate-local recovery rebuild visibility, boot asset semantics, runtime CAS validation, and docs-audit routing.
```

- [ ] **Step 3: Refresh the docs-audit summary**

Update `docs/superpowers/documentation-accuracy-audit-summary.md` so it reports:

```text
Total tracked doc-like files audited: 166
corrected: 67
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
git diff --check
```

Expected:

```text
166
archived 73
corrected 67
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 5: Commit the lock-in**

Run:

```bash
git add docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase22-generation-builder-decomposition-plan.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: plan generation builder decomposition"
git push
git status --short --branch
git rev-parse HEAD origin/main
```

Expected: clean synced `main`.

---

### Task 1: Extract Activation, Kernel Discovery, And Root Validation

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Create: `crates/conary-core/src/generation/builder/activation.rs`
- Create: `crates/conary-core/src/generation/builder/kernel.rs`
- Create: `crates/conary-core/src/generation/builder/root_validation.rs`

- [ ] **Step 1: Add module declarations and re-exports in the hub**

At the top of `crates/conary-core/src/generation/builder.rs`, keep the path comment and docs, then use this module surface:

```rust
mod activation;
mod erofs;
mod kernel;
mod root_validation;
mod runtime_inputs;

pub use activation::GenerationActivation;
pub use erofs::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image, hex_to_digest};
pub use kernel::detect_kernel_version_from_troves;
```

Remove the original inline `GenerationActivation` definition and the original inline `detect_kernel_version_from_troves` function after moving them.

- [ ] **Step 2: Create `activation.rs`**

```rust
// conary-core/src/generation/builder/activation.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationActivation {
    /// Publish the generated DB snapshot as the active state immediately.
    ///
    /// Use only for paths that also publish/mount the generation in the same
    /// operation, such as composefs-native package mutation.
    Active,
    /// Leave the generated DB snapshot inactive until an explicit generation
    /// switch selects it for the next boot.
    Inactive,
}

impl GenerationActivation {
    pub(super) fn activates_state(self) -> bool {
        matches!(self, Self::Active)
    }
}
```

- [ ] **Step 3: Create `kernel.rs`**

Move these functions exactly from `builder.rs`:

```rust
collect_boot_kernel_releases
collect_module_kernel_releases
push_unique_release
kernel_release_matches
module_kernel_path
kernel_module_dir
regular_file_exists
system_root_for_boot_root
detect_kernel_version_from_troves
```

Use this import surface:

```rust
// conary-core/src/generation/builder/kernel.rs

use std::path::{Path, PathBuf};

use crate::db::models::Trove;
```

Visibility after the move:

```rust
pub(super) fn collect_boot_kernel_releases(...)
pub(super) fn collect_module_kernel_releases(...)
pub(super) fn push_unique_release(...)
fn kernel_release_matches(...)
pub(super) fn module_kernel_path(...)
pub(super) fn kernel_module_dir(...)
pub(super) fn regular_file_exists(...)
pub(super) fn system_root_for_boot_root(...)
pub fn detect_kernel_version_from_troves(...)
```

Move these tests from the parent into `kernel.rs`:

```text
detect_kernel_version_does_not_panic
detect_kernel_version_prefers_payload_kernel_over_meta_package
```

Use this test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::TroveType;
}
```

- [ ] **Step 4: Create `root_validation.rs`**

Move these functions exactly from `builder.rs`:

```rust
validate_runtime_generation_root_is_self_contained
generation_root_has_init_entrypoint
generation_symlink_map
resolve_virtual_path
rewrite_first_symlink_component
normalize_virtual_path
parent_virtual_path
```

Use this import surface:

```rust
// conary-core/src/generation/builder/root_validation.rs

use std::collections::{HashMap, HashSet};

use super::{FileEntryRef, SymlinkEntryRef, hex_to_digest};
use crate::generation::metadata::ROOT_SYMLINKS;
```

Only `validate_runtime_generation_root_is_self_contained` should be `pub(super)`. The remaining helpers can stay private.

Move this test from the parent into `root_validation.rs`:

```text
runtime_root_init_detection_resolves_usr_merge_and_package_symlinks
```

Use this test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{FileEntryRef, SymlinkEntryRef};
}
```

- [ ] **Step 5: Import moved helpers in the still-large parent**

Until orchestration moves in later tasks, `builder.rs` still needs:

```rust
use kernel::{
    collect_boot_kernel_releases, collect_module_kernel_releases, kernel_module_dir,
    module_kernel_path, push_unique_release, regular_file_exists, system_root_for_boot_root,
};
use root_validation::validate_runtime_generation_root_is_self_contained;
```

The parent can continue calling `detect_kernel_version_from_troves` through its local re-export.

- [ ] **Step 6: Verify Task 1**

Run:

```bash
cargo fmt --check
cargo check -p conary-core
cargo test -p conary-core --lib generation::builder -- --list
cargo test -p conary-core --lib generation::builder
```

Expected:

```text
48 tests listed
48 passed
```

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/conary-core/src/generation/builder.rs \
  crates/conary-core/src/generation/builder/activation.rs \
  crates/conary-core/src/generation/builder/kernel.rs \
  crates/conary-core/src/generation/builder/root_validation.rs
git commit -m "refactor(core): extract generation builder primitives"
```

---

### Task 2: Extract CAS Plumbing And Runtime Sysroot Materialization

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Create: `crates/conary-core/src/generation/builder/cas.rs`
- Create: `crates/conary-core/src/generation/builder/sysroot.rs`

- [ ] **Step 1: Add child declarations**

Add to `builder.rs`:

```rust
mod cas;
mod sysroot;
```

- [ ] **Step 2: Create `cas.rs`**

Move these functions exactly from `builder.rs`:

```rust
cas_objects_from_file_refs
verify_runtime_generation_cas_object_presence
artifact_root_for_generations_root
```

Use this import surface:

```rust
// conary-core/src/generation/builder/cas.rs

use std::path::{Path, PathBuf};

use super::FileEntryRef;
use crate::generation::artifact::{CasObjectRef, verify_cas_object_files_exist_with_expected_sizes};
```

All three functions must be `pub(super)` because `create.rs`, `rebuild.rs`, and boot-asset resolution need them.

- [ ] **Step 3: Create `sysroot.rs`**

Move these functions exactly from `builder.rs`:

```rust
materialize_runtime_generation_sysroot
materialize_runtime_regular_file
materialize_runtime_symlink
materialize_root_symlinks
materialize_runtime_sysroot_base_dirs
relative_runtime_path
runtime_generation_architecture
```

Use this import surface:

```rust
// conary-core/src/generation/builder/sysroot.rs

use std::path::Path;

use super::{FileEntryRef, SymlinkEntryRef, runtime_inputs};
use crate::generation::metadata::ROOT_SYMLINKS;
```

Visibility:

```rust
pub(super) fn materialize_runtime_generation_sysroot(...)
pub(super) fn runtime_generation_architecture(...)
```

All other functions can stay private.

- [ ] **Step 4: Import moved helpers in the still-large parent**

Until `create.rs` and `rebuild.rs` exist, `builder.rs` still needs:

```rust
use cas::{
    artifact_root_for_generations_root, cas_objects_from_file_refs,
    verify_runtime_generation_cas_object_presence,
};
use sysroot::{materialize_runtime_generation_sysroot, runtime_generation_architecture};
```

`artifact_root_for_generations_root` will be used by `boot_assets.rs` in Task 3.

- [ ] **Step 5: Verify Task 2**

Run:

```bash
cargo fmt --check
cargo check -p conary-core
cargo test -p conary-core --lib generation::builder
```

Expected: `48 passed`.

- [ ] **Step 6: Commit Task 2**

```bash
git add crates/conary-core/src/generation/builder.rs \
  crates/conary-core/src/generation/builder/cas.rs \
  crates/conary-core/src/generation/builder/sysroot.rs
git commit -m "refactor(core): extract generation builder runtime inputs"
```

---

### Task 3: Extract Test Support, Initramfs Tooling, And Boot Asset Resolution

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Create: `crates/conary-core/src/generation/builder/test_support.rs`
- Create: `crates/conary-core/src/generation/builder/initramfs.rs`
- Create: `crates/conary-core/src/generation/builder/boot_assets.rs`

- [ ] **Step 1: Add child declarations**

Add to `builder.rs`:

```rust
mod boot_assets;
mod initramfs;

#[cfg(test)]
pub(super) mod test_support;
```

- [ ] **Step 2: Create `test_support.rs`**

Move these helpers from the parent tests module:

```text
write_executable
runtime_generation_db_with_invalid_regular_file
runtime_generation_db_with_missing_regular_file_cas_object
assert_invalid_runtime_input_error
assert_missing_cas_object_error
```

Use this import surface:

```rust
// conary-core/src/generation/builder/test_support.rs

#[cfg(unix)]
use std::path::Path;
#[cfg(feature = "composefs-rs")]
use std::path::PathBuf;

#[cfg(feature = "composefs-rs")]
use crate::db::models::{FileEntry, Trove, TroveType};
#[cfg(feature = "composefs-rs")]
use crate::db::schema::migrate;
#[cfg(feature = "composefs-rs")]
use crate::filesystem::CasStore;
```

Visibility:

```rust
#[cfg(unix)]
pub(super) fn write_executable(...)

#[cfg(feature = "composefs-rs")]
pub(super) fn runtime_generation_db_with_invalid_regular_file(...)

#[cfg(feature = "composefs-rs")]
pub(super) fn runtime_generation_db_with_missing_regular_file_cas_object(...)

pub(super) fn assert_invalid_runtime_input_error(...)
pub(super) fn assert_missing_cas_object_error(...)
```

- [ ] **Step 3: Add temporary parent-test imports**

Because creation and rebuild tests remain in the parent until Tasks 4 and 5, add this temporary import inside the existing parent `#[cfg(test)] mod tests` in `builder.rs`:

```rust
#[cfg(feature = "composefs-rs")]
use super::test_support::{
    assert_invalid_runtime_input_error, assert_missing_cas_object_error,
    runtime_generation_db_with_invalid_regular_file,
    runtime_generation_db_with_missing_regular_file_cas_object,
};
```

Remove this import when the parent tests module is deleted in Task 5.

- [ ] **Step 4: Create `initramfs.rs`**

Move these constants and functions exactly from `builder.rs`:

```rust
CONARY_DRACUT_MODULE_SETUP
CONARY_DRACUT_INIT
CONARY_DRACUT_GENERATOR
RUNTIME_DRACUT_ADD_MODULES
RUNTIME_DRACUT_OMIT_MODULES
generate_runtime_initramfs
ensure_initramfs_tool_available
prepare_dracut_workspace
link_or_copy_dracut_entry
ensure_kernel_module_metadata
write_dracut_module_file
```

Use this import surface:

```rust
// conary-core/src/generation/builder/initramfs.rs

use std::path::Path;

use super::kernel::{kernel_module_dir, regular_file_exists};

pub(super) const CONARY_DRACUT_MODULE_SETUP: &str =
    include_str!("../../../../../packaging/dracut/90conary/module-setup.sh");
const CONARY_DRACUT_INIT: &str =
    include_str!("../../../../../packaging/dracut/90conary/conary-init.sh");
const CONARY_DRACUT_GENERATOR: &str =
    include_str!("../../../../../packaging/dracut/90conary/conary-generator.sh");
pub(super) const RUNTIME_DRACUT_ADD_MODULES: &str = "conary";
pub(super) const RUNTIME_DRACUT_OMIT_MODULES: &str = "systemd";
```

The `include_str!` relative path changes because the file moves from `generation/builder.rs` to `generation/builder/initramfs.rs`.

`CONARY_DRACUT_MODULE_SETUP` is `pub(super)` only so the moved boot-asset test can keep the existing dracut-module assertion without creating an extra test. `generate_runtime_initramfs` must be `pub(super)`. Other helpers can stay private unless a sibling compile error proves otherwise.

- [ ] **Step 5: Create `boot_assets.rs`**

Move these types and functions exactly from `builder.rs`:

```rust
RuntimeBootAssetSources
InitramfsPolicy
stage_runtime_boot_assets_from_sources
resolve_runtime_boot_asset_sources
resolve_generation_boot_asset_sources
resolve_generation_boot_asset_sources_with_tools
resolve_runtime_boot_asset_sources_with_tools
resolve_runtime_boot_asset_sources_with_tools_and_policy
runtime_boot_asset_sources_for_release
select_existing_or_versioned_initramfs
```

Use this import surface:

```rust
// conary-core/src/generation/builder/boot_assets.rs

use std::path::{Path, PathBuf};

use super::cas::artifact_root_for_generations_root;
use super::initramfs::generate_runtime_initramfs;
use super::kernel::{
    collect_boot_kernel_releases, collect_module_kernel_releases,
    detect_kernel_version_from_troves, module_kernel_path, push_unique_release,
    regular_file_exists, system_root_for_boot_root,
};
use super::runtime_inputs;
use super::sysroot::materialize_runtime_generation_sysroot;
use crate::db::models::Trove;
use crate::generation::artifact::{BootAssetSources, BootAssetsManifest, stage_boot_assets};
```

Visibility:

```rust
pub(super) struct RuntimeBootAssetSources { ... }
pub(super) enum InitramfsPolicy { ... }
pub(super) fn stage_runtime_boot_assets_from_sources(...) -> crate::Result<BootAssetsManifest>
pub(super) fn resolve_generation_boot_asset_sources(...) -> crate::Result<RuntimeBootAssetSources>
```

`RuntimeBootAssetSources` fields must be `pub(super)`:

```rust
pub(super) kernel_version: String,
pub(super) kernel: PathBuf,
pub(super) initramfs: PathBuf,
pub(super) efi_bootloader: PathBuf,
pub(super) _sysroot_workspace: Option<tempfile::TempDir>,
```

The `#[cfg(test)] fn resolve_runtime_boot_asset_sources(...)` helper can remain private inside `boot_assets.rs`. The extracted `resolve_generation_boot_asset_sources_with_tools` and `resolve_runtime_boot_asset_sources_with_tools` helpers must remain reachable to the moved tests; keep them private if the tests live in the same module.

- [ ] **Step 6: Move boot asset tests into `boot_assets.rs`**

Move these tests from the parent:

```text
runtime_boot_asset_resolution_uses_arch_qualified_module_release
runtime_boot_asset_resolution_accepts_unversioned_boot_fixture_assets
generation_boot_asset_resolution_materializes_default_boot_from_cas_inputs
generation_boot_asset_resolution_regenerates_conary_initramfs_from_materialized_sysroot
runtime_boot_asset_resolution_generates_missing_initramfs_with_shell_dracut
runtime_boot_asset_resolution_runs_depmod_before_dracut_when_modules_dep_is_missing
runtime_boot_asset_resolution_reports_missing_cpio_before_dracut
```

Preserve the existing `#[cfg(unix)]` annotations on these four tests:

```text
generation_boot_asset_resolution_regenerates_conary_initramfs_from_materialized_sysroot
runtime_boot_asset_resolution_generates_missing_initramfs_with_shell_dracut
runtime_boot_asset_resolution_runs_depmod_before_dracut_when_modules_dep_is_missing
runtime_boot_asset_resolution_reports_missing_cpio_before_dracut
```

Use this test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{FileEntryRef, runtime_inputs};
    use super::super::initramfs::{
        CONARY_DRACUT_MODULE_SETUP, RUNTIME_DRACUT_ADD_MODULES, RUNTIME_DRACUT_OMIT_MODULES,
    };
    use crate::db::models::{Trove, TroveType};
    use crate::filesystem::CasStore;
    use std::path::Path;

    #[cfg(unix)]
    use super::super::test_support::write_executable;
}
```

Use the shared `test_support::write_executable` helper created in Step 2.

- [ ] **Step 7: Import moved helpers in the still-large parent**

Until orchestration moves, `builder.rs` still needs:

```rust
use boot_assets::{resolve_generation_boot_asset_sources, stage_runtime_boot_assets_from_sources};
```

- [ ] **Step 8: Verify Task 3**

Run:

```bash
cargo fmt --check
cargo check -p conary-core
cargo test -p conary-core --lib generation::builder -- --list
cargo test -p conary-core --lib generation::builder
```

Expected:

```text
48 tests listed
48 passed
```

- [ ] **Step 9: Commit Task 3**

```bash
git add crates/conary-core/src/generation/builder.rs \
  crates/conary-core/src/generation/builder/test_support.rs \
  crates/conary-core/src/generation/builder/initramfs.rs \
  crates/conary-core/src/generation/builder/boot_assets.rs
git commit -m "refactor(core): extract generation boot asset resolution"
```

---

### Task 4: Extract Generation Creation Orchestration

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Create: `crates/conary-core/src/generation/builder/create.rs`

- [ ] **Step 1: Add declarations and public re-exports**

Add to `builder.rs`:

```rust
mod create;

pub use create::{
    build_generation_from_db, build_generation_from_db_with_activation,
    build_generation_from_db_with_boot_root, build_generation_from_db_with_boot_root_and_activation,
};
```

- [ ] **Step 2: Create `create.rs`**

Move these public functions from `builder.rs`:

```rust
build_generation_from_db
build_generation_from_db_with_activation
build_generation_from_db_with_boot_root
build_generation_from_db_with_boot_root_and_activation
```

Move the nested `PendingGenerationGuard` into `create.rs` as a private helper.

Use this import surface:

```rust
// conary-core/src/generation/builder/create.rs

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use super::GenerationActivation;
use super::boot_assets::{resolve_generation_boot_asset_sources, stage_runtime_boot_assets_from_sources};
use super::cas::{cas_objects_from_file_refs, verify_runtime_generation_cas_object_presence};
use super::erofs::{BuildResult, build_erofs_image};
use super::root_validation::validate_runtime_generation_root_is_self_contained;
use super::runtime_inputs;
use super::sysroot::runtime_generation_architecture;
use crate::db::models::{FileEntry, StateEngine, SystemState, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, CasObjectVerification, deduplicate_sort_cas_objects,
    write_generation_artifact,
};
use crate::generation::metadata::{
    GENERATION_FORMAT, GenerationMetadata, clear_generation_pending, mark_generation_pending,
};
```

Preserve the inline `use fs2::FileExt as _;` immediately before `lock_file.lock_exclusive()`.

- [ ] **Step 3: Move generation creation tests into `create.rs`**

Move these tests:

```text
build_generation_from_db_writes_export_artifact_contract
build_generation_from_db_rejects_invalid_runtime_input
build_generation_from_db_rejects_missing_regular_file_cas_object
build_generation_from_db_rejects_root_without_init
```

Use this feature-gated test module and import block:

```rust
#[cfg(all(test, feature = "composefs-rs"))]
mod tests {
    use super::*;
    use super::super::test_support::{
        assert_invalid_runtime_input_error, assert_missing_cas_object_error,
        runtime_generation_db_with_invalid_regular_file,
        runtime_generation_db_with_missing_regular_file_cas_object,
    };
    use crate::db::models::{FileEntry, Trove, TroveType};
    use crate::db::schema::migrate;
    use crate::filesystem::CasStore;
    use crate::generation::metadata::GenerationMetadata;
}
```

Because the whole module is gated, the moved test functions may either keep their existing `#[cfg(feature = "composefs-rs")]` annotations or drop them as redundant during the move.

- [ ] **Step 4: Verify Task 4**

Run:

```bash
cargo fmt --check
cargo check -p conary-core
cargo test -p conary-core --lib generation::builder
cargo test -p conary-core --test generation_composefs_runtime_contract -- --list
```

Expected:

```text
48 passed
19 tests listed in generation_composefs_runtime_contract
```

- [ ] **Step 5: Commit Task 4**

```bash
git add crates/conary-core/src/generation/builder.rs \
  crates/conary-core/src/generation/builder/create.rs
git commit -m "refactor(core): extract generation creation orchestration"
```

---

### Task 5: Extract Recovery Rebuild Orchestration And Finalize The Hub

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`
- Create: `crates/conary-core/src/generation/builder/rebuild.rs`

- [ ] **Step 1: Add declaration and crate-local re-export**

Add to `builder.rs`:

```rust
mod rebuild;

pub(crate) use rebuild::{rebuild_generation_image, rebuild_generation_image_with_boot_root};
```

- [ ] **Step 2: Create `rebuild.rs`**

Move these functions from `builder.rs`:

```rust
rebuild_generation_image
rebuild_generation_image_with_boot_root
```

Use this import surface:

```rust
// conary-core/src/generation/builder/rebuild.rs

use std::path::Path;

use tracing::info;

use super::boot_assets::{resolve_generation_boot_asset_sources, stage_runtime_boot_assets_from_sources};
use super::cas::{cas_objects_from_file_refs, verify_runtime_generation_cas_object_presence};
use super::erofs::{BuildResult, build_erofs_image};
use super::root_validation::validate_runtime_generation_root_is_self_contained;
use super::runtime_inputs;
use super::sysroot::runtime_generation_architecture;
use crate::db::models::{FileEntry, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, CasObjectVerification, deduplicate_sort_cas_objects,
    write_generation_artifact,
};
use crate::generation::metadata::{GENERATION_FORMAT, GenerationMetadata, clear_generation_pending};
```

Both functions must be `pub(crate)`.

- [ ] **Step 3: Move recovery rebuild tests into `rebuild.rs`**

Move these tests:

```text
rebuild_generation_image_rejects_invalid_runtime_input
rebuild_generation_image_rejects_missing_regular_file_cas_object
rebuild_generation_image_clears_stale_pending_marker
```

Use this feature-gated test module and import block:

```rust
#[cfg(all(test, feature = "composefs-rs"))]
mod tests {
    use super::*;
    use super::super::test_support::{
        assert_invalid_runtime_input_error, assert_missing_cas_object_error,
        runtime_generation_db_with_invalid_regular_file,
        runtime_generation_db_with_missing_regular_file_cas_object,
    };
    use crate::db::models::{FileEntry, Trove, TroveType};
    use crate::db::schema::migrate;
    use crate::filesystem::CasStore;
    use crate::generation::metadata::{is_generation_pending, mark_generation_pending};
}
```

Because the whole module is gated, the moved test functions may either keep their existing `#[cfg(feature = "composefs-rs")]` annotations or drop them as redundant during the move.

- [ ] **Step 4: Remove stale parent imports and test module**

After this task, `builder.rs` should not contain:

```rust
use std::{collections::..., path::...};
use tracing::{info, warn};
use crate::db::models::{...};
use crate::generation::artifact::{...};
use crate::generation::metadata::{...};
#[cfg(test)]
mod tests { ... }
```

The final hub should contain only:

```rust
// conary-core/src/generation/builder.rs

//! Generation builder - creates EROFS images from system state.
//!
//! Public APIs are re-exported from focused child modules so callers can keep
//! using `conary_core::generation::builder::*`.

mod activation;
mod boot_assets;
mod cas;
mod create;
mod erofs;
mod initramfs;
mod kernel;
mod rebuild;
mod root_validation;
mod runtime_inputs;
mod sysroot;

#[cfg(test)]
pub(super) mod test_support;

pub use activation::GenerationActivation;
pub use create::{
    build_generation_from_db, build_generation_from_db_with_activation,
    build_generation_from_db_with_boot_root, build_generation_from_db_with_boot_root_and_activation,
};
pub use erofs::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image, hex_to_digest};
pub use kernel::detect_kernel_version_from_troves;

pub(crate) use rebuild::{rebuild_generation_image, rebuild_generation_image_with_boot_root};
```

- [ ] **Step 5: Verify Task 5**

Run:

```bash
cargo fmt --check
cargo check -p conary-core
cargo test -p conary-core --lib generation::builder -- --list
cargo test -p conary-core --lib generation::builder
rg -n "^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn " crates/conary-core/src/generation/builder.rs
rg -n -U "#\[cfg\(test\)\]\s*\n\s*mod tests" crates/conary-core/src/generation/builder.rs
```

Expected:

```text
48 tests listed
48 passed
no function bodies in builder.rs
no parent tests module in builder.rs
```

- [ ] **Step 6: Update source-text contract assertions**

`crates/conary-core/tests/generation_composefs_runtime_contract.rs` contains source-text assertions that currently read `crates/conary-core/src/generation/builder.rs`. After this decomposition, the asserted strings move into child modules, so update the tests in the same task that finalizes the hub:

```rust
let boot_assets_rs = fs::read_to_string(core_source("generation/builder/boot_assets.rs"))
    .expect("failed to read generation/builder/boot_assets.rs");
let sysroot_rs = fs::read_to_string(core_source("generation/builder/sysroot.rs"))
    .expect("failed to read generation/builder/sysroot.rs");
let initramfs_rs = fs::read_to_string(core_source("generation/builder/initramfs.rs"))
    .expect("failed to read generation/builder/initramfs.rs");
```

Apply these source-text target changes:

- In `generation_builder_stages_boot_assets_from_cas_sysroot_for_default_runtime_builds`:
  - check `boot_assets_rs` for `resolve_generation_boot_asset_sources(`.
  - check `sysroot_rs` for `materialize_runtime_generation_sysroot`.
  - check `initramfs_rs` for `.arg("--sysroot")` and `.arg("--kmoddir")`.
- In `runtime_generation_artifact_write_reuses_preverified_cas_inputs`:
  - read `generation/builder/create.rs` and `generation/builder/rebuild.rs`.
  - check both files for `verify_runtime_generation_cas_object_presence(generations_root, &cas_objects)?;`.
  - check both files for `cas_verification: CasObjectVerification::AlreadyVerified`.
  - keep the existing `artifact.rs` assertions unchanged.
- In `installed_generation_export_boot_assets_force_conary_initramfs`:
  - read `generation/builder/boot_assets.rs` instead of `generation/builder.rs`.
  - check `boot_assets.rs` for `InitramfsPolicy::GenerateConary`.
  - check `boot_assets.rs` for `resolve_generation_boot_asset_sources_with_tools`.

- [ ] **Step 7: Verify Task 5 contract coverage**

Run:

```bash
cargo test -p conary-core --test generation_composefs_runtime_contract
```

Expected: `19 passed`.

- [ ] **Step 8: Commit Task 5**

```bash
git add crates/conary-core/src/generation/builder.rs \
  crates/conary-core/src/generation/builder/rebuild.rs \
  crates/conary-core/tests/generation_composefs_runtime_contract.rs
git commit -m "refactor(core): extract generation rebuild orchestration"
```

---

### Task 6: Update Documentation Ownership Maps

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

- [ ] **Step 1: Update architecture module map**

In `docs/ARCHITECTURE.md`, update both the `crates/conary-core/src/generation/` file tree and the later generation module paragraph so they reference the new builder children:

```text
builder.rs (public generation-builder hub), builder/create.rs and
builder/rebuild.rs (generation creation and recovery rebuild orchestration),
builder/boot_assets.rs, builder/initramfs.rs, builder/kernel.rs, and
builder/sysroot.rs (runtime boot asset and sysroot materialization support),
builder/root_validation.rs and builder/runtime_inputs.rs (self-contained
runtime input validation), builder/erofs.rs (low-level EROFS construction),
artifact.rs (exportable generation contract and boot assets), export.rs ...
```

- [ ] **Step 2: Update assistant subsystem map**

In `docs/llms/subsystem-map.md`, expand the generation pointer list:

```text
`crates/conary-core/src/generation/builder.rs`,
`crates/conary-core/src/generation/builder/create.rs`,
`crates/conary-core/src/generation/builder/rebuild.rs`,
`crates/conary-core/src/generation/builder/boot_assets.rs`,
`crates/conary-core/src/generation/builder/initramfs.rs`,
`crates/conary-core/src/generation/builder/kernel.rs`,
`crates/conary-core/src/generation/builder/root_validation.rs`,
`crates/conary-core/src/generation/builder/runtime_inputs.rs`,
`crates/conary-core/src/generation/builder/erofs.rs`,
...
```

- [ ] **Step 3: Update feature ownership card**

In `docs/modules/feature-ownership.md`, update the `Generation Build, Switch, Recovery, And Export` `Start here` list to include:

```text
crates/conary-core/src/generation/builder.rs
crates/conary-core/src/generation/builder/create.rs
crates/conary-core/src/generation/builder/rebuild.rs
crates/conary-core/src/generation/builder/boot_assets.rs
crates/conary-core/src/generation/builder/initramfs.rs
crates/conary-core/src/generation/builder/kernel.rs
crates/conary-core/src/generation/builder/root_validation.rs
crates/conary-core/src/generation/builder/runtime_inputs.rs
crates/conary-core/src/generation/builder/erofs.rs
```

Keep the focused proof unchanged:

```text
cargo test -p conary-core generation::export
cargo test -p conary-core generation::builder
```

- [ ] **Step 4: Update post-generation roadmap wording only if needed**

If `docs/operations/post-generation-export-follow-up-roadmap.md` mentions `builder.rs` as the sole owner of generation-builder readiness, update that wording to mention the new child ownership. Do not add new roadmap promises.

- [ ] **Step 5: Refresh docs-audit files**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Update ledger notes for the docs touched in this task. The disposition counts should remain:

```text
archived 73
corrected 67
retained-historical 14
verified-no-change 12
```

Update existing ledger rows in place for docs touched by Task 6; do not add duplicate rows for `docs/ARCHITECTURE.md`, `docs/llms/subsystem-map.md`, `docs/modules/feature-ownership.md`, or `docs/operations/post-generation-export-follow-up-roadmap.md`. The only new ledger row in this phase is the Task 0 plan row, so the corrected count remains 67 after documentation ownership updates.

- [ ] **Step 6: Verify docs**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected:

```text
166
archived 73
corrected 67
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 7: Commit Task 6**

```bash
git add docs/ARCHITECTURE.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md \
  docs/operations/post-generation-export-follow-up-roadmap.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: record generation builder ownership"
```

---

### Task 7: Final Verification And Push

**Files:**
- Verify all files changed in Tasks 1-6.

- [ ] **Step 1: Format and compile**

Run:

```bash
cargo fmt --check
cargo check -p conary-core
cargo check --workspace --all-targets
```

Expected: all pass.

- [ ] **Step 2: Focused generation tests**

Run:

```bash
cargo test -p conary-core --lib generation::builder
cargo test -p conary-core generation::export
cargo test -p conary-core --test generation_composefs_runtime_contract
cargo test -p conary-core transaction::recovery
```

Expected:

```text
generation::builder: 48 passed
generation_composefs_runtime_contract: 19 passed
```

- [ ] **Step 3: Public caller compile checks**

Run:

```bash
cargo test -p conary --lib commands::generation
cargo test -p conary --lib commands::bootstrap
cargo test -p conary --lib commands::composefs_ops
cargo test -p conary-core --lib derivation::compose
cargo test -p conary-core --lib generation::delta
```

Expected: all pass or list/pass without compile errors. These prove the re-export surface still satisfies CLI/bootstrap/derivation/delta callers.

- [ ] **Step 4: Broad regression and lint gates**

Run:

```bash
cargo test -p conary-core --lib
cargo test --workspace --lib
cargo clippy -p conary-core --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all pass with zero warnings.

- [ ] **Step 5: Boundary checks**

Run:

```bash
scripts/line-count-report.sh 20
rg -n "^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn " crates/conary-core/src/generation/builder.rs
rg -n -U "#\[cfg\(test\)\]\s*\n\s*mod tests" crates/conary-core/src/generation/builder.rs
rg -n "use super::\*|use crate::\*" crates/conary-core/src/generation/builder.rs crates/conary-core/src/generation/builder
```

Expected:

```text
builder.rs drops out of the top hotspot position
no function bodies in builder.rs
no parent tests module in builder.rs
wildcard import hits, if any, are inside `#[cfg(test)]` modules only
```

- [ ] **Step 6: Docs-audit final check**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected:

```text
166
archived 73
corrected 67
retained-historical 14
verified-no-change 12
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 7: Push and prove synced main**

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
git status shows main...origin/main with no changes
git rev-parse HEAD origin/main prints the same SHA twice
git rev-list --left-right --count HEAD...origin/main prints 0 0
only /home/peter/Conary worktree is listed unless the user intentionally added another
```

---

## Self-Review Checklist

- All public generation builder APIs remain re-exported from `crates/conary-core/src/generation/builder.rs`.
- The crate-local recovery rebuild API remains available at `crate::generation::builder::rebuild_generation_image`.
- Existing `builder/erofs.rs` and `builder/runtime_inputs.rs` remain in place.
- No `builder/mod.rs` is introduced.
- `GenerationActivation::activates_state` visibility is widened to `pub(super)` for sibling orchestration.
- `RuntimeBootAssetSources` field visibility is widened only to `pub(super)`.
- The moved dracut `include_str!` paths account for the new file depth.
- All 17 parent tests are assigned exactly once:
  - `create.rs`: 4
  - `rebuild.rs`: 3
  - `root_validation.rs`: 1
  - `kernel.rs`: 2
  - `boot_assets.rs`: 7
- Existing child tests remain in place:
  - `erofs.rs`: 17
  - `runtime_inputs.rs`: 14
- Total focused builder test inventory remains 48.
- Docs-audit count math is explicit: the plan lock-in adds one corrected row, moving 165/66 to 166/67.
