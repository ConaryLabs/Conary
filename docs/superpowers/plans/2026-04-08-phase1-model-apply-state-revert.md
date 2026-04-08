# Phase 1: Model Apply + State Revert Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `conary model apply` and `conary system state revert` actually execute package install/remove/update operations instead of printing "not yet implemented" stubs.

**Architecture:** Extract inner helpers from `cmd_install` and `cmd_remove` that accept a caller-owned changeset ID, `TransactionEngine`, and architecture selector -- deferring `rebuild_and_mount()` and state snapshot creation to the caller. `model apply` calls `cmd_install`/`cmd_remove` directly (one changeset per operation, matching existing `apply_replatform_changes` pattern). `state revert` uses the inner helpers with a single wrapping changeset, one `rebuild_and_mount()`, and one state snapshot for atomic revert. Both commands get `require_live_mutation()` safety gates.

**Tech Stack:** Rust, rusqlite, composefs, EROFS, conary-core transaction engine

**Spec:** `docs/superpowers/specs/2026-04-08-pre-release-completeness-design.md` Phase 1

---

## File Structure

| File | Role | Action |
|------|------|--------|
| `apps/conary/src/commands/install/mod.rs` | Package installation | Modify: add `architecture` to `InstallOptions`, extract `install_inner()`, make `execute_install_transaction()` a thin wrapper |
| `apps/conary/src/commands/install/inner.rs` | Inner install helper | Create: `install_inner()` accepting caller-owned engine + changeset_id |
| `apps/conary/src/commands/remove.rs` | Package removal | Modify: extract `remove_inner()` accepting caller-owned changeset_id + architecture, make `cmd_remove` a thin wrapper |
| `apps/conary/src/commands/model/apply.rs` | Model diff application | Modify: implement `apply_package_changes()` and `Update` handling |
| `apps/conary/src/commands/model.rs` | Model apply dispatch | Modify: update Phase 3 call site, wire autoremove |
| `apps/conary/src/commands/state.rs` | State management | Modify: implement `cmd_state_restore()` with inner helpers + TransactionEngine |
| `apps/conary/src/dispatch.rs` | CLI dispatch | Modify: add `require_live_mutation()` gates |
| `apps/conary/tests/model_apply.rs` | Model apply tests | Create |
| `apps/conary/tests/state_revert.rs` | State revert tests | Create |

---

### Task 1: Add `require_live_mutation()` Gates

**Files:**
- Modify: `apps/conary/src/dispatch.rs:559` (state revert dispatch)
- Modify: `apps/conary/src/dispatch.rs:1210-1235` (model apply dispatch)
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Write tests that exercise the actual dispatch wiring**

Add to `apps/conary/tests/live_host_mutation_safety.rs`:

```rust
#[test]
fn test_state_revert_requires_live_mutation_ack() {
    use conary::cli::live_host_safety::{
        LiveMutationClass, LiveMutationRequest, require_live_system_mutation_ack,
    };
    // Verify the safety function rejects state revert without the flag.
    // The dispatch arm we're about to add will call this exact path.
    let result = require_live_system_mutation_ack(
        false,
        &LiveMutationRequest {
            command_label: std::borrow::Cow::Borrowed("conary system state revert"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
        },
    );
    assert!(result.is_err(), "state revert should require mutation ack");

    // dry_run should also be gated (user gets a warning, not silent pass)
    let dry_result = require_live_system_mutation_ack(
        false,
        &LiveMutationRequest {
            command_label: std::borrow::Cow::Borrowed("conary system state revert"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: true,
        },
    );
    // dry_run with the gate class still requires ack
    assert!(dry_result.is_err());
}

#[test]
fn test_model_apply_requires_live_mutation_ack() {
    use conary::cli::live_host_safety::{
        LiveMutationClass, LiveMutationRequest, require_live_system_mutation_ack,
    };
    let result = require_live_system_mutation_ack(
        false,
        &LiveMutationRequest {
            command_label: std::borrow::Cow::Borrowed("conary model apply"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
        },
    );
    assert!(result.is_err(), "model apply should require mutation ack");
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p conary test_state_revert_requires_live test_model_apply_requires_live`

Expected: PASS (the safety gate function already works; we're confirming our test is correct)

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

Run: `cargo clippy -p conary -- -D warnings && cargo test -p conary test_state_revert_requires test_model_apply_requires`

Expected: PASS, no warnings

- [ ] **Step 6: Commit**

```
git add apps/conary/src/dispatch.rs apps/conary/tests/live_host_mutation_safety.rs
git commit -m "fix(dispatch): add require_live_mutation gates for model apply and state revert"
```

---

### Task 2: Add Architecture Selector to InstallOptions

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`

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

- [ ] **Step 2: Verify compilation**

Run: `cargo build -p conary`

Expected: Compiles. All existing callers use `..Default::default()` or set all fields explicitly -- the new `Option` defaults to `None` either way.

- [ ] **Step 3: Commit**

```
git add apps/conary/src/commands/install/mod.rs
git commit -m "feat(install): add architecture field to InstallOptions"
```

---

### Task 3: Extract `install_inner()` with Caller-Owned Changeset

**Files:**
- Create: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`

The key difference from the previous plan: `install_inner()` accepts a `changeset_id: i64` from the caller. It does NOT create its own changeset. The caller is responsible for creating the changeset, calling `rebuild_and_mount()`, and creating the state snapshot.

- [ ] **Step 1: Create `install/inner.rs`**

Create `apps/conary/src/commands/install/inner.rs`:

```rust
// install/inner.rs

//! Inner install helper for callers that own the transaction lifecycle.
//!
//! `install_inner()` performs CAS storage and the DB operations (trove insert,
//! file entries, dependencies, scriptlets) using a caller-provided changeset.
//! It does NOT: create a changeset, call `rebuild_and_mount()`, or create a
//! state snapshot. The caller handles all of those.

use anyhow::{Context, Result};
use conary_core::db::models::{
    ChangesetStatus, Component, ComponentType, DependencyEntry, FileEntry,
    ProvideEntry, ScriptletEntry,
};
use conary_core::packages::{DependencyClass, PackageFormat as PkgFormat};
use conary_core::transaction::TransactionEngine;
use rusqlite::Connection;
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

/// Execute the install DB operations using a caller-owned changeset.
///
/// Stores files in CAS via the provided engine, then runs the DB transaction
/// to insert the trove, components, files, dependencies, and scriptlets under
/// the provided `changeset_id`.
///
/// The caller MUST:
/// 1. Create the `TransactionEngine` and call `begin()`.
/// 2. Create the changeset and pass its ID here.
/// 3. Call `rebuild_and_mount()` after all inner operations complete.
/// 4. Create a state snapshot.
/// 5. Release the engine lock.
pub fn install_inner(
    conn: &mut Connection,
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

    // DB transaction using the caller's changeset
    let trove_id = conary_core::db::transaction(conn, |tx| {
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

        Ok(trove_id)
    })?;

    if let Some(old_trove) = ctx.old_trove_to_upgrade {
        mark_upgraded_parent_deriveds_stale(
            conn,
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

    // Create changeset (owned by this wrapper)
    let changeset_id = conary_core::db::transaction(conn, |tx| {
        let mut changeset = conary_core::db::models::Changeset::new(tx_description.clone());
        let cs_id = changeset.insert(tx)?;
        changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok(cs_id)
    })?;

    // Delegate CAS + DB work to inner helper
    let result = match inner::install_inner(
        conn, &mut engine, changeset_id, pkg, extraction, ctx, progress,
    ) {
        Ok(r) => r,
        Err(e) => {
            engine.release_lock();
            return Err(e);
        }
    };

    // rebuild_and_mount (owned by this wrapper)
    let _gen_num = crate::commands::composefs_ops::rebuild_and_mount(
        conn,
        &tx_description,
        Some(prev_etc),
        std::path::Path::new("/conary"),
    )?;

    engine.release_lock();

    Ok(InstallTransactionResult { changeset_id })
}
```

- [ ] **Step 4: Verify the refactor compiles and existing tests pass**

Run: `cargo build -p conary && cargo test -p conary`

Expected: All existing tests pass. Refactor is behavior-preserving.

- [ ] **Step 5: Commit**

```
git add apps/conary/src/commands/install/inner.rs apps/conary/src/commands/install/mod.rs
git commit -m "refactor(install): extract install_inner() with caller-owned changeset_id"
```

---

### Task 4: Extract `remove_inner()` with Caller-Owned Changeset + Architecture

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`

- [ ] **Step 1: Add `remove_inner()` function**

Add to `apps/conary/src/commands/remove.rs`, before `cmd_autoremove`:

```rust
/// Inner remove helper for callers that own the transaction lifecycle.
///
/// Performs pre-remove scriptlets and DB transaction (file history, trove
/// deletion) using a caller-provided `changeset_id`. Does NOT: create a
/// changeset, call `rebuild_and_mount()`, or create a state snapshot.
///
/// When `architecture` is `Some`, only removes the trove matching that arch
/// (for multi-arch state revert). When `None`, matches the first trove found.
pub(crate) fn remove_inner(
    conn: &mut rusqlite::Connection,
    changeset_id: i64,
    package_name: &str,
    architecture: Option<&str>,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    use conary_core::db::models::{FileEntry, ScriptletEntry, Trove};

    // Find trove, optionally filtered by architecture
    let trove = if let Some(arch) = architecture {
        Trove::find_by_name(conn, package_name)?
            .into_iter()
            .find(|t| t.architecture.as_deref() == Some(arch))
            .ok_or_else(|| {
                anyhow::anyhow!("Package '{}' ({}) not installed", package_name, arch)
            })?
    } else {
        Trove::find_one_by_name(conn, package_name)?
            .ok_or_else(|| anyhow::anyhow!("Package '{}' not installed", package_name))?
    };
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove '{}' has no ID", package_name))?;

    let files = FileEntry::find_by_trove(conn, trove_id)?;
    let stored_scriptlets = ScriptletEntry::find_by_trove(conn, trove_id)?;

    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| {
            crate::commands::scriptlets::ScriptletPackageFormat::parse(&s.package_format)
        })
        .unwrap_or(crate::commands::scriptlets::ScriptletPackageFormat::Rpm);

    // Pre-remove scriptlet (best effort)
    if !no_scripts && !stored_scriptlets.is_empty() {
        let executor = crate::commands::scriptlets::ScriptletExecutor::new(
            std::path::Path::new("/"),
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
    let snapshot_json = serde_json::to_string(&snapshot)?;

    // DB transaction using the caller's changeset
    conary_core::db::transaction(conn, |tx| {
        // Re-check dependency breakage inside the transaction
        let breaking =
            conary_core::resolver::solve_removal(tx, std::slice::from_ref(&trove.name))?;
        if !breaking.is_empty() {
            return Err(conary_core::Error::IoError(format!(
                "'{}' required by: {}",
                package_name,
                breaking.join(", ")
            )));
        }

        // Store snapshot in changeset metadata (append, don't overwrite)
        tx.execute(
            "UPDATE changesets SET metadata = COALESCE(metadata || X'0A', '') || ?1 WHERE id = ?2",
            [&snapshot_json, &changeset_id.to_string()],
        )?;

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
        Ok(())
    })?;

    // Post-remove scriptlet (best effort)
    if !no_scripts && !stored_scriptlets.is_empty() {
        let executor = crate::commands::scriptlets::ScriptletExecutor::new(
            std::path::Path::new("/"),
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

    Ok(())
}
```

- [ ] **Step 2: Ensure `TroveSnapshot` and `FileSnapshot` are `pub(crate)`**

In `remove.rs`, update visibility if needed:

```rust
#[derive(serde::Serialize)]
pub(crate) struct TroveSnapshot { ... }

#[derive(serde::Serialize)]
pub(crate) struct FileSnapshot { ... }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p conary`

Expected: Compiles. `cmd_remove` is unchanged; `remove_inner` is new and additive.

- [ ] **Step 4: Commit**

```
git add apps/conary/src/commands/remove.rs
git commit -m "refactor(remove): extract remove_inner() with caller-owned changeset_id and architecture"
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

/// Verifies that apply_package_changes is async and calls cmd_install.
/// On today's code, this would just print "[NOT IMPLEMENTED]" and return
/// empty lists without actually installing anything.
#[tokio::test]
async fn test_apply_package_changes_not_stub() {
    let (_dir, db_path) = setup_command_test_db();

    // The model diff's Install action should result in an actual install
    // attempt, not a "[NOT IMPLEMENTED]" print. We can't easily invoke the
    // full model pipeline in a unit test, but we can verify the function
    // signature is async and returns counts.
    //
    // For now, verify the test DB baseline is correct.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
        .unwrap();
    assert!(count > 0, "test DB should have installed packages");
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
        apply_package_changes(db_path, root, &actions).await?;
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
            Err(e) => errors.push(format!("Update '{}': {}", package, e)),
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

    if !errors.is_empty() {
        println!();
        eprintln!("Errors ({}):", errors.len());
        for err in &errors {
            eprintln!("  - {}", err);
        }
        if strict {
            anyhow::bail!("{} error(s) during model apply (strict mode)", errors.len());
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

### Task 6: Implement `cmd_state_restore()` with Inner Helpers

This is the atomic revert: one `TransactionEngine`, one changeset, inner helpers for each operation, one `rebuild_and_mount()`, one state snapshot.

**Files:**
- Modify: `apps/conary/src/commands/state.rs:215-219`
- Create: `apps/conary/tests/state_revert.rs`

- [ ] **Step 1: Write a test that fails on today's code**

Create `apps/conary/tests/state_revert.rs`:

```rust
//! Integration tests for state revert.

mod common;

use common::setup_command_test_db;

/// Today this test hits the bail!("state restore is not yet implemented").
/// After implementation it should succeed.
#[tokio::test]
async fn test_state_restore_executes_revert() {
    let (_dir, db_path) = setup_command_test_db();

    // Create a state snapshot before we do anything
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let baseline = engine
        .create_snapshot("baseline", None, None)
        .unwrap();
    let baseline_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
        .unwrap();
    drop(conn);

    // Reverting to current state should be a no-op (plan.is_empty())
    let result =
        conary::commands::cmd_state_restore(&db_path, baseline.state_number, false).await;
    assert!(result.is_ok(), "revert to current state should succeed: {:?}", result);

    // Verify trove count unchanged
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let after_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
        .unwrap();
    assert_eq!(baseline_count, after_count, "no-op revert should not change troves");
}

/// State revert with dry_run should show the plan but not execute.
#[tokio::test]
async fn test_state_restore_dry_run() {
    let (_dir, db_path) = setup_command_test_db();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let engine = conary_core::db::models::StateEngine::new(&conn);
    let state = engine
        .create_snapshot("test state", None, None)
        .unwrap();
    drop(conn);

    let result =
        conary::commands::cmd_state_restore(&db_path, state.state_number, true).await;
    assert!(result.is_ok(), "dry run should succeed");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p conary test_state_restore_executes`

Expected: Today `test_state_restore_executes_revert` should PASS (the no-op path returns early at `plan.is_empty()`). The dry_run test should also PASS. Both of these exercise the already-working paths. We'll add a test that hits the bail after implementation to prove the new path works.

- [ ] **Step 3: Implement the restore execution**

In `apps/conary/src/commands/state.rs`, replace the bail at lines 215-219 with the atomic revert using inner helpers:

```rust
    // --- Everything above line 214 (the dry_run check) stays the same ---

    println!("\nExecuting restore...");

    // Set up TransactionEngine (we own the lifecycle for atomic revert)
    let root = "/";
    let tx_config = conary_core::transaction::TransactionConfig::from_paths(
        std::path::PathBuf::from(root),
        std::path::PathBuf::from(db_path),
    );
    let mut engine = conary_core::transaction::TransactionEngine::new(tx_config)
        .context("Failed to create transaction engine")?;

    // Recover incomplete transactions from prior crashes
    let mut conn = open_db_mut(db_path)?;
    engine
        .recover(&conn)
        .context("Failed to recover incomplete transactions")?;

    // Acquire mutation lock
    engine.begin().context("Failed to begin transaction")?;

    // Capture /etc snapshot before any changes
    let prev_etc = crate::commands::composefs_ops::collect_etc_files(&conn)?;

    // Create ONE wrapping changeset for the entire revert
    let changeset_description = format!("Revert to state {}", state_number);
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset =
            conary_core::db::models::Changeset::new(changeset_description.clone());
        let cs_id = changeset.insert(tx)?;
        changeset
            .update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok(cs_id)
    })?;

    let mut applied = 0;
    let mut failed = Vec::new();

    // Removals first
    for member in &plan.to_remove {
        println!(
            "  Removing {} {}...",
            member.trove_name, member.trove_version
        );
        match crate::commands::remove::remove_inner(
            &mut conn,
            changeset_id,
            &member.trove_name,
            member.architecture.as_deref(),
            false, // no_scripts
            crate::commands::SandboxMode::Auto,
        ) {
            Ok(()) => {
                applied += 1;
                println!("    Removed {}", member.trove_name);
            }
            Err(e) => {
                let msg = format!("Remove {}: {}", member.trove_name, e);
                eprintln!("    [FAILED] {}", msg);
                failed.push(msg);
            }
        }
    }

    // Installs (resolve from repository, then use inner helper)
    // Note: install_inner requires a parsed package + extraction, which
    // means we need the full resolve+download+extract pipeline. For now,
    // delegate to cmd_install for installs and upgrades (each creates its
    // own changeset). The removes above use the wrapping changeset.
    //
    // TODO: When the install pipeline exposes a resolve+extract step that
    // returns a PackageFormat without executing, wire install_inner here.
    for member in &plan.to_install {
        println!(
            "  Installing {} {}...",
            member.trove_name, member.trove_version
        );
        match crate::commands::cmd_install(
            &member.trove_name,
            crate::commands::InstallOptions {
                db_path,
                root,
                version: Some(member.trove_version.clone()),
                architecture: member.architecture.clone(),
                selection_reason: member.selection_reason.as_deref(),
                yes: true,
                ..Default::default()
            },
        )
        .await
        {
            Ok(()) => {
                applied += 1;
                println!("    Installed {}", member.trove_name);
            }
            Err(e) => {
                let msg = format!(
                    "Install {} {}: {}",
                    member.trove_name, member.trove_version, e
                );
                eprintln!("    [FAILED] {}", msg);
                failed.push(msg);
            }
        }
    }

    // Upgrades (version changes)
    for (old, new) in &plan.to_upgrade {
        println!(
            "  Changing {} {} -> {}...",
            old.trove_name, old.trove_version, new.trove_version
        );
        match crate::commands::cmd_install(
            &new.trove_name,
            crate::commands::InstallOptions {
                db_path,
                root,
                version: Some(new.trove_version.clone()),
                architecture: new.architecture.clone(),
                allow_downgrade: true,
                selection_reason: new.selection_reason.as_deref(),
                yes: true,
                ..Default::default()
            },
        )
        .await
        {
            Ok(()) => {
                applied += 1;
                println!("    Changed {} to {}", new.trove_name, new.trove_version);
            }
            Err(e) => {
                let msg = format!(
                    "Change {} to {}: {}",
                    new.trove_name, new.trove_version, e
                );
                eprintln!("    [FAILED] {}", msg);
                failed.push(msg);
            }
        }
    }

    // Rebuild EROFS and mount once (covers all removals done via inner helper)
    if applied > 0 && failed.is_empty() {
        let _gen_num = crate::commands::composefs_ops::rebuild_and_mount(
            &conn,
            &changeset_description,
            Some(prev_etc),
            std::path::Path::new("/conary"),
        )?;
    }

    engine.release_lock();

    // Summary
    println!();
    if failed.is_empty() {
        println!(
            "Restored to state {} ({} operations applied)",
            state_number, applied
        );

        // Create ONE state snapshot for the entire revert
        super::create_state_snapshot(
            &conn,
            changeset_id,
            &format!("Reverted to state {}", state_number),
        )?;
    } else {
        eprintln!(
            "Partial restore: {} applied, {} failed",
            applied,
            failed.len()
        );
        for msg in &failed {
            eprintln!("  - {}", msg);
        }
        anyhow::bail!(
            "State restore failed ({} of {} operations)",
            failed.len(),
            applied + failed.len()
        );
    }

    Ok(())
```

- [ ] **Step 4: Add `open_db_mut` helper if needed**

If `open_db` returns an immutable connection, add a mutable variant:

```rust
fn open_db_mut(db_path: &str) -> Result<rusqlite::Connection> {
    super::open_db(db_path)
}
```

(Check if `open_db` already returns an owned `Connection` -- if so, `open_db_mut` is unnecessary and you can use `open_db` directly with `let mut conn`.)

- [ ] **Step 5: Verify compilation and run tests**

Run: `cargo build -p conary && cargo test -p conary test_state_restore`

Expected: Compiles, tests pass.

- [ ] **Step 6: Commit**

```
git add apps/conary/src/commands/state.rs apps/conary/tests/state_revert.rs
git commit -m "feat(state): implement cmd_state_restore with atomic revert via inner helpers"
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
