# Phase 17 Scriptlet Module Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `crates/conary-core/src/scriptlet/mod.rs` into focused scriptlet child modules while preserving every public `conary_core::scriptlet::*` API, scriptlet execution behavior, sandbox diagnostics, and legacy replay behavior.

**Architecture:** Keep `crates/conary-core/src/scriptlet/mod.rs` as the stable public hub and move type definitions, outcomes, phase helpers, executor orchestration, distro argument mapping, protected sandbox policy, process execution, and legacy invocation contracts into sibling files under `crates/conary-core/src/scriptlet/`. Preserve the existing private `runtime.rs` support module and re-export all public API items from the hub so callers in `apps/conary`, `conary-core` CCS modules, and integration tests do not change.

**Tech Stack:** Rust 2024 workspace modules, `anyhow`, `serde`, `tempfile`, `nix`, `libc`, `seccompiler`, Conary `container`, `capability`, `child_wait`, `packages::traits`, existing conary-core module re-export pattern.

---

## Current Repo Facts

- Baseline SHA before this plan draft: `0e0aa2ff08a689f5f1955716468fd9380546efc5`.
- `HEAD` and `origin/main` match at the baseline SHA.
- Current hotspot ranking from `scripts/line-count-report.sh 30`:
  - `crates/conary-core/src/scriptlet/mod.rs` - 2408 lines.
  - `apps/conaryd/src/daemon/routes.rs` - 2345 lines.
  - `apps/conary/src/commands/model.rs` - 2260 lines.
  - `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` - 2178 lines.
  - `apps/conary/src/dispatch.rs` - 2177 lines.
- Current scriptlet module tree:
  - `crates/conary-core/src/scriptlet/mod.rs`
  - `crates/conary-core/src/scriptlet/runtime.rs`
- Current direct unit test inventory:
  - `cargo test -p conary-core --lib scriptlet::tests -- --list`
  - Expected: 37 tests, 0 benchmarks.
- Broader scriptlet-filtered conary-core inventory:
  - `cargo test -p conary-core scriptlet -- --list`
  - Expected baseline observed during planning: 108 tests, 0 benchmarks.
- Current baseline compile check:
  - `cargo check -p conary-core`
  - Expected: passes.
- Current public API caller inventory:
  - `SandboxMode` is used by `apps/conary/src/cli/mod.rs`, `apps/conary/src/commands/mod.rs`, install/update/remove/collection/system command code, `apps/conary/tests/conversion_integration.rs`, `apps/conary/tests/foreign_replay.rs`, `crates/conary-core/src/ccs/legacy_replay.rs`, and `crates/conary-core/src/ccs/target_compatibility.rs`.
  - `ScriptletExecutor`, `ExecutionMode`, scriptlet `PackageFormat`, `ScriptletOutcome`, `ScriptletFailureKind`, and `ScriptletFailureOutcome` are used by `apps/conary/src/commands/install/scriptlets.rs`, `apps/conary/src/commands/install/legacy_replay.rs`, and `apps/conary/src/commands/remove.rs`.
  - `LegacyScriptletExecution` and `LegacyInvocationRuntime` are used by `apps/conary/src/commands/install/legacy_replay.rs` and `apps/conary/src/commands/remove.rs`.
  - `set_seccomp_warn_override` is called by `apps/conary/src/app.rs`.
  - `phase_to_string` and `phase_from_string` have no production callers outside the scriptlet module today but remain public API and must stay re-exported.
- Current docs-audit baseline before locking this plan:
  - Inventory: 160 tracked doc-like files.
  - Ledger categories: `corrected 60`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
  - After lock-in of this plan: 161 tracked doc-like files, `corrected 61`.

## Why This Hotspot

`crates/conary-core/src/scriptlet/mod.rs` is now the largest Rust source file in the workspace. It owns several distinct concerns:

- Public scriptlet API types and serialization behavior.
- Scriptlet execution outcome typing and error classification.
- Distro-specific scriptlet argument mapping and Arch `.INSTALL` wrapper generation.
- Public `ScriptletExecutor` orchestration for package-parsed, DB-backed, and legacy bundle scriptlets.
- Protected live-root sandbox selection, preflight, and mount/capability policy construction.
- Direct, target-root, chroot, and sandboxed process execution.
- Legacy replay invocation contracts, body validation, derived native args, safe environment construction, and target-root skip behavior.
- Phase string conversion helpers.
- Runtime subprocess/seccomp helper tests that currently live in the parent test module.

These concerns have clear seams and mostly communicate through `ScriptletExecutor`, `ExecutionMode`, `PackageFormat`, `SandboxMode`, and typed outcomes. Moving them into child modules keeps behavior stable while making future scriptlet-security and legacy-replay work much easier to review.

## Alternatives Considered

### Option A: Complete Scriptlet Hub Split

Move all production bodies and direct tests out of `scriptlet/mod.rs`, leaving it as module declarations plus public re-exports.

**Pros:** Removes the current top hotspot in one `/goal`, creates focused owners for security-sensitive behavior, and follows the successful Phase 13-16 hub pattern.

**Cons:** Requires careful method visibility because inherent `impl ScriptletExecutor` blocks will live across sibling modules.

**Recommendation:** Choose this option.

### Option B: Move Only Tests And Runtime Helpers

Move the direct tests and runtime-specific checks first, leaving the executor body mostly flat.

**Pros:** Smallest compile-risk slice.

**Cons:** Does not solve the hotspot and leaves the security-sensitive executor hard to navigate.

### Option C: Skip To conaryd Routes

Defer core scriptlet work and split `apps/conaryd/src/daemon/routes.rs`.

**Pros:** Routes are mostly request/response/test mass and may be mechanically easier.

**Cons:** Scriptlet security is more central and is currently the top hotspot.

## Non-Goals

- Do not change public API names, signatures, enum variants, struct fields, serialization spelling, error messages, scriptlet arguments, sandbox selection, timeout behavior, seccomp behavior, target-root behavior, or legacy replay behavior.
- Do not move `crates/conary-core/src/scriptlet/runtime.rs` out of the `scriptlet/` directory.
- Do not split `crate::container`, `crate::capability`, `crate::packages`, CCS legacy replay, install command code, or tests outside the direct scriptlet module tree except for docs routing.
- Do not add schema migrations.
- Do not add new behavior tests unless a focused compile-proof test is needed to keep moved private helpers reachable.
- Do not archive this plan during implementation.

## Rust Visibility Contract

Rust privacy is central to this split:

- `scriptlet/mod.rs` remains the parent module and may publicly re-export items from private child modules.
- Child modules under `scriptlet/` can see private parent modules and items.
- Sibling child modules cannot call private methods defined in another sibling module unless those methods are at least `pub(super)`.
- `ScriptletExecutor` will move to `executor.rs`; its fields must become `pub(super)` so sibling `impl ScriptletExecutor` blocks in `arguments.rs`, `legacy.rs`, `sandbox.rs`, `process.rs`, and `outcome.rs` can read them.
- Methods called across sibling modules must be `pub(super)`. Methods used only inside their owner module can stay private.
- Public APIs re-exported by `scriptlet/mod.rs` must remain `pub` at their definition site.

## File Structure After Implementation

```text
crates/conary-core/src/scriptlet/
  mod.rs
  arguments.rs
  executor.rs
  legacy.rs
  outcome.rs
  phases.rs
  process.rs
  runtime.rs
  sandbox.rs
  types.rs
```

Every new Rust file created by this plan must start with the repository-standard path comment:

```rust
// conary-core/src/scriptlet/<file>.rs
```

### `crates/conary-core/src/scriptlet/mod.rs`

Hub only. It owns the module-level docs, module declarations, and public re-exports.

Required module declarations:

```rust
mod arguments;
mod executor;
mod legacy;
mod outcome;
mod phases;
mod process;
mod runtime;
mod sandbox;
mod types;
```

Required public re-export surface:

```rust
pub use executor::ScriptletExecutor;
pub use legacy::{LegacyInvocationRuntime, LegacyScriptletExecution};
pub use outcome::{ScriptletFailureKind, ScriptletFailureOutcome, ScriptletOutcome};
pub use phases::{phase_from_string, phase_to_string};
pub use runtime::set_seccomp_warn_override;
pub use sandbox::{EffectiveSandbox, SandboxMode};
pub use types::{ExecutionMode, PackageFormat};
```

`scriptlet/mod.rs` must not keep `ScriptletExecutor`, helper function bodies, constants, or the `#[cfg(test)] mod tests` block after Task 5.

### `crates/conary-core/src/scriptlet/types.rs`

Owns package-format and execution-mode value types:

- `pub enum PackageFormat`
- `impl PackageFormat`
- `pub enum ExecutionMode`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// conary-core/src/scriptlet/types.rs
```

No imports are required for the production items in `types.rs`.

Move these tests into `types.rs`:

- `test_package_format_from_str`

`ExecutionMode` has no direct tests today; do not add new ones in this refactor.

### `crates/conary-core/src/scriptlet/sandbox.rs`

Owns sandbox mode value types, effective sandbox labels, protected live-root policy, and sandbox preflight:

- `pub enum SandboxMode`
- `impl SandboxMode`
- `pub enum EffectiveSandbox`
- `impl EffectiveSandbox`
- `impl ScriptletExecutor` methods:
  - `pub(super) fn should_use_sandbox(...) -> bool`
  - `pub(super) fn effective_sandbox(...) -> EffectiveSandbox`
  - `pub(super) fn preflight_protected_live_sandbox(...) -> Result<()>`
  - `pub(super) fn live_sandbox_config(...) -> Result<ContainerConfig>`
- `fn is_live_sandbox_private_target(...) -> bool`
- `fn protected_scriptlet_sandbox_unavailable(...) -> Error`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// conary-core/src/scriptlet/sandbox.rs

use super::ScriptletExecutor;
use crate::capability::SyscallCapabilities;
use crate::capability::enforcement::{EnforcementMode, EnforcementPolicy};
use crate::container::{
    BindMount, ContainerConfig, ScriptRisk, analyze_script, isolation_available,
};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
```

This is the final `sandbox.rs` import surface after Task 3. During Task 1,
when `sandbox.rs` only owns `SandboxMode`, `EffectiveSandbox`, and the three
sandbox-mode tests, keep the production imports minimal:

```rust
use serde::{Deserialize, Serialize};
```

Constants owned here:

- `LIVE_SANDBOX_READONLY_ETC_FILES`

Move these tests into `sandbox.rs`:

- `test_sandbox_mode_default_is_always`
- `test_sandbox_mode_parse`
- `sandbox_mode_serde_round_trips_goal7_matrix_spellings`
- `test_live_sandbox_config_rebinds_critical_etc_files_readonly`
- `test_live_sandbox_config_uses_private_layers_for_writable_etc_and_var`
- `test_live_sandbox_config_fails_closed_on_protection_setup_failures`
- `test_live_sandbox_config_installs_scriptlet_seccomp_profile`
- `test_protected_live_root_preflight_reports_operator_diagnostic`

Test module imports:

```rust
#[cfg(test)]
mod tests {
    use super::SandboxMode;
    use super::super::runtime::ENV_LOCK;
    use super::super::{ExecutionMode, PackageFormat, ScriptletExecutor};
    use crate::capability::enforcement::EnforcementMode;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
    use std::path::{Path, PathBuf};
}
```

Use `ENV_LOCK` for `test_protected_live_root_preflight_reports_operator_diagnostic`.

### `crates/conary-core/src/scriptlet/outcome.rs`

Owns typed scriptlet outcomes and failure classification:

- `pub enum ScriptletFailureKind`
- `impl ScriptletFailureKind`
- `pub struct ScriptletFailureOutcome`
- `pub enum ScriptletOutcome`
- `impl ScriptletOutcome`
- `impl ScriptletExecutor` methods:
  - `pub(super) fn failure_outcome(...) -> ScriptletOutcome`
  - `pub(super) fn failure_from_error(...) -> ScriptletOutcome`
- `fn classify_scriptlet_failure(...) -> ScriptletFailureKind`

Import surface:

```rust
// conary-core/src/scriptlet/outcome.rs

use super::{EffectiveSandbox, SandboxMode, ScriptletExecutor};
use crate::error::{Error, Result};
```

This is the final `outcome.rs` import surface after Task 3. During Task 1,
omit `ScriptletExecutor` until `failure_outcome` and `failure_from_error` move.

Move this test into `executor.rs`, not `outcome.rs`, because it exercises the public executor path:

- `test_execute_with_outcome_records_requested_and_effective_sandbox`

### `crates/conary-core/src/scriptlet/phases.rs`

Owns public phase string conversions:

- `pub fn phase_to_string(...) -> String`
- `pub fn phase_from_string(...) -> Option<ScriptletPhase>`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// conary-core/src/scriptlet/phases.rs

use crate::packages::traits::ScriptletPhase;
```

Move this test into `phases.rs`:

- `test_phase_conversion`

Test module imports:

```rust
#[cfg(test)]
mod tests {
    use super::{phase_from_string, phase_to_string};
    use crate::packages::traits::ScriptletPhase;
}
```

### `crates/conary-core/src/scriptlet/executor.rs`

Owns the public executor type, constructor/configuration methods, non-legacy public API methods, and core execute/preflight orchestration:

- `pub struct ScriptletExecutor`
- `impl ScriptletExecutor`
  - `pub fn new(...) -> Self`
  - `pub fn with_timeout(...) -> Self`
  - `pub fn with_sandbox_mode(...) -> Self`
  - `pub fn execute(...) -> Result<()>`
  - `pub fn execute_with_outcome(...) -> ScriptletOutcome`
  - `pub fn execute_entry(...) -> Result<()>`
  - `pub fn execute_entry_with_outcome(...) -> ScriptletOutcome`
  - `pub fn preflight(...) -> Result<()>`
  - `pub fn preflight_entry(...) -> Result<()>`
  - `pub(super) fn is_live_root(...) -> bool`
  - `pub(super) fn clone_with_timeout(...) -> Self`
  - `#[cfg(test)] fn execute_impl(...) -> Result<()>`
  - `fn execute_impl_with_outcome(...) -> ScriptletOutcome`
  - `fn preflight_impl(...) -> Result<()>`
- `#[cfg(test)] mod tests`

The executor fields must become `pub(super)`:

```rust
pub struct ScriptletExecutor {
    pub(super) root: PathBuf,
    pub(super) package_name: String,
    pub(super) package_version: String,
    pub(super) package_format: PackageFormat,
    pub(super) timeout: Duration,
    pub(super) sandbox_mode: SandboxMode,
}
```

Import surface:

```rust
// conary-core/src/scriptlet/executor.rs

use super::{ExecutionMode, PackageFormat, SandboxMode, ScriptletOutcome};
use super::ScriptletFailureKind;
use crate::container::{ScriptRisk, analyze_script};
use crate::db::models::ScriptletEntry;
use crate::error::{Error, Result};
use crate::packages::traits::Scriptlet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};
```

Constants owned here:

- `DEFAULT_TIMEOUT`

Move these tests into `executor.rs`:

- `test_executor_default_sandbox_is_always`
- `test_execute_with_outcome_records_requested_and_effective_sandbox`
- `test_execute_impl_missing_interpreter`

Test module imports:

```rust
#[cfg(test)]
mod tests {
    use super::ScriptletExecutor;
    use super::super::{
        EffectiveSandbox, ExecutionMode, PackageFormat, SandboxMode, ScriptletFailureKind,
        ScriptletOutcome,
    };
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
    use std::path::Path;
}
```

While moving `execute_impl_with_outcome`, keep the risk-analysis logging behavior exactly as-is. Do not replace it with only `self.should_use_sandbox(...)`; the log includes `analysis.patterns` and must stay.

### `crates/conary-core/src/scriptlet/arguments.rs`

Owns distro-specific argument mapping and Arch `.INSTALL` wrapper generation:

- `impl ScriptletExecutor`
  - `pub(super) fn get_args(...) -> Vec<String>`
  - `pub(super) fn prepare_arch_wrapper(...) -> String`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// conary-core/src/scriptlet/arguments.rs

use super::{ExecutionMode, PackageFormat, ScriptletExecutor};
use tracing::warn;
```

Move these tests into `arguments.rs`:

- `test_rpm_args`
- `test_deb_args`
- `test_arch_args`
- `test_arch_wrapper_generation`

Test module imports:

```rust
#[cfg(test)]
mod tests {
    use super::super::{ExecutionMode, PackageFormat, ScriptletExecutor};
    use std::path::Path;
}
```

### `crates/conary-core/src/scriptlet/process.rs`

Owns concrete process execution paths:

- `impl ScriptletExecutor`
  - `pub(super) fn execute_sandbox_live(...) -> Result<()>`
  - `pub(super) fn execute_in_target(...) -> Result<()>`
  - `pub(super) fn execute_with_chroot(...) -> Result<()>`
  - `pub(super) fn execute_direct(...) -> Result<()>`
  - `pub(super) fn execute_direct_with_options(...) -> Result<()>`
- `#[cfg(test)] mod tests`

Preserve the existing `#[allow(clippy::too_many_arguments)]` attribute on `execute_direct_with_options`.

Import surface:

```rust
// conary-core/src/scriptlet/process.rs

use super::ScriptletExecutor;
use super::runtime::{
    apply_sanitized_command_env, build_scriptlet_seccomp, chroot_mount_private_flags,
    chroot_namespace_flags, current_seccomp_mode, log_script_output, wait_and_capture,
    write_executable_script,
};
use crate::container::Sandbox;
use crate::error::{Error, Result};
use std::fs;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};
```

Move these tests into `process.rs`:

- `test_execute_basic_success`
- `test_execute_script_failure`
- `test_execute_none_sandbox_runs_directly`
- `test_execute_timeout`
- `test_execute_with_env_vars`
- `test_execute_direct_clears_host_environment`
- `test_execute_direct_captures_stdout_stderr_without_echild`
- `test_execute_direct_timeout_no_double_wait_panic`
- `test_execute_with_chroot_requires_root`

Test module imports:

```rust
#[cfg(test)]
mod tests {
    use super::super::runtime::ENV_LOCK;
    use super::super::{PackageFormat, SandboxMode, ScriptletExecutor};
    use std::path::Path;
    use std::time::Duration;
}
```

Use `ENV_LOCK` for `test_execute_direct_clears_host_environment`.

### `crates/conary-core/src/scriptlet/legacy.rs`

Owns legacy bundle execution inputs, preflight, body validation, derived native args, environment construction, and legacy execution:

- `pub struct LegacyScriptletExecution<'a>`
- `pub struct LegacyInvocationRuntime<'a>`
- `impl ScriptletExecutor`
  - `pub fn preflight_legacy_entry(...) -> AnyhowResult<()>`
  - `pub fn execute_legacy_entry_with_outcome(...) -> ScriptletOutcome`
  - `fn validate_legacy_execution_contracts(...) -> AnyhowResult<()>`
  - `fn validate_legacy_interpreter_args(...) -> AnyhowResult<()>`
  - `fn derive_legacy_native_args(...) -> AnyhowResult<Vec<String>>`
  - `fn legacy_environment(...) -> AnyhowResult<Vec<(String, String)>>`
- Free helpers:
  - `fn decode_legacy_body(...) -> AnyhowResult<String>`
  - `fn runtime_old_version(...) -> AnyhowResult<String>`
  - `fn runtime_new_version(...) -> String`
  - `fn validate_stdin_contract(...) -> AnyhowResult<()>`
  - `fn validate_chroot_contract(...) -> AnyhowResult<()>`
  - `fn validate_legacy_environment_key(...) -> AnyhowResult<()>`
- `#[cfg(test)] mod tests`

Import surface:

```rust
// conary-core/src/scriptlet/legacy.rs

use super::{
    ExecutionMode, ScriptletExecutor, ScriptletFailureKind, ScriptletOutcome,
};
use anyhow::{Result as AnyhowResult, bail};
use std::path::PathBuf;
use std::time::Duration;
use tracing::warn;
```

Constants owned here:

- `LEGACY_MIN_TIMEOUT_MS`
- `LEGACY_MAX_TIMEOUT_MS`
- `LEGACY_SAFE_PATH`
- `DANGEROUS_LEGACY_ENV_KEYS`

Move these test helpers into `legacy.rs` tests:

- `legacy_execution_with_contracts`
- `upgrade_runtime`

Move these tests into `legacy.rs`:

- `legacy_native_arg_contracts_use_runtime_versions_and_literals`
- `legacy_native_arg_contracts_use_runtime_remove_count`
- `legacy_native_arg_contracts_refuse_malformed_or_missing_runtime_values`
- `legacy_preflight_refuses_unsupported_invocation_fields`
- `legacy_preflight_rejects_body_hash_mismatch`
- `legacy_execution_uses_safe_path_and_derived_args`
- `legacy_execution_skips_target_root_when_interpreter_is_absent`

Test module imports:

```rust
#[cfg(test)]
mod tests {
    use super::{LegacyInvocationRuntime, LegacyScriptletExecution};
    use super::super::{
        ExecutionMode, PackageFormat, SandboxMode, ScriptletExecutor, ScriptletOutcome,
    };
    use std::path::Path;
}
```

### `crates/conary-core/src/scriptlet/runtime.rs`

Keep this existing helper module but move its direct tests out of `mod.rs` and into `runtime.rs`.

Visibility changes:

- Change `set_seccomp_warn_override` from `pub(super) fn` to `pub fn` so `scriptlet/mod.rs` can re-export it publicly with `pub use runtime::set_seccomp_warn_override;`.
- Leave the other runtime helpers as `pub(super)` unless a sibling child module needs to call them through `super::runtime`.
- Add one shared test-only environment lock in `runtime.rs` so environment-mutating tests in different child modules cannot run concurrently:

```rust
#[cfg(test)]
pub(super) static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
```

Move these tests into `runtime.rs`:

- `test_build_scriptlet_seccomp_returns_filter`
- `test_current_seccomp_mode_defaults_to_enforce`
- `test_chroot_namespace_flags_include_mount_namespace`
- `test_chroot_mount_propagation_is_private_recursive`

Import surface for the new test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::enforcement::EnforcementMode;
}
```

## Task 0: Lock In The Plan And Docs-Audit Baseline

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Add: `docs/superpowers/plans/2026-06-08-project-maintainability-phase17-scriptlet-module-decomposition-plan.md`

- [ ] **Step 1: Stage the plan file before inventory regeneration**

```bash
git add docs/superpowers/plans/2026-06-08-project-maintainability-phase17-scriptlet-module-decomposition-plan.md
```

- [ ] **Step 2: Add a ledger row for the Phase 17 plan**

Add the row near the active maintainability plan rows, immediately after the Phase 16 row. The row must use literal tab characters and exactly 9 fields:

```tsv
docs/superpowers/plans/2026-06-08-project-maintainability-phase17-scriptlet-module-decomposition-plan.md	docs/superpowers/plans/2026-06-08-project-maintainability-phase17-scriptlet-module-decomposition-plan.md	planning	maintainer	maintainability; phase17; scriptlet-module; hotspot-decomposition; scriptlet-security; legacy-replay	crates/conary-core/src/scriptlet/mod.rs; crates/conary-core/src/scriptlet/runtime.rs; apps/conary/src/commands/install/scriptlets.rs; apps/conary/src/commands/install/legacy_replay.rs; apps/conary/src/commands/remove.rs; apps/conary/tests/bundle_replay.rs; apps/conary/tests/query_scripts.rs; docs/SCRIPTLET_SECURITY.md; docs/modules/feature-ownership.md; docs/llms/subsystem-map.md; scripts/line-count-report.sh	verified	corrected	Added the Phase 17 scriptlet module decomposition plan to split conary-core scriptlet execution into focused public type, outcome, phase, executor, argument, sandbox, process, runtime, and legacy invocation owners while preserving scriptlet security, sandbox diagnostics, and legacy replay behavior.
```

- [ ] **Step 3: Refresh the docs-audit inventory**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: inventory has 161 tracked doc-like rows after the plan is staged.

- [ ] **Step 4: Update the docs-audit summary**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, insert this paragraph after the existing Phase 16 paragraph in the `2026-06-06 Maintainability Planning` section:

```markdown
The Phase 17 scriptlet module decomposition plan targets the current largest
Rust hotspot, `crates/conary-core/src/scriptlet/mod.rs`. It keeps
`scriptlet/mod.rs` as the public API hub while planning focused owners for
scriptlet value types, typed outcomes, phase conversion, executor orchestration,
distro argument mapping, protected sandbox policy, process execution, runtime
helpers, and legacy bundle invocation contracts.
```

Then update the final counts at the bottom of the same file:

```diff
- Total tracked doc-like files audited: 160
+ Total tracked doc-like files audited: 161
  - `verified-no-change`: 13
- - `corrected`: 60
+ - `corrected`: 61
  - `archived`: 73
  - `retained-historical`: 14
  - Remaining pending rows: 0
```

- [ ] **Step 4.5: Refresh the docs-audit summary ledger row**

Update the existing row for `docs/superpowers/documentation-accuracy-audit-summary.md` in `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

- Add the Phase 17 plan path to `evidence_sources`:
  `docs/superpowers/plans/2026-06-08-project-maintainability-phase17-scriptlet-module-decomposition-plan.md`
- Add `phase17` and `scriptlet-module` to the `tags` field.
- Append a note fragment in the existing style:
  `and the Phase 17 scriptlet module decomposition.`

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

- Inventory count prints `161`.
- Ledger counts include `corrected 61`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
- Malformed-row check prints nothing.
- Ledger check passes.
- `git diff --cached --check` exits 0.

- [ ] **Step 6: Commit the locked plan**

```bash
git add docs/superpowers/documentation-accuracy-audit-ledger.tsv \
        docs/superpowers/documentation-accuracy-audit-summary.md \
        docs/superpowers/documentation-accuracy-audit-inventory.tsv \
        docs/superpowers/plans/2026-06-08-project-maintainability-phase17-scriptlet-module-decomposition-plan.md
git commit -m "docs: plan scriptlet module decomposition"
```

## Task 1: Extract Public Types, Outcomes, Phases, And Sandbox Mode Values

**Files:**
- Create: `crates/conary-core/src/scriptlet/types.rs`
- Create: `crates/conary-core/src/scriptlet/outcome.rs`
- Create: `crates/conary-core/src/scriptlet/phases.rs`
- Create: `crates/conary-core/src/scriptlet/sandbox.rs`
- Modify: `crates/conary-core/src/scriptlet/mod.rs`

- [ ] **Step 1: Add module declarations and public re-exports**

In `crates/conary-core/src/scriptlet/mod.rs`, keep `mod runtime;` and add:

```rust
mod outcome;
mod phases;
mod sandbox;
mod types;
```

Add:

```rust
pub use outcome::{ScriptletFailureKind, ScriptletFailureOutcome, ScriptletOutcome};
pub use phases::{phase_from_string, phase_to_string};
pub use sandbox::{EffectiveSandbox, SandboxMode};
pub use types::{ExecutionMode, PackageFormat};
```

Do not run `cargo check` while both the original definitions and the re-exports exist.

- [ ] **Step 2: Create `types.rs`**

Move `PackageFormat`, `impl PackageFormat`, `ExecutionMode`, and `test_package_format_from_str` into `types.rs`.

- [ ] **Step 3: Create `outcome.rs`**

Move `ScriptletFailureKind`, `impl ScriptletFailureKind`, `ScriptletFailureOutcome`, `ScriptletOutcome`, and `impl ScriptletOutcome` into `outcome.rs`.

Do not move `classify_scriptlet_failure`, `failure_outcome`, or `failure_from_error` yet; move them in Task 3 after `ScriptletExecutor` lives in `executor.rs`.

- [ ] **Step 4: Create `phases.rs`**

Move `phase_to_string`, `phase_from_string`, and `test_phase_conversion` into `phases.rs`.

- [ ] **Step 5: Create initial `sandbox.rs`**

Move `SandboxMode`, `impl SandboxMode`, `EffectiveSandbox`, `impl EffectiveSandbox`, and these tests into `sandbox.rs`:

- `test_sandbox_mode_default_is_always`
- `test_sandbox_mode_parse`
- `sandbox_mode_serde_round_trips_goal7_matrix_spellings`

Do not move protected sandbox helper methods yet; move them in Task 3.

- [ ] **Step 6: Remove moved definitions and tests from `mod.rs`**

Delete the original moved definitions and moved tests from `mod.rs`.

- [ ] **Step 7: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib scriptlet::types::tests
cargo test -p conary-core --lib scriptlet::sandbox::tests
cargo test -p conary-core --lib scriptlet::phases::tests
cargo test -p conary-core --lib scriptlet -- --list
```

Expected: all commands pass. The list command still shows all remaining unmoved tests under `scriptlet::tests` plus the newly moved child-module tests.

- [ ] **Step 8: Commit**

```bash
git add crates/conary-core/src/scriptlet/mod.rs \
        crates/conary-core/src/scriptlet/types.rs \
        crates/conary-core/src/scriptlet/outcome.rs \
        crates/conary-core/src/scriptlet/phases.rs \
        crates/conary-core/src/scriptlet/sandbox.rs
git commit -m "refactor(scriptlet): extract public types"
```

## Task 2: Extract Executor Core And Argument Semantics

**Files:**
- Create: `crates/conary-core/src/scriptlet/executor.rs`
- Create: `crates/conary-core/src/scriptlet/arguments.rs`
- Modify: `crates/conary-core/src/scriptlet/mod.rs`

- [ ] **Step 1: Add module declarations and executor re-export**

In `scriptlet/mod.rs`, add:

```rust
mod arguments;
mod executor;
```

Add:

```rust
pub use executor::ScriptletExecutor;
```

Do not run `cargo check` while both the original `ScriptletExecutor` and the re-export exist.

- [ ] **Step 2: Create `executor.rs`**

Move `ScriptletExecutor`, constructor/configuration methods, non-legacy public methods, `is_live_root`, `clone_with_timeout`, `execute_impl`, `execute_impl_with_outcome`, and `preflight_impl` into `executor.rs`.

Apply the field visibility contract:

```rust
pub struct ScriptletExecutor {
    pub(super) root: PathBuf,
    pub(super) package_name: String,
    pub(super) package_version: String,
    pub(super) package_format: PackageFormat,
    pub(super) timeout: Duration,
    pub(super) sandbox_mode: SandboxMode,
}
```

Keep `DEFAULT_TIMEOUT` in `executor.rs`.

- [ ] **Step 3: Create `arguments.rs`**

Move `get_args`, `prepare_arch_wrapper`, and the four argument/wrapper tests into `arguments.rs`.

Change `get_args` and `prepare_arch_wrapper` to `pub(super)` because `executor.rs` and `legacy.rs` call them across sibling modules:

```rust
pub(super) fn get_args(&self, mode: &ExecutionMode, phase: &str) -> Vec<String>
pub(super) fn prepare_arch_wrapper(&self, content: &str, phase: &str) -> String
```

- [ ] **Step 4: Remove moved definitions and tests from `mod.rs`**

Delete the original moved struct, methods, constants, and tests from `mod.rs`.

The legacy public methods, sandbox helper methods, process methods, and outcome helper methods may still be implemented in `mod.rs` during this task. They now target the re-exported `ScriptletExecutor` type and can access its `pub(super)` fields.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib scriptlet::executor::tests
cargo test -p conary-core --lib scriptlet::arguments::tests
cargo test -p conary-core --lib scriptlet::tests
```

Expected: all commands pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/scriptlet/mod.rs \
        crates/conary-core/src/scriptlet/executor.rs \
        crates/conary-core/src/scriptlet/arguments.rs
git commit -m "refactor(scriptlet): extract executor core"
```

## Task 3: Extract Sandbox Policy And Outcome Helpers

**Files:**
- Modify: `crates/conary-core/src/scriptlet/sandbox.rs`
- Modify: `crates/conary-core/src/scriptlet/outcome.rs`
- Modify: `crates/conary-core/src/scriptlet/mod.rs`

- [ ] **Step 1: Move protected sandbox policy into `sandbox.rs`**

Move these items from `mod.rs` into `sandbox.rs`:

- `LIVE_SANDBOX_READONLY_ETC_FILES`
- `should_use_sandbox`
- `effective_sandbox`
- `preflight_protected_live_sandbox`
- `live_sandbox_config`
- `is_live_sandbox_private_target`
- `protected_scriptlet_sandbox_unavailable`

The moved methods must be `pub(super)` because `executor.rs`, `legacy.rs`, and `process.rs` call them across sibling modules.

- [ ] **Step 2: Move sandbox tests**

Move these tests into `sandbox.rs`:

- `test_live_sandbox_config_rebinds_critical_etc_files_readonly`
- `test_live_sandbox_config_uses_private_layers_for_writable_etc_and_var`
- `test_live_sandbox_config_fails_closed_on_protection_setup_failures`
- `test_live_sandbox_config_installs_scriptlet_seccomp_profile`
- `test_protected_live_root_preflight_reports_operator_diagnostic`

Use the shared `super::super::runtime::ENV_LOCK` for the forced preflight test; do not define a second lock in `sandbox.rs`.

- [ ] **Step 3: Move outcome helpers into `outcome.rs`**

Move these items from `mod.rs` into `outcome.rs`:

- `failure_outcome`
- `failure_from_error`
- `classify_scriptlet_failure`

The moved methods must be `pub(super)` because `executor.rs` and `legacy.rs` call them across sibling modules:

```rust
pub(super) fn failure_outcome(...)
pub(super) fn failure_from_error(...)
```

- [ ] **Step 4: Remove moved definitions and tests from `mod.rs`**

Delete the original sandbox and outcome helper definitions plus the moved sandbox tests from `mod.rs`.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib scriptlet::sandbox::tests
cargo test -p conary-core --lib scriptlet::executor::tests
cargo test -p conary-core --lib scriptlet::tests
```

Expected: all commands pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/scriptlet/mod.rs \
        crates/conary-core/src/scriptlet/sandbox.rs \
        crates/conary-core/src/scriptlet/outcome.rs
git commit -m "refactor(scriptlet): extract sandbox policy"
```

## Task 4: Extract Process Execution And Runtime Tests

**Files:**
- Create: `crates/conary-core/src/scriptlet/process.rs`
- Modify: `crates/conary-core/src/scriptlet/runtime.rs`
- Modify: `crates/conary-core/src/scriptlet/mod.rs`

- [ ] **Step 1: Add process module declaration**

In `scriptlet/mod.rs`, add:

```rust
mod process;
```

- [ ] **Step 2: Create `process.rs`**

Move these methods from `mod.rs` into `process.rs`:

- `execute_sandbox_live`
- `execute_in_target`
- `execute_with_chroot`
- `execute_direct`
- `execute_direct_with_options`

Set the methods to `pub(super)` because `executor.rs`, `legacy.rs`, and process tests call them across modules.

Preserve:

```rust
#[allow(clippy::too_many_arguments)]
```

on `execute_direct_with_options`.

- [ ] **Step 3: Move process tests**

Move these tests into `process.rs`:

- `test_execute_basic_success`
- `test_execute_script_failure`
- `test_execute_none_sandbox_runs_directly`
- `test_execute_timeout`
- `test_execute_with_env_vars`
- `test_execute_direct_clears_host_environment`
- `test_execute_direct_captures_stdout_stderr_without_echild`
- `test_execute_direct_timeout_no_double_wait_panic`
- `test_execute_with_chroot_requires_root`

Use the shared `super::super::runtime::ENV_LOCK` for `test_execute_direct_clears_host_environment`; do not define a second lock in `process.rs`.

- [ ] **Step 4: Move runtime tests into `runtime.rs`**

Move these tests from `mod.rs` into `runtime.rs`:

- `test_build_scriptlet_seccomp_returns_filter`
- `test_current_seccomp_mode_defaults_to_enforce`
- `test_chroot_namespace_flags_include_mount_namespace`
- `test_chroot_mount_propagation_is_private_recursive`

Change `runtime::set_seccomp_warn_override` to `pub fn` and update `scriptlet/mod.rs` to re-export it:

```rust
pub use runtime::set_seccomp_warn_override;
```

Delete the old wrapper function from `mod.rs`.

- [ ] **Step 5: Remove moved definitions and tests from `mod.rs`**

Delete the original process methods, runtime wrapper, and moved process/runtime tests from `mod.rs`.

- [ ] **Step 6: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib scriptlet::process::tests
cargo test -p conary-core --lib scriptlet::runtime::tests
cargo test -p conary-core --lib scriptlet::executor::tests
cargo test -p conary-core --lib scriptlet::tests
```

Expected: all commands pass.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/scriptlet/mod.rs \
        crates/conary-core/src/scriptlet/process.rs \
        crates/conary-core/src/scriptlet/runtime.rs
git commit -m "refactor(scriptlet): extract process execution"
```

## Task 5: Extract Legacy Invocation Contracts And Finalize The Hub

**Files:**
- Create: `crates/conary-core/src/scriptlet/legacy.rs`
- Modify: `crates/conary-core/src/scriptlet/mod.rs`

- [ ] **Step 1: Add legacy module declaration and re-export**

In `scriptlet/mod.rs`, add:

```rust
mod legacy;
```

Add:

```rust
pub use legacy::{LegacyInvocationRuntime, LegacyScriptletExecution};
```

Do not run `cargo check` while both the original legacy structs and the re-export exist.

- [ ] **Step 2: Create `legacy.rs`**

Move these items from `mod.rs` into `legacy.rs`:

- `LegacyScriptletExecution`
- `LegacyInvocationRuntime`
- `LEGACY_MIN_TIMEOUT_MS`
- `LEGACY_MAX_TIMEOUT_MS`
- `LEGACY_SAFE_PATH`
- `DANGEROUS_LEGACY_ENV_KEYS`
- `preflight_legacy_entry`
- `execute_legacy_entry_with_outcome`
- `validate_legacy_execution_contracts`
- `validate_legacy_interpreter_args`
- `derive_legacy_native_args`
- `legacy_environment`
- `decode_legacy_body`
- `runtime_old_version`
- `runtime_new_version`
- `validate_stdin_contract`
- `validate_chroot_contract`
- `validate_legacy_environment_key`

Keep `preflight_legacy_entry` and `execute_legacy_entry_with_outcome` as public methods on `ScriptletExecutor`.

- [ ] **Step 3: Move legacy tests**

Move the legacy test helpers and seven legacy tests into `legacy.rs`.

Helpers:

- `legacy_execution_with_contracts`
- `upgrade_runtime`

Tests:

- `legacy_native_arg_contracts_use_runtime_versions_and_literals`
- `legacy_native_arg_contracts_use_runtime_remove_count`
- `legacy_native_arg_contracts_refuse_malformed_or_missing_runtime_values`
- `legacy_preflight_refuses_unsupported_invocation_fields`
- `legacy_preflight_rejects_body_hash_mismatch`
- `legacy_execution_uses_safe_path_and_derived_args`
- `legacy_execution_skips_target_root_when_interpreter_is_absent`

- [ ] **Step 4: Remove moved definitions and the parent test module**

Delete the original legacy definitions and tests from `mod.rs`.

After this step, `scriptlet/mod.rs` must contain only:

- Module-level docs.
- Module declarations.
- Public re-exports.

There must be no `#[cfg(test)] mod tests` block in `scriptlet/mod.rs`.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib scriptlet::legacy::tests
cargo test -p conary-core --lib scriptlet:: -- --list
```

Expected:

- All commands pass.
- The list output still includes 37 direct scriptlet tests, but under child module paths rather than `scriptlet::tests`.
- No tests remain under `scriptlet::tests`.

- [ ] **Step 6: Confirm final hub shape**

Run:

```bash
rg -n "^(pub |pub\\(|fn |async fn|struct |enum |impl |mod |pub use |#\\[cfg\\(test\\)\\])" crates/conary-core/src/scriptlet/mod.rs crates/conary-core/src/scriptlet -g '*.rs'
```

Expected:

- `scriptlet/mod.rs` only lists `mod ...;` and `pub use ...;` entries after the module docs.
- `ScriptletExecutor` appears in `executor.rs`.
- No `#[cfg(test)]` entry appears in `scriptlet/mod.rs`.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/scriptlet/mod.rs \
        crates/conary-core/src/scriptlet/legacy.rs
git commit -m "refactor(scriptlet): extract legacy contracts"
```

## Task 6: Update Docs Routing And Docs-Audit Ledger

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update subsystem map scriptlet routing**

In `docs/llms/subsystem-map.md`, update the install/scriptlet routing bullet so it lists the new core scriptlet owner modules after the install command files:

```markdown
  `crates/conary-core/src/scriptlet/mod.rs`,
  `crates/conary-core/src/scriptlet/executor.rs`,
  `crates/conary-core/src/scriptlet/arguments.rs`,
  `crates/conary-core/src/scriptlet/sandbox.rs`,
  `crates/conary-core/src/scriptlet/process.rs`,
  `crates/conary-core/src/scriptlet/legacy.rs`,
  `crates/conary-core/src/scriptlet/runtime.rs`, and
  `docs/modules/test-fixtures.md`
```

- [ ] **Step 2: Update feature ownership scriptlet routing**

In `docs/modules/feature-ownership.md`, update the Native Package Install card `Neighbor systems` field from:

```markdown
`crates/conary-core/src/db/`; `crates/conary-core/src/scriptlet/`;
```

to:

```markdown
`crates/conary-core/src/db/`; `crates/conary-core/src/scriptlet/mod.rs`;
`crates/conary-core/src/scriptlet/executor.rs`;
`crates/conary-core/src/scriptlet/sandbox.rs`;
`crates/conary-core/src/scriptlet/process.rs`;
`crates/conary-core/src/scriptlet/legacy.rs`;
```

Also update the CCS card `Neighbor systems` field so `scriptlet sandboxing` names the same core scriptlet path family rather than only the broad concept.

- [ ] **Step 3: Update Scriptlet Security path notes**

In `docs/SCRIPTLET_SECURITY.md`, add a short ownership note after the introductory paragraph:

```markdown
Code owners for this model live under `crates/conary-core/src/scriptlet/`:
`sandbox.rs` owns sandbox mode and protected live-root policy, `process.rs`
owns direct/target-root/chroot execution, `legacy.rs` owns legacy replay
invocation contracts, `arguments.rs` owns distro argument mapping, and
`runtime.rs` owns subprocess/seccomp helper plumbing.
```

Also update the `## Implementation Files` table from:

```markdown
| `crates/conary-core/src/scriptlet/mod.rs` | Scriptlet executor, cross-distro handling |
```

to:

```markdown
| `crates/conary-core/src/scriptlet/mod.rs` | Public scriptlet API hub and re-exports |
| `crates/conary-core/src/scriptlet/types.rs` | Package format and execution mode value types |
| `crates/conary-core/src/scriptlet/outcome.rs` | Typed scriptlet outcomes and failure classification |
| `crates/conary-core/src/scriptlet/phases.rs` | Scriptlet phase string conversions |
| `crates/conary-core/src/scriptlet/executor.rs` | Public `ScriptletExecutor` orchestration |
| `crates/conary-core/src/scriptlet/arguments.rs` | RPM, Debian, and Arch argument mapping |
| `crates/conary-core/src/scriptlet/sandbox.rs` | Sandbox mode and protected live-root policy |
| `crates/conary-core/src/scriptlet/process.rs` | Direct, target-root, chroot, and sandboxed process execution |
| `crates/conary-core/src/scriptlet/legacy.rs` | Legacy replay invocation contracts |
| `crates/conary-core/src/scriptlet/runtime.rs` | Subprocess, seccomp, and chroot helper plumbing |
```

- [ ] **Step 4: Update docs-audit ledger rows for touched docs**

Update the existing ledger rows for:

- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/SCRIPTLET_SECURITY.md`

For each row:

- Add the new scriptlet child module paths to `evidence_sources`.
- Add a tag such as `scriptlet-module` if not already present.
- Append a note fragment in the existing style that mentions Phase 17 scriptlet child-module ownership.

- [ ] **Step 5: Run docs verification**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected:

- Inventory remains `161`.
- Ledger counts remain `corrected 61`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
- Malformed-row check prints nothing.
- Ledger check passes.
- `git diff --check` exits 0.

- [ ] **Step 6: Commit**

```bash
git add docs/llms/subsystem-map.md \
        docs/modules/feature-ownership.md \
        docs/SCRIPTLET_SECURITY.md \
        docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: route scriptlet module owners"
```

## Task 7: Final Verification, Push, And Clean Sync Proof

**Files:** no planned edits.

- [ ] **Step 1: Run final focused scriptlet proof**

Run:

```bash
cargo test -p conary-core --lib scriptlet:: -- --list
cargo test -p conary-core scriptlet -- --list
cargo test -p conary-core --lib scriptlet::types::tests
cargo test -p conary-core --lib scriptlet::sandbox::tests
cargo test -p conary-core --lib scriptlet::phases::tests
cargo test -p conary-core --lib scriptlet::executor::tests
cargo test -p conary-core --lib scriptlet::arguments::tests
cargo test -p conary-core --lib scriptlet::process::tests
cargo test -p conary-core --lib scriptlet::runtime::tests
cargo test -p conary-core --lib scriptlet::legacy::tests
```

Expected:

- `scriptlet:: -- --list` shows 37 direct scriptlet tests under child module paths.
- `scriptlet -- --list` remains the broader 108-test scriptlet-related conary-core inventory.
- All focused child-module tests pass.

- [ ] **Step 2: Run interaction gates**

Run:

```bash
cargo test -p conary-core --lib scriptlet
cargo test -p conary-core --lib legacy_replay
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary --test query_scripts
cargo test -p conary --test live_host_mutation_safety install
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: all commands pass.

- [ ] **Step 3: Run package and workspace gates**

Run:

```bash
cargo fmt --check
cargo check -p conary-core
cargo test -p conary-core
cargo test -p conary
cargo clippy -p conary-core --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all commands pass. If workspace clippy finds unrelated pre-existing warnings, stop and document exact output before deciding whether it is in scope.

- [ ] **Step 4: Run docs and maintainability gates**

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

- `crates/conary-core/src/scriptlet/mod.rs` no longer appears as a large hotspot.
- Inventory remains `161`.
- Ledger counts remain `corrected 61`, `archived 73`, `retained-historical 14`, `verified-no-change 13`.
- Malformed-row check prints nothing.
- Ledger check passes.
- `git diff --check` exits 0.

- [ ] **Step 5: Inspect commit stack and push**

Run:

```bash
git status --short --branch
git log --oneline origin/main..HEAD
git push
```

Expected: branch is clean before push except being ahead by the Phase 17 commits; push succeeds.

- [ ] **Step 6: Prove clean synced main**

Run:

```bash
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected:

- `git status --short --branch` shows `## main...origin/main` with no changed files.
- `git rev-parse HEAD origin/main` prints the same SHA twice.
- Divergence prints `0	0`.
- Worktree list shows only `/home/peter/Conary` on `refs/heads/main`.

## Review Prompts

Use this prompt for both Gemini and DeepSeek after local review and before lock-in:

```text
You are reviewing a repository-grounded Rust refactor plan for Conary.

Repository: /home/peter/Conary
Plan under review:
docs/superpowers/plans/2026-06-08-project-maintainability-phase17-scriptlet-module-decomposition-plan.md

Task: Critically review the Phase 17 Scriptlet Module Decomposition Plan against
the actual repository. Do not implement it. Verify whether the proposed module
split for crates/conary-core/src/scriptlet/mod.rs is compile-safe, visibility-safe,
test-complete, docs-audit-consistent, and suitable for a single /goal execution.

Please check at least:
- Current git status and HEAD/origin sync.
- Current line-count hotspot ranking.
- Current public API surface of conary_core::scriptlet.
- All external callers of ScriptletExecutor, SandboxMode, EffectiveSandbox,
  ScriptletFailureKind, ScriptletFailureOutcome, ScriptletOutcome, PackageFormat,
  ExecutionMode, LegacyScriptletExecution, LegacyInvocationRuntime,
  phase_to_string, phase_from_string, and set_seccomp_warn_override.
- Rust privacy across sibling modules under scriptlet/.
- Whether ScriptletExecutor fields must be pub(super) for sibling impl blocks.
- Whether methods called across sibling modules are marked pub(super) in the plan.
- Whether runtime::set_seccomp_warn_override can be re-exported publicly after
  changing its definition visibility.
- Whether every one of the 37 current scriptlet::tests tests is assigned exactly
  once to a child module, including helper functions and environment locks.
- Whether import surfaces for executor.rs, arguments.rs, sandbox.rs, process.rs,
  legacy.rs, outcome.rs, phases.rs, types.rs, and runtime.rs are sufficient and
  avoid unused imports under clippy -D warnings.
- Whether docs updates and docs-audit count math are correct: baseline 160/60,
  lock-in 161/61.
- Whether the verification gates are sufficient, including conary-core,
  conary interaction tests, clippy, docs-audit, and maintainability drift.

Return:
1. Summary verdict: Ready / Ready with fixes / Not ready.
2. Critical findings that would cause compile failures, behavior regressions,
   broken public API, or invalid docs-audit state.
3. Important findings.
4. Minor findings.
5. Missing concerns.
6. Suggested exact edits to the plan.
7. Verification commands you ran and results.
8. Claims you verified against code.
9. Claims not verified and why.
```

## Self-Review Checklist

- [ ] `scriptlet/mod.rs` remains the public hub and keeps all current public API names.
- [ ] No behavior-changing edits are requested.
- [ ] Every new Rust file has a path comment.
- [ ] `ScriptletExecutor` fields are `pub(super)` because sibling impl modules need field access.
- [ ] Cross-sibling methods are `pub(super)`.
- [ ] `set_seccomp_warn_override` re-export plan changes the runtime definition visibility.
- [ ] All 37 direct scriptlet tests are assigned exactly once.
- [ ] Docs-audit counts move from 160/60 to 161/61 at plan lock-in and remain 161/61 through implementation.
- [ ] Final proof includes full conary-core and workspace clippy gates.
