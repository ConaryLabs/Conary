# Phase 1: Model Apply + State Revert Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `conary model apply` and `conary system state revert` actually execute package install/remove/update operations instead of printing "not yet implemented" stubs.

**Architecture:** Extract inner helpers from `cmd_install` and `cmd_remove` that accept a caller-owned DB transaction, changeset ID, `TransactionEngine`, root path, and architecture selector, while keeping generation/snapshot ownership inside `rebuild_and_mount()`. Treat RPM/DEB/Arch as ingress formats and CCS as the native install model: legacy artifacts may still be parsed or converted during preparation, but once a package reaches the shared mutation path it should flow through one format-neutral prepared-install contract instead of splitting into separate “legacy install” vs “CCS install” execution trees. `model apply` still calls `cmd_install`/`cmd_remove` directly (one changeset per operation, matching existing `apply_replatform_changes()`), but `state revert` uses the shared inner helpers plus restore-specific install preparation that supports both legacy and CCS resolution outcomes, validates dependencies against a capability-aware view of the target state without mutating the system during preflight, and then executes removals, installs, and upgrades under one wrapping changeset, one DB transaction commit, one `rebuild_and_mount()`, and the single generation snapshot created by that rebuild. Atomicity in this phase is for DB/generation state; scriptlet side effects keep the same non-rollback-safe caveat Conary already has today. Both commands get `require_live_mutation()` safety gates.

**Tech Stack:** Rust, rusqlite, composefs, EROFS, conary-core transaction engine

**Spec:** `docs/superpowers/specs/2026-04-08-pre-release-completeness-design.md` Phase 1

---

## File Structure

| File | Role | Action |
|------|------|--------|
| `apps/conary/src/commands/install/mod.rs` | Package installation | Modify: add `architecture` to `InstallOptions`, introduce an owned `PreparedInstall`, extract restore-safe resolution/preparation helpers that support legacy and CCS package paths, split pre-install and snapshot-free finalization helpers, make `execute_install_transaction()` a thin wrapper |
| `apps/conary/src/commands/install/prepare.rs` | Install-time version/upgrade semantics | Modify: move shared version-scheme / upgrade checks behind a format-neutral install-semantics struct instead of raw legacy-only `PackageFormatType` |
| `apps/conary/src/commands/install/scriptlets.rs` | Install-time scriptlet semantics | Modify: let shared pre/post install helpers consume format-neutral install semantics so CCS and legacy-prepared packages use the same execution path |
| `apps/conary/src/commands/install/inner.rs` | Inner install helper | Create: `install_inner()` accepting caller-owned DB transaction + engine + changeset_id |
| `apps/conary/src/commands/install/conversion.rs` | CCS conversion/install routing | Modify: expose non-executing CCS preparation for restore so Remi/`.ccs` resolution can feed the shared install path instead of directly calling `install_converted_ccs()` |
| `apps/conary/src/commands/install/resolve.rs` | Package resolution | Modify: thread `architecture` through `ResolutionOptions` for multi-arch correctness |
| `apps/conary/src/commands/remove.rs` | Package removal | Modify: extract `remove_inner()` accepting caller-owned DB transaction + changeset_id + root + architecture, return rollback snapshot data to the caller instead of writing incompatible multi-record metadata, make `cmd_remove` a thin wrapper |
| `apps/conary/src/commands/model/apply.rs` | Model diff application | Modify: implement `apply_package_changes()` and `Update` handling |
| `apps/conary/src/commands/model.rs` | Model apply dispatch | Modify: update Phase 3 call site, wire autoremove |
| `apps/conary/src/commands/state.rs` | State management | Modify: implement `cmd_state_restore()` with inner helpers + TransactionEngine, and add root-aware unit tests for the new restore executor |
| `apps/conary/src/commands/system.rs` | Rollback compatibility | Modify: accept both legacy single-snapshot metadata and the new revert metadata wrapper so wrapping revert changesets remain rollbackable |
| `apps/conary/src/commands/test_helpers.rs` | Command test fixtures | Modify: expose `setup_command_test_db()` for command-unit tests |
| `apps/conary/src/dispatch.rs` | CLI dispatch | Modify: add `require_live_mutation()` gates |
| `apps/conary/tests/model_apply.rs` | Model apply tests | Create |

---

### Task 1: Add `require_live_mutation()` Gates

**Files:**
- Modify: `apps/conary/src/dispatch.rs:559` (state revert dispatch)
- Modify: `apps/conary/src/dispatch.rs:1210-1235` (model apply dispatch)
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Write tests that exercise the actual dispatch wiring**

Add to `apps/conary/tests/live_host_mutation_safety.rs` using the existing `run_conary()` helper:

```rust
#[test]
fn state_revert_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&[
        "system",
        "state",
        "revert",
        "1",
        "--db-path",
        &db_path,
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system state revert"));
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn model_apply_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let model_dir = tempfile::tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        "[model]\nversion = 1\ninstall = [\"openssl\"]\nexclude = [\"nginx\"]\n",
    )
    .unwrap();

    let output = run_conary(&[
        "model",
        "apply",
        "--model",
        model_path.to_str().unwrap(),
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary model apply"));
    assert!(stderr.contains("--allow-live-system-mutation"));
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p conary state_revert_refuses_without_live_mutation_flag model_apply_refuses_without_live_mutation_flag`

Expected: FAIL before the dispatch changes because both commands still fall through to their old behavior instead of emitting the live-mutation refusal.

- [ ] **Step 3: Add the gate to state revert dispatch**

In `apps/conary/src/dispatch.rs`, find the `StateCommands::Revert` arm (line 559):

```rust
// BEFORE:
cli::StateCommands::Revert {
    state_number,
    db,
    dry_run,
} => commands::cmd_state_restore(&db.db_path, state_number, dry_run).await,
```

Replace with:

```rust
// AFTER:
cli::StateCommands::Revert {
    state_number,
    db,
    dry_run,
} => {
    require_live_mutation(
        allow_live_system_mutation,
        Cow::Borrowed("conary system state revert"),
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        dry_run,
    )?;
    commands::cmd_state_restore(&db.db_path, state_number, dry_run).await
}
```

- [ ] **Step 4: Add the gate to model apply dispatch**

In `apps/conary/src/dispatch.rs`, `dispatch_model_command` (line 1210) does not receive `allow_live_system_mutation`. Thread it:

```rust
// BEFORE (line 1210):
async fn dispatch_model_command(model_cmd: cli::ModelCommands) -> Result<()> {

// AFTER:
async fn dispatch_model_command(
    model_cmd: cli::ModelCommands,
    allow_live_system_mutation: bool,
) -> Result<()> {
```

Wrap the `ModelCommands::Apply` arm (line 1216):

```rust
cli::ModelCommands::Apply {
    model,
    common,
    dry_run,
    skip_optional,
    strict,
    no_autoremove,
    offline,
} => {
    require_live_mutation(
        allow_live_system_mutation,
        Cow::Borrowed("conary model apply"),
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        dry_run,
    )?;
    commands::cmd_model_apply(commands::ApplyOptions {
        // ... existing fields unchanged ...
    })
    .await
}
```

Update the call site at line 318:

```rust
// BEFORE:
Some(Commands::Model(model_cmd)) => dispatch_model_command(model_cmd).await,
// AFTER:
Some(Commands::Model(model_cmd)) => {
    dispatch_model_command(model_cmd, allow_live_system_mutation).await
}
```

- [ ] **Step 5: Verify**

Run: `cargo clippy -p conary -- -D warnings && cargo test -p conary state_revert_refuses_without_live_mutation_flag model_apply_refuses_without_live_mutation_flag`

Expected: PASS, no warnings

- [ ] **Step 6: Commit**

```
git add apps/conary/src/dispatch.rs apps/conary/tests/live_host_mutation_safety.rs
git commit -m "fix(dispatch): add require_live_mutation gates for model apply and state revert"
```

---

### Task 2: Add Architecture Selector to InstallOptions and Resolution

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/resolve.rs`

- [ ] **Step 1: Add `architecture` field to `InstallOptions`**

In `apps/conary/src/commands/install/mod.rs`, find `InstallOptions` struct (line 63) and add:

```rust
pub struct InstallOptions<'a> {
    // ... existing fields ...
    /// Filter to specific architecture (e.g. "x86_64"). Used by state revert
    /// for multi-arch precision.
    pub architecture: Option<String>,
}
```

Since `InstallOptions` derives `Default`, the new field defaults to `None`.

- [ ] **Step 2: Thread `architecture` through the install resolution flow**

In `apps/conary/src/commands/install/mod.rs`, thread `opts.architecture.as_deref()` into `resolve_and_parse_package(...)`.

Update `resolve_and_parse_package(...)` and `resolve_package_path_with_policy(...)` to accept `architecture: Option<&str>` and pass it through to `ResolutionOptions`:

```rust
let options = ResolutionOptions {
    version: version.map(String::from),
    repository: repo.map(String::from),
    architecture: architecture.map(String::from),
    output_dir: None,
    gpg_options: None,
    skip_cas: false,
    policy: policy_opts.policy.clone(),
    is_root: policy_opts.is_root,
    primary_flavor: policy_opts.primary_flavor,
};
```

This is required for both direct `cmd_install` calls and `state revert` to resolve
the correct multi-arch NEVRA.

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p conary`

Expected: Compiles. All existing callers use `..Default::default()` or set all fields explicitly -- the new `Option` defaults to `None` either way.

- [ ] **Step 4: Commit**

```
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/resolve.rs
git commit -m "feat(install): thread architecture through install resolution"
```

---

### Task 3: Extract `install_inner()` with Caller-Owned DB Transaction

**Files:**
- Create: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`

The key difference from the rejected plan: `install_inner()` accepts a caller-owned `rusqlite::Transaction<'_>` plus `changeset_id: i64`. It does NOT open or commit its own DB transaction, and it does NOT create its own changeset. The caller owns: DB transaction lifetime, changeset creation, `rebuild_and_mount()`, and changeset status transition.

- [ ] **Step 1: Create `install/inner.rs`**

Create `apps/conary/src/commands/install/inner.rs`:

```rust
// install/inner.rs

//! Inner install helper for callers that own the transaction lifecycle.
//!
//! `install_inner()` performs CAS storage and the DB operations (trove insert,
//! file entries, dependencies, scriptlets) using a caller-provided DB
//! transaction and changeset. It does NOT: create/commit a DB transaction,
//! create a changeset, or call `rebuild_and_mount()`.
//! The caller handles all of those.

use anyhow::{Context, Result};
use conary_core::db::models::{
    Component, ComponentType, DependencyEntry, FileEntry, ProvideEntry, ScriptletEntry,
};
use conary_core::packages::{DependencyClass, PackageFormat as PkgFormat};
use conary_core::transaction::TransactionEngine;
use rusqlite::Transaction;
use std::collections::HashMap;
use tracing::{info, warn};

use super::{
    ExtractionResult, InstallPhase, InstallProgress, TransactionContext,
    mark_upgraded_parent_deriveds_stale, scheme_to_string, version_scheme_for_format,
};

/// Result from `install_inner` -- the trove ID of the installed package.
pub struct InnerInstallResult {
    pub trove_id: i64,
}

/// Execute the install DB operations using a caller-owned DB transaction.
///
/// Stores files in CAS via the provided engine, then inserts the trove,
/// components, files, dependencies, and scriptlets into the caller-provided
/// transaction under the provided `changeset_id`.
///
/// The caller MUST:
/// 1. Create the outer DB transaction and `TransactionEngine`.
/// 2. Create the changeset and pass its ID here.
/// 3. Commit the outer DB transaction after all inner operations complete.
/// 4. Call `rebuild_and_mount()`.
/// 5. Mark the changeset `Applied` after `rebuild_and_mount()` succeeds.
/// 6. Release the engine lock.
pub fn install_inner(
    tx: &Transaction<'_>,
    engine: &mut TransactionEngine,
    changeset_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
) -> Result<InnerInstallResult> {
    let is_upgrade = ctx.old_trove_to_upgrade.is_some();

    // Store extracted file content in CAS
    progress.set_phase(pkg.name(), InstallPhase::Deploying);
    let mut file_hashes: Vec<(String, String, i64, i32, Option<String>)> =
        Vec::with_capacity(extraction.extracted_files.len());
    for file in &extraction.extracted_files {
        let hash = engine
            .cas()
            .store(&file.content)
            .with_context(|| format!("Failed to store {} in CAS", file.path))?;
        file_hashes.push((
            file.path.clone(),
            hash,
            file.size,
            file.mode,
            file.symlink_target.clone(),
        ));
    }

    info!(
        "Stored {} files in CAS for {}",
        file_hashes.len(),
        pkg.name()
    );

    let format = ctx.format;
    let selection_reason = ctx.selection_reason;
    let classified = &extraction.classified;
    let language_provides = &extraction.language_provides;
    let scriptlets = pkg.scriptlets();

    // DB writes using the caller-owned transaction
    let trove_id = {
        if let Some(old_trove) = ctx.old_trove_to_upgrade
            && let Some(old_id) = old_trove.id
        {
            info!("Removing old version {} before upgrade", old_trove.version);
            conary_core::db::models::Trove::delete(tx, old_id)?;
        }

        let mut trove = pkg.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.version_scheme = Some(scheme_to_string(version_scheme_for_format(format)));

        if let Some(reason) = selection_reason {
            trove.selection_reason = Some(reason.to_string());
        }

        if trove.install_source == conary_core::db::models::InstallSource::Repository {
            let repo_id: Option<i64> = tx
                .query_row(
                    "SELECT r.id FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE rp.name = ?1 AND rp.version = ?2
                       AND (?3 IS NULL OR rp.architecture IS NULL OR rp.architecture = ?3)
                     ORDER BY
                         (r.default_strategy_distro = (SELECT distro FROM distro_pin LIMIT 1)) DESC,
                         r.priority DESC, r.id ASC
                     LIMIT 1",
                    rusqlite::params![pkg.name(), pkg.version(), pkg.architecture()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(conary_core::Error::from)?;
            trove.installed_from_repository_id = repo_id;
        }

        let trove_id = trove.insert(tx)?;

        // Components
        let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
        for comp_type in classified.keys() {
            let mut component = Component::from_type(trove_id, *comp_type);
            component.description = Some(format!("{} files", comp_type.as_str()));
            let comp_id = component.insert(tx)?;
            component_ids.insert(*comp_type, comp_id);
        }

        let mut path_to_component: HashMap<&str, i64> = HashMap::new();
        for (comp_type, files) in classified {
            if let Some(&comp_id) = component_ids.get(comp_type) {
                for path in files {
                    path_to_component.insert(path.as_str(), comp_id);
                }
            }
        }

        // Files
        for (path, hash, size, mode, symlink_target) in &file_hashes {
            if hash.len() < 3 {
                warn!("Skipping file with short hash: {} (hash={})", path, hash);
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &size.to_string()],
            )?;

            let component_id = path_to_component.get(path.as_str()).copied();
            let mut file_entry = FileEntry::new(path.clone(), hash.clone(), *size, *mode, trove_id);
            file_entry.component_id = component_id;
            file_entry.symlink_target = symlink_target.clone();
            file_entry.insert(tx)?;

            let action = if is_upgrade { "modify" } else { "add" };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&changeset_id.to_string(), path, hash, action],
            )?;
        }

        // Dependencies
        for dep in pkg.dependencies() {
            let mut dep_entry = DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                None,
                dep.dep_type.as_str().to_string(),
                dep.version.clone(),
            );
            dep_entry.insert(tx)?;
        }

        // Scriptlets
        for scriptlet in scriptlets {
            let mut entry = ScriptletEntry::with_flags(
                trove_id,
                scriptlet.phase.to_string(),
                scriptlet.interpreter.clone(),
                scriptlet.content.clone(),
                scriptlet.flags.clone(),
                format.as_str(),
            );
            entry.insert(tx)?;
        }

        // Language provides
        for lang_dep in language_provides {
            let kind = match lang_dep.class {
                DependencyClass::Package => "package",
                _ => lang_dep.class.prefix(),
            };
            let mut provide = ProvideEntry::new_typed(
                trove_id,
                kind,
                lang_dep.name.clone(),
                lang_dep.version_constraint.clone(),
            );
            provide.insert_or_ignore(tx)?;
        }

        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            pkg.name().to_string(),
            Some(pkg.version().to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        trove_id
    };

    if let Some(old_trove) = ctx.old_trove_to_upgrade {
        mark_upgraded_parent_deriveds_stale(
            tx,
            pkg.name(),
            Some(&old_trove.version),
            pkg.version(),
        );
    }

    Ok(InnerInstallResult { trove_id })
}
```

- [ ] **Step 2: Register the inner module**

In `apps/conary/src/commands/install/mod.rs`, add with other `mod` declarations:

```rust
pub(crate) mod inner;
```

- [ ] **Step 3: Refactor `execute_install_transaction()` to use `install_inner()`**

In `apps/conary/src/commands/install/mod.rs`, replace the body of `execute_install_transaction()` (lines 1440-1699):

```rust
fn execute_install_transaction(
    conn: &mut rusqlite::Connection,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
) -> Result<InstallTransactionResult> {
    let db_path_buf = PathBuf::from(ctx.db_path);
    let tx_config = TransactionConfig::from_paths(PathBuf::from(ctx.root), db_path_buf);
    let mut engine =
        TransactionEngine::new(tx_config).context("Failed to create transaction engine")?;

    engine
        .recover(conn)
        .context("Failed to recover incomplete transactions")?;
    engine.begin().context("Failed to begin transaction")?;

    let tx_description = if let Some(old_trove) = ctx.old_trove_to_upgrade {
        format!("Upgrade {} from {} to {}", pkg.name(), old_trove.version, pkg.version())
    } else {
        format!("Install {}-{}", pkg.name(), pkg.version())
    };

    // Capture /etc snapshot BEFORE the DB transaction
    let prev_etc = crate::commands::composefs_ops::collect_etc_files(conn)?;

    // Create the outer DB transaction + pending changeset (owned by this wrapper)
    let mut changeset = conary_core::db::models::Changeset::new(tx_description.clone());
    let tx = conn.unchecked_transaction()?;
    let changeset_id = changeset.insert(&tx)?;

    // Delegate CAS + DB work to inner helper
    match inner::install_inner(
        &tx, &mut engine, changeset_id, pkg, extraction, ctx, progress,
    ) {
        Ok(_) => {}
        Err(e) => {
            engine.release_lock();
            return Err(e);
        }
    };

    tx.commit()?;

    let post_commit_result = (|| -> Result<()> {
        crate::commands::composefs_ops::rebuild_and_mount(
            conn,
            &tx_description,
            Some(prev_etc),
            std::path::Path::new("/conary"),
        )?;

        changeset.update_status(conn, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok(())
    })();

    engine.release_lock();
    post_commit_result?;

    Ok(InstallTransactionResult { changeset_id })
}
```

- [ ] **Step 4: Verify the refactor compiles and existing tests pass**

Run: `cargo build -p conary && cargo test -p conary`

Expected: All existing tests pass. Refactor is behavior-preserving.

- [ ] **Step 5: Commit**

```
git add apps/conary/src/commands/install/inner.rs apps/conary/src/commands/install/mod.rs
git commit -m "refactor(install): extract install_inner() with caller-owned transaction"
```

---

### Task 4: Extract `remove_inner()` with Caller-Owned DB Transaction + Architecture

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`

- [ ] **Step 1: Add `remove_inner()` function**

Add to `apps/conary/src/commands/remove.rs`, before `cmd_autoremove`:

```rust
/// Inner remove helper for callers that own the transaction lifecycle.
///
/// Performs pre-remove scriptlets and DB writes (file history, trove deletion)
/// using a caller-provided DB transaction and `changeset_id`. Returns the
/// removed trove snapshot so the caller can decide whether and how to persist
/// rollback metadata. Does NOT: open or commit a DB transaction, create a
/// changeset, or call `rebuild_and_mount()`.
///
/// When `architecture` is `Some`, only removes the trove matching that arch
/// (for multi-arch state revert). When `None`, matches the first trove found.
pub(crate) fn remove_inner(
    tx: &rusqlite::Transaction<'_>,
    changeset_id: i64,
    package_name: &str,
    root: &str,
    architecture: Option<&str>,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
) -> Result<TroveSnapshot> {
    use conary_core::db::models::{FileEntry, ScriptletEntry, Trove};

    // Find trove, optionally filtered by architecture
    let trove = if let Some(arch) = architecture {
        Trove::find_by_name(tx, package_name)?
            .into_iter()
            .find(|t| t.architecture.as_deref() == Some(arch))
            .ok_or_else(|| {
                anyhow::anyhow!("Package '{}' ({}) not installed", package_name, arch)
            })?
    } else {
        Trove::find_one_by_name(tx, package_name)?
            .ok_or_else(|| anyhow::anyhow!("Package '{}' not installed", package_name))?
    };
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove '{}' has no ID", package_name))?;

    let files = FileEntry::find_by_trove(tx, trove_id)?;
    let stored_scriptlets = ScriptletEntry::find_by_trove(tx, trove_id)?;

    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| {
            crate::commands::scriptlets::ScriptletPackageFormat::parse(&s.package_format)
        })
        .unwrap_or(crate::commands::scriptlets::ScriptletPackageFormat::Rpm);

    // Pre-remove scriptlet (best effort)
    if !no_scripts && !stored_scriptlets.is_empty() {
        let executor = crate::commands::scriptlets::ScriptletExecutor::new(
            std::path::Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        )
        .with_sandbox_mode(sandbox_mode);

        if let Some(pre) = stored_scriptlets.iter().find(|s| s.phase == "pre-remove") {
            tracing::info!("Running pre-remove scriptlet for {}...", package_name);
            if let Err(e) = executor
                .execute_entry(pre, &crate::commands::scriptlets::ExecutionMode::Remove)
            {
                tracing::warn!("Pre-remove scriptlet failed for {}: {}", package_name, e);
            }
        }
    }

    // Snapshot for rollback metadata
    let snapshot = TroveSnapshot {
        name: trove.name.clone(),
        version: trove.version.clone(),
        architecture: trove.architecture.clone(),
        description: trove.description.clone(),
        install_source: trove.install_source.as_str().to_string(),
        installed_from_repository_id: trove.installed_from_repository_id,
        files: files
            .iter()
            .map(|f| FileSnapshot {
                path: f.path.clone(),
                sha256_hash: f.sha256_hash.clone(),
                size: f.size,
                permissions: f.permissions,
                symlink_target: f.symlink_target.clone(),
            })
            .collect(),
    };
    // Re-check dependency breakage inside the caller-owned transaction
    let breaking = conary_core::resolver::solve_removal(tx, std::slice::from_ref(&trove.name))?;
    if !breaking.is_empty() {
        return Err(conary_core::Error::IoError(format!(
            "'{}' required by: {}",
            package_name,
            breaking.join(", ")
        ))
        .into());
    }

    // Record file removals in history
    for file in &files {
        let use_hash = if file.sha256_hash.len() == 64
            && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            let hash_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_contents WHERE sha256_hash = ?1)",
                [&file.sha256_hash],
                |row| row.get(0),
            )?;
            if hash_exists {
                Some(file.sha256_hash.as_str())
            } else {
                None
            }
        } else {
            None
        };

        match use_hash {
            Some(hash) => {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                    [&changeset_id.to_string(), &file.path, hash, "delete"],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, NULL, ?3)",
                    [&changeset_id.to_string(), &file.path, "delete"],
                )?;
            }
        }
    }

    Trove::delete(tx, trove_id)?;

    // Post-remove scriptlet (best effort)
    if !no_scripts && !stored_scriptlets.is_empty() {
        let executor = crate::commands::scriptlets::ScriptletExecutor::new(
            std::path::Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        )
        .with_sandbox_mode(sandbox_mode);

        if let Some(post) = stored_scriptlets.iter().find(|s| s.phase == "post-remove") {
            if let Err(e) = executor
                .execute_entry(post, &crate::commands::scriptlets::ExecutionMode::Remove)
            {
                tracing::warn!(
                    "Post-remove scriptlet failed for {}: {}",
                    package_name,
                    e
                );
            }
        }
    }

    Ok(snapshot)
}
```

- [ ] **Step 2: Refactor `cmd_remove()` to own the outer DB transaction**

In `apps/conary/src/commands/remove.rs`, keep the existing pre-checks and
`TransactionEngine` setup, but replace the inline delete transaction with:

```rust
let mut changeset = conary_core::db::models::Changeset::new(format!(
    "Remove {}-{}",
    trove.name, trove.version
));
let tx = conn.unchecked_transaction()?;
let remove_changeset_id = changeset.insert(&tx)?;

let snapshot = remove_inner(
    &tx,
    remove_changeset_id,
    package_name,
    root,
    trove.architecture.as_deref(),
    no_scripts,
    sandbox_mode,
)?;
let snapshot_json = serde_json::to_string(&snapshot)?;
tx.execute(
    "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
    [&snapshot_json, &remove_changeset_id.to_string()],
)?;

tx.commit()?;

let post_commit_result = (|| -> Result<()> {
    crate::commands::composefs_ops::rebuild_and_mount(
        &conn,
        &format!("Remove {}", package_name),
        Some(prev_etc),
        std::path::Path::new("/conary"),
    )?;

    changeset.update_status(&conn, conary_core::db::models::ChangesetStatus::Applied)?;
    Ok(())
})();

engine.release_lock();
post_commit_result?;
```

Do not change the adopted-package fast path in this task.

- [ ] **Step 3: Ensure `TroveSnapshot` and `FileSnapshot` are `pub(crate)`**

In `remove.rs`, update visibility if needed:

```rust
#[derive(serde::Serialize)]
pub(crate) struct TroveSnapshot { ... }

#[derive(serde::Serialize)]
pub(crate) struct FileSnapshot { ... }
```

- [ ] **Step 4: Verify compilation**

Run: `cargo build -p conary`

Expected: Compiles. `cmd_remove` now owns the outer transaction/rebuild lifecycle, `remove_inner` owns only the reusable inner delete logic, and snapshot creation continues to come from `rebuild_and_mount()` rather than a second explicit `create_state_snapshot()` call.

- [ ] **Step 5: Commit**

```
git add apps/conary/src/commands/remove.rs
git commit -m "refactor(remove): extract remove_inner() with caller-owned transaction"
```

---

### Task 5: Implement `apply_package_changes()` and Wire Model Apply

**Files:**
- Modify: `apps/conary/src/commands/model/apply.rs:370-400` and `:545-559`
- Modify: `apps/conary/src/commands/model.rs:560-575`
- Create: `apps/conary/tests/model_apply.rs`

- [ ] **Step 1: Write a test that fails on today's code**

Create `apps/conary/tests/model_apply.rs`:

```rust
//! Integration tests for model apply package operations.

mod common;

use common::setup_command_test_db;

use conary::commands::{ApplyOptions, cmd_model_apply};
use conary_core::db::models::Trove;

#[tokio::test]
async fn test_model_apply_executes_remove_actions() {
    let (_dir, db_path) = setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let model_dir = tempfile::tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1
install = ["openssl"]
exclude = ["nginx"]
"#,
    )
    .unwrap();

    let result = cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: root.path().to_str().unwrap(),
        dry_run: false,
        skip_optional: false,
        strict: false,
        autoremove: false,
        offline: true,
    })
    .await;

    assert!(result.is_ok(), "model apply should remove nginx: {:?}", result);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert!(Trove::find_one_by_name(&conn, "nginx").unwrap().is_none());
    assert!(Trove::find_one_by_name(&conn, "openssl").unwrap().is_some());
}

#[tokio::test]
async fn test_model_apply_returns_err_when_every_operation_fails() {
    let (_dir, db_path) = setup_command_test_db();
    let root = tempfile::tempdir().unwrap();
    let model_dir = tempfile::tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1
install = ["does-not-exist"]
exclude = ["openssl"]
"#,
    )
    .unwrap();

    let result = cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: root.path().to_str().unwrap(),
        dry_run: false,
        skip_optional: false,
        strict: false,
        autoremove: false,
        offline: true,
    })
    .await;

    assert!(result.is_err(), "all-failed model apply should return Err");
}
```

- [ ] **Step 2: Implement `apply_package_changes()`**

In `apps/conary/src/commands/model/apply.rs`, replace the stub at lines 370-400:

```rust
/// Apply package install/remove actions from the model diff.
///
/// Returns `(applied_count, error_list)`.
pub(super) async fn apply_package_changes(
    db_path: &str,
    root: &str,
    actions: &[&DiffAction],
    strict: bool,
) -> Result<(usize, Vec<String>)> {
    let mut applied = 0;
    let mut errors = Vec::new();

    // Removals first (order matters for dependency satisfaction)
    for action in actions {
        if let DiffAction::Remove {
            package,
            current_version,
            ..
        } = action
        {
            println!("Removing {} {}...", package, current_version);
            match crate::commands::cmd_remove(
                package,
                db_path,
                root,
                Some(current_version.clone()),
                false,
                crate::commands::SandboxMode::Auto,
                false,
            )
            .await
            {
                Ok(()) => {
                    println!("  Removed {}", package);
                    applied += 1;
                }
                Err(e) => {
                    let msg = format!("Remove '{}': {}", package, e);
                    eprintln!("  [FAILED] {}", msg);
                    if strict {
                        anyhow::bail!(msg);
                    }
                    errors.push(msg);
                }
            }
        }
    }

    // Then installs
    for action in actions {
        if let DiffAction::Install { package, pin, .. } = action {
            println!("Installing {}...", package);
            match crate::commands::cmd_install(
                package,
                crate::commands::InstallOptions {
                    db_path,
                    root,
                    version: pin.clone(),
                    dry_run: false,
                    no_deps: false,
                    no_scripts: false,
                    selection_reason: Some("Installed by model apply"),
                    sandbox_mode: crate::commands::SandboxMode::Auto,
                    yes: true,
                    ..Default::default()
                },
            )
            .await
            {
                Ok(()) => {
                    println!("  Installed {}", package);
                    applied += 1;
                }
                Err(e) => {
                    let msg = format!("Install '{}': {}", package, e);
                    eprintln!("  [FAILED] {}", msg);
                    if strict {
                        anyhow::bail!(msg);
                    }
                    errors.push(msg);
                }
            }
        }
    }

    Ok((applied, errors))
}
```

- [ ] **Step 3: Fix `DiffAction::Update` in `apply_metadata_changes()`**

Change the return type to include deferred updates and update the `Update` arm (around line 545):

```rust
pub(super) fn apply_metadata_changes(
    conn: &Connection,
    actions: &[&DiffAction],
) -> (usize, Vec<String>, Vec<(String, String)>) {
    let mut applied = 0;
    let mut errors = Vec::new();
    let mut deferred_updates: Vec<(String, String)> = Vec::new();

    // ... existing Pin/Unpin/MarkExplicit/MarkDependency arms unchanged ...

    // Replace the Update arm:
    DiffAction::Update {
        package,
        current_version: _,
        target_version,
    } => {
        deferred_updates.push((package.clone(), target_version.clone()));
    }

    // ... rest unchanged ...

    (applied, errors, deferred_updates)
}
```

- [ ] **Step 4: Update `cmd_model_apply` call sites**

In `apps/conary/src/commands/model.rs`, update the Phase 3 and Phase 5 dispatches:

```rust
    // Phase 3: package changes (was stub, now real)
    let (pkg_applied, pkg_errors) =
        apply_package_changes(db_path, root, &actions, strict).await?;
    errors.extend(pkg_errors);

    // ... Phase 4 unchanged ...

    // Phase 5: metadata changes (now returns deferred updates)
    let (metadata_applied, metadata_errors, deferred_updates) =
        apply_metadata_changes(&conn, &actions);
    errors.extend(metadata_errors);

    // Execute deferred updates
    for (package, target_version) in &deferred_updates {
        println!("Updating {} to {}...", package, target_version);
        match crate::commands::cmd_install(
            package,
            crate::commands::InstallOptions {
                db_path,
                root,
                version: Some(target_version.clone()),
                allow_downgrade: true,
                selection_reason: Some("Updated by model apply"),
                yes: true,
                ..Default::default()
            },
        )
        .await
        {
            Ok(()) => println!("  Updated {}", package),
            Err(e) => {
                if strict {
                    anyhow::bail!("Update '{}': {}", package, e);
                }
                errors.push(format!("Update '{}': {}", package, e));
            }
        }
    }

    // Replace autoremove stub (line 572-574):
    if autoremove {
        if let Err(e) = crate::commands::cmd_autoremove(
            db_path,
            root,
            dry_run,
            false, // no_scripts
            crate::commands::SandboxMode::Auto,
        )
        .await
        {
            errors.push(format!("Autoremove: {}", e));
        }
    }
```

Update the summary section to include new counts and `strict` error handling:

```rust
    println!();
    println!("Summary:");
    if pkg_applied > 0 {
        println!("  Packages installed/removed: {}", pkg_applied);
    }
    if !deferred_updates.is_empty() {
        println!("  Packages updated: {}", deferred_updates.len());
    }
    if replatform_executed > 0 {
        println!("  Replatform replacements: {}", replatform_executed);
    }
    if derived_built > 0 {
        println!("  Derived packages built: {}", derived_built);
    }
    if derived_rebuilt > 0 {
        println!("  Derived packages rebuilt: {}", derived_rebuilt);
    }
    if metadata_applied > 0 {
        println!("  Metadata changes: {}", metadata_applied);
    }

    let total_applied = pkg_applied
        + deferred_updates.len()
        + replatform_executed
        + derived_built
        + derived_rebuilt
        + metadata_applied;

    if !errors.is_empty() {
        println!();
        eprintln!("Errors ({}):", errors.len());
        for err in &errors {
            eprintln!("  - {}", err);
        }
        if strict {
            anyhow::bail!("{} error(s) during model apply (strict mode)", errors.len());
        }
        if total_applied == 0 {
            anyhow::bail!("model apply failed: every package operation failed");
        }
    }

    Ok(())
```

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo build -p conary && cargo test -p conary`

Expected: Compiles, all tests pass.

- [ ] **Step 6: Commit**

```
git add apps/conary/src/commands/model/apply.rs apps/conary/src/commands/model.rs apps/conary/tests/model_apply.rs
git commit -m "feat(model): implement apply_package_changes, wire update and autoremove"
```

---

### Task 6: Implement `cmd_state_restore()` with Shared-Transaction Revert

This is the real atomic revert path for Phase 1: pre-resolve every install/upgrade,
then execute removals, installs, and upgrades under one outer DB transaction, one
wrapping changeset, one `rebuild_and_mount()`, and the single generation snapshot
that `rebuild_and_mount()` already creates.

**Files:**
- Modify: `apps/conary/src/commands/state.rs:215-219`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/prepare.rs`
- Modify: `apps/conary/src/commands/install/scriptlets.rs`
- Modify: `apps/conary/src/commands/install/conversion.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/test_helpers.rs`

- [ ] **Step 1: Write unit tests that fail on today's code**

First, move `setup_command_test_db()` from `apps/conary/tests/common/mod.rs`
into `apps/conary/src/commands/test_helpers.rs` as `pub(crate)` so command-unit
tests can reuse the same seeded fixture.

Then add tests at the bottom of `apps/conary/src/commands/state.rs` next to a
new private helper `execute_restore_plan_with_root(...)` so the tests can use a
temp root instead of `/`.

Write two tests:

```rust
#[tokio::test]
async fn test_state_restore_remove_only_executes_and_creates_one_changeset_and_snapshot() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();

    // Add one extra package after the baseline so restore has real work to do.
    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = conary_core::db::models::Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        vim.architecture = Some("x86_64".to_string());
        vim.installed_by_changeset_id = Some(cs_id);
        vim.insert(tx)?;
        cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();
    let _drifted = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();

    let before_changesets: i64 = conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0)).unwrap();
    let before_states: i64 = conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0)).unwrap();
    drop(conn);

    let result = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await;

    assert!(result.is_ok(), "remove-only restore should succeed: {:?}", result);

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert!(conary_core::db::models::Trove::find_one_by_name(&conn, "vim").unwrap().is_none());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get::<_, i64>(0)).unwrap(),
        before_changesets + 1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get::<_, i64>(0)).unwrap(),
        before_states + 1
    );
}

#[tokio::test]
async fn test_state_restore_missing_repo_version_rolls_back_without_snapshot() {
    let (_tmp, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine.create_snapshot("baseline", None, None).unwrap();

    // Create real drift so plan_restore() has work to do.
    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = conary_core::db::models::Changeset::new("Install vim-9.1.0".to_string());
        let cs_id = cs.insert(tx)?;
        let mut vim = conary_core::db::models::Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        vim.architecture = Some("x86_64".to_string());
        vim.installed_by_changeset_id = Some(cs_id);
        vim.insert(tx)?;
        cs.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();
    let drifted = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot("drifted", None, None)
        .unwrap();
    assert!(drifted.state_number > baseline.state_number);

    // Make the target state ask for a version that cannot be resolved.
    conn.execute(
        "UPDATE state_members SET trove_version = '9.9.9' WHERE state_id = ?1 AND trove_name = 'nginx'",
        [baseline.id.unwrap()],
    )
    .unwrap();

    let before_changesets: i64 = conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0)).unwrap();
    let before_states: i64 = conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get(0)).unwrap();
    drop(conn);

    let result = execute_restore_plan_with_root(
        &db_path,
        root.path().to_str().unwrap(),
        baseline.state_number,
        false,
    )
    .await;

    assert!(result.is_err(), "missing repo version should fail");

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get::<_, i64>(0)).unwrap(),
        before_changesets
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM system_states", [], |r| r.get::<_, i64>(0)).unwrap(),
        before_states
    );
}
```

These should fail on today's code because any non-empty restore plan still hits
the `"state restore is not yet implemented"` bail.

Also add a small rollback-compatibility test in `apps/conary/src/commands/system.rs`
that proves the rollback metadata parser accepts both:
- the existing single `TroveSnapshot` JSON used by current remove/upgrade rollback
- the new revert wrapper format introduced in this task

- [ ] **Step 2: Extract install preparation/finalization helpers for shared transactions**

In `apps/conary/src/commands/install/mod.rs`, replace the rejected
`'static`-context approach with owned restore-prep helpers extracted from the
existing `cmd_install` flow:

```rust
struct PreparedInstall {
    pkg: Box<dyn conary_core::packages::PackageFormat>,
    extraction: ExtractionResult,
    selection_reason: Option<String>,
    old_trove_to_upgrade: Option<conary_core::db::models::Trove>,
    semantics: InstallSemantics,
}

enum PreparedSourceKind {
    Legacy { format: PackageFormatType },
    Ccs,
}

struct InstallSemantics {
    source: PreparedSourceKind,
    version_scheme: conary_core::repository::versioning::VersionScheme,
    scriptlet_format: conary_core::scriptlet::PackageFormat,
}

struct TargetStateView {
    members: std::collections::HashSet<(String, Option<String>)>,
    // Capability/provider view for the destination state, built using the same
    // matching rules Conary already uses for tracked provides.
    provides: TargetProvidesView,
}

async fn prepare_install_for_restore(
    conn: &rusqlite::Connection,
    package: &str,
    opts: InstallOptions<'_>,
    target_state: &TargetStateView,
) -> Result<PreparedInstall>

fn run_pre_install_for_prepared(
    conn: &rusqlite::Connection,
    root: &str,
    no_scripts: bool,
    sandbox_mode: crate::commands::SandboxMode,
    prepared: &PreparedInstall,
    progress: &InstallProgress,
) -> Result<PreScriptletState>

fn finalize_install_without_snapshot(
    conn: &rusqlite::Connection,
    prepared: &PreparedInstall,
    pre_state: &PreScriptletState,
    tx_result: &InstallTransactionResult,
    root: &str,
    no_scripts: bool,
    sandbox_mode: crate::commands::SandboxMode,
    progress: &InstallProgress,
) -> Result<()>
```

Rules for `prepare_install_for_restore(...)`:
- reuse existing canonical resolution / policy setup and package parsing code
- support every current install resolution outcome: legacy package, Remi CCS, local `.ccs`, and converted CCS
- parse CCS packages into `conary_core::ccs::CcsPackage` and return them as `Box<dyn PackageFormat>` instead of executing `install_converted_ccs()`
- keep the helper pure with respect to the live system: do NOT call `handle_dep_adoptions()`, `handle_dep_installs()`, `cmd_adopt()`, `BatchInstaller::install_batch()`, or pre-install scriptlets during preflight
- validate dependencies against a capability-aware target-state view plus currently installed packages; do not key only on package names
- reuse Conary's existing tracked-provider semantics (`ProvideEntry::find_satisfying_provider_fuzzy`, `check_provides_dependencies`, or an equivalent provider view), so restore preflight handles capabilities like `soname(...)` the same way normal installs do
- if the destination state requires a package whose provides are only known after extraction/preparation, add those provides to the in-memory target-state view before validating later prepared packages
- if the target state cannot satisfy a dependency, return `Err` before lock acquisition
- keep `PreparedInstall` fully owned; construct `TransactionContext` and `ScriptletContext` on the stack when executing, never as `'static` fields
- implement the ingress/native split explicitly: legacy formats are parsed or converted during preparation, while shared execution consumes `InstallSemantics` and never branches on “legacy vs CCS” for transaction ownership
- refactor any helper that currently requires raw `PackageFormatType` (`check_upgrade_status`, `version_scheme_for_format`, `to_scriptlet_format`, `TransactionContext`, `ScriptletContext`) so it can consume `InstallSemantics` instead
- if a helper truly needs source-specific behavior, hang that off `PreparedSourceKind` inside `InstallSemantics` rather than reintroducing a second top-level install pipeline

`run_pre_install_for_prepared(...)` is the extracted current pre-install phase so
restore can run pre-install scriptlets after the mutation lock is held but before
the shared DB transaction commits.

`finalize_install_without_snapshot(...)` must be an extraction of the current
`finalize_install(...)` body minus the final `create_state_snapshot(...)` call.
Then `finalize_install(...)` becomes a thin wrapper that calls
`finalize_install_without_snapshot(...)`; snapshot creation remains owned by
`rebuild_and_mount()`.

- [ ] **Step 3: Implement the shared-transaction restore executor**

In `apps/conary/src/commands/state.rs`, add a private helper:

```rust
async fn execute_restore_plan_with_root(
    db_path: &str,
    root: &str,
    state_number: i64,
    dry_run: bool,
) -> Result<()>
```

Make public `cmd_state_restore(...)` a thin wrapper:

```rust
pub async fn cmd_state_restore(db_path: &str, state_number: i64, dry_run: bool) -> Result<()> {
    execute_restore_plan_with_root(db_path, "/", state_number, dry_run).await
}
```

Implementation requirements for `execute_restore_plan_with_root(...)`:

- Load the target state and restore plan exactly as today.
- If `dry_run`, keep the existing early return.
- Open the DB and build a `TargetStateView` for the destination state. This must include both destination members and a capability/provider view compatible with Conary's existing provides matching rules.
- Pre-resolve every install/upgrade with `prepare_install_for_restore(...)`. This preflight must:
  - return a fully owned `PreparedInstall` for legacy or CCS artifacts
  - validate dependencies against the capability-aware destination-state view and current tracked packages
  - fail before lock acquisition on missing versions or unsatisfied dependencies
  - avoid `cmd_adopt()`, `BatchInstaller::install_batch()`, and any filesystem mutation
  - produce `InstallSemantics` for every prepared package so the execution phase is format-neutral
- Re-open the DB, create one `TransactionEngine`, call `recover()`, then `begin()`.
- Collect `prev_etc`, create one wrapping changeset, and open one outer DB transaction.
- For each `plan.to_remove`, call `remove_inner(...)` under the outer transaction and collect the returned `TroveSnapshot`s in memory. Do not append newline-delimited JSON into `changesets.metadata`.
- Serialize those removed troves into a rollback-compatible revert metadata wrapper and persist it on the wrapping changeset before commit.
- Extend rollback in `apps/conary/src/commands/system.rs` so it accepts either:
  - the legacy single `TroveSnapshot` JSON used by existing remove/upgrade changesets
  - the new revert metadata wrapper containing `removed_troves: Vec<TroveSnapshot>`
- For rollback of a revert changeset, use the same Conary-shaped behavior as today:
  - remove any troves installed by the revert changeset via `installed_by_changeset_id`
  - restore every removed trove from the revert metadata
  - mark the original revert changeset `rolled_back`
- For each prepared install/upgrade:
  - build stack-local `TransactionContext` and `ScriptletContext` values from the owned `PreparedInstall` + `InstallSemantics`
  - run `run_pre_install_for_prepared(...)` after the lock is held
  - call `install_inner(...)` under the same outer transaction
  - retain `(prepared, pre_state, tx_result, progress)` for post-commit finalization
- On any failure before commit: roll back the outer transaction, release the lock, and return `Err`.
- Commit DB state once.
- Post-commit:
  - call `rebuild_and_mount(...)` once for the full revert; this already creates the new active `system_states` snapshot through `build_generation_from_db()`
  - mark the wrapping changeset `Applied` only after rebuild succeeds
  - call `finalize_install_without_snapshot(...)` for each install/upgrade
  - do not call `create_state_snapshot()` afterward
- Release the mutation lock on both success and failure paths.

Do not keep a partial-success path in restore for DB/generation state. This command
should either fully succeed or roll back the outer transaction and return `Err`.
Keep the existing Conary caveat explicit: pre/post scriptlet side effects are not
fully rollback-safe, just as current remove/upgrade scriptlets are not.

- [ ] **Step 4: Verify compilation and targeted tests**

Run: `cargo build -p conary && cargo test -p conary test_state_restore_remove_only_executes_and_creates_one_changeset_and_snapshot test_state_restore_missing_repo_version_rolls_back_without_snapshot && cargo test -p conary rollback_`

Expected: Compiles, both new tests pass.

- [ ] **Step 5: Commit**

```
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/prepare.rs apps/conary/src/commands/install/scriptlets.rs apps/conary/src/commands/install/conversion.rs apps/conary/src/commands/state.rs apps/conary/src/commands/system.rs apps/conary/src/commands/test_helpers.rs
git commit -m "feat(state): implement shared-transaction state revert"
```

---

### Task 7: Final Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p conary && cargo test -p conary-core`

Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p conary -- -D warnings`

Expected: No warnings.

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt --check`

Expected: No formatting issues.

- [ ] **Step 4: Verify help text is clean**

Run: `cargo run -p conary -- model apply --help 2>&1 | grep -i "not implemented" ; cargo run -p conary -- system state revert --help 2>&1 | grep -i "not implemented"`

Expected: No output (no "not implemented" in help text).

- [ ] **Step 5: Commit any final fixes**

```
git add -A && git commit -m "chore: Phase 1 final cleanup"
```
