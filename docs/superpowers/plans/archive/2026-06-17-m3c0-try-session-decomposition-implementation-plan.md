# M3c0 Try-Session Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose the 3065-line `try_session.rs` command module into focused try-session modules while preserving CLI behavior, one-active-session safety, and reviewed tiny behavior fixes.

**Architecture:** Keep `apps/conary/src/commands/try_session/mod.rs` as the narrow command-facing API and move ownership into `validation.rs`, `install.rs`, `session.rs`, `namespace.rs`, `executor.rs`, and private `util.rs` only when needed to avoid sibling coupling. Add characterization coverage before code moves; tiny behavior fixes are isolated into named commits with focused tests.

**Tech Stack:** Rust 2024, `anyhow`, `rusqlite`, `tempfile`, `uuid`, existing `conary-core` CCS manifest/hook APIs, existing `TrySession` DB model, existing composefs generation helpers, existing CLI dispatch tests.

---

## Scope Locks

M3c0 includes:

- Replacing `apps/conary/src/commands/try_session.rs` with `apps/conary/src/commands/try_session/`.
- A narrow crate-facing allowlist:
  - `cmd_try_package`
  - `cmd_try_status`
  - `cmd_try_rollback`
  - `cmd_try_keep`
  - `rollback_active_try_session`
  - `begin_try_session`
  - `TryStartRequest`
  - `TryStartOutcome`
  - `current_boot_id`
  - `namespace_try_session_is_decision_pending`
  - `activated_try_session_is_live`
- Moving active/orphan liveness helpers and test-aware boot-id lookup into try-session ownership.
- Keeping `env_forces_non_interactive` and preflight orchestration in `dispatch/root.rs`.
- Preserving validation-before-active-session-row behavior.
- Preserving declarative hook pre-before-post ordering and failure messages.
- Accepted tiny fixes:
  - canonical test-aware `current_boot_id`
  - `TrySession` model helper for launcher clearing / boot-only recording, if used
  - previous current-generation restoration when keep fails after publishing the try generation link

M3c0 excludes:

- `conary try --watch`
- debounce, file watching, cook orchestration, record-mode traces, or streaming events
- schema migrations
- public CLI output redesign
- relaxing one-active-session enforcement
- exposing namespace/install/executor internals for future watch code

## File Structure

Create:

- `apps/conary/src/commands/try_session/mod.rs`: command-facing surface, request/outcome types, module declarations, narrow re-exports.
- `apps/conary/src/commands/try_session/validation.rs`: `TryExecutionRoot` and package/manifest policy validation.
- `apps/conary/src/commands/try_session/install.rs`: scratch install plan, transaction config, copied-package installation.
- `apps/conary/src/commands/try_session/session.rs`: begin/keep/rollback orchestration, DB copy/promotion/restore, liveness helpers, canonical boot id.
- `apps/conary/src/commands/try_session/namespace.rs`: namespace root exposure, hook upperdir, declarative try hooks, mounts/unmounts, mountinfo, test materialization, root-relative path checks.
- `apps/conary/src/commands/try_session/executor.rs`: command launching, bubblewrap/activated execution, launcher liveness record/clear.
- `apps/conary/src/commands/try_session/util.rs`: private shared path/SQLite helpers only if both `session.rs` and `namespace.rs` need them.

Modify:

- `apps/conary/src/commands/mod.rs`: keep `pub(crate) mod try_session;` and the same re-export names.
- `apps/conary/src/dispatch/root.rs`: import try-session liveness helpers and canonical boot id; keep preflight orchestration and non-interactive policy local.
- `crates/conary-core/src/db/models/try_session.rs`: add launcher helper methods only if replacing command-layer raw SQL.
- `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`: mark M3c0 landed after implementation.
- `docs/llms/subsystem-map.md`: route try-session work to the new directory.
- `docs/modules/feature-ownership.md`: update packaging ownership card and proofs.

Behavior-gate mapping:

| Gate | Existing coverage before edits | Added or changed by this plan |
|------|-------------------------------|-------------------------------|
| Begin creates active session, copied artifact, namespace root, try generation | `commands::try_session::tests::namespace_try_start_creates_active_session_and_copied_artifact`; `apps/conary/tests/packaging_m1b.rs::try_package_creates_session` | No new test unless mapping finds a gap |
| Validation refusal before active row | `commands::try_session::tests::namespace_try_start_rejects_unsupported_declarative_hook_classes_before_session` | Preserve through move |
| Rollback cleanup/retryability | `namespace_rollback_marks_rolled_back_and_removes_work_dir`; `namespace_rollback_unmounts_namespace_before_generation_root`; `namespace_rollback_leaves_session_retryable_when_unmount_fails`; `namespace_rollback_leaves_session_retryable_when_work_dir_removal_fails`; integration `try_rollback_clears_session` | No new test unless mapping finds a gap |
| Keep promotion and DB restore | `namespace_keep_publishes_try_generation_and_marks_kept`; `namespace_keep_restores_live_db_after_post_backup_failure`; integration `try_keep_promotes_generation` | Add post-current-link recovery test |
| One-active refusal | `namespace_try_start_with_active_session_errors_with_active_id`; integration `try_package_creates_session` second try assertion | Preserve through move |
| Orphan/liveness preflight | `dispatch::root` tests for namespace/activated live/orphaned sessions | Move pure helpers and rerun dispatch tests |
| Launcher liveness | `namespace_launcher_executes_bubblewrap_when_available`; `try_command_records_child_liveness_before_wait_and_clears_after_exit` | Extend boot-id assertion after canonical helper |
| Boot identity | Dispatch tests use `CONARY_TEST_BOOT_ID`; command-side boot recording lacks override | Add failing tests, then fix helper |
| Hook policy | Existing policy matrix tests in `commands::try_session` | Preserve through move |
| Declarative hook execution | Existing effect-location tests | Add pre-failure and post-failure characterization tests |

Focused verification commands:

```bash
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib dispatch::root
cargo test -p conary --test packaging_m1b
cargo fmt --check
```

Conditional verification when `crates/conary-core/src/db/models/try_session.rs` changes:

```bash
cargo test -p conary-core db::models::try_session
```

Merge gate:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

---

### Task 1: Add Pre-Move Characterization Tests

**Files:**
- Modify: `apps/conary/src/commands/try_session.rs`
- Test: `apps/conary/src/commands/try_session.rs`

- [ ] **Step 1: Add declarative hook execution characterization tests**

Add these tests inside `#[cfg(test)] mod tests` in `apps/conary/src/commands/try_session.rs`, near `declarative_try_hooks_refuse_host_root`:

```rust
#[test]
fn declarative_try_hooks_abort_post_hooks_when_pre_hooks_fail() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let mut manifest = CcsManifest::new_minimal("bad-pre-hook", "1.0.0");
    manifest.hooks.users.push(UserHook {
        name: "BadName!".to_string(),
        system: true,
        home: None,
        shell: Some("/usr/sbin/nologin".to_string()),
        group: None,
        reversible: None,
    });
    manifest.hooks.sysctl.push(SysctlHook {
        key: "kernel.modules_disabled".to_string(),
        value: "1".to_string(),
        only_if_lower: false,
        reversible: None,
    });

    let err = apply_declarative_try_hooks(&manifest, temp.path())
        .expect_err("pre-hook failure should abort try hook execution");
    let message = format!("{err:#}");

    assert!(
        message.contains("failed to execute try declarative pre-hooks"),
        "{message}"
    );
    assert!(
        !temp.path().join("etc/sysctl.d").exists(),
        "post-hook sysctl config must not be written after pre-hook failure"
    );
    Ok(())
}

#[test]
fn declarative_try_hooks_collect_post_hook_failures() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let mut manifest = CcsManifest::new_minimal("bad-post-hooks", "1.0.0");
    manifest.hooks.sysctl.push(SysctlHook {
        key: "kernel.modules_disabled".to_string(),
        value: "1".to_string(),
        only_if_lower: false,
        reversible: None,
    });
    manifest.hooks.alternatives.push(AlternativeHook {
        name: "bad/name".to_string(),
        path: "/usr/bin/demo".to_string(),
        priority: 50,
        reversible: None,
    });

    let err = apply_declarative_try_hooks(&manifest, temp.path())
        .expect_err("post-hook failures should be collected");
    let message = format!("{err:#}");

    assert!(
        message.contains("failed to execute try declarative post-hooks"),
        "{message}"
    );
    assert!(
        message.contains("sysctl 'kernel.modules_disabled' failed"),
        "{message}"
    );
    assert!(
        message.contains("alternatives 'bad/name' failed"),
        "{message}"
    );
    Ok(())
}
```

- [ ] **Step 2: Run only the new characterization tests**

Run:

```bash
cargo test -p conary --lib declarative_try_hooks_
```

Expected: both tests pass on the monolith before any move.

- [ ] **Step 3: Run the existing validation-before-session test**

Run:

```bash
cargo test -p conary --lib namespace_try_start_rejects_unsupported_declarative_hook_classes_before_session
```

Expected: pass. This proves the implementation still refuses invalid packages before opening an active session row.

- [ ] **Step 4: Commit characterization coverage**

```bash
git add apps/conary/src/commands/try_session.rs
git commit -m "test(try): characterize declarative hook execution"
```

### Task 2: Add Failing Tests For Accepted Tiny Fixes

**Files:**
- Modify: `apps/conary/src/commands/try_session.rs`
- Modify: `crates/conary-core/src/db/models/try_session.rs`
- Test: `apps/conary/src/commands/try_session.rs`
- Test: `crates/conary-core/src/db/models/try_session.rs`

- [ ] **Step 1: Add model tests for launcher clearing and boot-only recording**

In `crates/conary-core/src/db/models/try_session.rs`, add these tests near `set_launcher_records_process_and_boot_identity`:

```rust
#[test]
fn clear_launcher_clears_process_identity_on_open_session() {
    let (_temp, conn) = create_test_db();
    let session = create_namespace_session(&conn, "try-a");
    session.set_launcher(&conn, 4242, "boot-123").unwrap();
    force_old_updated_at(&conn, "try-a");

    session.clear_launcher(&conn).unwrap();

    let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
    assert_eq!(stored.launcher_pid, None);
    assert_eq!(stored.launcher_boot_id, None);
    assert_ne!(stored.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
}

#[test]
fn record_boot_without_launcher_records_boot_and_clears_pid_on_open_session() {
    let (_temp, conn) = create_test_db();
    let session = create_namespace_session(&conn, "try-a");
    session.set_launcher(&conn, 4242, "old-boot").unwrap();
    force_old_updated_at(&conn, "try-a");

    session.record_boot_without_launcher(&conn, "boot-456").unwrap();

    let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
    assert_eq!(stored.launcher_pid, None);
    assert_eq!(stored.launcher_boot_id.as_deref(), Some("boot-456"));
    assert_ne!(stored.updated_at.as_deref(), Some("2000-01-01T00:00:00Z"));
}

#[test]
fn launcher_identity_helpers_refuse_terminal_sessions() {
    let (_temp, conn) = create_test_db();
    let kept = create_namespace_session(&conn, "try-kept");
    kept.mark_kept(&conn).unwrap();

    for err in [
        kept.clear_launcher(&conn).unwrap_err(),
        kept.record_boot_without_launcher(&conn, "boot-789")
            .unwrap_err(),
    ] {
        let message = err.to_string();
        assert!(message.contains("Conflict"), "{message}");
        assert!(message.contains("try-kept"), "{message}");
        assert!(message.contains("not active or orphaned"), "{message}");
    }
}
```

These tests intentionally fail before the model helpers exist.

- [ ] **Step 2: Add command-side boot-id override tests**

In `apps/conary/src/commands/try_session.rs`, replace the body of `activated_no_command_session_records_boot_without_launcher_pid` with this version:

```rust
#[test]
fn activated_no_command_session_records_boot_without_launcher_pid() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 1);
    let package = fixture.write_package(
        "try-activated-no-command",
        CcsManifest::new_minimal("try-activated-no-command", "1.0.0"),
    );

    let outcome = begin_activated_try(&fixture, &package)?;

    let stored = stored_session(&fixture, &outcome.session_id);
    assert_eq!(stored.launcher_boot_id.as_deref(), Some("boot-a"));
    assert_eq!(stored.launcher_pid, None);
    Ok(())
}
```

Then, inside `try_command_records_child_liveness_before_wait_and_clears_after_exit`, add this guard near the existing launcher env guards:

```rust
let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-launcher");
```

In the same test, after the existing live-session liveness assertions, replace
`assert!(live_session.launcher_boot_id.is_some());` with:

```rust
assert_eq!(
    live_session.launcher_boot_id.as_deref(),
    Some("boot-launcher")
);
```

- [ ] **Step 3: Add failing current-link recovery test**

Add this test near `namespace_keep_restores_live_db_after_post_backup_failure`:

```rust
#[test]
fn namespace_keep_restores_current_link_after_post_link_failure() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _fail_guard =
        EnvVarGuard::set("CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP", "after-current-link");
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 7);
    let package = fixture.write_package(
        "try-restore-current-link",
        CcsManifest::new_minimal("try-restore-current-link", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;
    assert_ne!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(outcome.try_generation_id)
    );

    let err = keep_active_try_session(&fixture.db_path_string)
        .expect_err("forced post-link failure should abort keep");
    let error_chain = format!("{err:#}");
    assert!(
        error_chain.contains("forced try keep failure"),
        "{error_chain}"
    );
    assert!(
        error_chain.contains("restored live DB checkpoint"),
        "{error_chain}"
    );

    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(7),
        "current generation link must be restored after post-link keep failure"
    );
    let conn = fixture.open();
    let installed_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM troves WHERE name = 'try-restore-current-link'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(installed_count, 0);
    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Active
    );
    Ok(())
}
```

This test intentionally fails until the `after-current-link` injection point and current-link restore are added.

- [ ] **Step 4: Run failing tests and confirm expected failures**

Run:

```bash
cargo test -p conary-core db::models::try_session
cargo test -p conary --lib activated_no_command_session_records_boot_without_launcher_pid
cargo test -p conary --lib try_command_records_child_liveness_before_wait_and_clears_after_exit
cargo test -p conary --lib namespace_keep_restores_current_link_after_post_link_failure
```

Expected:

- `conary-core` test compile fails with missing `clear_launcher` and `record_boot_without_launcher`.
- Command-side boot-id assertions fail until `current_boot_id` honors `CONARY_TEST_BOOT_ID`.
- `namespace_keep_restores_current_link_after_post_link_failure` fails until the `after-current-link` seam exists and recovery restores the previous current link.

- [ ] **Step 5: Commit failing tests**

```bash
git add apps/conary/src/commands/try_session.rs crates/conary-core/src/db/models/try_session.rs
git commit -m "test(try): lock m3c0 tiny-fix regressions"
```

### Task 3: Implement Launcher Model Helpers And Canonical Boot ID

**Files:**
- Modify: `crates/conary-core/src/db/models/try_session.rs`
- Modify: `apps/conary/src/commands/try_session.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Test: `crates/conary-core/src/db/models/try_session.rs`
- Test: `apps/conary/src/commands/try_session.rs`
- Test: `apps/conary/src/dispatch/root.rs`

- [ ] **Step 1: Add `TrySession` model helpers**

In `crates/conary-core/src/db/models/try_session.rs`, add these methods after `set_launcher`:

```rust
pub fn clear_launcher(&self, conn: &Connection) -> Result<()> {
    let affected = conn.execute(
        "UPDATE try_sessions
         SET launcher_pid = NULL,
             launcher_boot_id = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE id = ?1
           AND status IN ('active', 'orphaned')",
        params![self.id],
    )?;
    self.require_open_update(conn, affected)
}

pub fn record_boot_without_launcher(&self, conn: &Connection, boot_id: &str) -> Result<()> {
    let affected = conn.execute(
        "UPDATE try_sessions
         SET launcher_pid = NULL,
             launcher_boot_id = ?1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE id = ?2
           AND status IN ('active', 'orphaned')",
        params![boot_id, self.id],
    )?;
    self.require_open_update(conn, affected)
}
```

- [ ] **Step 2: Replace command-layer launcher SQL wrappers**

In `apps/conary/src/commands/try_session.rs`, replace `clear_try_launcher` and `record_activated_try_boot` with model-backed wrappers:

```rust
fn clear_try_launcher(conn: &rusqlite::Connection, session_id: &str) -> Result<()> {
    let session = TrySession::find_by_id(conn, session_id)?
        .ok_or_else(|| anyhow::anyhow!("try session {session_id} not found"))?;
    session.clear_launcher(conn)
}

fn record_activated_try_boot(
    conn: &rusqlite::Connection,
    session_id: &str,
    boot_id: &str,
) -> Result<()> {
    let session = TrySession::find_by_id(conn, session_id)?
        .ok_or_else(|| anyhow::anyhow!("try session {session_id} not found"))?;
    session.record_boot_without_launcher(conn, boot_id)
}
```

- [ ] **Step 3: Make command-side `current_boot_id` test-aware**

In `apps/conary/src/commands/try_session.rs`, replace the current helper with:

```rust
pub(crate) fn current_boot_id() -> String {
    if let Ok(value) = std::env::var("CONARY_TEST_BOOT_ID") {
        return value;
    }
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unknown-boot".to_string())
}
```

In `apps/conary/src/dispatch/root.rs`, replace the local `current_boot_id` body with a forwarding helper:

```rust
fn current_boot_id() -> String {
    commands::try_session::current_boot_id()
}
```

Do not move `env_forces_non_interactive`; it remains in `dispatch/root.rs`.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary-core db::models::try_session
cargo test -p conary --lib activated_no_command_session_records_boot_without_launcher_pid
cargo test -p conary --lib try_command_records_child_liveness_before_wait_and_clears_after_exit
cargo test -p conary --lib dispatch::root
```

Expected: all listed tests pass.

- [ ] **Step 5: Commit model and boot-id fixes**

```bash
git add crates/conary-core/src/db/models/try_session.rs apps/conary/src/commands/try_session.rs apps/conary/src/dispatch/root.rs
git commit -m "fix(try): centralize launcher and boot identity"
```

### Task 4: Implement Current-Link Recovery After Post-Link Keep Failure

**Files:**
- Modify: `apps/conary/src/commands/try_session.rs`
- Test: `apps/conary/src/commands/try_session.rs`

- [ ] **Step 1: Add the `after-current-link` failure point**

In `keep_active_try_session_inner`, capture the previous current generation
immediately before the namespace `promotion_result` closure. Do not declare it
inside the closure; the recovery block outside the closure must also see it.

```rust
let previous_current_generation =
    conary_core::generation::mount::current_generation(runtime_root.root())?;

let promotion_result = (|| -> Result<()> {
    replace_live_db_with_session_copy(Path::new(db_path), &copied_db_path)?;
    maybe_force_try_keep_post_backup_failure("after-db-promote")?;
    // Keep the remaining namespace promotion statements in this closure.
```

Then add the new injection point immediately after `publish_generation_link`
and before `mark_generation_state_active` inside that closure:

```rust
crate::commands::composefs_ops::publish_generation_link(db_path, try_generation_id)?;
maybe_force_try_keep_post_backup_failure("after-current-link")?;
crate::commands::composefs_ops::mark_generation_state_active(
    &promoted_conn,
    try_generation_id,
)?;
```

- [ ] **Step 2: Restore the previous current-generation link on failed promotion**

Add this helper near the DB restore helpers:

```rust
fn restore_previous_current_generation_link(
    db_path: &str,
    runtime_root: &ConaryRuntimeRoot,
    previous_generation: Option<i64>,
) -> Result<()> {
    match previous_generation {
        Some(generation) => {
            crate::commands::composefs_ops::publish_generation_link(db_path, generation)
        }
        None => {
            let current_link = runtime_root.current_link();
            match std::fs::remove_file(&current_link) {
                Ok(()) => conary_core::filesystem::durable::sync_parent_directory(&current_link)
                    .map_err(|error| anyhow::anyhow!(error))
                    .with_context(|| {
                        format!(
                            "failed to sync parent directory after removing {}",
                            current_link.display()
                        )
                    }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error)
                    .with_context(|| format!("failed to remove {}", current_link.display())),
            }
        }
    }
}
```

In the `if let Err(error) = promotion_result` block, after a successful DB restore, call:

```rust
if let Err(link_error) = restore_previous_current_generation_link(
    db_path,
    &runtime_root,
    previous_current_generation,
) {
    return Err(error.context(format!(
        "try keep promotion failed after backup; restored live DB checkpoint but failed to restore current generation link: {link_error}"
    )));
}
```

Then keep the existing success message:

```rust
return Err(error.context(
    "try keep promotion failed after backup; restored live DB checkpoint",
));
```

If the restore DB call itself fails, preserve the existing error path.

- [ ] **Step 3: Run focused keep recovery tests**

Run:

```bash
cargo test -p conary --lib namespace_keep_restores_live_db_after_post_backup_failure
cargo test -p conary --lib namespace_keep_restores_current_link_after_post_link_failure
```

Expected: both pass.

- [ ] **Step 4: Commit current-link recovery**

```bash
git add apps/conary/src/commands/try_session.rs
git commit -m "fix(try): restore current link after failed keep"
```

### Task 5: Convert The Monolith Into A Module Directory

**Files:**
- Move: `apps/conary/src/commands/try_session.rs` -> `apps/conary/src/commands/try_session/mod.rs`
- Create: `apps/conary/src/commands/try_session/validation.rs`
- Create: `apps/conary/src/commands/try_session/install.rs`
- Test: `apps/conary/src/commands/try_session/mod.rs`

- [ ] **Step 1: Move the monolith into `mod.rs`**

Run:

```bash
mkdir -p apps/conary/src/commands/try_session
git mv apps/conary/src/commands/try_session.rs apps/conary/src/commands/try_session/mod.rs
```

Change the first line in the moved file to:

```rust
// apps/conary/src/commands/try_session/mod.rs
```

The existing `pub(crate) mod try_session;` declaration in `apps/conary/src/commands/mod.rs` should continue to compile because Rust accepts a directory module with `mod.rs`.

- [ ] **Step 2: Add child module declarations**

At the top of `apps/conary/src/commands/try_session/mod.rs`, after imports, add:

```rust
mod executor;
mod install;
mod namespace;
mod session;
mod util;
mod validation;
```

During this task only `validation` and `install` will contain moved code. Add the other files as empty module files with path comments so the declaration compiles:

```rust
// apps/conary/src/commands/try_session/executor.rs
```

```rust
// apps/conary/src/commands/try_session/namespace.rs
```

```rust
// apps/conary/src/commands/try_session/session.rs
```

```rust
// apps/conary/src/commands/try_session/util.rs
```

- [ ] **Step 3: Move validation code**

Create `apps/conary/src/commands/try_session/validation.rs` with:

```rust
// apps/conary/src/commands/try_session/validation.rs
//! Try-session package and manifest policy.

use anyhow::{Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::manifest::{CcsManifest, HookExecutionRoot};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TryExecutionRoot {
    Namespace,
    Generation,
    Host,
}

impl TryExecutionRoot {
    fn hook_execution_root(self) -> HookExecutionRoot {
        match self {
            Self::Namespace => HookExecutionRoot::TryRoot,
            Self::Generation => HookExecutionRoot::GenerationRoot,
            Self::Host => HookExecutionRoot::HostRoot,
        }
    }
}
```

Move these existing items from `mod.rs` into `validation.rs` below that header:

- `validate_try_package_policy`
- `validate_try_manifest_policy`
- `validate_m1b_try_declarative_hook_support`
- `unsupported_declarative_hook_error`
- `script_hook_policy_error`

In `mod.rs`, import what callers/tests still use:

```rust
use validation::{
    TryExecutionRoot, validate_try_manifest_policy, validate_try_package_policy,
};
```

- [ ] **Step 4: Move install planning code**

Create `apps/conary/src/commands/try_session/install.rs` with:

```rust
// apps/conary/src/commands/try_session/install.rs
//! Scratch install planning for try sessions.

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::db::models::TrySessionMode;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::TransactionConfig;
use std::path::{Path, PathBuf};

use crate::commands::install::{
    CcsTransactionInstallOptions, ComponentSelection, LegacyReplayOptions,
    install_ccs_package_transactionally_with_config,
};
```

Move these existing items from `mod.rs` into `install.rs`:

- `TryInstallPlan`
- `install_try_package`
- `build_try_install_plan`
- `build_try_transaction_config`

Use these visibilities:

```rust
pub(super) struct TryInstallPlan {
    pub(super) install_root: PathBuf,
    pub(super) copied_db_path: PathBuf,
    pub(super) transaction_config: TransactionConfig,
    pub(super) no_scripts: bool,
}

pub(super) fn install_try_package(
    conn: &mut rusqlite::Connection,
    package: &CcsPackage,
    plan: &TryInstallPlan,
) -> Result<()>;

pub(super) fn build_try_install_plan(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_db_path: PathBuf,
    _mode: TrySessionMode,
) -> TryInstallPlan;

pub(super) fn build_try_transaction_config(
    runtime_root: &ConaryRuntimeRoot,
    copied_db_path: PathBuf,
) -> TransactionConfig;
```

Move the implementations for those exact functions from `mod.rs` into
`install.rs`, changing only visibility and imports.

In `mod.rs`, import:

```rust
use install::{build_try_install_plan, build_try_transaction_config, install_try_package};
```

- [ ] **Step 5: Move validation tests into `validation.rs`**

Move these tests and their small helpers into a `#[cfg(test)] mod tests` inside `validation.rs`:

- `validate_manifest`
- `assert_policy_error_contains`
- `minimal_package`
- `manifest_with_post_install_script`
- `manifest_with_pre_remove_script`
- `manifest_with_declarative_hook`
- `manifest_with_systemd_hook`
- `manifest_with_tmpfiles_hook`
- `manifest_with_sysctl_hook`
- `manifest_with_alternative_hook`
- `manifest_with_service_hook`
- `manifest_with_legacy_scriptlet_bundle`
- `package_with_no_hooks_is_allowed`
- `declarative_hooks_are_allowed_only_for_try_or_generation_roots`
- `post_install_script_hooks_are_rejected_by_default`
- `pre_remove_script_hooks_are_rejected_by_default`
- `legacy_scriptlet_bundles_are_rejected_by_default`
- `service_hooks_are_rejected_in_m1b`
- `unsupported_declarative_hook_classes_are_rejected_in_m1b_try_policy`
- `package_round_trip_preserves_service_hooks_for_policy`
- `package_round_trip_preserves_declarative_reversibility_for_policy`
- `allow_irreversible_does_not_permit_scripts_legacy_or_services`

Start the validation test module with these imports, trimming only if a moved
helper no longer needs one:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::{
        AlternativeHook, CcsManifest, DirectoryHook, ScriptHook, Service, ServiceAction,
        SysctlHook, SystemdHook, TmpfilesHook,
    };
    use conary_core::ccs::{BuildResult, CcsPackage, ComponentData, FileEntry, FileType};
    use conary_core::packages::traits::PackageFormat;
}
```

Keep runtime tests that call `begin_try_session` in `mod.rs` for now. If a
manifest helper is used by both validation and runtime tests, either duplicate
the tiny helper in the moved test module or leave the shared copy in `mod.rs`
until Task 6 introduces `test_support`.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p conary --lib commands::try_session
```

Expected: all try-session unit tests pass.

- [ ] **Step 7: Commit validation and install split**

```bash
git add apps/conary/src/commands/try_session apps/conary/src/commands/mod.rs
git commit -m "refactor(try): split validation and install planning"
```

### Task 6: Move Namespace And Executor Ownership

**Files:**
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Modify: `apps/conary/src/commands/try_session/namespace.rs`
- Modify: `apps/conary/src/commands/try_session/executor.rs`
- Modify: `apps/conary/src/commands/try_session/util.rs`
- Test: `apps/conary/src/commands/try_session/namespace.rs`
- Test: `apps/conary/src/commands/try_session/executor.rs`

- [ ] **Step 1: Move namespace helpers into `namespace.rs`**

Replace `apps/conary/src/commands/try_session/namespace.rs` with:

```rust
// apps/conary/src/commands/try_session/namespace.rs
//! Try-session namespace root exposure, mounts, and declarative hook execution.

use anyhow::{Context, Result, bail};
use conary_core::ccs::manifest::CcsManifest;
use conary_core::db::models::FileEntry;
use conary_core::runtime_root::ConaryRuntimeRoot;
use rusqlite::Connection;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
```

Move these existing items from `mod.rs` into `namespace.rs`:

- `apply_declarative_try_hooks`
- `promotable_try_hook_root`
- `expose_try_namespace_root`
- `mount_try_namespace_overlay`
- `teardown_try_namespace_mounts`
- `unmount_try_path_if_mounted`
- `try_path_is_mounted`
- `read_try_mountinfo`
- `decode_mountinfo_path`
- `run_try_unmount`
- `materialize_test_try_namespace_root`
- `recreate_path_symlink`
- `create_symlink`
- `set_file_mode`
- `root_relative_path`
- `hook_effect_relative_path`
- `hook_account_entry_exists`
- `passwd_like_file_contains_name`

Use `pub(super)` for functions called by `session.rs`:

```rust
pub(super) fn apply_declarative_try_hooks(
    manifest: &CcsManifest,
    root: &Path,
) -> Result<()>;

pub(super) fn promotable_try_hook_root(
    runtime_root: &ConaryRuntimeRoot,
    try_generation_id: i64,
) -> Result<PathBuf>;

pub(super) fn expose_try_namespace_root(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_conn: &Connection,
    try_generation_id: i64,
    hook_upperdir: &Path,
) -> Result<PathBuf>;

pub(super) fn teardown_try_namespace_mounts(work_dir: &Path) -> Result<()>;
pub(super) fn root_relative_path(path: &str) -> Result<PathBuf>;
pub(super) fn hook_account_entry_exists(
    generation_root: &Path,
    etc_state_root: &Path,
    relative_file: &str,
    name: &str,
) -> bool;
```

Move the implementations for those exact functions from `mod.rs` into
`namespace.rs`, changing only visibility and imports.

Keep the rest private unless a test in the same module needs it. In particular,
`passwd_like_file_contains_name` stays private behind
`hook_account_entry_exists`.

- [ ] **Step 2: Move executor helpers into `executor.rs`**

Replace `apps/conary/src/commands/try_session/executor.rs` with:

```rust
// apps/conary/src/commands/try_session/executor.rs
//! Try-session command launcher and launcher-liveness bookkeeping.

use anyhow::{Context, Result, bail};
use conary_core::db::models::TrySession;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use super::current_boot_id;
```

Move these existing items from `mod.rs` into `executor.rs`:

- `RunningTryCommand`
- `run_try_command_for_session`
- `launch_try_command`
- `spawn_try_command`
- `running_try_command`
- `wait_try_command`
- `clear_try_launcher`
- `find_command`

Use `pub(super)` for `run_try_command_for_session`; keep the rest private except `launch_try_command` under `#[cfg(test)]`.

- [ ] **Step 3: Put shared path helpers in `util.rs` only if needed**

If both `namespace.rs` and `session.rs` need these helpers, move them into `util.rs`:

```rust
// apps/conary/src/commands/try_session/util.rs
//! Private shared filesystem helpers for try-session modules.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(super) fn remove_dir_if_exists(path: PathBuf) -> Result<()>;
pub(super) fn remove_path_if_exists(path: &Path) -> Result<()>;
```

Move the implementations from `mod.rs` and preserve the current test-injection
behavior in `remove_dir_if_exists`.

If `remove_sqlite_sidecars`, `sqlite_database_paths`, `sqlite_sidecar_path`, and `quarantine_path` are used only by `session.rs`, leave them in `session.rs` during Task 7.

- [ ] **Step 4: Update imports in `mod.rs`**

Add:

```rust
use executor::run_try_command_for_session;
use namespace::{
    apply_declarative_try_hooks, expose_try_namespace_root, promotable_try_hook_root,
    teardown_try_namespace_mounts,
};
```

Remove imports from `mod.rs` that are now only used inside child modules.

- [ ] **Step 5: Move namespace/executor tests**

Move these tests into `namespace.rs`:

- `declarative_try_hooks_refuse_host_root`
- `declarative_try_hooks_abort_post_hooks_when_pre_hooks_fail`
- `declarative_try_hooks_collect_post_hook_failures`
- `namespace_declarative_hooks_write_to_live_etc_state_not_workdir`
- `namespace_command_sees_generation_files_and_hook_upperdir`
- `activated_declarative_hooks_use_promotable_etc_state_before_publish`
- `namespace_rollback_unmounts_namespace_before_generation_root`
- `namespace_rollback_leaves_session_retryable_when_unmount_fails`

Move these tests into `executor.rs`:

- `namespace_launcher_executes_bubblewrap_when_available`
- `try_command_records_child_liveness_before_wait_and_clears_after_exit`

If a moved test needs fixture helpers that remain in `mod.rs`, make the helper `pub(super)` inside a `#[cfg(test)] pub(super) mod test_support` block in `mod.rs` rather than exporting production helpers.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p conary --lib commands::try_session
```

Expected: all try-session tests pass after namespace/executor moves.

- [ ] **Step 7: Commit namespace and executor split**

```bash
git add apps/conary/src/commands/try_session
git commit -m "refactor(try): split namespace and executor"
```

### Task 7: Move Session Lifecycle And Dispatch Liveness

**Files:**
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Modify: `apps/conary/src/commands/try_session/session.rs`
- Modify: `apps/conary/src/commands/try_session/util.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Test: `apps/conary/src/commands/try_session/session.rs`
- Test: `apps/conary/src/dispatch/root.rs`

- [ ] **Step 1: Move session lifecycle into `session.rs`**

Replace `apps/conary/src/commands/try_session/session.rs` with:

```rust
// apps/conary/src/commands/try_session/session.rs
//! Try-session lifecycle orchestration and liveness policy.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use conary_core::ccs::CcsPackage;
use conary_core::db::backup::{CheckpointReason, create_checkpoint};
use conary_core::db::models::{CreateTrySession, TrySession, TrySessionMode};
use conary_core::packages::traits::PackageFormat;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::executor::run_try_command_for_session;
use super::install::{build_try_install_plan, build_try_transaction_config, install_try_package};
use super::namespace::{
    apply_declarative_try_hooks, expose_try_namespace_root, hook_account_entry_exists,
    promotable_try_hook_root, root_relative_path, teardown_try_namespace_mounts,
};
use super::validation::{TryExecutionRoot, validate_try_package_policy};
use super::{TryStartOutcome, TryStartRequest};
```

Move these remaining lifecycle items from `mod.rs` into `session.rs`. The
`restore_previous_current_generation_link` helper is introduced in Task 4 and
should also move here during this step.

- `begin_try_session`
- `rollback_active_try_session`
- `keep_active_try_session`
- `keep_active_try_session_with_probe`
- `keep_active_try_session_inner`
- `verify_namespace_try_hook_effects`
- `maybe_force_try_keep_post_backup_failure`
- `restore_previous_current_generation_link`
- `restore_live_db_from_checkpoint`
- `verify_sqlite_file`
- `checkpoint_session_db`
- `replace_live_db_with_session_copy`
- `sync_try_db_parent_directory`
- `vacuum_db_into`
- `remove_sqlite_sidecars`
- `sqlite_database_paths`
- `sqlite_sidecar_path`
- `quarantine_path`
- `record_activated_try_boot`
- `current_boot_id`

Use this visibility:

```rust
pub(crate) fn begin_try_session(request: TryStartRequest<'_>) -> Result<TryStartOutcome>;
pub(crate) fn rollback_active_try_session(db_path: &str) -> Result<()>;
pub(crate) fn keep_active_try_session(db_path: &str) -> Result<()>;
pub(crate) fn current_boot_id() -> String;
```

Move the implementations for those exact functions from `mod.rs` into
`session.rs`, changing only imports and sibling-module qualifiers.
After `current_boot_id` moves into `session.rs`, update `executor.rs` to import
it from the session module instead of the parent module:

```rust
use super::session::current_boot_id;
```

Inside `verify_namespace_try_hook_effects`, use `root_relative_path` for
directory effect paths and call `namespace::hook_account_entry_exists` for
user/group effect checks.

Keep internal helpers private unless needed by a sibling module. If `session.rs`
exceeds 1500 lines after the move, add a follow-up note naming the DB-promotion
helpers (`restore_live_db_from_checkpoint`, `checkpoint_session_db`,
`replace_live_db_with_session_copy`, `vacuum_db_into`, SQLite sidecar helpers)
as the likely future `db.rs` boundary, but do not split them during M3c0.

- [ ] **Step 2: Move pure liveness helpers from dispatch into `session.rs`**

Move these functions from `apps/conary/src/dispatch/root.rs` into `session.rs`:

```rust
pub(crate) fn namespace_try_session_is_decision_pending(
    session: &TrySession,
    current_boot_id: &str,
) -> bool {
    if session
        .launcher_boot_id
        .as_deref()
        .is_some_and(|boot_id| boot_id != current_boot_id)
    {
        return false;
    }

    session.launcher_pid.is_none_or(try_launcher_pid_is_alive)
}

pub(crate) fn activated_try_session_is_live(
    session: &TrySession,
    current_boot_id: &str,
    current_generation: Option<i64>,
) -> bool {
    session.launcher_boot_id.as_deref() == Some(current_boot_id)
        && session.try_generation_id.is_some()
        && current_generation == session.try_generation_id
        && session.launcher_pid.is_none_or(try_launcher_pid_is_alive)
}

fn try_launcher_pid_is_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    Path::new("/proc").join(pid.to_string()).exists()
}
```

Compilation may be temporarily broken after this step until Step 4 updates
`dispatch/root.rs` imports. Do not stop between those steps except to inspect a
specific compiler error.

Do not move:

```rust
fn env_forces_non_interactive() -> bool {
    std::env::var("CONARY_NON_INTERACTIVE").as_deref() == Ok("1")
}
```

- [ ] **Step 3: Re-export the allowlisted session API from `mod.rs`**

In `apps/conary/src/commands/try_session/mod.rs`, add:

```rust
pub(crate) use session::{
    activated_try_session_is_live, begin_try_session, current_boot_id,
    namespace_try_session_is_decision_pending, rollback_active_try_session,
};
```

Keep command entrypoints in `mod.rs`:

- `cmd_try_package`
- `cmd_try_status`
- `cmd_try_rollback`
- `cmd_try_keep`

`cmd_try_rollback` and `cmd_try_keep` should call `session::rollback_active_try_session` and `session::keep_active_try_session`.

- [ ] **Step 4: Update dispatch imports and calls**

In `apps/conary/src/dispatch/root.rs`, replace local liveness helper usage with imports:

```rust
use crate::commands::try_session::{
    activated_try_session_is_live, current_boot_id, namespace_try_session_is_decision_pending,
};
```

Remove the local `namespace_try_session_is_decision_pending`, `activated_try_session_is_live`, `current_boot_id`, and `try_launcher_pid_is_alive` functions.

Keep calls unchanged:

```rust
let current_boot_id = current_boot_id();
```

- [ ] **Step 5: Move session tests**

Move lifecycle tests into `session.rs`, including:

- `activated_no_command_session_records_boot_without_launcher_pid`
- `namespace_try_start_rejects_unsupported_declarative_hook_classes_before_session`
- `namespace_try_start_creates_active_session_and_copied_artifact`
- `namespace_try_start_with_active_session_errors_with_active_id`
- `try_generation_build_leaves_current_link_and_writes_live_runtime_artifacts`
- `activated_try_publishes_generation_records_previous_and_marks_mode`
- `activated_rollback_uses_copied_package_after_original_is_deleted`
- `namespace_rollback_marks_rolled_back_and_removes_work_dir`
- `namespace_rollback_leaves_session_retryable_when_work_dir_removal_fails`
- `namespace_keep_publishes_try_generation_and_marks_kept`
- `namespace_keep_removes_stale_sidecars_before_promoted_db_reopen`
- `db_promotion_syncs_parent_after_quarantine_and_final_rename`
- `namespace_keep_holds_runtime_lock_until_session_is_marked`
- `activated_keep_holds_runtime_lock_while_marking_kept`
- `namespace_keep_restores_live_db_after_post_backup_failure`
- `namespace_keep_restores_current_link_after_post_link_failure`
- `namespace_keep_fails_when_declarative_hook_effect_is_not_promotable`
- `keep_time_hook_verification_checks_user_group_effects`
- `proc_liveness_probe_rejects_non_positive_pids`

Keep shared test fixture helpers in `mod.rs` as `#[cfg(test)] pub(super) mod test_support` if more than one child module needs them.
Move `proc_liveness_probe_rejects_non_positive_pids` out of
`dispatch/root.rs` because `try_launcher_pid_is_alive` becomes private in
`session.rs`. Remove the now-unused `std::path::Path` import from the dispatch
test module if no remaining dispatch test needs it.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib dispatch::root
```

Expected: all try-session and dispatch preflight tests pass.

- [ ] **Step 7: Commit session and dispatch split**

```bash
git add apps/conary/src/commands/try_session apps/conary/src/dispatch/root.rs
git commit -m "refactor(try): split session lifecycle" \
  -m "Preserve namespace_try_session_is_decision_pending name from dispatch; a rename is deferred to a separate cleanup slice."
```

### Task 8: Reduce Visibility And Remove Old Monolith Coupling

**Files:**
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Modify: `apps/conary/src/commands/try_session/*.rs`
- Test: `apps/conary/src/commands/try_session/*.rs`

- [ ] **Step 1: Audit crate-facing symbols**

Run:

```bash
rg -n "pub\\(crate\\)|pub fn|pub struct|pub enum" apps/conary/src/commands/try_session
```

Expected crate-facing allowlist only:

```text
cmd_try_package
cmd_try_status
cmd_try_rollback
cmd_try_keep
rollback_active_try_session
begin_try_session
TryStartRequest
TryStartOutcome
current_boot_id
namespace_try_session_is_decision_pending
activated_try_session_is_live
```

Run a separate sibling-visibility review:

```bash
rg -n "pub\\(super\\)" apps/conary/src/commands/try_session
```

Expected: sibling visibility is limited to named collaboration points between
the child modules, such as install planning, namespace exposure, executor
launching, and private test support.

- [ ] **Step 2: Tighten visibility**

For any additional `pub(crate)` symbol, either:

1. reduce it to private or `pub(super)`, or
2. add a one-sentence implementation note in the commit body naming the caller and reason.

Example allowed sibling visibility:

```rust
pub(super) fn root_relative_path(path: &str) -> Result<PathBuf>;
```

This is allowed because `session.rs` needs to pass user-visible hook paths
through the same root-relative normalization owned by `namespace.rs`.

Example not allowed without explanation:

```rust
pub(crate) fn mount_try_namespace_overlay(
    lower_root: &Path,
    hook_upperdir: &Path,
    overlay_workdir: &Path,
    namespace_root: &Path,
) -> Result<()>;
```

This would expose a namespace-internal mount primitive outside the
try-session module boundary and must stay private.

- [ ] **Step 3: Run full focused proof**

Run:

```bash
cargo test -p conary-core db::models::try_session
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib dispatch::root
cargo test -p conary --test packaging_m1b
cargo fmt --check
```

Expected: all pass.

- [ ] **Step 4: Commit visibility cleanup**

```bash
git add apps/conary/src/commands/try_session
git commit -m "refactor(try): narrow module visibility"
```

### Task 9: Update Assistant Routing Docs

**Files:**
- Modify: `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`

- [ ] **Step 1: Run the coherency-ledger precheck**

Before editing public or assistant-facing docs, run the AGENTS-required ledger
lookup:

```bash
rg -n "docs/llms/subsystem-map.md|docs/modules/feature-ownership.md|2026-06-15-m3-packaging-differentiators-design|apps/conary/src/commands/try_session" docs/superpowers/feature-coherency-ledger.tsv
```

Expected: if rows match, run the coherency checks named by the ledger before
committing docs. If no rows match, record that no ledger-gated checks were
required.

- [ ] **Step 2: Mark M3c0 landed in the M3 umbrella**

In `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`, change:

```markdown
**Status:** M3a and M3b landed; M3c0 try-session decomposition is next
```

to:

```markdown
**Status:** M3a, M3b, and M3c0 landed; M3c watch mode is next
```

In the milestone table, change the M3c0 row gate from:

```markdown
| M3c0 | Try-session decomposition | Reviewed move map and parity tests before watch behavior |
```

to:

```markdown
| M3c0 | Try-session decomposition | Landed: try-session module boundary, parity tests, and no watch behavior |
```

Do not mark M3c watch as landed.

- [ ] **Step 3: Update `docs/llms/subsystem-map.md`**

Replace the `apps/conary/src/commands/try_session.rs` entry in the packaging look-here list with:

```markdown
  `apps/conary/src/commands/try_session/`,
```

If the file list includes individual tests, keep `apps/conary/tests/packaging_m1b.rs`.

- [ ] **Step 4: Update `docs/modules/feature-ownership.md`**

In the packaging ownership card, replace:

```markdown
`apps/conary/src/commands/try_session.rs`;
```

with:

```markdown
`apps/conary/src/commands/try_session/`;
```

Ensure the focused proof list includes:

```markdown
`cargo test -p conary --lib commands::try_session`;
`cargo test -p conary --lib dispatch::root`;
`cargo test -p conary --test packaging_m1b`.
```

- [ ] **Step 5: Run docs checks**

Run:

```bash
rg -n "apps/conary/src/commands/try_session\\.rs" \
  docs/llms/subsystem-map.md \
  docs/modules/feature-ownership.md
```

Expected: no active look-here reference remains in routing docs.

Then check the M3 umbrella separately:

```bash
rg -n "apps/conary/src/commands/try_session\\.rs" \
  docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md
```

Expected: any remaining matches are explicitly historical/past-tense context.
Prefer updating or removing them if they still read as current routing.

- [ ] **Step 6: Commit docs routing**

```bash
git add docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md
git commit -m "docs(packaging): mark m3c0 try-session split landed"
```

### Task 10: Final Verification And Review

**Files:**
- Verify only unless review requires a patch.

- [ ] **Step 1: Run complete focused verification**

Run:

```bash
cargo test -p conary-core db::models::try_session
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib dispatch::root
cargo test -p conary --test packaging_m1b
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all pass.

- [ ] **Step 2: Review moved-code diff**

Run:

```bash
git diff --stat origin/main..HEAD
git diff --color-moved=dimmed-zebra origin/main..HEAD -- apps/conary/src/commands/try_session apps/conary/src/commands/try_session.rs apps/conary/src/dispatch/root.rs crates/conary-core/src/db/models/try_session.rs
```

Expected: most try-session code changes are moves. Non-move behavior changes should be limited to:

- `current_boot_id` honoring `CONARY_TEST_BOOT_ID`
- model-backed launcher clearing / boot-only recording
- current-link restoration on post-link keep failure
- added characterization tests
- visibility/import adjustments required by module boundaries

- [ ] **Step 3: Run local agentic review**

Dispatch a local reviewer against `origin/main..HEAD` with this prompt:

```text
Review the M3c0 try-session decomposition implementation against
docs/superpowers/specs/2026-06-17-m3c0-try-session-decomposition-design.md
and docs/superpowers/plans/2026-06-17-m3c0-try-session-decomposition-implementation-plan.md.

Check for:
- validation before active session row
- one-active-session invariant
- active/orphan liveness equivalence after helper move
- CONARY_TEST_BOOT_ID behavior across command and dispatch paths
- keep rollback DB/current-link recovery
- declarative hook ordering and failure aggregation
- narrow API allowlist
- unintended watch behavior
- tests and docs updated
```

Expected: no Critical or Important findings. Patch valid findings before continuing.

- [ ] **Step 4: Commit review fixes if any**

If the local review requires fixes:

```bash
git add apps/conary/src/commands/try_session \
  apps/conary/src/dispatch/root.rs \
  crates/conary-core/src/db/models/try_session.rs \
  docs/llms/subsystem-map.md \
  docs/modules/feature-ownership.md \
  docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md
git commit -m "fix(try): address m3c0 review findings"
```

If there are no fixes, do not create an empty commit.

- [ ] **Step 5: Final status**

Run:

```bash
git status --short --branch
git log --oneline --decorate --max-count=12
```

Expected: clean working tree on the M3c0 implementation branch.
