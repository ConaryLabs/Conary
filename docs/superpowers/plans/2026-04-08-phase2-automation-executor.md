# Phase 2 Plan: Automation Executor

**Date:** 2026-04-08

**Goal:** Make `conary automation` actually execute the actions it discovers, persist history, read and write real automation config from `system.toml`, and remove the misleading daemon/background dead-end.

**Scope:** This phase lands the full approved Phase 2 spec in one pass. No follow-up TODO bucket. We stay inside Conary's existing shape: core produces typed plans, the CLI executes those plans through existing `cmd_install` / `cmd_remove` / `cmd_restore` entry points, history is a small SQLite table, and config writes use `toml_edit` against the real model file.

**Current gaps to close:**
- `ActionExecutor::execute()` is still a stub and returns `Ok(ActionStatus::Failed { ... })`, which makes the CLI count non-executed actions as success.
- `cmd_automation_apply()` currently passes `no_scripts` into `ActionExecutor::new(...)`, but that constructor argument is `dry_run`.
- `PendingAction` has only free-form `details`, so updates and repairs are not machine-dispatchable.
- `MajorUpgrades` exists in the checker but is not wired through the CLI parser, status output, check output, or daemon summary.
- `automation configure` prints hardcoded defaults and write operations are no-ops.
- `automation history` still bails.
- `automation daemon` still advertises a background mode we do not actually support.

## Files

| File | Role | Action |
|------|------|--------|
| `crates/conary-core/src/automation/mod.rs` | PendingAction shape | Modify: add `InstalledPackageRef` + `ActionPayload` to `PendingAction` |
| `crates/conary-core/src/automation/action.rs` | Action builder and executor | Modify: add payload builder support; replace `execute()` with `plan()` returning `ActionPlan` / `PlannedOp` |
| `crates/conary-core/src/automation/check.rs` | Automation detection | Modify: populate typed payloads, thread security target versions, group repair candidates by package, preserve MajorUpgrades |
| `crates/conary-core/src/db/schema.rs` | Schema version | Modify: bump to v66 and add dispatcher arm |
| `crates/conary-core/src/db/migrations/v41_current.rs` | Latest migration block | Modify: add `automation_history` table migration |
| `apps/conary/src/dispatch.rs` | CLI safety gate | Modify: require live-mutation approval for `automation apply` |
| `apps/conary/src/commands/automation.rs` | CLI execution | Modify: execute plans through existing commands, insert/query history rows, add path-aware config helpers for tests, wire configure to real model TOML, wire MajorUpgrades, drop background stub |
| `apps/conary/src/cli/automation.rs` | Automation CLI surface | Modify: accept `major_upgrades`, remove `--foreground`, fix daemon help text |
| `apps/conary/src/commands/restore.rs` | Restore execution | Modify: accept concrete trove selectors and honor the passed root |
| `apps/conary/src/commands/remove.rs` | Remove execution | Modify if needed so automation can target a concrete installed trove without ambiguous name-only removal |

## Task 1: Add typed payloads to PendingAction

- [ ] Add `ActionPayload` to `crates/conary-core/src/automation/mod.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackageRef {
    pub name: String,
    pub version: Option<String>,
    pub architecture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionPayload {
    UpdatePackage {
        target_version: String,
        architecture: Option<String>,
    },
    RemovePackages {
        installed: Vec<InstalledPackageRef>,
    },
    RestorePackage {
        installed: InstalledPackageRef,
    },
}
```

- [ ] Add `pub payload: ActionPayload` to `PendingAction`.

- [ ] Extend `ActionBuilder` in `crates/conary-core/src/automation/action.rs`:
  - add a `payload: Option<ActionPayload>` field
  - add a `.payload(...)` setter
  - make `build()` require a payload and panic in tests / dev if a builder path forgets to set one
  - replace the current timestamp-based `id` generation with a deterministic action key derived from normalized category + payload + concrete package selectors
  - keep `identified_at` as the scan timestamp; only the action identity becomes stable

- [ ] Update existing action builders to set payloads explicitly:
  - `security_update_action()` -> `UpdatePackage { target_version, architecture }`
  - `package_update_action()` -> `UpdatePackage { target_version, architecture }`
  - `major_upgrade_action()` -> `UpdatePackage { target_version, architecture }`
  - `orphan_cleanup_action()` -> `RemovePackages { installed }`
  - `integrity_repair_action()` -> `RestorePackage { installed }`

- [ ] Thread security target versions through detection before building payloads:
  - change `find_security_updates()` in `crates/conary-core/src/automation/check.rs` to return the repository target version it already selects from `repository_packages`
  - extend the security/update/major-upgrade queries to select installed and repository architecture explicitly (`t.architecture`, `rp.architecture`) instead of implicitly leaving payload architecture as `None`
  - pass that repo version into `security_update_action()`
  - derive payload architecture from the explicit query results, preferring the repo package architecture when present and otherwise falling back to the installed trove architecture
  - preserve the existing severity filtering and deadline calculation

- [ ] Keep typed payloads tied to current Conary identity, not a new subsystem:
  - use `InstalledPackageRef { name, version, architecture }`
  - keep `PendingAction.packages` as display-oriented strings
  - do not add a separate automation-only package database or resolver

- [ ] Add unit tests in `crates/conary-core/src/automation/action.rs`:
  - `test_security_update_action_sets_update_payload`
  - `test_orphan_cleanup_action_sets_remove_payload`
  - `test_integrity_repair_action_sets_restore_payload`
  - `test_same_logical_action_builds_stable_id`
  - `test_payload_change_changes_action_id`

- [ ] Verify:
  - `cargo test -p conary-core automation::action::tests`

## Task 2: Make repair package-based and wire MajorUpgrades end-to-end

- [ ] Replace the raw-path repair aggregation in `crates/conary-core/src/automation/check.rs`:
  - query `files` joined to `troves`
  - group corrupted paths by owning package
  - emit one `PendingAction` per package
  - keep the corrupted paths in `details` for human display
  - set `payload: ActionPayload::RestorePackage { installed }` with the concrete installed trove identity

- [ ] Keep orphan cleanup concrete instead of name-only:
  - when building orphan actions, preserve the installed trove version and architecture in `RemovePackages { installed }`
  - keep `packages: Vec<String>` for human output, but do not flatten execution identity down to names

- [ ] Make the query changes explicit instead of implicit:
  - update the underlying `SELECT` lists and row parsing in `check.rs` so architecture is actually available when building typed payloads
  - cover security, regular updates, major upgrades, orphan detection, and repair grouping where concrete trove selectors are needed

- [ ] Keep the existing major-upgrade detection split in `check_updates()`, but wire it through the CLI:
  - in `apps/conary/src/cli/automation.rs`, allow `major_upgrades` and `major-upgrades` in `Check.categories` and `Apply.categories`
  - in `apps/conary/src/commands/automation.rs`, include `results.major_upgrades.len()` in:
    - `AutomationSummary`
    - JSON/text `status`
    - `automation check`
    - daemon summary output

- [ ] Add tests:
  - core unit test: grouped integrity check produces one repair action per package
  - CLI unit test: category parsing accepts `major_upgrades`
  - CLI unit test: `cmd_automation_status(..., "json", ...)` reports non-zero `major_upgrades`

- [ ] Verify:
  - `cargo test -p conary automation::`
  - `cargo test -p conary-core automation::check::tests`

## Task 3: Replace ActionExecutor with a planner

- [ ] In `crates/conary-core/src/automation/action.rs`, remove the stub executor behavior and introduce:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedOp {
    Install {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Remove {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Restore {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPlan {
    pub ops: Vec<PlannedOp>,
    pub category: AutomationCategory,
    pub action_id: String,
}
```

- [ ] Rename `ActionExecutor::execute()` to `plan()`.

- [ ] Keep the planner in `conary-core`; it must not call CLI code.

- [ ] Planner mapping:
  - `Security` + `UpdatePackage` -> one `PlannedOp::Install` per package with `Some(target_version)` and the target architecture
  - `Updates` + `UpdatePackage` -> one `PlannedOp::Install` per package with `Some(target_version)` and the target architecture
  - `MajorUpgrades` + `UpdatePackage` -> one `PlannedOp::Install` per package with `Some(target_version)` and the target architecture
  - `Orphans` + `RemovePackages { installed }` -> one `PlannedOp::Remove` per concrete installed trove
  - `Repair` + `RestorePackage { installed }` -> one `PlannedOp::Restore` for that concrete installed trove

- [ ] Treat mismatched category/payload combinations as real errors, not silent fallthrough.

- [ ] Remove the old `dry_run` field from the planner type entirely. Dry-run belongs in the CLI execution layer, not in the core planner.

- [ ] Keep planner output aligned with existing CLI entry points:
  - if current `cmd_remove` or `cmd_restore` only accept a package name, extend them to accept `version` / `architecture` selectors rather than adding a parallel automation-only executor
  - `cmd_restore` must also honor the passed `root` instead of hardcoding `"/conary"` so automation repair works in tests and non-default roots
  - for restore specifically, replace the current `find_one_by_name()` lookup with `find_by_name()` plus explicit version/architecture filtering, matching the safety model already used by `cmd_remove`

- [ ] Add unit tests:
  - `test_plan_update_action_produces_install_with_version`
  - `test_plan_major_upgrade_produces_install_with_version`
  - `test_plan_repair_action_produces_restore`
  - `test_plan_mismatched_payload_errors`

- [ ] Verify:
  - `cargo test -p conary-core automation::action::tests`

## Task 4: Add schema v66 for automation history

- [ ] Bump `SCHEMA_VERSION` in `crates/conary-core/src/db/schema.rs` from `65` to `66`.

- [ ] Add the migration dispatcher arm in `crates/conary-core/src/db/schema.rs`:
  - `66 => migrations::migrate_v66(conn)`
  - keep the unknown-version error as the fallback arm

- [ ] Add `migrate_v66()` in `crates/conary-core/src/db/migrations/v41_current.rs`:

```sql
CREATE TABLE automation_history (
    id INTEGER PRIMARY KEY,
    action_id TEXT NOT NULL,
    category TEXT NOT NULL,
    packages TEXT,
    status TEXT NOT NULL,
    error_message TEXT,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_automation_history_applied_at ON automation_history(applied_at DESC);
CREATE INDEX idx_automation_history_category ON automation_history(category);
CREATE INDEX idx_automation_history_status ON automation_history(status);
```

- [ ] Use direct SQL helpers in `apps/conary/src/commands/automation.rs` for insert/query. Do not introduce a new automation-history subsystem.

- [ ] Add migration coverage in `v41_current.rs` tests:
  - schema version reaches `66`
  - `automation_history` exists
  - inserting and querying a row succeeds

- [ ] Verify:
  - `cargo test -p conary-core db::migrations::v41_current::tests`

## Task 5: Implement CLI-side automation apply with real execution and history

- [ ] Add the live-host safety gate in `apps/conary/src/dispatch.rs`:
  - extend `dispatch_automation_command(...)` to accept `allow_live_system_mutation`
  - call `require_live_mutation(...)` before dispatching `AutomationCommands::Apply`
  - treat `automation apply --dry-run` like other gated mutators: dry-run is allowed, live mutation without the override is not

- [ ] Rewrite `cmd_automation_apply()` in `apps/conary/src/commands/automation.rs` around the planner:
  - build filtered `PendingAction`s
  - call `plan()` for each action
  - in `--dry-run`, print the planned ops without mutation
  - in `--yes`, execute planned ops immediately
  - in interactive mode, execute planned ops for the selected actions only

- [ ] Execute ops through existing commands:
  - `PlannedOp::Install` -> `cmd_install(package, InstallOptions { version, architecture, no_scripts, yes: true, ... })`
  - `PlannedOp::Remove` -> existing `cmd_remove` with a concrete selector; if the current signature is still name-only, extend it to accept `version` / `architecture` instead of inventing a new automation runner
  - `PlannedOp::Restore` -> existing `cmd_restore` with a concrete selector and the caller's `root`; extend the command signature if needed so it is not first-match-by-name

- [ ] Preserve current Conary behavior instead of inventing new runners:
  - use the existing CLI commands directly
  - keep one history row per action, not per low-level op
  - mark action status:
    - `applied` when every op succeeds
    - `failed` when every op fails
    - `partial` when some ops succeed and some fail

- [ ] Insert `automation_history` rows immediately after each action attempt with:
  - `action_id`
  - `category`
  - JSON-encoded `packages`
  - `status`
  - `error_message` for failed / partial actions

- [ ] Fix the current `no_scripts` / `dry_run` mixup as part of this rewrite:
  - `dry_run` stays in CLI flow only
  - `no_scripts` only affects `cmd_install` / `cmd_remove`

- [ ] Make CLI success accounting reflect actual execution:
  - planned-only or failed actions must not count as applied
  - if any selected action fails, return a non-zero error after printing the summary

- [ ] Add CLI tests:
  - `automation apply` is blocked by `require_live_mutation` on `/` without the override flag
  - `automation apply --yes` executes install/remove/restore through the real command paths
  - multi-version / multi-arch fixtures execute against the intended installed trove instead of failing on ambiguous name-only selection
  - failed actions produce `failed` or `partial` history rows instead of false success
  - `--dry-run` does not insert history rows

- [ ] Verify:
  - `cargo test -p conary cmd_automation_apply`

## Task 6: Implement automation history and real configure reads/writes

- [ ] Implement `cmd_automation_history()` in `apps/conary/src/commands/automation.rs`:
  - query `automation_history`
  - honor `limit`, `category`, `status`, and `since`
  - print a readable table in text mode
  - keep the existing CLI flags; no new subcommands

- [ ] Implement `cmd_automation_configure()` against the real model file:
  - add a small path-aware helper in `apps/conary/src/commands/automation.rs` so tests can pass a temp model path without writing to `/etc/conary/system.toml`
  - `--show`: call that helper with the real default path in production and a temp path in tests, then print the parsed `AutomationConfig`
  - writes: load the raw TOML from the selected model path, edit the `[automation]` section with `toml_edit`, write the file back in place

- [ ] Keep writes minimal and Conary-shaped:
  - only change the `[automation]` and `[automation.ai_assist]` fields requested by the flags
  - preserve comments and unrelated formatting
  - do not add a new `save_model()` API in `conary-core`

- [ ] Reuse the current model path and existing TOML schema:
  - global `mode`
  - category enable/disable overrides
  - `check_interval`
  - AI assist enable/disable

- [ ] Make daemon/config behavior explicit:
  - after a successful config write, print a short note that an already-running foreground automation daemon must be restarted to pick up the new settings
  - dynamic daemon config reload is out of scope for Phase 2

- [ ] Add tests:
  - `cmd_automation_history` returns inserted rows in the requested order
  - `cmd_automation_configure --show` reflects real values from a temp `system.toml`
  - `cmd_automation_configure --mode auto` updates the file and preserves comments
  - `cmd_automation_configure --enable security` only edits the automation section

- [ ] Verify:
  - `cargo test -p conary cmd_automation_history`
  - `cargo test -p conary cmd_automation_configure`

## Task 7: Make daemon foreground-only and finish verification

- [ ] In `apps/conary/src/cli/automation.rs`:
  - change daemon help text to `Run automation daemon (use systemd for background operation)`
  - remove the `--foreground` flag entirely

- [ ] In `apps/conary/src/commands/automation.rs`:
  - remove the `if !foreground` bail
  - make foreground execution the only behavior
  - include `MajorUpgrades` in the daemon status summary

- [ ] Run final verification from repo root:
  - `cargo fmt --check`
  - `cargo clippy -p conary -- -D warnings`
  - `cargo test -p conary`
  - `cargo test -p conary-core`

- [ ] Manual CLI sanity checks:
  - `cargo run -p conary -- automation status --format json`
  - `cargo run -p conary -- automation check --categories major_upgrades`
  - `cargo run -p conary -- automation configure --show`
  - `cargo run -p conary -- automation history`
  - `cargo run -p conary -- automation daemon --help`

## Exit Criteria

Phase 2 is done when all of the following are true:

- `conary automation apply --yes` executes real install/remove/restore work instead of counting planner stubs as success
- update and repair actions are machine-dispatchable through typed payloads
- repair actions are package-based and delegate to existing `cmd_restore`
- `MajorUpgrades` is visible everywhere the user would expect it
- `automation_history` exists, is written after execution, and can be queried
- `automation configure --show` reads real model values and write flags update the real `system.toml`
- `automation daemon` no longer claims a background mode we do not support
- `cargo fmt --check`, `cargo clippy -p conary -- -D warnings`, `cargo test -p conary`, and `cargo test -p conary-core` all pass
