# Composefs Atomic Modernization Implementation Plan

> **Historical status:** Completed and archived on 2026-05-14 after composefs
> atomic switching was implemented, validated, merged to `main`, and pushed as
> `db938294`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make composefs atomic generations the single supported runtime contract for package mutation, activation, recovery, export, and bootstrap paths before Conary's limited public release.

**Architecture:** Implement the approved umbrella design in independent phases. Start with the public mutation path by routing CCS/Remi/converted installs into the existing CAS/DB/generation apply lifecycle, then canonicalize runtime paths, remove boot/recovery/live-root fallbacks, unify export loading on `GenerationArtifact`, and update defaults/docs only after behavior is real. Each phase must land with a failing regression test first, then code, then focused verification.

**Tech Stack:** Rust workspace, rusqlite, Tokio command tests, Conary `TransactionEngine`, composefs/EROFS generation builder, dracut shell generator, `conary-test` integration harness, QEMU validation.

---

## Scope Check

The approved spec is repo-wide and covers several subsystems. Do not implement it as one patch. Execute this plan phase-by-phase, stopping after each task for review and verification. The first production code phase is CCS/install unification because it removes the largest public duplicate mutation path.

## File Structure

- Modify: `apps/conary/src/commands/install/mod.rs`
  - Owns the shared install transaction lifecycle and will expose a `pub(crate)` CCS install entrypoint that reuses `execute_install_transaction()`.
- Modify: `apps/conary/src/commands/install/conversion.rs`
  - Keeps dependency resolution for converted CCS packages, then calls the shared CCS transaction entrypoint instead of `cmd_ccs_install()`.
- Modify: `apps/conary/src/commands/ccs/install.rs`
  - Keeps CCS-specific verification, capability policy, component selection, dependency checks, and hooks; removes direct payload deployment.
- Create: `crates/conary-core/src/runtime_root.rs`
  - Defines canonical runtime-root paths for DB, CAS, generations, mount state, `/etc` state, GC roots, and current pointer.
- Modify: `crates/conary-core/src/lib.rs`
  - Exports `runtime_root`.
- Modify: `crates/conary-core/src/transaction/mod.rs`
  - Uses `ConaryRuntimeRoot` in `TransactionConfig::from_paths()`.
- Modify: `crates/conary-core/src/generation/metadata.rs`
  - Stops hard-coding installed generation paths outside the runtime-root abstraction.
- Modify: `apps/conary/src/commands/composefs_ops.rs`
  - Uses `ConaryRuntimeRoot` for generation/mount/current paths.
- Modify: `apps/conary/src/commands/generation/commands.rs`
  - Uses `ConaryRuntimeRoot` and removes mixed `/conary` plus `/var/lib/conary` DB/generation discovery behavior.
- Modify: `apps/conary/src/commands/generation/switch.rs`
  - Deletes public live switching or moves it behind a debug-only command that fails closed.
- Modify: `packaging/dracut/90conary/conary-generator.sh`
  - Removes legacy bind-mount fallback for generation directories without `root.erofs`.
- Modify: `crates/conary-core/src/bootstrap/system_config.rs`
  - Keeps embedded initramfs behavior aligned with dracut fail-closed activation.
- Modify: `crates/conary-core/src/transaction/recovery.rs`
  - Uses generation artifact/metadata validation and preserves verity requirements.
- Modify: `apps/conary/src/commands/remove.rs`
  - Removes release-facing no-generation live-root file removal.
- Modify: `apps/conary/src/commands/system.rs`
  - Removes release-facing no-generation live-root rollback restore/removal.
- Modify: `apps/conary/src/commands/export.rs`
  - Loads generation source through `GenerationArtifact` for OCI export.
- Modify: `crates/conary-core/src/generation/export.rs`
  - Adds the shared source loader or public helper needed by OCI.
- Modify: `crates/conary-core/src/model/parser.rs`
  - Changes preview-facing convergence default after behavior changes are complete.
- Modify: `apps/conary/src/commands/install/dep_mode.rs`
  - Keeps dependency defaults aligned with the new convergence default.
- Modify: `apps/conary/src/commands/generation/takeover.rs`
  - Keeps `generation` as the public path and hides or labels lower phases as debug/internal.
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`
  - Source-contract tests for removed activation and recovery fallbacks.
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`
  - Add runtime assertions when a removed fallback needs QEMU coverage.
- Modify: `docs/ARCHITECTURE.md`, `docs/modules/bootstrap.md`, `docs/operations/post-generation-export-follow-up-roadmap.md`, `docs/llms/README.md`
  - Update docs after code behavior is implemented and verified.

---

## Phase 1: CCS And Install Unification

### Task 1.1: Add A Regression Test Proving CCS Does Not Deploy Payloads Directly

**Files:**
- Modify: `apps/conary/src/commands/ccs/install.rs`

- [ ] **Step 1: Write the failing test**

Add this test to the existing `#[cfg(test)] mod tests` in `apps/conary/src/commands/ccs/install.rs`. It must fail on current `main` because `cmd_ccs_install()` writes `usr/bin/from-ccs` directly under the install root.

```rust
#[tokio::test]
async fn ccs_install_records_payload_without_direct_live_root_write() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("composefs-only.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();

    let content = b"from ccs".to_vec();
    let file_hash = hash::sha256(&content);
    let files = vec![FileEntry {
        path: "/usr/bin/from-ccs".to_string(),
        hash: file_hash.clone(),
        size: content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("composefs-only", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(file_hash.clone(), content.clone())]),
        total_size: content.len() as u64,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    super::cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(
        !install_root.join("usr/bin/from-ccs").exists(),
        "CCS install must not deploy package payloads directly into the live root"
    );

    let conn = conary_core::db::open(db_path_str).unwrap();
    let stored_path: String = conn
        .query_row("SELECT path FROM files WHERE path = '/usr/bin/from-ccs'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored_path, "/usr/bin/from-ccs");

    let current = std::fs::read_link(temp_dir.path().join("current"));
    assert!(
        current.is_ok(),
        "test-mode composefs apply must still publish an active generation pointer"
    );
}
```

On current `main`, the test must fail on the live-root payload assertion before it reaches the active-pointer assertion.

- [ ] **Step 2: Run the focused test and confirm RED**

Run:

```bash
cargo test -p conary ccs_install_records_payload_without_direct_live_root_write -- --nocapture
```

Expected: FAIL with the message `CCS install must not deploy package payloads directly into the live root`.

### Task 1.2: Add A Shared CCS Transaction Entrypoint

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/conversion.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`

- [ ] **Step 1: Add the shared options/result structs**

In `apps/conary/src/commands/install/mod.rs`, add these structs near `InstallOptions`:

```rust
pub(crate) struct CcsTransactionInstallOptions<'a> {
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub no_scripts: bool,
    pub sandbox_mode: SandboxMode,
    pub allow_downgrade: bool,
    pub selection_reason: Option<&'a str>,
    pub component_selection: ComponentSelection,
}

pub(crate) struct CcsTransactionInstallResult {
    pub changeset_id: i64,
}
```

- [ ] **Step 2: Add the shared install function**

In `apps/conary/src/commands/install/mod.rs`, add this function below `execute_install_transaction()`:

```rust
pub(crate) fn install_ccs_package_transactionally(
    conn: &mut rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    opts: CcsTransactionInstallOptions<'_>,
) -> Result<CcsTransactionInstallResult> {
    let progress = InstallProgress::new();
    let semantics = InstallSemantics::ccs();
    let upgrade = check_upgrade_status(conn, pkg, &semantics, opts.allow_downgrade)?;
    let old_trove = match &upgrade {
        UpgradeCheck::FreshInstall => None,
        UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => Some(trove.as_ref()),
    };

    let extraction = extract_and_classify_files(pkg, &opts.component_selection, &progress)?;

    if opts.dry_run {
        print_dry_run_summary(pkg, &opts.component_selection, &extraction);
        return Ok(CcsTransactionInstallResult { changeset_id: 0 });
    }

    let scriptlet_ctx = ScriptletContext {
        root: opts.root,
        no_scripts: opts.no_scripts,
        sandbox_mode: opts.sandbox_mode,
        semantics,
        old_trove,
    };
    let pre_state =
        run_pre_install_phase(conn, pkg, &extraction.installed_component_types, &scriptlet_ctx, &progress)?;

    let tx_ctx = TransactionContext {
        db_path: opts.db_path,
        root: opts.root,
        semantics,
        selection_reason: opts.selection_reason,
        old_trove_to_upgrade: old_trove,
    };
    let tx_result = execute_install_transaction(conn, pkg, &extraction, &tx_ctx, &progress)?;

    finalize_install_without_snapshot(conn, pkg, &extraction, &scriptlet_ctx, &pre_state, &tx_result, &progress)?;

    Ok(CcsTransactionInstallResult {
        changeset_id: tx_result.changeset_id,
    })
}
```

Make `print_dry_run_summary()`, `extract_and_classify_files()`, `run_pre_install_phase()`, `execute_install_transaction()`, and `finalize_install_without_snapshot()` `pub(crate)` if the new function cannot call them while they are private. Do not duplicate their logic.

- [ ] **Step 3: Convert selected CCS components to shared component selection**

In `apps/conary/src/commands/ccs/install.rs`, add a method near `SelectedCcsComponents`:

```rust
impl SelectedCcsComponents {
    fn to_install_component_selection(&self) -> crate::commands::install::ComponentSelection {
        if self.names.is_empty() {
            return crate::commands::install::ComponentSelection::Specific(Vec::new());
        }
        if self.recognized_types.len() == self.names.len() {
            crate::commands::install::ComponentSelection::Specific(self.recognized_types.clone())
        } else {
            crate::commands::install::ComponentSelection::All
        }
    }
}
```

This preserves current CCS behavior for custom component names by installing all extracted CCS files selected by `cmd_ccs_install()` until the shared classifier can preserve custom names natively.

- [ ] **Step 4: Replace direct deployment in `cmd_ccs_install()`**

In `apps/conary/src/commands/ccs/install.rs`, keep verification, dependency checks, component selection, capability policy, and hook execution preflight. Replace the block starting at `let mut engine = TransactionEngine::new(...)` through direct file deployment and DB insertion with:

```rust
let mut conn = open_db(db_path)?;
let install_result = crate::commands::install::install_ccs_package_transactionally(
    &mut conn,
    &ccs_pkg,
    crate::commands::install::CcsTransactionInstallOptions {
        db_path,
        root,
        dry_run,
        no_scripts: false,
        sandbox_mode: sandbox,
        allow_downgrade: false,
        selection_reason: Some("ccs-install"),
        component_selection: selected_components.to_install_component_selection(),
    },
)?;
let applied_changeset_id = install_result.changeset_id;
```

Retain the existing CCS hook execution after the shared transaction. If a hook failure occurs, keep using `mark_changeset_post_hooks_failed()` with `applied_changeset_id`.

- [ ] **Step 5: Update converted CCS install to use the shared entrypoint**

In `apps/conary/src/commands/install/conversion.rs`, replace the closing call to `super::super::ccs::cmd_ccs_install(...)` with:

```rust
let mut conn = open_db(db_path)?;
let ccs_pkg = CcsPackage::parse(ccs_path).context("Failed to parse converted CCS package")?;
super::install_ccs_package_transactionally(
    &mut conn,
    &ccs_pkg,
    super::CcsTransactionInstallOptions {
        db_path,
        root,
        dry_run,
        no_scripts,
        sandbox_mode,
        allow_downgrade,
        selection_reason: Some("converted-ccs"),
        component_selection: super::ComponentSelection::All,
    },
)?;
Ok(())
```

- [ ] **Step 6: Run focused tests and confirm GREEN**

Run:

```bash
cargo test -p conary ccs_install_records_payload_without_direct_live_root_write -- --nocapture
cargo test -p conary ccs_install_rejects_child_write_beneath_package_symlink -- --nocapture
cargo test -p conary ccs_install_allows_standard_usrmerge_root_symlink_ancestor -- --nocapture
```

Expected: all three tests pass. If usr-merge behavior moved from live-root deployment to generation build, update the usr-merge test to assert the DB path and generation artifact instead of live-root bytes.

- [ ] **Step 7: Commit Phase 1**

Run:

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/conversion.rs apps/conary/src/commands/ccs/install.rs
git commit -m "fix(install): route CCS through composefs transactions"
```

---

## Phase 2: Runtime Root Canonicalization

### Task 2.1: Introduce `ConaryRuntimeRoot`

**Files:**
- Create: `crates/conary-core/src/runtime_root.rs`
- Modify: `crates/conary-core/src/lib.rs`
- Modify: `crates/conary-core/src/transaction/mod.rs`
- Modify: `crates/conary-core/src/generation/metadata.rs`

- [ ] **Step 1: Write runtime-root unit tests**

Create `crates/conary-core/src/runtime_root.rs` with tests first:

```rust
// conary-core/src/runtime_root.rs

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConaryRuntimeRoot {
    root: PathBuf,
    db_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::ConaryRuntimeRoot;
    use std::path::Path;

    #[test]
    fn defaults_keep_boot_visible_generation_state_under_conary() {
        let root = ConaryRuntimeRoot::default();

        assert_eq!(root.root(), Path::new("/conary"));
        assert_eq!(root.db_path(), Path::new("/var/lib/conary/conary.db"));
        assert_eq!(root.objects_dir(), Path::new("/conary/objects"));
        assert_eq!(root.generations_dir(), Path::new("/conary/generations"));
        assert_eq!(root.current_link(), Path::new("/conary/current"));
        assert_eq!(root.mount_dir(), Path::new("/conary/mnt"));
        assert_eq!(root.etc_state_dir(), Path::new("/conary/etc-state"));
    }

    #[test]
    fn test_roots_can_use_temp_runtime_state_without_changing_db_name() {
        let root = ConaryRuntimeRoot::for_test_root("/tmp/conary-test");

        assert_eq!(root.root(), Path::new("/tmp/conary-test"));
        assert_eq!(root.db_path(), Path::new("/tmp/conary-test/conary.db"));
        assert_eq!(root.generation_path(7), Path::new("/tmp/conary-test/generations/7"));
    }
}
```

- [ ] **Step 2: Run tests and confirm RED**

Run:

```bash
cargo test -p conary-core runtime_root -- --nocapture
```

Expected: FAIL because the methods do not exist.

- [ ] **Step 3: Implement the runtime-root methods**

Replace the top of `runtime_root.rs` with:

```rust
// conary-core/src/runtime_root.rs

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConaryRuntimeRoot {
    root: PathBuf,
    db_path: PathBuf,
}

impl Default for ConaryRuntimeRoot {
    fn default() -> Self {
        Self {
            root: PathBuf::from("/conary"),
            db_path: PathBuf::from("/var/lib/conary/conary.db"),
        }
    }
}

impl ConaryRuntimeRoot {
    pub fn new(root: impl Into<PathBuf>, db_path: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            db_path: db_path.into(),
        }
    }

    pub fn for_test_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            db_path: root.join("conary.db"),
            root,
        }
    }

    pub fn from_db_path(db_path: impl Into<PathBuf>) -> Self {
        Self {
            root: PathBuf::from("/conary"),
            db_path: db_path.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    pub fn generations_dir(&self) -> PathBuf {
        self.root.join("generations")
    }

    pub fn generation_path(&self, number: i64) -> PathBuf {
        self.generations_dir().join(number.to_string())
    }

    pub fn current_link(&self) -> PathBuf {
        self.root.join("current")
    }

    pub fn mount_dir(&self) -> PathBuf {
        self.root.join("mnt")
    }

    pub fn etc_state_dir(&self) -> PathBuf {
        self.root.join("etc-state")
    }

    pub fn gc_roots_dir(&self) -> PathBuf {
        self.root.join("gc-roots")
    }
}
```

Add to `crates/conary-core/src/lib.rs`:

```rust
pub mod runtime_root;
```

- [ ] **Step 4: Thread `ConaryRuntimeRoot` into transaction and generation helpers**

Replace generation path hard-coding in `crates/conary-core/src/generation/metadata.rs` with default runtime-root calls:

```rust
pub fn generations_dir() -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().generations_dir()
}

pub fn generation_path(number: i64) -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().generation_path(number)
}

pub fn current_link() -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().current_link()
}

pub fn gc_roots_dir() -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().gc_roots_dir()
}
```

In `TransactionConfig::from_paths()` inside `crates/conary-core/src/transaction/mod.rs`, derive `objects_dir`, `generations_dir`, and `mount_point` from `ConaryRuntimeRoot::from_db_path(db_path.clone())` unless tests pass an explicit non-default root. Preserve existing tests by adjusting expected generation paths to `/conary/generations` where production defaults are being asserted.

- [ ] **Step 5: Update CLI generation commands**

In `apps/conary/src/commands/composefs_ops.rs`, replace `conary_root_for_db_path(db_path)` with:

```rust
fn runtime_root_for_db_path(db_path: &str) -> conary_core::runtime_root::ConaryRuntimeRoot {
    conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path))
}
```

Then use `runtime_root.root()`, `runtime_root.generations_dir()`, `runtime_root.mount_dir()`, and `runtime_root.etc_state_dir()` instead of local string joins.

In `apps/conary/src/commands/generation/commands.rs` and `apps/conary/src/commands/generation/switch.rs`, replace direct `Path::new("/conary")`, `"/conary/objects"`, `"/conary/mnt"`, `"/conary/etc-state"`, and `generation_path(number)` calls with `ConaryRuntimeRoot::default()` methods.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p conary-core runtime_root -- --nocapture
cargo test -p conary-core generation_composefs_runtime_contract -- --nocapture
cargo test -p conary current_base_generation_for_merge_reads_db_column -- --nocapture
```

Expected: all pass.

- [ ] **Step 7: Commit Phase 2**

Run:

```bash
git add crates/conary-core/src/runtime_root.rs crates/conary-core/src/lib.rs crates/conary-core/src/transaction/mod.rs crates/conary-core/src/generation/metadata.rs apps/conary/src/commands/composefs_ops.rs apps/conary/src/commands/generation/commands.rs apps/conary/src/commands/generation/switch.rs crates/conary-core/tests/generation_composefs_runtime_contract.rs
git commit -m "refactor(generation): centralize runtime root paths"
```

---

## Phase 3: Strict Boot Activation

### Task 3.1: Remove Dracut Legacy Bind Fallback

**Files:**
- Modify: `packaging/dracut/90conary/conary-generator.sh`
- Modify: `crates/conary-core/src/bootstrap/system_config.rs`
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`

- [ ] **Step 1: Replace the source-contract test**

In `crates/conary-core/tests/generation_composefs_runtime_contract.rs`, replace `initramfs_generation_mounts_have_empty_usr_symlink_fallback` with:

```rust
#[test]
fn initramfs_generation_mounts_expose_usr_without_partial_generation_fallback() {
    let dracut_generator = fs::read_to_string(workspace_file(
        "packaging/dracut/90conary/conary-generator.sh",
    ))
    .expect("failed to read conary dracut generator");
    let bootstrap_config = fs::read_to_string(core_source("bootstrap/system_config.rs"))
        .expect("failed to read bootstrap system config");

    assert!(
        !dracut_generator.contains("Fall back to legacy bind-mount"),
        "dracut must not describe missing root.erofs as a compatibility path"
    );
    assert!(
        !dracut_generator.contains("mount --bind \"${GEN_DIR}/${dir}\""),
        "dracut must not bind-mount usr/etc from partial generation directories"
    );
    assert!(
        dracut_generator.contains("[ -f \"$EROFS_IMG\" ] ||"),
        "dracut must hard-fail when root.erofs is absent"
    );

    for (label, source) in [
        ("dracut generator", dracut_generator.as_str()),
        ("bootstrap initramfs", bootstrap_config.as_str()),
    ] {
        assert!(
            source.contains("expose_generation_usr"),
            "{label} must route generation /usr exposure through the shared post-composefs helper"
        );
        assert!(
            source.contains("ensure_root_symlink sbin usr/sbin"),
            "{label} must ensure /sbin resolves through usr-merge before switch_root"
        );
    }
}
```

- [ ] **Step 2: Run the test and confirm RED**

Run:

```bash
cargo test -p conary-core initramfs_generation_mounts_expose_usr_without_partial_generation_fallback -- --nocapture
```

Expected: FAIL because the dracut script still contains the legacy fallback.

- [ ] **Step 3: Remove the fallback from dracut**

In `packaging/dracut/90conary/conary-generator.sh`, replace the missing EROFS block with:

```bash
if [ ! -f "$EROFS_IMG" ]; then
    echo "conary: generation ${CONARY_GEN} is missing root.erofs at ${EROFS_IMG}" >&2
    exit 1
fi
```

Keep `expose_generation_usr` and usr-merge symlink handling after the composefs mount succeeds.

- [ ] **Step 4: Align embedded bootstrap initramfs if needed**

In `crates/conary-core/src/bootstrap/system_config.rs`, confirm the embedded script already fails on missing `root.erofs`. If the wording differs, make it match:

```sh
[ -f "$EROFS_IMG" ] || fail "generation $CONARY_GEN is missing root.erofs"
```

- [ ] **Step 5: Run focused checks**

Run:

```bash
cargo test -p conary-core initramfs_generation_mounts_expose_usr_without_partial_generation_fallback -- --nocapture
cargo test -p conary-core generation_composefs_runtime_contract -- --nocapture
```

Expected: all pass.

- [ ] **Step 6: Commit Phase 3.1**

Run:

```bash
git add packaging/dracut/90conary/conary-generator.sh crates/conary-core/src/bootstrap/system_config.rs crates/conary-core/tests/generation_composefs_runtime_contract.rs
git commit -m "fix(boot): fail closed without generation root erofs"
```

### Task 3.2: Make Live Switch Debug-Only Or Remove It From Public Commands

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `apps/conary/src/commands/generation/switch.rs`
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`

- [ ] **Step 1: Add source-contract tests for live switch status**

Add this test to `crates/conary-core/tests/generation_composefs_runtime_contract.rs`:

```rust
#[test]
fn release_generation_commands_do_not_expose_live_switch_as_normal_activation() {
    let commands_rs = fs::read_to_string(app_source("commands/generation/commands.rs"))
        .expect("failed to read generation commands");
    let cli_rs = fs::read_to_string(workspace_file("apps/conary/src/cli/generation.rs"))
        .expect("failed to read generation cli");

    assert!(
        !commands_rs.contains("switch_live(number)?;"),
        "release-facing generation switch must not call live switch directly"
    );
    assert!(
        !commands_rs.contains("switch_live(*previous)?;"),
        "release-facing rollback must not call live switch directly"
    );
    assert!(
        cli_rs.contains("debug") || !cli_rs.contains("Switch"),
        "live switching must be removed from public CLI or explicitly labeled debug/unsafe"
    );
}
```

- [ ] **Step 2: Run the test and confirm RED**

Run:

```bash
cargo test -p conary-core release_generation_commands_do_not_expose_live_switch_as_normal_activation -- --nocapture
```

Expected: FAIL because `cmd_generation_switch()` and rollback call `switch_live()`.

- [ ] **Step 3: Implement the chosen behavior**

Prefer removal from release-facing commands. Change `cmd_generation_switch()` and rollback in `apps/conary/src/commands/generation/commands.rs` so they update the active pointer and print a reboot-required message:

```rust
let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::default();
conary_core::generation::mount::update_current_symlink(runtime_root.root(), number)?;
println!("Generation {number} selected for next boot.");
println!("Reboot to activate the selected composefs generation.");
```

For rollback:

```rust
let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::default();
conary_core::generation::mount::update_current_symlink(runtime_root.root(), *previous)?;
println!("Generation {previous} selected for next boot.");
println!("Reboot to activate the rollback generation.");
```

Move `switch_live()` behind a debug-only command if the CLI already has a debug namespace. If no debug namespace exists, leave `switch_live()` uncalled and add this doc comment:

```rust
/// Developer-only live switch helper.
///
/// Release-facing generation activation selects the next boot generation
/// instead of attempting to make a running process tree coherent in place.
```

- [ ] **Step 4: Make debug live switch fail hard on `/etc` overlay failure**

In `apps/conary/src/commands/generation/switch.rs`, replace the non-fatal overlay failure branch with:

```rust
Err(e) => {
    let _ = run_command("umount", &["/usr"]);
    let _ = unmount_generation(Path::new(staging));
    return Err(e).context("Failed to mount /etc overlay for live debug switch");
}
```

Update or remove the existing source-contract test that expects warning-only behavior for `generation_switch_prints_etc_overlay_failures_to_stderr`.

- [ ] **Step 5: Run focused checks**

Run:

```bash
cargo test -p conary-core release_generation_commands_do_not_expose_live_switch_as_normal_activation -- --nocapture
cargo test -p conary-core generation_composefs_runtime_contract -- --nocapture
```

Expected: all pass.

- [ ] **Step 6: Commit Phase 3.2**

Run:

```bash
git add apps/conary/src/cli/generation.rs apps/conary/src/commands/generation/commands.rs apps/conary/src/commands/generation/switch.rs crates/conary-core/tests/generation_composefs_runtime_contract.rs
git commit -m "fix(generation): make activation boot-time first"
```

---

## Phase 4: Recovery And Live-Root Fallback Removal

### Task 4.1: Make Recovery Use Artifact Evidence

**Files:**
- Modify: `crates/conary-core/src/transaction/recovery.rs`
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`

- [ ] **Step 1: Add a source-contract test against magic-only promotion**

Add to `crates/conary-core/tests/generation_composefs_runtime_contract.rs`:

```rust
#[test]
fn recovery_does_not_promote_generations_by_erofs_magic_only() {
    let recovery_rs = fs::read_to_string(core_source("transaction/recovery.rs"))
        .expect("failed to read recovery.rs");

    assert!(
        recovery_rs.contains("load_installed_generation_artifact")
            || recovery_rs.contains("load_generation_artifact"),
        "recovery must load the generation artifact contract before promoting a generation"
    );
    assert!(
        !recovery_rs.contains("verity: false,\n                digest: None,"),
        "recovery must not hard-code plain composefs when metadata requests verity"
    );
}
```

- [ ] **Step 2: Run the test and confirm RED**

Run:

```bash
cargo test -p conary-core recovery_does_not_promote_generations_by_erofs_magic_only -- --nocapture
```

Expected: FAIL because recovery currently uses `is_valid_erofs_image()` and hard-coded plain composefs options.

- [ ] **Step 3: Load artifacts in recovery**

In `crates/conary-core/src/transaction/recovery.rs`, import:

```rust
use crate::generation::artifact::load_generation_artifact;
use crate::generation::metadata::GenerationMetadata;
```

Replace current-image validation with:

```rust
let gen_dir = self.config.generations_dir.join(current_num.to_string());
let artifact = match load_generation_artifact(&gen_dir) {
    Ok(artifact) => artifact,
    Err(error) => {
        tracing::warn!(
            "Recovery: active generation {} failed artifact validation: {}",
            current_num,
            error
        );
        // Continue to DB rebuild.
        return self.rebuild_or_scan(conn, Some(current_num));
    }
};
return self.mount_artifact_and_link(current_num, &artifact);
```

Implement `rebuild_or_scan()` by moving the existing DB rebuild and scan logic into a helper. Keep the same fallback ordering, but scanning must call `load_generation_artifact(&candidate_dir)` and skip candidates whose artifact validation fails.

- [ ] **Step 4: Mount with metadata verity**

Replace `mount_and_link()` with a helper that reads metadata and maps it into mount options:

```rust
fn mount_artifact_and_link(
    &self,
    gen_num: i64,
    artifact: &crate::generation::artifact::GenerationArtifact,
) -> Result<()> {
    let metadata = GenerationMetadata::read_from(&artifact.generation_dir)?;
    let requested_verity = metadata.fsverity_enabled && metadata.erofs_verity_digest.is_some();
    crate::generation::mount::mount_generation(&crate::generation::mount::MountOptions {
        image_path: artifact.erofs_path.clone(),
        basedir: self.config.objects_dir.clone(),
        mount_point: self.config.mount_point.clone(),
        verity: requested_verity,
        digest: if requested_verity {
            metadata.erofs_verity_digest.clone()
        } else {
            None
        },
        upperdir: None,
        workdir: None,
    })?;

    crate::generation::mount::update_current_symlink(&self.config.root, gen_num)?;
    Ok(())
}
```

The required `GenerationArtifact` public fields are `generation`, `generation_dir`, `metadata`, `erofs_path`, `cas_dir`, `cas_objects`, and `boot_assets`.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p conary-core recovery_does_not_promote_generations_by_erofs_magic_only -- --nocapture
cargo test -p conary-core recover -- --nocapture
cargo test -p conary-core mounted_generation_policy -- --nocapture
```

Expected: all pass.

### Task 4.2: Remove No-Generation Live-Root Remove/Rollback Fallbacks

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/system.rs`

- [ ] **Step 1: Add focused command tests**

In `apps/conary/src/commands/remove.rs`, add:

```rust
#[test]
fn remove_requires_active_generation_before_live_root_mutation() {
    let source = std::fs::read_to_string("apps/conary/src/commands/remove.rs")
        .unwrap_or_else(|_| include_str!("remove.rs").to_string());
    assert!(
        !source.contains("remove_files_from_live_root(Path::new(root)"),
        "remove must not fall back to direct live-root mutation when no active generation exists"
    );
}
```

In `apps/conary/src/commands/system.rs`, add:

```rust
#[test]
fn rollback_requires_active_generation_before_live_root_mutation() {
    let source = std::fs::read_to_string("apps/conary/src/commands/system.rs")
        .unwrap_or_else(|_| include_str!("system.rs").to_string());
    assert!(
        !source.contains("restore_snapshots_to_live_root(root_path"),
        "rollback must not restore package payloads directly into the live root"
    );
    assert!(
        !source.contains("remove_snapshots_from_live_root(root_path"),
        "rollback must not remove package payloads directly from the live root"
    );
}
```

- [ ] **Step 2: Run tests and confirm RED**

Run:

```bash
cargo test -p conary remove_requires_active_generation_before_live_root_mutation -- --nocapture
cargo test -p conary rollback_requires_active_generation_before_live_root_mutation -- --nocapture
```

Expected: FAIL because the fallback calls still exist.

- [ ] **Step 3: Replace fallback branches with explicit errors**

In `apps/conary/src/commands/remove.rs`, replace the `else` branch after `if active_generation.is_some()` with:

```rust
anyhow::bail!(
    "Cannot remove {package_name} without an active composefs generation. \
     Build or activate a generation first, then retry the remove operation."
);
```

In `apps/conary/src/commands/system.rs`, replace no-active-generation rollback branches with:

```rust
anyhow::bail!(
    "Cannot roll back changeset {changeset_id} without an active composefs generation. \
     Build or activate a generation first, then retry rollback."
);
```

Keep `remove_files_from_live_root()` and `restore_snapshots_to_live_root()` only if existing unit tests still need them. Rename them to `test_remove_files_from_live_root()` and `test_restore_snapshots_to_live_root()` or move them under `#[cfg(test)]` so release code cannot call them.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary remove_requires_active_generation_before_live_root_mutation -- --nocapture
cargo test -p conary rollback_requires_active_generation_before_live_root_mutation -- --nocapture
cargo test -p conary remove -- --nocapture
cargo test -p conary system -- --nocapture
```

Expected: all pass.

- [ ] **Step 5: Commit Phase 4**

Run:

```bash
git add crates/conary-core/src/transaction/recovery.rs crates/conary-core/tests/generation_composefs_runtime_contract.rs apps/conary/src/commands/remove.rs apps/conary/src/commands/system.rs
git commit -m "fix(generation): fail closed on recovery and rollback fallbacks"
```

---

## Phase 5: OCI Export Unification

### Task 5.1: Load OCI Source Through `GenerationArtifact`

**Files:**
- Modify: `apps/conary/src/commands/export.rs`
- Modify: `crates/conary-core/src/generation/export.rs`
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`

- [ ] **Step 1: Add source-contract test**

Add to `crates/conary-core/tests/generation_composefs_runtime_contract.rs`:

```rust
#[test]
fn oci_generation_export_uses_generation_artifact_loader() {
    let export_rs = fs::read_to_string(app_source("commands/export.rs"))
        .expect("failed to read commands/export.rs");

    assert!(
        export_rs.contains("load_generation_artifact")
            || export_rs.contains("load_installed_generation_artifact"),
        "OCI export must use the same GenerationArtifact loader as raw/qcow2 export"
    );
    assert!(
        !export_rs.contains("let gen_dir = generation_path(gen_number);"),
        "OCI export must not independently resolve generation paths"
    );
}
```

- [ ] **Step 2: Run test and confirm RED**

Run:

```bash
cargo test -p conary-core oci_generation_export_uses_generation_artifact_loader -- --nocapture
```

Expected: FAIL because `apps/conary/src/commands/export.rs` currently resolves generation paths itself.

- [ ] **Step 3: Add public artifact loading helper if needed**

If `crates/conary-core/src/generation/artifact.rs` already exports both `load_generation_artifact()` and `load_installed_generation_artifact()`, use them directly. If not re-exported, update `crates/conary-core/src/generation/mod.rs`:

```rust
pub use artifact::{load_generation_artifact, load_installed_generation_artifact, GenerationArtifact};
```

- [ ] **Step 4: Update OCI export**

In `apps/conary/src/commands/export.rs`, replace generation resolution with:

```rust
let artifact = match generation {
    Some(n) => conary_core::generation::artifact::load_installed_generation_artifact(n)?,
    None => conary_core::generation::artifact::load_generation_artifact(Path::new("/conary/current"))?,
};
let gen_number = artifact.generation;
let gen_dir = artifact.generation_dir.clone();
let erofs_path = artifact.erofs_path.clone();
```

Then pass `&artifact.cas_dir` or the existing `objects_dir` only after asserting they match:

```rust
if artifact.cas_dir != objects_dir {
    tracing::warn!(
        requested = %objects_dir.display(),
        artifact = %artifact.cas_dir.display(),
        "Using CAS object directory from generation artifact"
    );
}
let objects_dir = artifact.cas_dir.as_path();
```

Use `artifact.cas_objects` when scoping the OCI layer instead of re-querying the DB for generation membership.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p conary-core oci_generation_export_uses_generation_artifact_loader -- --nocapture
cargo test -p conary export -- --nocapture
```

Expected: all pass.

- [ ] **Step 6: Commit Phase 5**

Run:

```bash
git add apps/conary/src/commands/export.rs crates/conary-core/src/generation/export.rs crates/conary-core/src/generation/mod.rs crates/conary-core/tests/generation_composefs_runtime_contract.rs
git commit -m "refactor(export): load OCI generations from artifacts"
```

---

## Phase 6: Defaults, Takeover, And Docs

### Task 6.1: Align Preview Defaults With Generation-Backed Ownership

**Files:**
- Modify: `crates/conary-core/src/model/parser.rs`
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/commands/install/dep_mode.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `apps/conary/src/commands/generation/takeover.rs`
- Modify: `AGENTS.md`
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/operations/bootstrap-follow-up-investigations.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`
- Modify: `docs/llms/README.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-*.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Decide default convergence for preview**

Use `ConvergenceIntent::CasBacked` unless the previous phases prove full ownership is needed for every default install. The plan recommendation is `CasBacked` because it moves package content into CAS without making every dependency takeover implicit.

- [ ] **Step 2: Write failing tests**

In `crates/conary-core/src/model/parser.rs`, update the default test expectation:

```rust
assert_eq!(model.system.convergence, ConvergenceIntent::CasBacked);
```

In `apps/conary/src/commands/install/dep_mode.rs`, add:

```rust
#[test]
fn preview_default_convergence_uses_adopt_dep_mode() {
    assert_eq!(
        DepMode::from_convergence_intent(&ConvergenceIntent::default()),
        DepMode::Adopt
    );
}
```

In `apps/conary/src/cli/mod.rs`, add a failing check that omitted `conary update --dep-mode`
does not advertise or parse as a hard-coded `satisfy` default.

- [ ] **Step 3: Run tests and confirm RED**

Run:

```bash
cargo test -p conary-core convergence -- --nocapture
cargo test -p conary preview_default_convergence_uses_adopt_dep_mode -- --nocapture
cargo test -p conary update_dep_mode -- --nocapture
```

Expected: parser default test fails until the enum default moves, and update
help/parse tests fail until the CLI no longer hard-codes `satisfy`.

- [ ] **Step 4: Change the enum default**

In `crates/conary-core/src/model/parser.rs`, move `#[default]` from `TrackOnly` to `CasBacked`:

```rust
pub enum ConvergenceIntent {
    TrackOnly,
    #[default]
    CasBacked,
    FullOwnership,
}
```

Update nearby comments to say non-interactive preview flows default to CAS-backed content, while `TrackOnly` remains an explicit low-disruption mode.

Make omitted install/update dependency modes derive through the model convergence
intent. If no model exists yet, use `ConvergenceIntent::default()` rather than
falling back to `DepMode::Satisfy`, so first-run preview behavior follows the
same CAS-backed default.

- [ ] **Step 5: Keep takeover generation as the public path**

In `apps/conary/src/commands/generation/takeover.rs`, keep the existing default test:

```rust
let level = TakeoverLevel::default();
assert!(matches!(level, TakeoverLevel::Generation));
```

Remove user-facing "Next steps" text that encourages stopping at `cas` or `owned` as a normal path. Replace with text that labels those stop-points internal/debug when they still exist.

- [ ] **Step 6: Update docs after behavior is green**

Update docs with these exact truths:

- `docs/ARCHITECTURE.md`: runtime mutation is DB/CAS/generation/active-pointer first; direct live-root mutation is not a supported release path.
- `docs/modules/bootstrap.md`: bootstrap can build mutable inputs, but published runtime output is a complete generation artifact.
- `docs/operations/post-generation-export-follow-up-roadmap.md`: remove completed dracut fallback and OCI artifact-loader items after those phases are merged.
- `docs/llms/README.md`: use tool-specific entrypoint wording if the root `AGENTS.md` no longer asks for compatibility shims.

- [ ] **Step 7: Run docs and focused tests**

Run:

```bash
cargo test -p conary-core convergence -- --nocapture
cargo test -p conary dep_mode -- --nocapture
cargo test -p conary update_dep_mode -- --nocapture
cargo test -p conary missing_model_uses_preview_convergence_dep_mode -- --nocapture
cargo test -p conary automation_install_leaves_dependency_mode_model_derived -- --nocapture
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
rg -n "legacy bind-mount fallback|OCI should use that same loader|TrackOnly.*default|compatibility shims" docs apps crates packaging
```

Expected: tests pass. The audit ledger is complete. The `rg` output contains
only historical/archive/planning references or no matches.

- [ ] **Step 8: Commit Phase 6**

Run:

```bash
git add AGENTS.md README.md ROADMAP.md apps/conary/src/cli/mod.rs apps/conary/src/cli/system.rs apps/conary/src/commands/automation.rs apps/conary/src/commands/generation/takeover.rs apps/conary/src/commands/install/dep_mode.rs apps/conary/src/commands/install/mod.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/update.rs apps/remi/src/server/handlers/oci.rs crates/conary-core/src/generation/builder.rs crates/conary-core/src/generation/builder/runtime_inputs.rs crates/conary-core/src/model/parser.rs crates/conary-core/src/repository/parsers/mod.rs docs/ARCHITECTURE.md docs/conaryopedia-v2.md docs/llms/README.md docs/modules/bootstrap.md docs/operations/bootstrap-follow-up-investigations.md docs/operations/post-generation-export-follow-up-roadmap.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/plans/2026-05-06-limited-public-release-readiness-plan.md docs/superpowers/plans/archive/2026-05-12-composefs-atomic-modernization-plan.md
git commit -m "docs: align preview defaults with generation ownership"
```

---

## Phase 7: Integrated Verification And Completion Audit

### Task 7.1: Run Fast Workspace Gates

**Files:**
- No source edits unless a verification failure identifies a real defect.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 2: Run focused package tests**

Run:

```bash
cargo test -p conary-core
cargo test -p conary
```

Expected: PASS.

- [ ] **Step 3: Run integration manifest inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: PASS and the output lists the active suites without parse errors.

- [ ] **Step 4: Run clippy**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

### Task 7.2: Run Release-Gate QEMU Validation

**Files:**
- Modify validation docs only to record final evidence after the run passes.

- [ ] **Step 1: Run the active generation export QEMU gate**

Run:

```bash
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3
```

Expected: PASS for `TGE01`, `TGE03`, `TGE04`, and `TGE02`.

- [ ] **Step 2: Add or run fallback-removal manifests**

If no existing manifest proves missing `root.erofs`, no-generation rollback, and OCI artifact loading, add a focused manifest under:

```text
apps/conary/tests/integration/remi/manifests/phase3-composefs-modernization.toml
```

It must include checks that:

- a generation directory without `root.erofs` is rejected
- remove/rollback without active generation fails
- OCI export rejects partial generation artifacts

Run:

```bash
cargo run -p conary-test -- list
cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3
```

Expected: manifest parses and the suite passes.

### Task 7.3: Completion Audit Before Marking The Goal Complete

**Files:**
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`
- Modify: `ROADMAP.md` if any requirement remains incomplete.

- [ ] **Step 1: Build the prompt-to-artifact checklist**

Create a checklist in the final validation notes that maps each requirement to evidence:

```text
Requirement: every public install path uses shared CAS/DB/generation lifecycle
Evidence: code refs, tests, and passing commands

Requirement: one runtime root policy
Evidence: ConaryRuntimeRoot code refs and tests

Requirement: dracut rejects partial generations
Evidence: source-contract test and QEMU/manifest result

Requirement: remove/rollback do not mutate live root without generation
Evidence: code refs and focused tests

Requirement: recovery uses artifact/metadata/verity
Evidence: code refs and tests

Requirement: OCI export uses GenerationArtifact
Evidence: code refs and tests

Requirement: preview defaults/docs point to generation-backed ownership
Evidence: code refs, doc refs, and stale-phrase sweep

Requirement: workspace verification is green
Evidence: command outputs with dates
```

- [ ] **Step 2: Update docs with concrete evidence**

Update validation docs only after commands pass. Include exact command lines, date `2026-05-12` or later, and result summaries. Do not claim a QEMU gate passed unless the real command output shows it.

- [ ] **Step 3: Run stale-phrase sweep**

Run:

```bash
rg -n "legacy bind-mount fallback|still contains|remaining.*OCI|TrackOnly.*default|directly because no active generation|may be stale|compatibility shim|compatibility layer" README.md ROADMAP.md AGENTS.md docs apps crates packaging
```

Expected: only archived/historical references remain, or active docs describe completed behavior truthfully.

- [ ] **Step 4: Commit verification docs**

Run:

```bash
git add docs/INTEGRATION-TESTING.md docs/operations/post-generation-export-follow-up-roadmap.md ROADMAP.md
git commit -m "docs: record composefs modernization validation"
```

Only include `ROADMAP.md` if it changed.

- [ ] **Step 5: Final goal audit**

Before marking the `/goal` complete, inspect:

```bash
git status --short
git log --oneline -8
```

Then compare the actual repo state against the acceptance criteria in `docs/superpowers/specs/archive/2026-05-12-composefs-atomic-modernization-design.md`. If any requirement is missing or weakly verified, keep the goal active and create a follow-up task instead of marking it complete.
