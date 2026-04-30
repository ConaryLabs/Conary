# Installed Runtime Generations Self-Contained Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make installed runtime generation qcow2 export boot from explicit CAS-backed package inputs, after migrating the active Fedora integration baseline from Fedora 43 to Fedora 44.

**Architecture:** First migrate the active Fedora test, CI, packaging, and documentation baseline to Fedora 44 as an independently verifiable prerequisite. Then add a shared runtime-generation input classifier/validator in `conary-core` so new builds and rebuilds accept only CAS-backed package inputs, validate symlinks/directories/special files before EROFS construction, preserve `AdoptedTrack` fail-closed behavior, and continue using `write_generation_artifact` as the single regular-file CAS integrity gate. Finally tighten full adoption/takeover bridge behavior and add QEMU validation that exports and boots a truly self-contained installed runtime generation.

**Tech Stack:** Rust, rusqlite, composefs-rs EROFS builder, Conary CAS, systemd-repart/qemu-img export path, Podman/Fedora container fixtures, GitHub Actions, TOML conary-test manifests, QEMU integration suites.

**Spec:** `docs/superpowers/specs/2026-04-30-installed-runtime-generations-self-contained-design.md`

---

## Current State

- `main` contains the reviewed design spec at commit `7eb1f0d3`.
- `conary system generation export` is the raw/qcow2 generation disk export surface.
- Runtime generation build already writes `.conary-artifact.json`, `cas-manifest.json`, and `boot-assets/manifest.json`.
- Runtime generation export still fails closed when the installed root is not self-contained.
- `build_generation_from_db_with_boot_root` and `rebuild_generation_image_with_boot_root` duplicate file collection/filtering and call `validate_runtime_generation_root_is_self_contained`.
- `collect_symlink_refs` silently returns an empty vector if the `symlink_target` column is unavailable.
- `write_generation_artifact` already verifies regular-file CAS object existence, size, and content hash via `verify_cas_objects`.
- The active integration baseline is still `fedora43` / `fedora-43` across `conary-test`, CI, docs, and examples.

## Scope Guard

- Do not add export-time live-root scraping.
- Do not convert `AdoptedTrack` packages into CAS-backed content during generation export.
- Do not reuse `InstallSource::is_conary_owned()` for generation input eligibility; it intentionally excludes `AdoptedFull`.
- Do not re-hash every CAS object in the pre-EROFS validator. Keep regular-file CAS integrity verification in `write_generation_artifact`.
- Do not pass directory entries to the EROFS builder as regular `FileEntryRef` values.
- Do not keep `fedora43` and `fedora44` fixtures in active test rotation unless Fedora 44 itself has a concrete fixture blocker.
- Do not hand-edit generated site output under `site/build/` or `site/.svelte-kit/`.
- Do not rewrite archived plans/reviews or dated validation records merely because they mention Fedora 43 historically.

## File Map

| File | Responsibility |
| --- | --- |
| `docs/superpowers/specs/2026-04-30-installed-runtime-generations-self-contained-design.md` | Reviewed design source of truth |
| `docs/superpowers/plans/2026-04-30-installed-runtime-generations-self-contained-plan.md` | This implementation plan |
| `apps/conary/tests/integration/remi/config.toml` | Active conary-test distro mapping and default repo cleanup list |
| `apps/conary/tests/integration/remi/containers/Containerfile.fedora43` -> `Containerfile.fedora44` | Fedora integration-test container fixture |
| `apps/conary-test/src/lib.rs` | Test helpers currently named `test_global_config_with_fedora` / `test_app_state` |
| `apps/conary-test/src/config/mod.rs` | Inline sample config and assertions for distro config parsing |
| `apps/conary-test/src/container/image.rs` | Hardcoded Fedora containerfile staging tests/paths |
| `apps/conary-test/src/container/lifecycle.rs` | Hardcoded Fedora containerfile path tests |
| `apps/conary-test/src/engine/{variables.rs,runner.rs}` | Test fixture distro keys and Remi distro variables |
| `apps/conary-test/src/server/{service.rs,handlers.rs,mcp.rs}` | conary-test server fixture distro keys and comments |
| `.github/workflows/merge-validation.yml` | Merge validation Fedora default |
| `.github/workflows/scheduled-ops.yml` | Scheduled Fedora matrix and single-distro QEMU job |
| `apps/remi/src/server/test_db.rs` | Remi test-run fixture data that uses the active Fedora distro key |
| `data/distros.toml` | Canonical product distro catalog; add Fedora 44 and decide Fedora 43 retention |
| `apps/conary/src/commands/system.rs` | Default repositories created by `conary system init` |
| `apps/conary/src/commands/distro.rs` | Static distro list output and related tests |
| `apps/conary/src/cli/distro.rs` | Distro CLI help examples |
| `apps/conary/src/commands/install/mod.rs` | Fedora distro-name flavor test |
| `packaging/rpm/{Containerfile.build,build.sh}` | RPM build container baseline and comments |
| `deploy/FORGE.md` | Operator docs with Fedora containerfile path and host OS note |
| `README.md`, `apps/conary-test/README.md`, `docs/INTEGRATION-TESTING.md`, `docs/modules/source-selection.md`, `docs/conaryopedia-v2.md` | Living user/developer docs that should default to Fedora 44 |
| `site/src/routes/install/+page.svelte` | Canonical site source for Fedora RPM support note |
| `crates/conary-core/src/generation/builder.rs` | Generation build/rebuild orchestration and init closure validation |
| `crates/conary-core/src/generation/builder/runtime_inputs.rs` | New focused runtime input classifier and pre-EROFS validator |
| `crates/conary-core/src/generation/builder/erofs.rs` | Existing EROFS builder and shared `hex_to_digest` parser |
| `crates/conary-core/src/generation/artifact.rs` | Existing regular-file CAS object integrity gate |
| `crates/conary-core/src/db/models/file_entry.rs` | File entry model; optional `new_symlink` helper for tests |
| `apps/conary/src/commands/adopt/system.rs` | Full adoption CAS identity computation and failure behavior |
| `apps/conary/src/commands/adopt/{packages.rs,refresh.rs,mod.rs}` | Single-package full adoption, adopted-package refresh, and shared adoption helper exports |
| `apps/conary/src/commands/generation/takeover.rs` | Track-to-CAS and Taken bridge behavior |
| `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml` | Fail-closed, negative CAS, and positive installed-runtime QEMU validation |

## Semantic Fedora 43 References To Preserve

These files use `fedora-43` as source-selection/replatform/policy fixture data rather than the active Fedora integration baseline. Do not mechanically rewrite them unless a test specifically describes the current Fedora fixture:

- `crates/conary-core/src/repository/selector.rs`
- `crates/conary-core/src/db/models/trove.rs`
- `crates/conary-core/src/model/diff.rs`
- `crates/conary-core/src/model/replatform.rs`
- `crates/conary-core/src/packages/mod.rs`
- `crates/conary-core/src/repository/effective_policy.rs`
- most app-level model/update/replatform rendering tests that use `fedora-43` as a from-distro in replatform scenarios

`data/distros.toml` is different: keep `fedora-43` only if explicitly retained as a supported previous-release catalog entry, not as the active default example.

---

## Chunk 1: Fedora 44 Baseline Migration

### Task 1: Prepare The Implementation Worktree

**Files:**
- No source edits in this task

- [ ] **Step 1: Create an isolated worktree for implementation**

Run from `/home/peter/Conary`:

```bash
git fetch origin
git worktree add ../Conary-fedora44-runtime-generations -b feat/fedora44-runtime-generations main
cd ../Conary-fedora44-runtime-generations
```

Expected: new worktree on `feat/fedora44-runtime-generations` from local
`main`, which must already contain the reviewed design and plan commits. Do not
use `origin/main` here unless those documentation commits have been pushed.

- [ ] **Step 2: Verify starting state**

Run:

```bash
git status --short --branch
git log -1 --oneline
```

Expected: clean worktree, with the latest commit equal to the reviewed plan
commit or a later commit that intentionally contains it.

### Task 2: Rename The Active Fedora conary-test Fixture

**Files:**
- Rename: `apps/conary/tests/integration/remi/containers/Containerfile.fedora43` -> `apps/conary/tests/integration/remi/containers/Containerfile.fedora44`
- Modify: `apps/conary/tests/integration/remi/config.toml`
- Modify: `apps/conary-test/src/lib.rs`
- Modify: `apps/conary-test/src/config/mod.rs`
- Modify: `apps/conary-test/src/container/image.rs`
- Modify: `apps/conary-test/src/container/lifecycle.rs`
- Modify: `apps/conary-test/src/engine/variables.rs`
- Modify: `apps/conary-test/src/engine/runner.rs`
- Modify: `apps/conary-test/src/server/service.rs`
- Modify: `apps/conary-test/src/server/handlers.rs`
- Modify: `apps/conary-test/src/server/mcp.rs`
- Modify: `apps/conary-test/README.md`

- [ ] **Step 1: Rename the fixture file**

Run:

```bash
git mv apps/conary/tests/integration/remi/containers/Containerfile.fedora43 apps/conary/tests/integration/remi/containers/Containerfile.fedora44
```

- [ ] **Step 2: Update the Fedora containerfile contents**

Edit `apps/conary/tests/integration/remi/containers/Containerfile.fedora44`:

```dockerfile
# tests/integration/remi/containers/Containerfile.fedora44
# Minimal Fedora 44 container for Remi integration testing

FROM registry.fedoraproject.org/fedora:44
...
ENV DISTRO=fedora44
```

- [ ] **Step 3: Update the integration config**

In `apps/conary/tests/integration/remi/config.toml`:

```toml
[distros.fedora44]
remi_distro = "fedora"
repo_name = "fedora-remi"
...

[setup]
remove_default_repos = [
    "arch-core", "arch-extra", "arch-multilib",
    "fedora-44", "ubuntu-noble",
]
```

- [ ] **Step 4: Update test fixture constructors**

In `apps/conary-test/src/lib.rs`, update the existing
`test_global_config_with_fedora()` and `test_app_state()` helpers. Either keep
the current generic names or rename them to `test_global_config_with_fedora44()`
and `test_app_state_with_fedora44()`; in either case, update comments, internal
distro strings, and all call sites:

```rust
pub fn test_global_config_with_fedora44() -> GlobalConfig {
    let mut config = test_global_config();
    config.distros.insert(
    "fedora44".to_string(),
    DistroConfig {
        remi_distro: "fedora".to_string(),
        repo_name: "conary-fedora44".to_string(),
        containerfile: Some("Containerfile.fedora44".to_string()),
        ...
    },
    );
    config
}
```

Update all call sites.

- [ ] **Step 5: Update inline config parser tests**

In `apps/conary-test/src/config/mod.rs`, update the inline TOML and assertions:

```toml
[distros.fedora44]
remi_distro = "fedora"
repo_name = "conary-fedora44"
containerfile = "Containerfile.fedora44"
```

Assertions should read `config.distros["fedora44"]`, `remi_distro == "fedora"`, and `containerfile == Some("Containerfile.fedora44")`.

- [ ] **Step 6: Update conary-test source references**

Use targeted search:

```bash
rg -n "fedora43|Containerfile\\.fedora43|conary-fedora43" apps/conary-test/src apps/conary-test/README.md
```

Update active fixture keys, comments, and assertions to `fedora44`, `Containerfile.fedora44`, and `conary-fedora44`. Keep `remi_distro = "fedora"` unless a test is intentionally exercising the canonical Remi distro value.

- [ ] **Step 7: Run conary-test focused unit checks**

Run:

```bash
cargo test -p conary-test config
cargo test -p conary-test engine::variables
cargo test -p conary-test engine::runner
cargo test -p conary-test server::service
cargo test -p conary-test server::handlers
cargo test -p conary-test server::mcp
cargo test -p conary-test container
```

Expected: all selected tests pass.

- [ ] **Step 8: Commit**

```bash
git add apps/conary/tests/integration/remi apps/conary-test
git commit -m "test(conary-test): migrate Fedora fixture to 44"
```

### Task 3: Move CI, Product Defaults, Packaging, And Living Docs To Fedora 44

**Files:**
- Modify: `.github/workflows/merge-validation.yml`
- Modify: `.github/workflows/scheduled-ops.yml`
- Modify: `apps/remi/src/server/test_db.rs`
- Modify: `data/distros.toml`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/distro.rs`
- Modify: `apps/conary/src/cli/distro.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/adopt/convert.rs` if it is judged current-baseline fixture data
- Modify: `crates/conary-core/src/canonical/sync.rs`
- Modify: `packaging/rpm/Containerfile.build`
- Modify: `packaging/rpm/build.sh`
- Modify: `deploy/FORGE.md`
- Modify: `README.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `site/src/routes/install/+page.svelte`

- [ ] **Step 1: Update GitHub Actions defaults and single-distro jobs**

Change:

```yaml
default: fedora43
SMOKE_DISTRO: ${{ github.event.inputs.smoke_distro || 'fedora43' }}
distro: [fedora43, ubuntu-noble, arch]
```

to:

```yaml
default: fedora44
SMOKE_DISTRO: ${{ github.event.inputs.smoke_distro || 'fedora44' }}
distro: [fedora44, ubuntu-noble, arch]
```

Also update flat command arguments such as:

```yaml
run: cargo run -p conary-test -- run --distro fedora44 --suite tests/integration/remi/manifests/phase3-group-n-qemu.toml
```

- [ ] **Step 2: Update the product distro catalog**

Add a Fedora 44 entry to `data/distros.toml`. Verify the release and support dates from official Fedora release/schedule sources at implementation time. Use the release date `2026-04-28`; if the EOL date is still forecasted, use the official Fedora schedule date and keep Fedora 43 only as a previous supported release:

```toml
[[distros]]
name = "fedora-44"
display = "Fedora 44"
format = "rpm"
release = "2026-04-28"
eol = "2027-06-02" # current Fedora 44 schedule value; verify before editing

[[distros]]
name = "fedora-43"
display = "Fedora 43"
format = "rpm"
release = "2025-10-28"
eol = "2026-12-09"
```

If the project decides not to retain Fedora 43, remove its catalog entry and record that choice in the commit body.

- [ ] **Step 3: Update Conary default Fedora repo examples**

In `apps/conary/src/commands/system.rs`, change the default repo tuple to:

```rust
(
    "fedora-44",
    "https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Everything/x86_64/os",
    90,
    "Fedora 44",
),
```

In `apps/conary/src/commands/distro.rs`, list `fedora-44        Fedora 44`.

In `apps/conary/src/cli/distro.rs`, update help examples to use `fedora-44`.

In `apps/conary/src/commands/install/mod.rs`, update the `distro_name_to_flavor_known` test from `fedora43` to `fedora44` if that test is current-baseline fixture data.

- [ ] **Step 4: Update Remi fixture data and RPM packaging baseline**

In `apps/remi/src/server/test_db.rs`, update active test-run fixture data from
`fedora43` to `fedora44`. These are Remi server test database rows, not
historical validation records.

In `packaging/rpm/Containerfile.build`:

```dockerfile
# Fedora 44 build container for Conary RPMs.
FROM registry.fedoraproject.org/fedora:44
```

In `packaging/rpm/build.sh`, update comments that mention Fedora 43.

- [ ] **Step 5: Update active integration manifests**

Run:

```bash
rg -n "fedora43|fedora-43|Fedora 43" apps/conary/tests/integration/remi/manifests
```

Update active distro override keys and current-baseline repo cleanup commands. Examples:

```toml
[distro_overrides.fedora44]
...
replatform_target = "fedora44"
```

and:

```toml
run = "... ${CONARY_BIN} repo remove fedora-44 --db-path ${DB_PATH} ..."
```

- [ ] **Step 6: Update living docs and site source**

Update active user-facing examples to default to Fedora 44:

```toml
allowed_distros = ["fedora-44", "arch"]
distro = "fedora-44"
```

```bash
cargo run -p conary-test -- run --suite phase1-core --distro fedora44 --phase 1
```

In `docs/conaryopedia-v2.md`, use `fedora-44` as the default Fedora example. If a section intentionally discusses multi-version repository behavior, label Fedora 43 as a previous release instead of leaving it as the apparent default.

In `site/src/routes/install/+page.svelte`, decide the product copy:

```svelte
<p class="distro-note">Fedora 44+ / RPM-based</p>
```

Do not edit `site/build/` or `site/.svelte-kit/` directly.

- [ ] **Step 7: Run stale-reference sweep**

Run:

```bash
rg -n "fedora43|fedora-43|Fedora 43|conary-fedora43" docs apps crates packaging deploy .github site data --glob '!target' --glob '!docs/**/archive/**' --glob '!site/build/**' --glob '!site/.svelte-kit/**'
```

Expected: remaining matches are either:

- archived or historical dated validation notes
- semantic test data in the skip-list
- retained previous-release `data/distros.toml` entry
- explicitly labeled previous-release docs examples

Record any remaining matches in the implementation notes or commit body with a
per-file reason. The current design/plan files may keep Fedora 43 where they
describe the historical pre-migration baseline, but those matches must be
intentional and easy to distinguish from active instructions.

- [ ] **Step 8: Run focused checks**

Run:

```bash
cargo fmt --check
cargo test -p conary-test config
cargo test -p conary-test engine::variables
cargo test -p conary install::tests::distro_name_to_flavor_known
cargo test -p remi server::test_db
cargo run -p conary-test -- list
```

Expected: all commands pass.

- [ ] **Step 9: Commit**

```bash
git add .github data apps/conary apps/conary-test apps/conary/tests/integration/remi apps/remi packaging/rpm deploy README.md docs site/src
git commit -m "chore(test): switch active Fedora baseline to 44"
```

### Task 4: Preflight The Fedora 44 Fixture

**Files:**
- No source edits unless preflight exposes a Fedora 44 package/base-image issue

- [ ] **Step 1: Build the Fedora 44 integration image**

Run:

```bash
cargo run -p conary-test -- images build --distro fedora44
```

Expected: Fedora 44 image builds, `dnf install` succeeds for `ca-certificates curl python3 sqlite`, and the image uses `DISTRO=fedora44`.

- [ ] **Step 2: Run a small Fedora 44 smoke**

Run:

```bash
cargo run -p conary-test -- run --suite phase1-core --distro fedora44 --phase 1
```

Expected: Phase 1 core smoke passes. If it fails because of a Fedora 44 base-image/package-set issue, capture the failure and decide whether temporary dual fixture support is justified. If it fails because Conary behavior changed under Fedora 44, fix Conary on Fedora 44.

- [ ] **Step 3: Commit any preflight fix**

If no edits were needed, skip this step. If edits were needed:

```bash
git add <changed-files>
git commit -m "fix(test): stabilize Fedora 44 fixture"
```

---

## Chunk 2: Runtime Generation Input Classification And Validation

### Task 5: Add A Focused Runtime Input Classifier Module

**Files:**
- Create: `crates/conary-core/src/generation/builder/runtime_inputs.rs`
- Modify: `crates/conary-core/src/generation/builder.rs`
- Test: `crates/conary-core/src/generation/builder/runtime_inputs.rs`

- [ ] **Step 1: Create and wire a module with a failing eligibility test**

In `crates/conary-core/src/generation/builder.rs`, wire the module before
running the red test:

```rust
mod erofs;
mod runtime_inputs;
```

Create `crates/conary-core/src/generation/builder/runtime_inputs.rs` with the
repo-required path comment, compile-critical imports, and tests that reference
the still-missing helper:

```rust
// conary-core/src/generation/builder/runtime_inputs.rs

use crate::db::models::InstallSource;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_input_source_classification_is_not_is_conary_owned() {
        assert!(!is_generation_input_source(InstallSource::AdoptedTrack));
        assert!(is_generation_input_source(InstallSource::AdoptedFull));
        assert!(is_generation_input_source(InstallSource::Taken));
        assert!(is_generation_input_source(InstallSource::Repository));
        assert!(is_generation_input_source(InstallSource::File));
    }
}
```

- [ ] **Step 2: Run test and verify red**

Run:

```bash
cargo test -p conary-core generation::builder::runtime_inputs::tests::generation_input_source_classification_is_not_is_conary_owned
```

Expected: compile failure because `is_generation_input_source` does not exist
yet. If the command reports zero tests, the module was not wired correctly.

- [ ] **Step 3: Add the minimal helper**

In `runtime_inputs.rs`:

```rust
pub(super) fn is_generation_input_source(source: InstallSource) -> bool {
    matches!(
        source,
        InstallSource::AdoptedFull
            | InstallSource::Taken
            | InstallSource::Repository
            | InstallSource::File
    )
}
```

- [ ] **Step 4: Run test and verify green**

Run the same `cargo test -p conary-core ...generation_input_source...` command.

Expected: pass.

### Task 6: Add File-Type, Digest-Shape, Symlink, Directory, And Special-File Validation

**Files:**
- Modify: `crates/conary-core/src/generation/builder/runtime_inputs.rs`
- Modify if useful: `crates/conary-core/src/db/models/file_entry.rs`
- Test: `crates/conary-core/src/generation/builder/runtime_inputs.rs`

- [ ] **Step 1: Add failing classification tests**

Add tests for:

- non-empty `symlink_target` wins over mode bits
- symlink mode with no target fails
- directory mode bypasses digest validation and is excluded from EROFS input
- bare permission-only mode such as `0o755` is treated as regular
- FIFO/device/socket mode fails when included
- regular file invalid digest fails through `hex_to_digest`
- symlink hash must match `CasStore::compute_symlink_hash(target)`
- non-excluded special files under `/etc`, `/usr`, or `/boot` fail clearly
- missing symlink targets and non-excluded special files are ultimately reported
  with package name, path, and remediation text, not as raw path-only errors

Use helper constructors in the test module:

```rust
fn file_entry(path: &str, hash: &str, mode: i32, trove_id: i64) -> FileEntry {
    let mut entry = FileEntry::new(path.to_string(), hash.to_string(), 0, mode, trove_id);
    entry.owner = Some("root".to_string());
    entry.group_name = Some("root".to_string());
    entry
}

fn symlink_entry(path: &str, target: &str, hash: &str, mode: i32, trove_id: i64) -> FileEntry {
    let mut entry = file_entry(path, hash, mode, trove_id);
    entry.symlink_target = Some(target.to_string());
    entry
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-core generation::builder::runtime_inputs
```

Expected: new tests fail until classification exists.

- [ ] **Step 3: Implement validation types**

Add the path comment/imports if they are not already present, then add focused
structs/enums:

```rust
// conary-core/src/generation/builder/runtime_inputs.rs

use crate::db::models::{FileEntry, InstallSource, Trove};
use crate::filesystem::CasStore;
use crate::generation::metadata::is_excluded;
use super::{FileEntryRef, SymlinkEntryRef, hex_to_digest};

pub(super) struct RuntimeGenerationInputs {
    pub file_refs: Vec<FileEntryRef>,
    pub symlink_refs: Vec<SymlinkEntryRef>,
    pub adopted_track_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeEntryKind {
    Regular,
    Symlink { target: String },
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeEntryProblem {
    MissingSymlinkTarget,
    UnsupportedFileType(i32),
}
```

Use constants:

```rust
const S_IFMT: i32 = 0o170000;
const S_IFREG: i32 = 0o100000;
const S_IFDIR: i32 = 0o040000;
const S_IFLNK: i32 = 0o120000;
```

Classification rule:

```rust
fn classify_file_entry(file: &FileEntry) -> Result<RuntimeEntryKind, RuntimeEntryProblem> {
    if let Some(target) = file.symlink_target.as_deref().filter(|target| !target.is_empty()) {
        return Ok(RuntimeEntryKind::Symlink {
            target: target.to_string(),
        });
    }

    match file.permissions & S_IFMT {
        S_IFLNK => Err(RuntimeEntryProblem::MissingSymlinkTarget),
        S_IFDIR => Ok(RuntimeEntryKind::Directory),
        S_IFREG | 0 => Ok(RuntimeEntryKind::Regular),
        other => Err(RuntimeEntryProblem::UnsupportedFileType(other)),
    }
}
```

- [ ] **Step 4: Implement pre-build digest and symlink validation**

Use the shared parser, but wrap errors so user-facing messages include package
name, path, and remediation:

```rust
let kind = classify_file_entry(file).map_err(|problem| {
    let detail = match problem {
        RuntimeEntryProblem::MissingSymlinkTarget => {
            "symlink entry is missing symlink_target".to_string()
        }
        RuntimeEntryProblem::UnsupportedFileType(mode) => {
            format!("unsupported special file mode {mode:o} for generation root")
        }
    };
    runtime_input_error(package_name, &file.path, detail)
})?;

hex_to_digest(&file.sha256_hash).map_err(|error| {
    runtime_input_error(
        package_name,
        &file.path,
        format!("invalid SHA-256 digest for regular file: {error}"),
    )
})?;
```

For symlinks:

```rust
let expected = CasStore::compute_symlink_hash(&target);
if file.sha256_hash != expected {
    return Err(runtime_input_error(
        package_name,
        &file.path,
        format!("symlink hash mismatch: expected {expected}, got {}", file.sha256_hash),
    ));
}
```

The helper should produce text in this shape:

```rust
fn runtime_input_error(package_name: &str, path: &str, detail: impl std::fmt::Display) -> crate::Error {
    crate::Error::InvalidPath(format!(
        "exportable runtime generation is not self-contained: package {package_name} has unresolved CAS-backed path {path}: {detail}. Run conary system adopt --system --full for bulk adoption, conary system adopt <pkg> --full for a single package, or conary system takeover --up-to cas before building this generation."
    ))
}
```

Directories return no `FileEntryRef` or `SymlinkEntryRef`.

- [ ] **Step 5: Run tests and verify green**

Run:

```bash
cargo test -p conary-core generation::builder::runtime_inputs
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/generation/builder.rs crates/conary-core/src/generation/builder/runtime_inputs.rs crates/conary-core/src/db/models/file_entry.rs
git commit -m "feat(generation): classify runtime generation inputs"
```

### Task 7: Share The Validator Across New Builds And Rebuilds

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Modify: `crates/conary-core/src/generation/builder/runtime_inputs.rs`
- Test: `crates/conary-core/src/generation/builder.rs`

- [ ] **Step 1: Add failing parity tests**

In `crates/conary-core/src/generation/builder.rs` tests, add cases that both `build_generation_from_db_with_boot_root` and `rebuild_generation_image_with_boot_root` reject the same invalid CAS-backed regular file digest.

Expected failure text should contain the package/file path and avoid publishing `.conary-artifact.json`.

Also add collection-level cases proving excluded paths are applied before
classification/validation:

- excluded paths bypass type, digest-shape, and symlink-hash validation
- excluded special files do not fail generation input validation
- non-excluded special files under `/etc`, `/usr`, or `/boot` still fail with
  package name, path, and remediation text

- [ ] **Step 2: Run the targeted tests and verify red**

Run:

```bash
cargo test -p conary-core generation::builder::tests::build_generation_from_db_rejects_invalid_runtime_input
cargo test -p conary-core generation::builder::tests::rebuild_generation_image_rejects_invalid_runtime_input
```

Expected: fail until both paths share the classifier.

- [ ] **Step 3: Implement DB collection helper**

In `runtime_inputs.rs`, add:

```rust
pub(super) fn collect_runtime_generation_inputs(
    troves: &[Trove],
    files: Vec<FileEntry>,
) -> crate::Result<RuntimeGenerationInputs> {
    ...
}
```

Build a map of `trove_id -> (name, install_source)`. For each file:

- reject file entries whose `trove_id` is not present in the trove map before
  source filtering; orphaned file entries are a data-integrity failure, so
  return a hard generation-build error naming the `trove_id` and file path
  rather than silently skipping them
- skip `AdoptedTrack`
- include only `AdoptedFull`, `Taken`, `Repository`, `File`
- apply `is_excluded(&file.path)` immediately after install-source eligibility
  and before file-type, digest-shape, symlink-target, or symlink-hash validation
- classify and validate by file type only for non-excluded included paths
- collect package/path samples for concise errors

- [ ] **Step 4: Replace duplicated collection in both build paths**

In `build_generation_from_db_with_boot_root` and `rebuild_generation_image_with_boot_root`, replace the adopted-track filtering and `FileEntryRef` mapping with:

```rust
let troves = Trove::list_all(conn)?;
let all_files = FileEntry::find_all_ordered(conn)?;
let runtime_inputs = runtime_inputs::collect_runtime_generation_inputs(&troves, all_files)?;
validate_runtime_generation_root_is_self_contained(
    &runtime_inputs.file_refs,
    &runtime_inputs.symlink_refs,
)?;
let result = build_erofs_image(
    &runtime_inputs.file_refs,
    &runtime_inputs.symlink_refs,
    &gen_dir,
)?;
...
cas_objects: cas_objects_from_file_refs(&runtime_inputs.file_refs),
```

Remove or retire `collect_symlink_refs`; a missing `symlink_target` schema must be a hard generation-build error through normal `FileEntry::find_all_ordered` failure, not a silent empty symlink set.

- [ ] **Step 5: Run parity tests and existing generation tests**

Run:

```bash
cargo test -p conary-core generation::builder
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/generation/builder.rs crates/conary-core/src/generation/builder/runtime_inputs.rs
git commit -m "feat(generation): share runtime input validation"
```

### Task 8: Preserve The Single CAS Integrity Gate And Add Negative Coverage

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Possibly modify: `crates/conary-core/src/generation/artifact.rs`

- [ ] **Step 1: Add missing-CAS-object build and rebuild tests**

In `builder.rs` tests, create a valid-looking regular file entry whose SHA-256
has no object under the test `objects` directory. Include an executable
`/usr/sbin/init` with a valid object so pre-build validation and init closure
pass. Cover both `build_generation_from_db_with_boot_root` and
`rebuild_generation_image_with_boot_root`, either with two tests or a small
test helper parameterized by build path.

Expected: EROFS build can proceed, but `write_generation_artifact` fails before
manifest publication with a missing CAS object error in both build paths.

- [ ] **Step 2: Run the test and verify red or current failure shape**

Run:

```bash
cargo test -p conary-core generation::builder::tests::build_generation_from_db_rejects_missing_regular_file_cas_object
cargo test -p conary-core generation::builder::tests::rebuild_generation_image_rejects_missing_regular_file_cas_object
```

Expected: fail until error reporting/cleanup matches the assertion.

- [ ] **Step 3: Adjust assertions or error wrapping without moving re-hash validation**

Do not add a second CAS re-hash pass in `runtime_inputs.rs`. Keep `write_generation_artifact` as the object existence/size/hash gate. Only improve propagation or cleanup if needed.

- [ ] **Step 4: Verify generation artifact tests**

Run:

```bash
cargo test -p conary-core generation::artifact generation::builder
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/generation/builder.rs crates/conary-core/src/generation/artifact.rs
git commit -m "test(generation): cover missing runtime CAS object"
```

---

## Chunk 3: Adoption And Takeover Bridge Hardening

### Task 9: Make All Full Adoption CAS Identity Fallible

**Files:**
- Modify: `apps/conary/src/commands/adopt/system.rs`
- Modify: `apps/conary/src/commands/adopt/packages.rs`
- Modify: `apps/conary/src/commands/adopt/refresh.rs`
- Modify: `apps/conary/src/commands/adopt/mod.rs`
- Create or modify: `apps/conary/src/commands/adopt/cas_capture.rs`
- Test: `apps/conary/src/commands/adopt/system.rs`
- Test: `apps/conary/src/commands/adopt/packages.rs`
- Test: `apps/conary/src/commands/adopt/refresh.rs`

- [ ] **Step 1: Add tests for full adoption identity and package preparation**

Add unit tests for a shared helper that can be called without a live package
manager:

- regular file with CAS available stores/returns a real CAS hash
- symlink with target returns `CasStore::compute_symlink_hash`
- directory does not require CAS content
- excluded paths under `EXCLUDED_DIRS` do not block full adoption if they are
  unreadable or special
- non-excluded special files under `/etc`, `/usr`, or `/boot` fail clearly
- missing/unreadable regular file in full mode returns an error instead of a placeholder
- track mode may still use package-manager digest/placeholder behavior
- a package with one required included regular-file CAS failure is skipped and
  reports package name plus path instead of being inserted as `AdoptedFull`

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary --bin conary adopt::cas_capture::tests::full_adoption_regular_file_requires_cas_storage
cargo test -p conary --bin conary adopt::cas_capture::tests::full_adoption_missing_regular_file_fails_package_preparation
```

Expected: fail until the helper exists.

- [ ] **Step 3: Add a shared fallible CAS capture helper**

Keep `compute_file_hash` for track-mode compatibility if needed, but add a
shared fallible full-mode helper in `apps/conary/src/commands/adopt/`. Export
it from `adopt/mod.rs` as `pub(crate)` so `system.rs`, `packages.rs`,
`refresh.rs`, and `generation/takeover.rs` can all use the same behavior:

```rust
pub(crate) mod cas_capture;
```

```rust
pub(crate) fn compute_cas_backed_file_hash(
    file_path: &str,
    file_mode: i32,
    file_digest: Option<&str>,
    link_target: Option<&str>,
    cas: &conary_core::filesystem::CasStore,
) -> Result<String> {
    ...
}
```

For full mode:

- symlink requires target from PM metadata or readable filesystem symlink
- directory returns `file_digest` if provided, or a stable placeholder string
  that generation classification will skip as a directory
- regular file must be a readable regular file and must successfully `hardlink_from_existing`
- paths excluded by `conary_core::generation::metadata::is_excluded` do not
  block full adoption; keep their package-manager digest if available or use a
  stable placeholder because generation export will exclude them before CAS
  validation
- unsupported non-excluded special files should error with package/path context

Add a package preparation helper that accepts a package name and a
`Vec<FileInfoTuple>` and returns either CAS-backed file data or an error naming
the package and first failing path. This keeps tests independent from the live
RPM/dpkg/pacman query functions. `FileInfoTuple` is already public and
re-exported from `adopt/mod.rs`, so the helper can accept `&[FileInfoTuple]`
without additional visibility changes.

- [ ] **Step 4: Use the helper in every `AdoptedFull` producer**

Apply the shared helper to:

- `conary system adopt --system --full` in `system.rs`
- `conary system adopt <pkg> --full` in `packages.rs`
- refresh of existing `AdoptedFull` packages in `refresh.rs`

When `full == true`, if any included regular file cannot be stored in CAS, do
not insert or refresh that package as `AdoptedFull`. Increment/report the
package error count and continue the existing best-effort bulk adoption flow
for other packages.

- [ ] **Step 5: Run adoption tests**

Run:

```bash
cargo test -p conary --bin conary adopt::system
cargo test -p conary --bin conary adopt::packages
cargo test -p conary --bin conary adopt::refresh
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/adopt
git commit -m "fix(adopt): require CAS storage for full adoption"
```

### Task 10: Tighten Takeover CAS Upgrade And Ownership Transfer

**Files:**
- Modify: `apps/conary/src/commands/generation/takeover.rs`
- Test: `apps/conary/src/commands/generation/takeover.rs`

- [ ] **Step 1: Add tests for package promotion safety**

Add tests or helper-level coverage proving:

- `upgrade_to_cas_backed` does not mark a package `AdoptedFull` if a required regular file cannot be CAS-backed
- `take_ownership` does not mark a package `Taken` if CAS capture fails for required file content
- symlink hashes written during CAS upgrade match `CasStore::compute_symlink_hash`
- takeover CAS capture can be tested from fake `FileInfoTuple` values without
  invoking the host package manager

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary --bin conary generation::takeover
```

Expected: fail until takeover paths distinguish CAS failures from successful capture.

- [ ] **Step 3: Reuse the shared CAS capture helper**

Avoid duplicating subtle symlink/regular-file behavior. Use the shared helper
from `apps/conary/src/commands/adopt/` and import it explicitly through
`adopt/mod.rs`.

Extract the CAS-capture portion of `upgrade_to_cas_backed` and
`take_ownership` so tests can feed fake file tuples and assert that a CAS
failure prevents the later DB update to `AdoptedFull` or `Taken`. Update both
paths so a package is promoted only after required CAS writes succeed.

- [ ] **Step 4: Run takeover tests**

Run:

```bash
cargo test -p conary --bin conary generation::takeover
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/generation/takeover.rs apps/conary/src/commands/adopt
git commit -m "fix(takeover): promote packages only after CAS capture"
```

---

## Chunk 4: Installed Runtime QEMU Validation

### Task 11: Add Integration Negative Case For Missing Or Corrupt Runtime CAS

**Files:**
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`

- [ ] **Step 1: Add a negative test after the fail-closed metadata-only case**

Keep the existing `TGE02 bootstrap_run_generation_export_boots` ID unchanged.
Add the new negative case as `TGE03
installed_generation_build_rejects_missing_runtime_cas_object`. The guest flow
should:

- initialize Conary
- add/sync Remi and install `sqlite` if `sqlite3` is not already available
- perform full system adoption or takeover to CAS-backed state
- select one included regular file hash from the DB
- remove or corrupt the corresponding object under `/var/lib/conary/objects/<prefix>/<suffix>`
- run generation build
- assert the command fails before `.conary-artifact.json` publication with a missing/corrupt CAS object error

Each `qemu_boot.commands` entry runs in a separate `sh -lc`, so keep variables
inside one command or write them to temp files. Command sketch:

```toml
[[test]]
id = "TGE03"
name = "installed_generation_build_rejects_missing_runtime_cas_object"
description = "A CAS-backed installed runtime generation fails before artifact publication if an included regular-file CAS object disappears"
timeout = 1800
group = "generation-export"

[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
memory_mb = 2048
timeout_seconds = 1500
ssh_port = 2244
stage_conary = true
commands = [
    "conary system init",
    "conary repo remove remi || true",
    "conary repo add remi ${REMI_ENDPOINT} --default-strategy remi --remi-endpoint ${REMI_ENDPOINT} --remi-distro ${REMI_DISTRO} --no-gpg-check",
    "conary repo sync remi --force",
    "command -v sqlite3 || conary install sqlite --repo remi --yes --sandbox never --allow-live-system-mutation",
    "conary system adopt --system --full",
    "DB=/var/lib/conary/conary.db; HASH=$(sqlite3 \"$DB\" \"select f.sha256_hash from files f join troves t on t.id = f.trove_id where t.install_source in ('adopted-full','taken','repository','file') and f.symlink_target is null and length(f.sha256_hash) = 64 and f.sha256_hash not glob '*[^0-9a-f]*' and ((f.permissions & 61440) = 0 or (f.permissions & 61440) = 32768) and f.path not like '/var/%' and f.path not like '/tmp/%' and f.path not like '/run/%' and f.path not like '/home/%' and f.path not like '/root/%' and f.path not like '/srv/%' and f.path not like '/opt/%' and f.path not like '/proc/%' and f.path not like '/sys/%' and f.path not like '/dev/%' and f.path not like '/mnt/%' and f.path not like '/media/%' limit 1\"); test -n \"$HASH\"; PREFIX=$(printf '%s' \"$HASH\" | cut -c1-2); SUFFIX=$(printf '%s' \"$HASH\" | cut -c3-); OBJ=/var/lib/conary/objects/$PREFIX/$SUFFIX; test -f \"$OBJ\"; rm -f \"$OBJ\"; echo \"$HASH\" > /var/tmp/missing-cas.hash",
    "conary system generation build --allow-live-system-mutation > /var/tmp/missing-cas.log 2>&1; code=$?; cat /var/tmp/missing-cas.log; test \"$code\" -ne 0",
    "grep -Eq 'missing CAS object|Checksum mismatch|size mismatch' /var/tmp/missing-cas.log",
    "test ! -e /conary/generations/0/.conary-artifact.json",
    "echo installed-runtime-missing-cas-rejected",
]
expect_output = [
    "installed-runtime-missing-cas-rejected",
]

[test.step.assert]
exit_code = 0
```

Adjust the exact DB path and object path only if the fixture changes the
default `/var/lib/conary/conary.db` layout. Do not use Bash-only substring
syntax such as `${HASH:0:2}`; QEMU commands run through `sh -lc`.
The SQL bitwise permissions filter assumes positive Unix mode values stored in
the signed `files.permissions` column, which is the expected RPM/Fedora shape.

- [ ] **Step 2: Run manifest parser tests**

Run:

```bash
cargo test -p conary-test config::manifest engine::variables
cargo run -p conary-test -- list
cargo run -p conary-test -- list | rg 'TGE03'
```

Expected: pass and list the new negative test.

- [ ] **Step 3: Commit**

```bash
git add apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml
git commit -m "test(generation): reject installed export with missing CAS"
```

### Task 12: Add Positive Installed Runtime Export And Boot Case

**Files:**
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`

- [ ] **Step 1: Add the positive installed runtime generation export test**

Add a new `TGE04 installed_runtime_generation_export_boots` test after the
negative case. The guest flow should:

- boot `minimal-boot-v2`
- initialize Conary
- ensure export tooling exists (`dosfstools`, `qemu-img`, `erofs-utils`)
- run `conary system adopt --system --full` or `conary system takeover --up-to cas`
- build a generation
- confirm artifact manifests exist
- export qcow2 to the scratch disk
- copy qcow2 to the host

Command sketch:

```toml
[[test]]
id = "TGE04"
name = "installed_runtime_generation_export_boots"
description = "A full CAS-backed installed runtime generation exports to qcow2 and boots under UEFI"
timeout = 7200
group = "generation-export"

[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
memory_mb = 2048
timeout_seconds = 5400
ssh_port = 2244
stage_conary = true
scratch_disk_mb = 65536
commands = [
    "conary system init",
    "conary repo remove remi || true",
    "conary repo add remi ${REMI_ENDPOINT} --default-strategy remi --remi-endpoint ${REMI_ENDPOINT} --remi-distro ${REMI_DISTRO} --no-gpg-check",
    "conary repo sync remi --force",
    "conary install dosfstools --repo remi --yes --sandbox never --allow-live-system-mutation",
    "conary install qemu-img --repo remi --yes --sandbox never --allow-live-system-mutation",
    "conary install erofs-utils --repo remi --yes --sandbox never --allow-live-system-mutation",
    "conary system adopt --system --full",
    "for i in $(seq 1 20); do test -b /dev/disk/by-id/virtio-conary-scratch && exit 0; sleep 1; done; ls -l /dev/disk/by-id /dev/vd*; false",
    "mkfs.ext4 -F /dev/disk/by-id/virtio-conary-scratch",
    "mkdir -p /mnt/conary-scratch",
    "mount -o noatime /dev/disk/by-id/virtio-conary-scratch /mnt/conary-scratch",
    "mkdir -p /mnt/conary-scratch/export",
    "conary system generation build --allow-live-system-mutation > /var/tmp/installed-runtime-build.log 2>&1; cat /var/tmp/installed-runtime-build.log; GEN=$(sed -n 's/.*Generation \\([0-9][0-9]*\\).*/\\1/p' /var/tmp/installed-runtime-build.log | tail -1); test -n \"$GEN\"; echo \"$GEN\" > /mnt/conary-scratch/export/installed-runtime-generation.txt; test -f /conary/generations/$GEN/.conary-artifact.json; test -f /conary/generations/$GEN/cas-manifest.json; test -f /conary/generations/$GEN/boot-assets/manifest.json; conary system generation export --path /conary/generations/$GEN --format qcow2 --output /mnt/conary-scratch/export/installed-runtime-generation.qcow2",
    "test -s /mnt/conary-scratch/export/installed-runtime-generation.qcow2",
    "echo installed-runtime-generation-export-ok",
]
copy_from_guest = [
    { source = "/mnt/conary-scratch/export/installed-runtime-generation.qcow2", dest = "/tmp/conary-generation-export/installed-runtime-generation.qcow2" },
    { source = "/mnt/conary-scratch/export/installed-runtime-generation.txt", dest = "/tmp/conary-generation-export/installed-runtime-generation.txt" },
]
expect_output = [
    "installed-runtime-generation-export-ok",
]

[test.step.assert]
exit_code = 0
```

Use the existing TGE02 scratch-disk setup and `copy_from_guest` pattern if the
exact device path changes.

- [ ] **Step 2: Add the boot step for the exported image**

Use `local_image_path`:

```toml
[[test.step]]
[test.step.qemu_boot]
image = "local-installed-runtime-generation-export"
local_image_path = "/tmp/conary-generation-export/installed-runtime-generation.qcow2"
memory_mb = 2048
timeout_seconds = 420
ssh_port = 2245
copy_to_guest = [
    { source = "/tmp/conary-generation-export/installed-runtime-generation.txt", dest = "/tmp/expected-generation" },
]
commands = [
    "EXPECTED=$(cat /tmp/expected-generation); grep -q \"conary.generation=$EXPECTED\" /proc/cmdline",
    "EXPECTED=$(cat /tmp/expected-generation); test -f /conary/generations/$EXPECTED/.conary-artifact.json",
    "EXPECTED=$(cat /tmp/expected-generation); test -f /conary/generations/$EXPECTED/cas-manifest.json",
    "EXPECTED=$(cat /tmp/expected-generation); test -f /conary/generations/$EXPECTED/boot-assets/manifest.json",
    "EXPECTED=$(cat /tmp/expected-generation); TARGET=$(readlink /conary/current); test \"$TARGET\" = \"generations/$EXPECTED\" || test \"$TARGET\" = \"$EXPECTED\" || test \"$(readlink -f /conary/current)\" = \"/conary/generations/$EXPECTED\"",
    "echo installed-runtime-generation-export-booted",
]
expect_output = [
    "installed-runtime-generation-export-booted",
]

[test.step.assert]
exit_code = 0
```

Do not accept an arbitrary generation `0` or `1`; verify the exact generation
captured from the build/export step. The `readlink -f` branch assumes the
Fedora-based `minimal-boot-v2` image provides GNU coreutils; if the base image
changes, verify that availability or replace it with a POSIX-compatible check.

- [ ] **Step 3: Run manifest parser tests**

Run:

```bash
cargo test -p conary-test config::manifest engine::variables engine::qemu
cargo run -p conary-test -- list
cargo run -p conary-test -- list | rg 'TGE03'
cargo run -p conary-test -- list | rg 'TGE04'
```

Expected: pass and list the positive installed runtime export tests.

- [ ] **Step 4: Commit**

```bash
git add apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml
git commit -m "test(generation): boot installed runtime export"
```

### Task 13: Run Full Verification And Fix Regressions

**Files:**
- Modify only files required by failures discovered in this task

- [ ] **Step 1: Run formatting and focused unit checks**

Run:

```bash
cargo fmt --check
cargo test -p conary-core generation
cargo test -p conary
cargo test -p conary-test config::manifest engine::variables engine::qemu
cargo test -p remi server::test_db
cargo run -p conary-test -- list
```

Expected: all pass.

- [ ] **Step 2: Run clippy**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 3: Run stale Fedora reference sweep**

Run:

```bash
rg -n "fedora43|fedora-43|Fedora 43|conary-fedora43" docs apps crates packaging deploy .github site data --glob '!target' --glob '!docs/**/archive/**' --glob '!site/build/**' --glob '!site/.svelte-kit/**'
```

Expected: remaining matches are justified historical context, semantic fixture data, retained previous-release catalog entries, or explicitly labeled previous-release docs examples.

- [ ] **Step 4: Run Fedora 44 generation export QEMU suite**

Run:

```bash
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3
```

Expected: fail-closed metadata-only case passes, negative CAS case passes,
bootstrap-run export boot passes, and installed-runtime export boot passes.
Inspect the run output/logs and record that the QEMU tests actually executed
instead of being reported as informational skips for missing host tooling.

- [ ] **Step 5: Commit any verification fixes**

If verification required changes:

```bash
git add <changed-files>
git commit -m "fix(generation): stabilize installed runtime export validation"
```

### Task 14: Final Branch Review And Handoff

**Files:**
- No source edits unless final review finds an issue

- [ ] **Step 1: Review commit stack**

Run:

```bash
git log --oneline origin/main..HEAD
git diff --stat origin/main..HEAD
```

Expected: commits are scoped to Fedora 44 migration, runtime input validation, adoption/takeover bridge hardening, and QEMU validation.

- [ ] **Step 2: Run final status**

Run:

```bash
git status --short --branch
```

Expected: clean worktree.

- [ ] **Step 3: Prepare PR summary**

Include:

- problem statement
- Fedora 44 migration summary
- runtime input validation summary
- adoption/takeover bridge summary
- QEMU validation result
- verification commands and outcomes

---

## Final Verification Checklist

Before claiming the slice is complete, run and record:

```bash
cargo fmt --check
cargo test -p conary-core generation
cargo test -p conary
cargo test -p conary-test config::manifest engine::variables engine::qemu
cargo test -p remi server::test_db
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- images build --distro fedora44
cargo run -p conary-test -- list
cargo run -p conary-test -- list | rg 'TGE03'
cargo run -p conary-test -- list | rg 'TGE04'
rg -n "fedora43|fedora-43|Fedora 43|conary-fedora43" docs apps crates packaging deploy .github site data --glob '!target' --glob '!docs/**/archive/**' --glob '!site/build/**' --glob '!site/.svelte-kit/**'
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3
```

The stale-reference command may return intentional matches, but each remaining
active-tree match must be justified as historical context, semantic fixture
data, retained previous-release catalog data, or an explicitly labeled
previous-release example.
The QEMU command must be recorded as non-skipped; an exit-0 skip caused by
missing host QEMU tooling is not acceptance evidence.
