# Phase 16 Install Module Completion Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose the remaining `apps/conary/src/commands/install/mod.rs` hotspot into focused install child modules while preserving `conary install`, restore preparation, direct CCS transaction install, batch install, update callers, and state restore behavior.

**Architecture:** Keep `apps/conary/src/commands/install/mod.rs` as the stable hub module and move the remaining orchestration, source-policy, validation, dependency-flow, execution-path, lifecycle, transaction, option, and semantic helpers into sibling files under `apps/conary/src/commands/install/`. Preserve the existing public re-export contract from `apps/conary/src/commands/mod.rs` and preserve the sibling-module imports used by `batch.rs`, `inner.rs`, `restore.rs`, `ccs_transaction.rs`, `conversion.rs`, and `prepare.rs`.

**Tech Stack:** Rust 2024 workspace modules, `anyhow`, `rusqlite`, `tokio`, Conary DB models, `conary_core::repository`, `conary_core::transaction`, `conary_core::scriptlet`, existing Conary command re-export pattern.

---

## Current Repo Facts

- Baseline SHA before this plan draft: `601bf9d1a00102222d521959c841a9963bb2ab91`.
- `HEAD` and `origin/main` match at the baseline SHA.
- Current hotspot ranking from `scripts/line-count-report.sh 20`:
  - `apps/conary/src/commands/install/mod.rs` - 2874 lines.
  - `crates/conary-core/src/scriptlet/mod.rs` - 2408 lines.
  - `apps/conaryd/src/daemon/routes.rs` - 2345 lines.
  - `apps/conary/src/commands/model.rs` - 2260 lines.
  - `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` - 2178 lines.
- Current install child module tree:
  - `batch.rs`
  - `blocklist.rs`
  - `ccs_transaction.rs`
  - `conversion.rs`
  - `dep_mode.rs`
  - `dep_resolution.rs`
  - `dependencies.rs`
  - `execute.rs`
  - `inner.rs`
  - `legacy_replay.rs`
  - `prepare.rs`
  - `resolve.rs`
  - `restore.rs`
  - `scriptlets.rs`
  - `system_pm.rs`
- Current public command re-export from `apps/conary/src/commands/mod.rs`:
  - `pub use install::{DepMode, InstallOptions, LegacyReplayOptions, cmd_install};`
- Current direct install call sites outside `install/` include:
  - `apps/conary/src/dispatch.rs`
  - `apps/conary/src/commands/automation.rs`
  - `apps/conary/src/commands/collection.rs`
  - `apps/conary/src/commands/model/apply.rs`
  - `apps/conary/src/commands/update/package.rs`
  - `apps/conary/src/commands/state.rs`
- Current non-public install helpers used by sibling or neighbor modules:
  - `repository_install_provenance_from_package` from `update/package.rs` and `install/conversion.rs`.
  - `resolve_default_dep_mode_from_model` from `update/package.rs` and `update/collection.rs`.
  - `InstallSemantics`, `PreparedSourceKind`, `ExtractionResult`, `TransactionContext`, `ScriptletContext`, `PreScriptletState`, and `InstallTransactionResult` from install child modules.
  - `prepare_install_environment_before_scriptlets`, `preflight_extracted_live_root_file_ownership`, `live_root_files_from_stored_files`, `run_triggers`, `mark_upgraded_parent_deriveds_stale`, `build_resolution_policy`, `resolve_canonical_name`, `extract_and_classify_files`, `run_pre_install_phase`, `show_dry_run_summary`, `execute_install_transaction`, and `finalize_install_without_snapshot` from install child modules.
- Current remaining `install/mod.rs` test inventory:
  - `cargo test -p conary --lib commands::install::tests -- --list`
  - Expected: 20 tests, 0 benchmarks.
- Current docs-audit baseline before locking this plan:
  - Inventory: 159 tracked doc-like files.
  - Ledger categories: `corrected 59`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
  - After lock-in of this plan: 160 tracked doc-like files, `corrected 60`.
- Current baseline compile check:
  - `cargo check -p conary`
  - Expected: passes.

## Why This Hotspot

`apps/conary/src/commands/install/mod.rs` is now the largest Rust source file in the workspace. Earlier phases extracted legacy replay, direct CCS transaction install, restore preparation, inner transaction row insertion, batch install, and CCS install. The remaining hub still owns several distinct concerns:

- Public `cmd_install` orchestration.
- Install options and repository provenance.
- Package source-policy overlay and canonical name resolution.
- Component/adopted-package validation and dependency promotion.
- Package acquisition and legacy-to-CCS handoff.
- Dependency resolution and adoption/install side effects.
- Mutable live-root versus generation-aware execution path checks.
- Extraction, scriptlet lifecycle, trigger execution, and state snapshot finalization.
- Install transaction execution and CCS manifest provide persistence.

These concerns have clear call boundaries and can move without behavior changes if the hub keeps stable re-exports.

## Alternatives Considered

### Option A: Complete The Install Hub Split

Move every remaining production helper and the 20 direct tests out of `install/mod.rs`, leaving it as a module declaration and re-export hub.

**Pros:** Removes the current top hotspot in one `/goal`, gives future agents narrow owner files, and follows the successful Phase 13 and Phase 15 pattern.

**Cons:** Larger visibility and import surface than a single helper extraction. Several existing child modules must keep compiling through hub re-exports.

**Recommendation:** Choose this option.

### Option B: Move Only `cmd_install`

Create `install/command.rs` and move only `InstallOptions`, `cmd_install`, and directly needed helpers.

**Pros:** Smaller first slice.

**Cons:** Leaves most helper clusters in `mod.rs`, keeps it as a hotspot, and creates another immediate follow-up phase.

### Option C: Skip To Core Scriptlet Or conaryd Routes

Move to the next non-install hotspot.

**Pros:** Avoids the complicated install visibility surface.

**Cons:** Leaves the largest file and the install docs routes stale after the recent CCS and update decompositions.

## Non-Goals

- Do not change `conary install` CLI flags, output text, dependency semantics, source-policy behavior, component selection behavior, adopted-package refusal behavior, live-root safety behavior, generation publication behavior, state snapshot behavior, scriptlet execution behavior, or legacy replay behavior.
- Do not rename `apps/conary/src/commands/install/mod.rs` to `apps/conary/src/commands/install.rs`.
- Do not move or rewrite existing major child modules such as `batch.rs`, `restore.rs`, `inner.rs`, `ccs_transaction.rs`, `conversion.rs`, `legacy_replay.rs`, `resolve.rs`, `prepare.rs`, `scriptlets.rs`, or `dep_resolution.rs` except for import updates needed by the split.
- Do not move `InstalledPackageSelector` or package-target code.
- Do not add schema migrations.
- Do not add new runtime behavior tests unless a focused compile/list test is needed to prove the split.
- Do not archive this plan during implementation.

## File Structure After Implementation

```text
apps/conary/src/commands/install/
  mod.rs
  acquire.rs
  batch.rs
  blocklist.rs
  ccs_transaction.rs
  command.rs
  conversion.rs
  dep_mode.rs
  dep_resolution.rs
  dependencies.rs
  execute.rs
  inner.rs
  legacy_replay.rs
  lifecycle.rs
  options.rs
  prepare.rs
  resolve.rs
  restore.rs
  scriptlets.rs
  semantics.rs
  source_policy.rs
  transaction.rs
  validation.rs
  system_pm.rs
```

Every new Rust file created by this plan must start with the repository-standard path comment:

```rust
// src/commands/install/<file>.rs
```

### `apps/conary/src/commands/install/mod.rs`

Hub only. It owns module declarations and stable re-exports.

Required production module declarations:

```rust
mod acquire;
mod batch;
mod blocklist;
mod ccs_transaction;
mod command;
mod conversion;
mod dep_mode;
mod dep_resolution;
mod dependencies;
mod execute;
mod inner;
mod legacy_replay;
mod lifecycle;
mod options;
mod prepare;
mod resolve;
mod restore;
mod scriptlets;
mod semantics;
mod source_policy;
mod system_pm;
mod transaction;
mod validation;
```

Required public and crate-visible re-export surface:

```rust
pub use batch::{BatchInstaller, prepare_package_for_batch};
pub use blocklist::is_blocked as is_package_blocked;
pub(crate) use ccs_transaction::{
    CcsTransactionInstallOptions, CcsTransactionInstallResult, install_ccs_package_transactionally,
};
pub use command::cmd_install;
pub use dep_mode::DepMode;
pub(crate) use dependencies::resolve_default_dep_mode_from_model;
pub use legacy_replay::LegacyReplayOptions;
pub(crate) use legacy_replay::{
    AcceptedLegacyBundleInstall, LegacyReplayAuditContext, LegacyReplayInstallState,
};
pub(super) use legacy_replay::{
    merge_old_upgrade_legacy_replay_state, plan_ccs_fresh_install_legacy_replay,
    plan_ccs_old_installed_upgrade_legacy_replay,
};
pub use options::InstallOptions;
pub(crate) use options::{
    RepositoryInstallProvenance, repository_install_provenance_from_package,
};
pub use prepare::{ComponentSelection, UpgradeCheck};
pub(crate) use restore::{
    add_prepared_install_to_target_state, build_target_state_view,
    finalize_prepared_install_without_snapshot, install_prepared_inner,
    prepare_install_for_restore, run_pre_install_for_prepared,
    validate_prepared_install_dependencies,
};
```

Required hub-private sibling alias surface:

```rust
use execute::{
    PackageExecutionPath, live_root_files_from_stored_files,
    preflight_extracted_live_root_file_ownership, prepare_install_environment_before_scriptlets,
    run_triggers,
};
use lifecycle::{
    ExtractionResult, PreScriptletState, ScriptletContext, extract_and_classify_files,
    finalize_install, finalize_install_without_snapshot, mark_upgraded_parent_deriveds_stale,
    run_pre_install_phase, show_dry_run_summary,
};
use prepare::check_upgrade_status;
use semantics::{InstallSemantics, PreparedSourceKind, scheme_to_string};
use source_policy::{build_resolution_policy, resolve_canonical_name};
use super::progress::{InstallPhase, InstallProgress};
use super::{PackageFormatType, detect_package_format};
use transaction::{
    InstallTransactionResult, TransactionContext, execute_install_transaction,
};
```

`install/mod.rs` must not keep `cmd_install`, direct helper function bodies, or the `#[cfg(test)] mod tests` block after Task 6.

Within Tasks 1 through 6, treat each extraction task as one mechanical move. Do not run `cargo check` after adding hub re-exports but before removing the original definitions from `install/mod.rs`; duplicate names are expected until each task's removal step is complete.

Keep the hub-private alias surface as private `use`, not `pub(super) use`. The
moved child-module items are intentionally only `pub(super)` inside their owner
modules; a `pub(super) use` from `install/mod.rs` would try to re-export them to
`commands/`, which is wider than their definition visibility and fails with
E0364. Private hub aliases remain visible to descendant install child modules,
including their tests.

### `apps/conary/src/commands/install/options.rs`

Owns install options and repository provenance:

- `pub struct InstallOptions<'a>`
- `pub(crate) struct RepositoryInstallProvenance`
- `pub(crate) fn repository_install_provenance_from_package(...) -> Result<RepositoryInstallProvenance>`

Keep all existing `InstallOptions` fields and visibility unchanged. Keep `RepositoryInstallProvenance` fields `pub` inside the `pub(crate)` struct exactly as they are now because `batch.rs`, `conversion.rs`, `inner.rs`, `update/package.rs`, and tests construct or read those values.

Import surface:

```rust
// src/commands/install/options.rs

use super::{DepMode, LegacyReplayOptions};
use anyhow::Result;
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::repository::versioning::{VersionScheme, resolve_package_version_scheme};
use conary_core::scriptlet::SandboxMode;
```

### `apps/conary/src/commands/install/semantics.rs`

Owns source semantics and version-scheme string rendering:

- `pub(super) enum PreparedSourceKind`
- `pub(super) struct InstallSemantics`
- `impl InstallSemantics`
- `pub(super) fn scheme_to_string(...) -> String`

Visibility requirements:

- `PreparedSourceKind` must be `pub(super)` because `inner.rs` matches on `super::PreparedSourceKind`.
- `InstallSemantics` must be `pub(super)`.
- `InstallSemantics` fields `source`, `version_scheme`, and `scriptlet_format` must be `pub(super)` because `prepare.rs`, `inner.rs`, `scriptlets` lifecycle code, `restore.rs`, and `ccs_transaction.rs` read them.
- `InstallSemantics::legacy` and `InstallSemantics::ccs` must be `pub(super)` because siblings construct semantics after the move.

Import surface:

```rust
// src/commands/install/semantics.rs

use super::{PackageFormatType, prepare, scriptlets::to_scriptlet_format};
use conary_core::repository::versioning::VersionScheme;
use conary_core::scriptlet::PackageFormat as ScriptletPackageFormat;
```

Update the moved `InstallSemantics::legacy` body to keep using the prepare helper through the sibling module:

```rust
version_scheme: prepare::version_scheme_for_format(format),
```

### `apps/conary/src/commands/install/source_policy.rs`

Owns source-policy request scoping and canonical package name selection:

- `pub(super) fn build_resolution_policy(...) -> ResolutionPolicy`
- `pub(super) fn resolve_canonical_name(...) -> Result<Option<String>>`
- `fn distro_name_to_flavor(...) -> Option<RepositoryDependencyFlavor>`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// src/commands/install/source_policy.rs

use anyhow::Result;
use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
use conary_core::repository::distro::flavor_from_distro_name;
use conary_core::repository::resolution_policy::{RequestScope, ResolutionPolicy};
use tracing::{info, warn};
```

Move these two tests into `source_policy.rs`:

- `distro_name_to_flavor_known`
- `distro_name_to_flavor_unknown`

Apply these path updates while moving the bodies so the imports above are used:

- Shorten `conary_core::repository::resolution_policy::ResolutionPolicy` to
  `ResolutionPolicy` in `build_resolution_policy` and `resolve_canonical_name`.
- Remove the nested `use conary_core::repository::resolution_policy::RequestScope;`
  from `build_resolution_policy` and use the module import.
- Shorten the `distro_name_to_flavor` return type to
  `Option<RepositoryDependencyFlavor>`.
- Replace the body call to
  `conary_core::repository::distro::flavor_from_distro_name(distro)` with
  `flavor_from_distro_name(distro)`.
- Remove the now-redundant inner
  `use conary_core::repository::dependency_model::RepositoryDependencyFlavor;`
  inside `distro_name_to_flavor_known`; the module-level import covers it.

### `apps/conary/src/commands/install/validation.rs`

Owns pre-resolution argument validation and installed dependency promotion:

- `pub(super) fn parse_component_and_validate(...) -> Result<(String, ComponentSelection)>`
- `pub(super) fn try_promote_existing_dep(...) -> Result<bool>`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// src/commands/install/validation.rs

use super::{ComponentSelection, DepMode, blocklist};
use anyhow::Result;
use conary_core::components::{ComponentType, parse_component_spec};
use conary_core::db::models::{InstallReason, Trove};
use conary_core::packages::SystemPackageManager;
use tracing::info;
```

Move these two tests into `validation.rs`:

- `force_install_over_adopted_package_is_not_silent_takeover`
- `explicit_takeover_over_adopted_package_is_allowed`

Apply these path updates while moving the bodies so the imports above are used:

- Replace `conary_core::db::models::Trove::find_one_by_name(...)` with
  `Trove::find_one_by_name(...)`.
- Replace `conary_core::db::models::InstallReason::Dependency` with
  `InstallReason::Dependency`.
- Replace `conary_core::packages::SystemPackageManager::detect()` with
  `SystemPackageManager::detect()`.

### `apps/conary/src/commands/install/acquire.rs`

Owns package acquisition, direct CCS handoff, optional conversion handoff, and "did you mean" suggestions:

- `pub(super) struct CcsInstallParams<'a>`
- `pub(super) async fn resolve_and_parse_package(...) -> Result<Option<(Box<dyn PackageFormat>, PackageFormatType, Option<RepositoryInstallProvenance>)>>`
- `fn install_provenance_from_resolved(...) -> Option<RepositoryInstallProvenance>`
- `fn find_package_suggestions(...) -> std::result::Result<Vec<(String, String)>, rusqlite::Error>`
- `fn print_package_suggestions(...)`

Visibility requirements:

- `CcsInstallParams` and all fields must be `pub(super)` because `command.rs` constructs it and `acquire.rs` reads it.
- `resolve_and_parse_package` must be `pub(super)` because `command.rs` calls it.
- Suggestion helpers stay private.

Import surface:

```rust
// src/commands/install/acquire.rs

use super::conversion::{
    ConversionResult, ConvertedCcsInstallOptions, DEFAULT_CCS_DEPENDENCY_PASSES,
    install_converted_ccs, try_convert_to_ccs,
};
use super::prepare::parse_package;
use super::resolve::{
    PolicyOptions, ResolutionOutcome, ResolvedPackage, ResolvedSourceType,
    resolve_package_path_with_policy,
};
use super::{
    DepMode, InstallPhase, InstallProgress, LegacyReplayOptions, PackageFormatType,
    RepositoryInstallProvenance, detect_package_format,
};
use anyhow::{Context, Result};
use conary_core::packages::PackageFormat;
use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
use conary_core::repository::resolution_policy::ResolutionPolicy;
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use tracing::info;
```

Keep the existing `#[allow(clippy::too_many_arguments)]` on `resolve_and_parse_package`.

Apply these path updates while moving the bodies so the imports above are used:

- Shorten `policy: &conary_core::repository::resolution_policy::ResolutionPolicy`
  to `policy: &ResolutionPolicy`.
- Shorten
  `primary_flavor: Option<conary_core::repository::dependency_model::RepositoryDependencyFlavor>`
  to `primary_flavor: Option<RepositoryDependencyFlavor>`.
- Shorten the return type from `Box<dyn conary_core::packages::PackageFormat>`
  to `Box<dyn PackageFormat>`.
- Remove the local `use std::collections::HashMap;` inside
  `find_package_suggestions`; the module-level import provides it.

Update the moved `install_provenance_from_resolved` signature from the old
parent-module path:

```rust
fn install_provenance_from_resolved(
    resolved: &resolve::ResolvedPackage,
) -> Option<RepositoryInstallProvenance>
```

to:

```rust
fn install_provenance_from_resolved(
    resolved: &ResolvedPackage,
) -> Option<RepositoryInstallProvenance>
```

### `apps/conary/src/commands/install/dependencies.rs`

Expand the existing dependency extraction module into the owner for direct-install dependency flow:

Existing owner remains:

- `pub struct RuntimeDep`
- `pub fn extract_runtime_deps(...) -> Vec<RuntimeDep>`

Move these functions from `install/mod.rs`:

- `pub(crate) fn resolve_default_dep_mode_from_model() -> DepMode`
- `fn classify_dep_type(...) -> &'static str`
- `fn report_provides_check(...) -> Result<()>`
- `pub(super) async fn handle_dependencies(...) -> Result<()>`
- `fn missing_repository_deps_from_sat_result(...) -> Vec<MissingDependency>`
- `async fn handle_dep_adoptions(...)`
- `async fn handle_dep_installs(...) -> Result<()>`
- `fn check_unresolvable_deps(...) -> Result<()>`

Move or define the context:

- `pub(super) struct DepAnalysisContext<'a>`

Visibility requirements:

- `DepAnalysisContext` and all fields must be `pub(super)` because `command.rs` constructs it and `dependencies.rs` reads it.
- `handle_dependencies` must be `pub(super)` because `command.rs` calls it.
- `missing_repository_deps_from_sat_result` stays private, with tests in the same module.

Additional imports to add to the existing file:

```rust
use super::{
    BatchInstaller, DepMode, InstallPhase, InstallProgress, LegacyReplayOptions,
    PackageExecutionPath, prepare_package_for_batch, repository_install_provenance_from_package,
};
use super::dep_resolution;
use anyhow::{Context, Result};
use conary_core::db::paths::keyring_dir;
use conary_core::repository;
use conary_core::resolver::{MissingDependency, SatResolution, SatSource};
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use tempfile::TempDir;
use tracing::{debug, info, warn};
```

Also add:

```rust
use super::resolve::check_provides_dependencies;
```

Preserve the existing `#[allow(dead_code)]` on `report_provides_check`; it is still an intentionally retained diagnostic helper and would otherwise fail the clippy `-D warnings` gate.

Apply these path updates while moving the bodies so the imports above are used:

- Shorten `sat_result: &conary_core::resolver::SatResolution` to
  `sat_result: &SatResolution`.
- Replace `conary_core::resolver::SatSource::Repository` with
  `SatSource::Repository`.

Move these eight tests into `dependencies.rs`:

- `classify_dep_type_packages`
- `classify_dep_type_capabilities`
- `classify_dep_type_files`
- `classify_dep_type_rpmlib`
- `classify_dep_type_conditional`
- `classify_dep_type_or_group`
- `missing_model_uses_preview_convergence_dep_mode`
- `missing_repository_deps_preserve_sat_selected_version`

### `apps/conary/src/commands/install/execute.rs`

Keep the existing file and add execution-path, live-root recovery, preflight, CAS-to-live-root, and trigger helpers:

Existing owner remains:

- `convert_extracted_files`
- `get_files_to_remove`

Move these items from `install/mod.rs`:

- `pub(super) enum PackageExecutionPath`
- `fn package_execution_path(...) -> Result<PackageExecutionPath>`
- `pub(super) fn prepare_install_environment_before_scriptlets(...) -> Result<PackageExecutionPath>`
- `fn recover_mutable_journals_before_scriptlets(...) -> Result<()>`
- `pub(super) fn preflight_extracted_live_root_file_ownership(...) -> Result<()>`
- `pub(super) fn live_root_files_from_stored_files(...) -> Result<Vec<LiveRootFile>>`
- `pub(super) fn run_triggers(...)`
- `#[cfg(test)] mod tests`

Visibility requirements:

- `PackageExecutionPath`, `prepare_install_environment_before_scriptlets`, `preflight_extracted_live_root_file_ownership`, `live_root_files_from_stored_files`, and `run_triggers` must be `pub(super)` because sibling modules call them.
- `recover_mutable_journals_before_scriptlets` can stay private because its test moves inside this module.

Update the existing `use anyhow::Result;` import to `use anyhow::{Context, Result};`, then add the remaining imports:

```rust
use super::inner;
use super::ExtractionResult;
use crate::commands::LiveRootFile;
use conary_core::filesystem::CasStore;
use conary_core::packages::PackageFormat;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
```

Apply these path updates while moving the bodies:

- Replace `super::live_root::recover_pending_journals_with_changesets(...)` with `crate::commands::live_root::recover_pending_journals_with_changesets(...)`.
- Shorten the `preflight_extracted_live_root_file_ownership` package argument from `&dyn conary_core::packages::PackageFormat` to `&dyn PackageFormat` so the added import is used.
- Shorten the CAS argument in `live_root_files_from_stored_files` from `&conary_core::filesystem::CasStore` to `&CasStore` so the added import is used.
- Shorten `Vec<crate::commands::LiveRootFile>` and
  `crate::commands::LiveRootFile { ... }` in `live_root_files_from_stored_files`
  to `Vec<LiveRootFile>` and `LiveRootFile { ... }` so the added import is used.

Move these four tests into `execute.rs`:

- `recover_mutable_journals_runs_before_scriptlets`
- `live_root_files_are_loaded_from_stored_cas_objects`
- `package_execution_path_fails_closed_on_invalid_generation_state`
- `package_execution_path_fails_closed_on_dangling_current_symlink`

Keep their existing fully qualified `tempfile::tempdir()` calls. No module-level
`use tempfile;` import is required.

### `apps/conary/src/commands/install/lifecycle.rs`

Owns extraction, scriptlet phase orchestration, trigger finalization, snapshot follow-up, and derived package stale marking:

- `pub(super) struct ScriptletContext<'a>`
- `pub(super) struct PreScriptletState`
- `pub(super) struct ExtractionResult`
- `pub(super) fn mark_upgraded_parent_deriveds_stale(...)`
- `pub(super) fn show_dry_run_summary(...)`
- `pub(super) fn extract_and_classify_files(...) -> Result<ExtractionResult>`
- `pub(super) fn run_pre_install_phase(...) -> Result<PreScriptletState>`
- `pub(super) fn finalize_install_without_snapshot(...) -> Result<()>`
- `pub(super) fn finalize_install(...) -> Result<()>`

Visibility requirements:

- `ExtractionResult` and all fields must be `pub(super)` because `ccs_transaction.rs`, `restore.rs`, `inner.rs`, `transaction.rs`, and tests construct or read them.
- `ScriptletContext` and all fields must be `pub(super)` because `command.rs`, `restore.rs`, and `ccs_transaction.rs` construct it and lifecycle functions read it.
- `PreScriptletState` must be `pub(super)`. Its fields can stay private if only lifecycle functions read them.

Import surface:

```rust
// src/commands/install/lifecycle.rs

use super::scriptlets::{
    build_execution_mode, get_old_package_scriptlets, preflight_install_scriptlets,
    preflight_old_remove_scriptlets, run_old_post_remove, run_old_pre_remove, run_post_install,
    run_pre_install,
};
use super::{
    ComponentSelection, InstallPhase, InstallProgress, InstallSemantics, InstallTransactionResult,
    run_triggers,
};
use anyhow::{Context, Result};
use conary_core::components::{ComponentClassifier, ComponentType, should_run_scriptlets};
use conary_core::db::models::DerivedPackage;
use conary_core::dependencies::{LanguageDep, LanguageDepDetector};
use conary_core::packages::PackageFormat;
use conary_core::packages::traits::ExtractedFile;
use conary_core::scriptlet::{ExecutionMode, PackageFormat as ScriptletPackageFormat, SandboxMode};
use crate::commands::create_state_snapshot;
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};
```

Apply these path/type updates while moving the bodies:

- Use the imported `create_state_snapshot(...)` in `finalize_install`.
- Shorten `PreScriptletState` field types to the imported `ScriptletPackageFormat` and `ExecutionMode`, or remove those imports if fully qualified types are kept.
- Shorten all lifecycle package arguments from
  `&dyn conary_core::packages::PackageFormat` to `&dyn PackageFormat`.
- Shorten `ExtractionResult.extracted_files` from
  `Vec<conary_core::packages::traits::ExtractedFile>` to `Vec<ExtractedFile>`.
- Shorten `ExtractionResult.language_provides` from
  `Vec<conary_core::dependencies::LanguageDep>` to `Vec<LanguageDep>`.

### `apps/conary/src/commands/install/transaction.rs`

Owns the main install transaction execution and CCS manifest provide persistence:

- `pub(super) struct TransactionContext<'a>`
- `pub(super) struct InstallTransactionResult`
- `pub(super) fn execute_install_transaction(...) -> Result<InstallTransactionResult>`
- `fn persist_ccs_manifest_provides(...) -> Result<()>`
- `fn insert_ccs_manifest_typed_provide(...) -> Result<()>`
- `#[cfg(test)] mod tests`

Visibility requirements:

- `TransactionContext` and all fields must be `pub(super)` because `command.rs`, `restore.rs`, `ccs_transaction.rs`, and `inner.rs` construct or read it.
- `InstallTransactionResult` and field `changeset_id` must be `pub(super)` because `restore.rs` constructs it and lifecycle functions read it.
- `execute_install_transaction` must be `pub(super)` because `command.rs` and `ccs_transaction.rs` call it.
- Preserve the existing `#[allow(dead_code)]` attribute on the
  `accepted_legacy_bundle` field of `TransactionContext`.

Import surface:

```rust
// src/commands/install/transaction.rs

use super::{
    AcceptedLegacyBundleInstall, ExtractionResult, InstallProgress, InstallSemantics,
    LegacyReplayOptions, PackageExecutionPath, RepositoryInstallProvenance, inner,
    live_root_files_from_stored_files,
};
use anyhow::{Context, Result};
use conary_core::db::models::{Changeset, ChangesetStatus, ProvideEntry};
use conary_core::dependencies::DependencyClass;
use conary_core::packages::PackageFormat;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::path::{Path, PathBuf};
use tracing::info;
```

Apply this parent-module path update while moving the body:

- Replace `super::live_root::recover_pending_journals_with_changesets(...)` with `crate::commands::live_root::recover_pending_journals_with_changesets(...)`.
- Shorten all transaction package arguments from
  `&dyn conary_core::packages::PackageFormat` to `&dyn PackageFormat`.
- Replace `conary_core::db::models::ProvideEntry::new(...)` and
  `conary_core::db::models::ProvideEntry::new_typed(...)` with
  `ProvideEntry::new(...)` and `ProvideEntry::new_typed(...)`.

Move these two tests into `transaction.rs`:

- `no_generation_install_transaction_materializes_live_root_file`
- `no_generation_install_conflict_preflight_preserves_live_root_file`

Keep their existing fully qualified `tempfile::tempdir()` calls. No module-level
`use tempfile;` import is required.

Add this import inside `transaction.rs` `#[cfg(test)] mod tests` because these
tests construct `InstallSemantics::legacy(PackageFormatType::Rpm)`:

```rust
use crate::commands::PackageFormatType;
```

### `apps/conary/src/commands/install/command.rs`

Owns the public `conary install` command orchestration:

- `pub async fn cmd_install(package: &str, opts: InstallOptions<'_>) -> Result<()>`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// src/commands/install/command.rs

use super::acquire::{CcsInstallParams, resolve_and_parse_package};
use super::dependencies::{DepAnalysisContext, handle_dependencies};
use super::prepare::check_upgrade_status;
use super::validation::{parse_component_and_validate, try_promote_existing_dep};
use super::{
    InstallOptions, InstallProgress, InstallSemantics, ScriptletContext, TransactionContext,
    UpgradeCheck, build_resolution_policy, execute_install_transaction,
    extract_and_classify_files, finalize_install, preflight_extracted_live_root_file_ownership,
    prepare_install_environment_before_scriptlets, resolve_canonical_name,
    resolve_default_dep_mode_from_model, run_pre_install_phase,
};
use crate::commands::open_db;
use anyhow::Result;
use conary_core::components::parse_component_spec;
use conary_core::repository::resolution_policy::RequestScope;
```

Apply these path updates while moving the body:

- Replace `super::hint_unconfigured_source_policy()` with `crate::commands::hint_unconfigured_source_policy()`.
- Replace `conary_core::repository::resolution_policy::RequestScope::Any` with `RequestScope::Any`, or remove the `RequestScope` import if the fully qualified path is kept.

Keep the existing `#[allow(clippy::too_many_arguments)]` attributes on moved helpers in their destination modules. `cmd_install` currently does not need that attribute.

Move these two ordering tests into `command.rs` and update their source scan from `include_str!("mod.rs")` to `include_str!("command.rs")`:

- `package_execution_path_is_prepared_before_dependency_handling`
- `direct_install_preflights_live_root_ownership_before_scriptlets`

The tests should search the whole `command.rs` source instead of relying on the old `"// ---------------------------------------------------------------------------"` helper boundary. Expected string ordering remains:

- `prepare_install_environment_before_scriptlets` before `handle_dependencies(&dep_ctx).await?`.
- `extract_and_classify_files` before `preflight_extracted_live_root_file_ownership` before `run_pre_install_phase`.

## Task 0: Lock In The Plan And Docs-Audit Baseline

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Add: `docs/superpowers/plans/archive/2026-06-07-project-maintainability-phase16-install-module-completion-decomposition-plan.md`

- [ ] **Step 1: Stage the plan file before inventory regeneration**

```bash
git add docs/superpowers/plans/archive/2026-06-07-project-maintainability-phase16-install-module-completion-decomposition-plan.md
```

- [ ] **Step 2: Add a ledger row for the Phase 16 plan**

Add the row near the active maintainability plan rows, immediately after the Phase 15 row. The row must use literal tab characters and exactly 9 fields:

```tsv
docs/superpowers/plans/archive/2026-06-07-project-maintainability-phase16-install-module-completion-decomposition-plan.md	docs/superpowers/plans/archive/2026-06-07-project-maintainability-phase16-install-module-completion-decomposition-plan.md	planning	maintainer	maintainability; phase16; install-module; hotspot-decomposition; native-install; live-root-safety	apps/conary/src/commands/install/mod.rs; apps/conary/src/commands/install/batch.rs; apps/conary/src/commands/install/inner.rs; apps/conary/src/commands/install/restore.rs; apps/conary/src/commands/install/ccs_transaction.rs; apps/conary/src/commands/install/conversion.rs; apps/conary/src/commands/update/package.rs; apps/conary/src/commands/update/collection.rs; apps/conary/tests/bundle_replay.rs; apps/conary/tests/live_host_mutation_safety.rs; apps/conary/tests/workflow.rs; scripts/line-count-report.sh; docs/modules/feature-ownership.md; docs/modules/source-selection.md; docs/llms/subsystem-map.md	verified	corrected	Added the Phase 16 install module completion decomposition plan to finish splitting install/mod.rs into focused command, acquisition, validation, dependency-flow, execution-path, lifecycle, transaction, option, semantic, and source-policy owners while preserving public install routes and sibling helper re-exports.
```

- [ ] **Step 3: Refresh the docs-audit inventory**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: inventory has 160 tracked doc-like rows after the plan is staged.

- [ ] **Step 4: Update the docs-audit summary**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, insert this paragraph after the existing Phase 15 paragraph in the `2026-06-06 Maintainability Planning` section:

```markdown
The Phase 16 install module completion decomposition plan targets the current
largest Rust hotspot, `apps/conary/src/commands/install/mod.rs`. It keeps
`install/mod.rs` as the stable module hub while planning focused owners for
command orchestration, acquisition and conversion handoff, validation,
dependency flow, execution-path safety, lifecycle/finalization, transaction
execution, install options, source semantics, and source-policy resolution.
```

Then update the final counts at the bottom of the same file:

```diff
- Total tracked doc-like files audited: 159
+ Total tracked doc-like files audited: 160
  - `verified-no-change`: 13
- - `corrected`: 59
+ - `corrected`: 60
  - `archived`: 73
  - `retained-historical`: 14
  - Remaining pending rows: 0
```

- [ ] **Step 4.5: Refresh the docs-audit summary ledger row**

Update the existing row for `docs/superpowers/documentation-accuracy-audit-summary.md` in `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

- Add the Phase 16 plan path to `evidence_sources`:
  `docs/superpowers/plans/archive/2026-06-07-project-maintainability-phase16-install-module-completion-decomposition-plan.md`
- Add `phase16` and `install-module-completion` to the `tags` field.
- Append a note fragment in the existing style:
  `and the Phase 16 install module completion decomposition.`

- [ ] **Step 5: Verify docs-audit lock-in math**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
```

Expected:

- Inventory count prints `160`.
- Ledger counts include `corrected 60`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
- Malformed-row check prints nothing.
- Ledger check passes.
- `git diff --cached --check` exits 0.

- [ ] **Step 6: Commit the locked plan**

```bash
git add docs/superpowers/documentation-accuracy-audit-ledger.tsv \
        docs/superpowers/documentation-accuracy-audit-summary.md \
        docs/superpowers/documentation-accuracy-audit-inventory.tsv \
        docs/superpowers/plans/archive/2026-06-07-project-maintainability-phase16-install-module-completion-decomposition-plan.md
git commit -m "docs: plan install module completion"
```

## Task 1: Extract Options, Semantics, And Source Policy

**Files:**
- Create: `apps/conary/src/commands/install/options.rs`
- Create: `apps/conary/src/commands/install/semantics.rs`
- Create: `apps/conary/src/commands/install/source_policy.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/prepare.rs`
- Modify: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`

- [ ] **Step 1: Add module declarations and hub re-exports**

In `apps/conary/src/commands/install/mod.rs`, add:

```rust
mod options;
mod semantics;
mod source_policy;
```

Add hub re-exports and private sibling aliases:

```rust
pub use options::InstallOptions;
pub(crate) use options::{
    RepositoryInstallProvenance, repository_install_provenance_from_package,
};
use semantics::{InstallSemantics, PreparedSourceKind, scheme_to_string};
use source_policy::{build_resolution_policy, resolve_canonical_name};
```

Do not run `cargo check` while both the original definitions and the hub re-exports exist. Treat Steps 1 through 5 as one mechanical move: create the destination files, add the re-exports, and remove the original definitions before the first compile.

- [ ] **Step 2: Create `options.rs`**

Move `InstallOptions`, `RepositoryInstallProvenance`, and `repository_install_provenance_from_package` from `install/mod.rs` into `options.rs`. Use the import surface listed above.

- [ ] **Step 3: Create `semantics.rs`**

Move `PreparedSourceKind`, `InstallSemantics`, and `scheme_to_string` from `install/mod.rs` into `semantics.rs`. Apply the visibility requirements listed above.

- [ ] **Step 4: Create `source_policy.rs`**

Move `build_resolution_policy`, `resolve_canonical_name`, `distro_name_to_flavor`, and the two distro-flavor tests into `source_policy.rs`.

- [ ] **Step 5: Remove moved definitions from `install/mod.rs`**

After the new files compile, delete the original definitions for:

- `InstallOptions`
- `RepositoryInstallProvenance`
- `repository_install_provenance_from_package`
- `PreparedSourceKind`
- `InstallSemantics`
- `scheme_to_string`
- `build_resolution_policy`
- `resolve_canonical_name`
- `distro_name_to_flavor`
- `distro_name_to_flavor_known`
- `distro_name_to_flavor_unknown`

- [ ] **Step 6: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::install::source_policy::tests
cargo test -p conary --lib commands::install::prepare::tests
cargo test -p conary --lib commands::install::restore::tests
```

Expected: all commands pass.

- [ ] **Step 7: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs \
        apps/conary/src/commands/install/options.rs \
        apps/conary/src/commands/install/semantics.rs \
        apps/conary/src/commands/install/source_policy.rs \
        apps/conary/src/commands/install/prepare.rs \
        apps/conary/src/commands/install/inner.rs \
        apps/conary/src/commands/install/restore.rs \
        apps/conary/src/commands/install/ccs_transaction.rs \
        apps/conary/src/commands/install/batch.rs
git commit -m "refactor(install): extract options and source policy"
```

## Task 2: Move Execution-Path And Live-Root Helpers Into `execute.rs`

**Files:**
- Modify: `apps/conary/src/commands/install/execute.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`

- [ ] **Step 1: Add hub-private aliases for execution helpers**

In `install/mod.rs`, add:

```rust
use execute::{
    PackageExecutionPath, live_root_files_from_stored_files,
    preflight_extracted_live_root_file_ownership, prepare_install_environment_before_scriptlets,
    run_triggers,
};
```

- [ ] **Step 2: Move execution-path helpers**

Move these items from `install/mod.rs` into `execute.rs`:

- `PackageExecutionPath`
- `package_execution_path`
- `prepare_install_environment_before_scriptlets`
- `recover_mutable_journals_before_scriptlets`
- `preflight_extracted_live_root_file_ownership`
- `live_root_files_from_stored_files`
- `run_triggers`

Apply the visibility requirements listed above.

- [ ] **Step 3: Move execution tests**

Move these tests into `execute.rs`:

- `recover_mutable_journals_runs_before_scriptlets`
- `live_root_files_are_loaded_from_stored_cas_objects`
- `package_execution_path_fails_closed_on_invalid_generation_state`
- `package_execution_path_fails_closed_on_dangling_current_symlink`

- [ ] **Step 4: Remove moved code from `install/mod.rs`**

Delete the original moved helper bodies and the four moved tests from `install/mod.rs`.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::install::execute::tests
cargo test -p conary --lib commands::install::batch::tests::batch_install_preflights_before_pre_scripts
cargo test -p conary --lib commands::install::restore::tests::restore_pre_install_preflight_stays_before_scriptlets
cargo test -p conary --lib commands::install::ccs_transaction::tests::ccs_transaction_install_preflights_live_root_ownership_before_hooks_and_scriptlets
```

Expected: all commands pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs \
        apps/conary/src/commands/install/execute.rs \
        apps/conary/src/commands/install/batch.rs \
        apps/conary/src/commands/install/restore.rs \
        apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "refactor(install): extract execution path helpers"
```

## Task 3: Move Lifecycle And Extraction Helpers

**Files:**
- Create: `apps/conary/src/commands/install/lifecycle.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`

- [ ] **Step 1: Add the lifecycle module and re-exports**

In `install/mod.rs`, add:

```rust
mod lifecycle;

use lifecycle::{
    ExtractionResult, PreScriptletState, ScriptletContext, extract_and_classify_files,
    finalize_install, finalize_install_without_snapshot, mark_upgraded_parent_deriveds_stale,
    run_pre_install_phase, show_dry_run_summary,
};
use super::progress::{InstallPhase, InstallProgress};
```

- [ ] **Step 2: Move lifecycle types and helpers**

Move these items from `install/mod.rs` into `lifecycle.rs`:

- `ScriptletContext`
- `PreScriptletState`
- `ExtractionResult`
- `mark_upgraded_parent_deriveds_stale`
- `show_dry_run_summary`
- `extract_and_classify_files`
- `run_pre_install_phase`
- `finalize_install_without_snapshot`
- `finalize_install`

Apply the visibility requirements listed above.

- [ ] **Step 3: Remove moved code from `install/mod.rs`**

Delete the original lifecycle type and helper definitions from `install/mod.rs`.

- [ ] **Step 4: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::install::inner::tests
cargo test -p conary --lib commands::install::restore::tests
cargo test -p conary --lib commands::install::ccs_transaction::tests
cargo test -p conary --lib commands::install::batch::tests::batch_upgrade_pre_remove_is_not_guarded_by_new_scriptlets
```

Expected: all commands pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs \
        apps/conary/src/commands/install/lifecycle.rs \
        apps/conary/src/commands/install/inner.rs \
        apps/conary/src/commands/install/restore.rs \
        apps/conary/src/commands/install/ccs_transaction.rs \
        apps/conary/src/commands/install/batch.rs
git commit -m "refactor(install): extract install lifecycle helpers"
```

## Task 4: Move Main Install Transaction Execution

**Files:**
- Create: `apps/conary/src/commands/install/transaction.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`

- [ ] **Step 1: Add the transaction module and re-exports**

In `install/mod.rs`, add:

```rust
mod transaction;

use transaction::{
    InstallTransactionResult, TransactionContext, execute_install_transaction,
};
```

- [ ] **Step 2: Move transaction context and execution**

Move these items from `install/mod.rs` into `transaction.rs`:

- `TransactionContext`
- `InstallTransactionResult`
- `execute_install_transaction`
- `persist_ccs_manifest_provides`
- `insert_ccs_manifest_typed_provide`

Apply the visibility requirements listed above.

- [ ] **Step 3: Move transaction tests**

Move these tests into `transaction.rs`:

- `no_generation_install_transaction_materializes_live_root_file`
- `no_generation_install_conflict_preflight_preserves_live_root_file`

- [ ] **Step 4: Remove moved code from `install/mod.rs`**

Delete the original transaction type/helper definitions and two moved tests from `install/mod.rs`.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::install::transaction::tests
cargo test -p conary --lib commands::install::inner::tests
cargo test -p conary --lib commands::install::ccs_transaction::tests
cargo test -p conary --test live_host_mutation_safety install
```

Expected: all commands pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs \
        apps/conary/src/commands/install/transaction.rs \
        apps/conary/src/commands/install/inner.rs \
        apps/conary/src/commands/install/restore.rs \
        apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "refactor(install): extract transaction execution"
```

## Task 5: Expand Dependency Flow Ownership

**Files:**
- Modify: `apps/conary/src/commands/install/dependencies.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`

- [ ] **Step 1: Move dependency flow into `dependencies.rs`**

Move these items from `install/mod.rs` into `dependencies.rs`:

- `resolve_default_dep_mode_from_model`
- `classify_dep_type`
- `report_provides_check`
- `DepAnalysisContext`
- `handle_dependencies`
- `missing_repository_deps_from_sat_result`
- `handle_dep_adoptions`
- `handle_dep_installs`
- `check_unresolvable_deps`

Apply the visibility requirements listed above.

- [ ] **Step 2: Add the hub re-export**

In `install/mod.rs`, add:

```rust
pub(crate) use dependencies::resolve_default_dep_mode_from_model;
```

Do not publicly re-export `DepAnalysisContext`; `command.rs` should import it with:

```rust
use super::dependencies::{DepAnalysisContext, handle_dependencies};
```

- [ ] **Step 3: Move dependency tests**

Move these tests into `dependencies.rs`:

- `classify_dep_type_packages`
- `classify_dep_type_capabilities`
- `classify_dep_type_files`
- `classify_dep_type_rpmlib`
- `classify_dep_type_conditional`
- `classify_dep_type_or_group`
- `missing_model_uses_preview_convergence_dep_mode`
- `missing_repository_deps_preserve_sat_selected_version`

- [ ] **Step 4: Remove moved code from `install/mod.rs`**

Delete the original dependency flow helper definitions and eight moved tests from `install/mod.rs`.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::install::dependencies::tests
cargo test -p conary --lib commands::install::batch::tests
cargo test -p conary --lib commands::update::package::tests
cargo test -p conary --lib commands::update::collection::tests
```

Expected: all commands pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs \
        apps/conary/src/commands/install/dependencies.rs \
        apps/conary/src/commands/install/batch.rs
git commit -m "refactor(install): extract dependency flow"
```

## Task 6: Move Validation, Acquisition, And `cmd_install`

**Files:**
- Create: `apps/conary/src/commands/install/acquire.rs`
- Create: `apps/conary/src/commands/install/command.rs`
- Create: `apps/conary/src/commands/install/validation.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs`
- Modify: `apps/conary/src/commands/install/conversion.rs`
- Modify: `apps/conary/src/commands/update/package.rs`

- [ ] **Step 1: Add module declarations**

In `install/mod.rs`, add:

```rust
mod acquire;
mod command;
mod validation;
```

Add:

```rust
pub use command::cmd_install;
```

- [ ] **Step 2: Create `validation.rs`**

Move `parse_component_and_validate`, `try_promote_existing_dep`, and the two validation tests into `validation.rs`.

- [ ] **Step 3: Create `acquire.rs`**

Move `CcsInstallParams`, `resolve_and_parse_package`, `install_provenance_from_resolved`, `find_package_suggestions`, and `print_package_suggestions` into `acquire.rs`.

- [ ] **Step 4: Create `command.rs`**

Move `cmd_install` into `command.rs`. Update imports to use the destination helper modules.

- [ ] **Step 5: Move command ordering tests**

Move these tests into `command.rs` and update the source scan to `include_str!("command.rs")`:

- `package_execution_path_is_prepared_before_dependency_handling`
- `direct_install_preflights_live_root_ownership_before_scriptlets`

The updated tests should not search for the old helper-boundary comment. They should inspect the whole `command.rs` source.

- [ ] **Step 6: Remove moved code and clean imports from `install/mod.rs`**

Remove from `install/mod.rs`:

- `cmd_install`
- `CcsInstallParams`
- `resolve_and_parse_package`
- `install_provenance_from_resolved`
- `find_package_suggestions`
- `print_package_suggestions`
- `parse_component_and_validate`
- `try_promote_existing_dep`
- the final direct `#[cfg(test)] mod tests` block
- all stale private imports left from the old monolithic helper bodies

Expected `install/mod.rs` after cleanup:

- module declarations
- public and crate-visible re-exports
- hub-private aliases and sibling-visible public/crate re-exports needed by child modules
- no direct helper bodies
- no `#[cfg(test)] mod tests`

Keep the intentional hub-private aliases from the required alias surface, such
as `use super::{PackageFormatType, detect_package_format};`; child modules
access those aliases through `super::...`, so they are not unused imports.

- [ ] **Step 7: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib commands::install::command::tests
cargo test -p conary --lib commands::install::validation::tests
cargo test -p conary --lib commands::install -- --list
cargo test -p conary --test live_host_mutation_safety install
cargo test -p conary --test workflow install
cargo test -p conary --test bundle_replay ccs_install
```

Expected:

- `commands::install -- --list` still includes the moved tests under their new module paths and does not include `commands::install::tests`.
- live-host install safety tests pass.
- workflow install tests pass.
- CCS install replay integration subset passes.

- [ ] **Step 8: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs \
        apps/conary/src/commands/install/acquire.rs \
        apps/conary/src/commands/install/command.rs \
        apps/conary/src/commands/install/validation.rs \
        apps/conary/src/commands/install/ccs_transaction.rs \
        apps/conary/src/commands/install/conversion.rs \
        apps/conary/src/commands/update/package.rs
git commit -m "refactor(install): extract install command orchestration"
```

## Task 7: Update Install Routing Docs And Ledger Rows

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update subsystem map install paths**

In `docs/llms/subsystem-map.md`, expand the native install path cluster currently listing:

```markdown
  `apps/conary/src/commands/install/mod.rs`,
  `apps/conary/src/commands/install/legacy_replay.rs`,
  `apps/conary/src/commands/install/inner.rs`,
  `apps/conary/src/commands/install/batch.rs`,
  `apps/conary/src/commands/install/restore.rs`, and
```

to include the new owners:

```markdown
  `apps/conary/src/commands/install/mod.rs`,
  `apps/conary/src/commands/install/command.rs`,
  `apps/conary/src/commands/install/acquire.rs`,
  `apps/conary/src/commands/install/validation.rs`,
  `apps/conary/src/commands/install/dependencies.rs`,
  `apps/conary/src/commands/install/execute.rs`,
  `apps/conary/src/commands/install/lifecycle.rs`,
  `apps/conary/src/commands/install/transaction.rs`,
  `apps/conary/src/commands/install/options.rs`,
  `apps/conary/src/commands/install/semantics.rs`,
  `apps/conary/src/commands/install/source_policy.rs`,
  `apps/conary/src/commands/install/legacy_replay.rs`,
  `apps/conary/src/commands/install/inner.rs`,
  `apps/conary/src/commands/install/batch.rs`,
  `apps/conary/src/commands/install/restore.rs`, and
```

- [ ] **Step 2: Update feature ownership install card**

In `docs/modules/feature-ownership.md`, expand the "Native Package Install, Update, Remove, And Live-Root Mutation" `Start here` list to include:

```markdown
`apps/conary/src/commands/install/command.rs`;
`apps/conary/src/commands/install/acquire.rs`;
`apps/conary/src/commands/install/validation.rs`;
`apps/conary/src/commands/install/dependencies.rs`;
`apps/conary/src/commands/install/execute.rs`;
`apps/conary/src/commands/install/lifecycle.rs`;
`apps/conary/src/commands/install/transaction.rs`;
`apps/conary/src/commands/install/options.rs`;
`apps/conary/src/commands/install/semantics.rs`;
`apps/conary/src/commands/install/source_policy.rs`;
```

Keep `install/mod.rs`, `legacy_replay.rs`, `inner.rs`, `batch.rs`, and `restore.rs` in the same card.

- [ ] **Step 3: Update active docs-audit ledger rows**

Update the existing active rows for:

- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/source-selection.md`

For `docs/llms/subsystem-map.md`, add these paths to `evidence_sources`:

- `apps/conary/src/commands/install/command.rs`
- `apps/conary/src/commands/install/acquire.rs`
- `apps/conary/src/commands/install/validation.rs`
- `apps/conary/src/commands/install/dependencies.rs`
- `apps/conary/src/commands/install/execute.rs`
- `apps/conary/src/commands/install/lifecycle.rs`
- `apps/conary/src/commands/install/transaction.rs`
- `apps/conary/src/commands/install/options.rs`
- `apps/conary/src/commands/install/semantics.rs`
- `apps/conary/src/commands/install/source_policy.rs`

Add `install-module-completion` to the `tags` field and append this note
fragment:

```text
and Phase 16 install module completion child-module ownership.
```

For `docs/modules/feature-ownership.md`, add the same ten install child paths
to `evidence_sources`, add `install-module-completion` to `tags`, and append
the same Phase 16 note fragment.

For `docs/modules/source-selection.md`, add
`apps/conary/src/commands/install/source_policy.rs` to `evidence_sources`, add
`install-module-completion` to `tags`, and append this note fragment:

```text
and Phase 16 install source-policy path ownership.
```

- [ ] **Step 3.5: Update the source-selection read-next path**

In `docs/modules/source-selection.md`, add this bullet under
`## Where To Read Next`, after the runtime policy loading entry and before the
update module entries:

```markdown
- `apps/conary/src/commands/install/source_policy.rs` for install request-scope
  policy construction and canonical package name resolution
```

- [ ] **Step 4: Verify docs audit**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected:

- Inventory count remains `160`.
- Ledger counts remain `corrected 60`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
- Malformed-row check prints nothing.
- Ledger check passes.

- [ ] **Step 5: Commit**

```bash
git add docs/llms/subsystem-map.md \
        docs/modules/feature-ownership.md \
        docs/modules/source-selection.md \
        docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: route install module owners"
```

## Task 8: Final Verification, Push, And Clean State

**Files:** No intentional file edits.

- [ ] **Step 1: Verify final install module structure**

Run:

```bash
rg -n "^(pub |pub\\(|fn |async fn|struct |enum |impl |mod |pub use |pub\\(crate\\) use|pub\\(super\\) use|#\\[cfg\\(test\\)\\])" \
  apps/conary/src/commands/install/mod.rs \
  apps/conary/src/commands/install -g '*.rs'
```

Expected:

- `install/mod.rs` contains module declarations and re-exports only.
- `install/mod.rs` contains no `pub async fn cmd_install`.
- `install/mod.rs` contains no `#[cfg(test)] mod tests`.
- `command.rs` contains `pub async fn cmd_install`.
- `transaction.rs`, `lifecycle.rs`, `execute.rs`, `dependencies.rs`, `source_policy.rs`, and `validation.rs` contain the moved tests.

- [ ] **Step 2: Verify focused install tests**

Run:

```bash
cargo test -p conary --lib commands::install -- --list
cargo test -p conary --lib commands::install::command::tests
cargo test -p conary --lib commands::install::validation::tests
cargo test -p conary --lib commands::install::dependencies::tests
cargo test -p conary --lib commands::install::execute::tests
cargo test -p conary --lib commands::install::source_policy::tests
cargo test -p conary --lib commands::install::transaction::tests
cargo test -p conary --lib commands::install::inner::tests
cargo test -p conary --lib commands::install::restore::tests
cargo test -p conary --lib commands::install::ccs_transaction::tests
cargo test -p conary --lib commands::install::batch::tests
```

Expected: all focused commands pass. The list command should still report the broader install module tree and no direct `commands::install::tests` module.

- [ ] **Step 3: Verify behavior gates**

Run:

```bash
cargo test -p conary --test live_host_mutation_safety install
cargo test -p conary --test workflow install
cargo test -p conary --test bundle_replay ccs_install
cargo test -p conary --test batch_install
cargo test -p conary --test conversion_integration
cargo test -p conary --test native_pm_live_root
cargo test -p conary --test cli_daily_ux
```

Expected: all commands pass.

- [ ] **Step 4: Verify package and workspace gates**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo test -p conary
cargo clippy -p conary --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all commands pass. If workspace clippy reports a pre-existing unrelated warning, capture the exact output before deciding whether it belongs in this phase.

- [ ] **Step 5: Verify docs and maintainability gates**

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

- `install/mod.rs` no longer appears as the top hotspot.
- Docs-audit inventory remains `160`.
- Ledger counts remain `corrected 60`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
- Malformed-row check prints nothing.
- Docs-audit check passes.
- `git diff --check` exits 0.

- [ ] **Step 6: Push and prove clean synced main**

Run:

```bash
git status --short --branch
git log --oneline origin/main..HEAD
git push
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected:

- Before push, only intentional Phase 16 commits are ahead of `origin/main`.
- Push succeeds.
- After push, status is clean.
- `git rev-parse HEAD origin/main` prints the same SHA twice.
- Divergence is `0 0`.
- Only the main worktree is listed unless the user intentionally created another.

## Self-Review Checklist

- [ ] `install/mod.rs` keeps the public contract: `DepMode`, `InstallOptions`, `LegacyReplayOptions`, and `cmd_install` remain re-exported through `apps/conary/src/commands/mod.rs`.
- [ ] `repository_install_provenance_from_package` remains `pub(crate)` and usable from `update/package.rs` and `install/conversion.rs`.
- [ ] `resolve_default_dep_mode_from_model` remains `pub(crate)` and usable from `update/package.rs` and `update/collection.rs`.
- [ ] `InstallSemantics::legacy` and `InstallSemantics::ccs` become `pub(super)`, not private.
- [ ] `ExtractionResult`, `TransactionContext`, and `ScriptletContext` fields that siblings construct/read become `pub(super)`.
- [ ] Existing ordering tests are updated from `include_str!("mod.rs")` to `include_str!("command.rs")`.
- [ ] No behavior text, CLI flags, database schema, or command signatures change.
- [ ] Docs-audit counts move from `159/59` to `160/60` when the plan is locked in and remain `160/60` through implementation.
- [ ] The implementation goal ends with clean, pushed `main`.
