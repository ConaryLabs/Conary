# Project Maintainability Phase 9 CCS Payload Paths Decomposition Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. This is the Phase 9 child packet
> under
> `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Extract CCS payload path normalization and symlink safety from
`apps/conary/src/commands/ccs/install.rs` into a focused CCS module without
changing install behavior, runtime extraction behavior, package formats,
live-root safety, or CCS transaction semantics.

**Architecture:** Add
`apps/conary/src/commands/ccs/payload_paths.rs` as the owner for package-path
sanitization, usr-merge path rewriting, duplicate deployment-path coalescing,
and symlink ancestor rejection. Keep dependency validation, component
selection, capability policy, signature verification, and `ccs install`
orchestration in `ccs/install.rs`, and preserve existing cross-module imports
through `commands::ccs` re-exports where callers already use that surface.

**Tech Stack:** Rust, existing Conary CCS command modules, existing
`conary_core::ccs` package APIs, existing `ExtractedFile` package trait type,
existing cargo tests, docs-audit scripts.

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
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase8-install-ccs-transaction-decomposition-plan.md`
- `apps/conary/src/commands/ccs/mod.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/ccs/runtime.rs`
- `apps/conary/src/commands/install/ccs_transaction.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/tests/component.rs`
- `apps/conary/tests/bundle_replay.rs`
- `apps/conary/tests/conversion_integration.rs`

## Design Summary

Phase 8 moved direct CCS transaction install ownership out of
`install/mod.rs`. The next highest-leverage slice is not another
`install/mod.rs` extraction, but a smaller cut from the new largest file:
`apps/conary/src/commands/ccs/install.rs`.

The first coherent owner inside `ccs/install.rs` is payload path safety. It is
a compact cluster at the top of the file, already has a few focused unit tests,
and is shared by three surfaces:

- `ccs install` validates selected payloads before transaction execution.
- `install/ccs_transaction.rs` normalizes extracted CCS payloads and manifest
  file paths before storing transaction metadata.
- `ccs/runtime.rs` sanitizes package paths when exporting or running CCS
  runtime payloads.

This slice should not split all CCS install logic. Dependency validation,
version constraints, component selection, capability policy, trust
verification, and command orchestration should stay in `ccs/install.rs`.

## Current Repo-Grounded Inputs

| Signal | Current value | Phase 9 interpretation |
|--------|---------------|------------------------|
| Largest Rust files | `apps/conary/src/commands/ccs/install.rs` 3441 lines; `apps/conary/src/commands/update.rs` 3334 lines; `apps/remi/src/server/conversion.rs` 2999 lines; `apps/conary/src/commands/install/mod.rs` 2874 lines | `ccs/install.rs` is now the top maintainability hotspot |
| Existing CCS modules | `build.rs`, `enhance.rs`, `init.rs`, `inspect.rs`, `install.rs`, `runtime.rs`, `signing.rs` | A sibling `payload_paths.rs` follows the existing file-style module layout |
| Current payload-path exports | `commands::ccs::{normalize_ccs_extracted_files, normalize_ccs_package_path, validate_ccs_payload_paths}` | Preserve these re-exported paths for `install/ccs_transaction.rs` |
| Current runtime sanitizer use | `ccs/runtime.rs` imports `super::install::sanitize_package_relative_path` | Move this import to `super::payload_paths::sanitize_package_relative_path` |
| Docs-audit baseline | 152 tracked doc-like files, 52 corrected rows | Lock-in should add one planning file and update counts to 153 total / 53 corrected |

Evidence commands used to shape this packet:

```bash
scripts/line-count-report.sh 10
find apps/conary/src/commands/ccs -maxdepth 2 -type f | sort
rg -n "normalize_ccs_package_path|normalize_ccs_extracted_files|validate_ccs_payload_paths|sanitize_package_relative_path" apps/conary/src/commands/ccs apps/conary/src/commands/install -g '*.rs'
cargo test -p conary --lib sanitize -- --list
cargo test -p conary --lib usrmerge -- --list
cargo test -p conary --lib ccs_install_rejects_child_write_beneath_package_symlink -- --list
cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload -- --list
cargo test -p conary --test component -- --list
cargo test -p conary --test bundle_replay -- --list
```

Current filter-discovery results:

| Filter | Current matches |
|--------|-----------------|
| `cargo test -p conary --lib sanitize -- --list` | 3 sanitizer unit tests |
| `cargo test -p conary --lib usrmerge -- --list` | 3 CCS install usr-merge tests |
| `cargo test -p conary --lib ccs_install_rejects_child_write_beneath_package_symlink -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_rejects_child_before_package_symlink -- --list` | 2 tests, including converted CCS install coverage |
| `cargo test -p conary --lib ccs_install_persists_usrmerge_payload_under_usr_path -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_coalesces_identical_usrmerge_duplicate_files -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_rejects_conflicting_usrmerge_duplicate_files -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_replaces_existing_leaf_symlink_destination -- --list` | 1 test |
| `cargo test -p conary --lib ccs_install_allows_identical_existing_symlink_destination -- --list` | 1 test |
| `cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload -- --list` | 1 test |
| `cargo test -p conary --lib converted_ccs_install_rejects_child_before_package_symlink -- --list` | 1 test |
| `cargo test -p conary --test component -- --list` | 8 tests |
| `cargo test -p conary --test bundle_replay -- --list` | 26 tests |
| `cargo test -p conary --test conversion_integration golden_conversion -- --list` | 4 tests |

## Module Boundary

Create:

- `apps/conary/src/commands/ccs/payload_paths.rs`

Move these items from `apps/conary/src/commands/ccs/install.rs` into
`payload_paths.rs`:

- `sanitize_package_relative_path`
- `deployed_mode`
- `is_symlink_mode`
- `is_extracted_symlink`
- `symlink_target_for_file`
- `DeploymentFile`
- `standard_usrmerge_target`
- `rewrite_standard_usrmerge_root_symlink`
- `deployment_path_to_package_path`
- `normalize_ccs_package_path`
- `package_deployment_relative_path`
- `identical_regular_deployment`
- `find_symlink_blocker`
- `ensure_no_symlink_ancestor`
- `validate_ccs_payload_paths`
- `normalize_ccs_extracted_files`
- direct sanitizer unit tests:
  - `sanitize_rejects_path_traversal`
  - `sanitize_accepts_normal_paths`
  - `sanitize_rejects_empty_path`

Keep these items in `apps/conary/src/commands/ccs/install.rs` for this slice:

- `package_provided_names`
- `package_self_provides`
- `enforce_ccs_capability_policy`
- dependency and version validation helpers
- `SelectedCcsComponents`
- component selection helpers
- `repo_constraint_*` helpers
- `cmd_ccs_install`
- `cmd_ccs_install_with_replay_options`
- existing command-level CCS install tests, including usr-merge, symlink
  safety, component selection, capability policy, legacy replay, and
  persisted-provides tests

Update `apps/conary/src/commands/ccs/mod.rs`:

```rust
mod payload_paths;

pub(crate) use payload_paths::{
    normalize_ccs_extracted_files, normalize_ccs_package_path,
    validate_ccs_payload_paths,
};
```

Update `apps/conary/src/commands/ccs/runtime.rs`:

```rust
use super::payload_paths::sanitize_package_relative_path;
```

`sanitize_package_relative_path` should be `pub(super)` in
`payload_paths.rs`, because it is used only by CCS sibling modules.
`normalize_ccs_package_path`, `normalize_ccs_extracted_files`, and
`validate_ccs_payload_paths` should remain `pub(crate)` because they are
re-exported through `commands::ccs` and consumed by
`install/ccs_transaction.rs`.

## Non-Goals

- Do not change CCS manifest schema, package archive format, package payload
  hashing, symlink target semantics, or persisted DB layout.
- Do not change live-system mutation UX, command flags, conaryd API fields, or
  integration manifest syntax.
- Do not move dependency validation, version constraint validation, component
  selection, capability policy, trust verification, or command orchestration.
- Do not convert `ccs/install.rs` into an `install/` directory module in this
  slice.
- Do not move the command-level CCS install tests unless a test directly names
  a moved private helper.
- Do not weaken usr-merge duplicate handling, symlink ancestor rejection,
  selected-component payload validation, or converted CCS install safety.

## Review Focus

Reviewers should check:

- whether `payload_paths.rs` owns a coherent path-safety boundary;
- whether `sanitize_package_relative_path` visibility remains narrow while
  preserving `ccs/runtime.rs`;
- whether `commands::ccs` re-exports preserve
  `install/ccs_transaction.rs` imports;
- whether the plan avoids a noisy file-to-directory conversion for
  `ccs/install.rs`;
- whether the verification list covers both direct normalization behavior and
  install/conversion behavior that depends on it.

## Tasks

### Task 0: Lock Planning Doc Into The Docs Audit

**Files:**

- Create:
  `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase9-ccs-payload-paths-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the new plan before regenerating inventory**

Run:

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase9-ccs-payload-paths-decomposition-plan.md
```

Expected: the new plan is staged so the tracked-file inventory script can see
it.

- [ ] **Step 2: Regenerate docs-audit inventory**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: the inventory includes the Phase 9 plan as a planning/maintainer
row. If another docs file lands first, use the regenerated inventory as the
source of truth and update counts accordingly.

- [ ] **Step 3: Add the ledger row**

Add this literal-tab row near the active maintainability plan rows, after the
Phase 8 row in `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```tsv
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase9-ccs-payload-paths-decomposition-plan.md	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase9-ccs-payload-paths-decomposition-plan.md	planning	maintainer	maintainability; phase9; ccs; payload-paths; hotspot-decomposition	apps/conary/src/commands/ccs/install.rs; apps/conary/src/commands/ccs/runtime.rs; apps/conary/src/commands/install/ccs_transaction.rs; docs/modules/ccs.md	verified	corrected	Added Phase 9 plan for extracting CCS payload path normalization and symlink safety into a focused CCS module while preserving install and runtime behavior.
```

- [ ] **Step 4: Update the audit summary narrative and counts**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph to the existing `### 2026-06-06 Maintainability Planning` section:

```markdown
The Phase 9 CCS payload paths decomposition plan continues the CCS
maintainability work by targeting the current largest source file,
`apps/conary/src/commands/ccs/install.rs`. It extracts CCS payload path
normalization and symlink safety helpers into
`apps/conary/src/commands/ccs/payload_paths.rs`, while keeping dependency
validation, component selection, capability policy, trust verification, and
command orchestration in `ccs/install.rs`, and preserving existing
cross-module re-exports.
```

Then update the final counts from:

```markdown
- Total tracked doc-like files audited: 152
- `verified-no-change`: 13
- `corrected`: 52
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

to:

```markdown
- Total tracked doc-like files audited: 153
- `verified-no-change`: 13
- `corrected`: 53
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its
`claim_clusters`, `evidence_sources`, and notes include the Phase 9 planning
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
153
archived 73
corrected 53
retained-historical 14
verified-no-change 13
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 6: Commit the locked plan**

Run:

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase9-ccs-payload-paths-decomposition-plan.md \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv \
    docs/superpowers/documentation-accuracy-audit-inventory.tsv \
    docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan ccs payload path decomposition"
```

Expected: docs-only planning commit succeeds.

### Task 1: Create The CCS Payload Paths Module

**Files:**

- Create: `apps/conary/src/commands/ccs/payload_paths.rs`
- Modify: `apps/conary/src/commands/ccs/mod.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`
- Modify: `apps/conary/src/commands/ccs/runtime.rs`

- [ ] **Step 1: Add the module and re-export surface**

In `apps/conary/src/commands/ccs/mod.rs`, add the module next to the existing
CCS siblings:

```rust
mod payload_paths;
```

Replace the current payload-path re-export from `install`:

```rust
pub(crate) use install::{
    enforce_ccs_capability_policy, normalize_ccs_extracted_files, normalize_ccs_package_path,
    validate_ccs_payload_paths,
};
```

with:

```rust
pub(crate) use install::enforce_ccs_capability_policy;
pub(crate) use payload_paths::{
    normalize_ccs_extracted_files, normalize_ccs_package_path, validate_ccs_payload_paths,
};
```

Expected: external callers still import the three payload helpers through
`crate::commands::ccs`.

- [ ] **Step 2: Move payload-path helpers into the new file**

Create `apps/conary/src/commands/ccs/payload_paths.rs` with the path comment
and moved helpers:

```rust
// src/commands/ccs/payload_paths.rs

//! CCS payload path normalization and symlink safety.

use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::{ExtractedFile, PackageFormat};
use std::collections::{HashMap, HashSet};
use std::path::{Component as PathComponent, Path, PathBuf};

pub(super) fn sanitize_package_relative_path(path: &str) -> Result<PathBuf> {
    let candidate = path.strip_prefix('/').unwrap_or(path);
    let mut normalized = PathBuf::new();

    for component in Path::new(candidate).components() {
        match component {
            PathComponent::CurDir => {}
            PathComponent::Normal(part) => normalized.push(part),
            PathComponent::ParentDir => {
                anyhow::bail!("path traversal detected in package path: {path}")
            }
            PathComponent::RootDir | PathComponent::Prefix(_) => {
                anyhow::bail!("invalid package path component in {path}")
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        anyhow::bail!("empty package path is not allowed");
    }

    Ok(normalized)
}

fn deployed_mode(mode: i32) -> (i32, bool) {
    let stripped = mode & !0o6000;
    (stripped, stripped != mode)
}

fn is_symlink_mode(mode: i32) -> bool {
    (mode & 0o170000) == 0o120000
}

fn is_extracted_symlink(file: &ExtractedFile) -> bool {
    is_symlink_mode(file.mode) || file.symlink_target.is_some()
}

fn symlink_target_for_file(file: &ExtractedFile) -> Result<String> {
    if let Some(target) = &file.symlink_target {
        return Ok(target.clone());
    }

    String::from_utf8(file.content.clone()).context("invalid symlink target in package payload")
}

struct DeploymentFile {
    file: ExtractedFile,
    relative_path: PathBuf,
    symlink_target: Option<String>,
}
```

Then move the remaining helper bodies exactly as they exist today:

- `standard_usrmerge_target`
- `rewrite_standard_usrmerge_root_symlink`
- `deployment_path_to_package_path`
- `normalize_ccs_package_path`
- `package_deployment_relative_path`
- `identical_regular_deployment`
- `find_symlink_blocker`
- `ensure_no_symlink_ancestor`
- `validate_ccs_payload_paths`
- `normalize_ccs_extracted_files`

Expected: `payload_paths.rs` owns all imports it needs; `install.rs` no
longer imports `ExtractedFile`, `PathComponent`, `PathBuf`, or the top-level
`HashMap`/`HashSet` collection import for the moved helpers.

- [ ] **Step 3: Update `ccs/install.rs` to call the new module**

At the top of `apps/conary/src/commands/ccs/install.rs`, keep
`PackageFormat` and `Path` for the remaining command-level code, and import
the moved validator:

```rust
use super::payload_paths::validate_ccs_payload_paths;
use conary_core::packages::traits::PackageFormat;
use std::path::Path;
```

Remove these imports from `install.rs` if they are no longer needed outside
tests. The two remaining tests that use `PathBuf` import it inside their test
functions, so no top-level `PathBuf` import should remain:

```rust
use conary_core::packages::traits::ExtractedFile;
use std::collections::{HashMap, HashSet};
use std::path::{Component as PathComponent, PathBuf};
```

Expected: `cmd_ccs_install_with_replay_options` still calls:

```rust
validate_ccs_payload_paths(Path::new(root), &ccs_pkg, &selected_components.names)?;
```

`PackageFormat` should remain in `install.rs` because the command still uses
trait methods such as `CcsPackage::parse`, `ccs_pkg.name()`, and
`ccs_pkg.version()`. `payload_paths.rs` should import `PackageFormat` because
`validate_ccs_payload_paths` calls `ccs_pkg.extract_file_contents()`.

- [ ] **Step 4: Update `ccs/runtime.rs` sanitizer import**

Replace:

```rust
use super::install::sanitize_package_relative_path;
```

with:

```rust
use super::payload_paths::sanitize_package_relative_path;
```

Expected: runtime export/run code keeps the same sanitizer behavior through
the new owner module.

- [ ] **Step 5: Move direct sanitizer tests**

Move these tests from the `install.rs` test module into a `#[cfg(test)]`
module at the bottom of `payload_paths.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::sanitize_package_relative_path;
    use std::path::PathBuf;

    #[test]
    fn sanitize_rejects_path_traversal() {
        let err = sanitize_package_relative_path("../../etc/shadow").unwrap_err();
        assert!(err.to_string().contains("path traversal"));

        let err = sanitize_package_relative_path("/usr/../../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn sanitize_accepts_normal_paths() {
        assert_eq!(
            sanitize_package_relative_path("/usr/bin/hello").unwrap(),
            PathBuf::from("usr/bin/hello")
        );
        assert_eq!(
            sanitize_package_relative_path("usr/lib/libfoo.so").unwrap(),
            PathBuf::from("usr/lib/libfoo.so")
        );
        assert_eq!(
            sanitize_package_relative_path("/usr/./bin/./hello").unwrap(),
            PathBuf::from("usr/bin/hello")
        );
    }

    #[test]
    fn sanitize_rejects_empty_path() {
        let err = sanitize_package_relative_path("").unwrap_err();
        assert!(err.to_string().contains("empty package path"));
    }
}
```

Expected: the tests now resolve under
`commands::ccs::payload_paths::tests::*`.

- [ ] **Step 6: Run focused compile and sanitizer tests**

Run:

```bash
cargo test -p conary --lib sanitize
cargo check -p conary
```

Expected: all 3 sanitizer tests pass and `conary` compiles.

- [ ] **Step 7: Commit the module extraction scaffold**

Run:

```bash
git add apps/conary/src/commands/ccs/mod.rs \
    apps/conary/src/commands/ccs/payload_paths.rs \
    apps/conary/src/commands/ccs/install.rs \
    apps/conary/src/commands/ccs/runtime.rs
git commit -m "refactor(ccs): move payload path helpers"
```

Expected: the first code commit contains only the helper move, module wiring,
and direct unit-test relocation.

### Task 2: Preserve Install And Conversion Safety Proofs

**Files:**

- Modify: `apps/conary/src/commands/ccs/payload_paths.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`
- Verify: `apps/conary/src/commands/install/ccs_transaction.rs`
- Verify: `apps/conary/src/commands/install/conversion.rs`

- [ ] **Step 1: Run direct CCS path-safety tests**

Run:

```bash
cargo test -p conary --lib ccs_install_rejects_child_write_beneath_package_symlink
cargo test -p conary --lib ccs_install_rejects_child_before_package_symlink
cargo test -p conary --lib ccs_install_persists_usrmerge_payload_under_usr_path
cargo test -p conary --lib ccs_install_coalesces_identical_usrmerge_duplicate_files
cargo test -p conary --lib ccs_install_rejects_conflicting_usrmerge_duplicate_files
cargo test -p conary --lib ccs_install_replaces_existing_leaf_symlink_destination
cargo test -p conary --lib ccs_install_allows_identical_existing_symlink_destination
```

Expected: all focused CCS install path-safety tests pass. The
`ccs_install_rejects_child_before_package_symlink` filter is expected to match
both the direct CCS install test and the converted CCS install test.

- [ ] **Step 2: Run converted CCS path-safety tests**

Run:

```bash
cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload
cargo test -p conary --lib converted_ccs_install_rejects_child_before_package_symlink
```

Expected: both converted CCS install tests pass, proving the shared
transaction path still receives normalized payload paths from the re-exported
helpers.

- [ ] **Step 3: Run component and replay integration tests**

Run:

```bash
cargo test -p conary --test component
cargo test -p conary --test bundle_replay
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: component selection, legacy replay, and golden conversion behavior
remain unchanged.

- [ ] **Step 4: Check for stale imports and accidental visibility broadening**

Run:

```bash
rg -n "super::install::sanitize_package_relative_path|pub\\(crate\\) fn sanitize_package_relative_path|PathComponent|ExtractedFile" apps/conary/src/commands/ccs -g '*.rs'
if sed -n '1,40p' apps/conary/src/commands/ccs/install.rs | rg -n 'use std::collections::\{HashMap, HashSet\}|Component as PathComponent|ExtractedFile|PathBuf'; then
    echo "unexpected stale top-level import in ccs/install.rs" >&2
    exit 1
fi
rg -n "normalize_ccs_package_path|normalize_ccs_extracted_files|validate_ccs_payload_paths" apps/conary/src/commands/install/ccs_transaction.rs apps/conary/src/commands/ccs/mod.rs apps/conary/src/commands/ccs/payload_paths.rs
```

Expected:

- no `super::install::sanitize_package_relative_path` references remain;
- no `pub(crate) fn sanitize_package_relative_path` exists;
- `PathComponent` and `ExtractedFile` appear in `payload_paths.rs`, not in
  `ccs/install.rs` unless tests still need a local import;
- the top-level `install.rs` import block no longer contains
  `HashMap`/`HashSet`, `PathComponent`, `ExtractedFile`, or `PathBuf`;
- the three re-exported helpers still appear in `ccs/mod.rs` and are consumed
  by `install/ccs_transaction.rs`.

- [ ] **Step 5: Commit safety proof updates if code changed**

If Tasks 2 steps required any import or visibility fixes, commit them:

```bash
git add apps/conary/src/commands/ccs/mod.rs \
    apps/conary/src/commands/ccs/payload_paths.rs \
    apps/conary/src/commands/ccs/install.rs \
    apps/conary/src/commands/ccs/runtime.rs \
    apps/conary/src/commands/install/ccs_transaction.rs
git commit -m "refactor(ccs): preserve payload path safety callers"
```

Expected: if no code changed after Task 1, skip this commit and record the
passing commands in the final implementation notes.

### Task 3: Final Workspace Verification

**Files:**

- Verify: `apps/conary/src/commands/ccs/install.rs`
- Verify: `apps/conary/src/commands/ccs/payload_paths.rs`
- Verify: `apps/conary/src/commands/ccs/runtime.rs`
- Verify: `apps/conary/src/commands/install/ccs_transaction.rs`

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
cargo test -p conary --lib sanitize
cargo test -p conary --lib usrmerge
cargo test -p conary --lib ccs_install_rejects_child_write_beneath_package_symlink
cargo test -p conary --lib ccs_install_rejects_child_before_package_symlink
cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload
cargo test -p conary --lib converted_ccs_install_rejects_child_before_package_symlink
cargo test -p conary --test component
cargo test -p conary --test bundle_replay
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: all tests pass.

- [ ] **Step 4: Run Clippy for the touched package**

Run:

```bash
cargo clippy -p conary --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 5: Verify hotspot reduction**

Run:

```bash
scripts/line-count-report.sh 10
wc -l apps/conary/src/commands/ccs/install.rs apps/conary/src/commands/ccs/payload_paths.rs
```

Expected: `ccs/install.rs` has dropped by roughly the size of the moved helper
cluster and `payload_paths.rs` is a focused path-safety module.

- [ ] **Step 6: Check diff hygiene**

Run:

```bash
git diff --check
git status --short --branch
```

Expected: no whitespace errors; status shows only the intentional working
tree changes if commits are not yet made, or a clean branch if all task commits
have landed.

- [ ] **Step 7: Commit final verification fixes if needed**

If formatting or Clippy required changes after prior commits, commit them:

```bash
git add apps/conary/src/commands/ccs/mod.rs \
    apps/conary/src/commands/ccs/payload_paths.rs \
    apps/conary/src/commands/ccs/install.rs \
    apps/conary/src/commands/ccs/runtime.rs
git commit -m "refactor(ccs): finish payload path split"
```

Expected: no uncommitted code changes remain after the final task.

## Final Verification Before Merge

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
cargo fmt --check
cargo check -p conary
cargo test -p conary --lib sanitize
cargo test -p conary --lib usrmerge
cargo test -p conary --lib ccs_install_rejects_child_write_beneath_package_symlink
cargo test -p conary --lib ccs_install_rejects_child_before_package_symlink
cargo test -p conary --lib converted_ccs_install_rejects_symlink_child_payload
cargo test -p conary --lib converted_ccs_install_rejects_child_before_package_symlink
cargo test -p conary --test component
cargo test -p conary --test bundle_replay
cargo test -p conary --test conversion_integration golden_conversion
cargo clippy -p conary --all-targets -- -D warnings
git diff --check
```

Expected:

- docs-audit inventory count is `153` after plan lock-in;
- docs-audit ledger check passes;
- format, check, focused tests, integration tests, and Clippy all pass;
- `git diff --check` reports no whitespace errors.

## Rollback

If a later task exposes an unexpected regression, revert the Phase 9 commits in
reverse order. Because this slice only moves helpers and preserves the
`commands::ccs` public re-export surface, a clean rollback should restore
`ccs/install.rs` as the single owner without requiring schema, data, or docs
migration rollback.
