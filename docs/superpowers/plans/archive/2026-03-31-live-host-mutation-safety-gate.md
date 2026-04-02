# Live Host Mutation Safety Gate Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fail-closed `--allow-live-system-mutation` acknowledgment gate for the approved set of live-host mutating CLI commands, while also proving the gated commands are actually wired and functional today with passing tests before we treat the feature as release-ready.

**Architecture:** Keep the policy at the CLI boundary, but centralize the enforcement decision in one helper-owned seam so retiring the gate later is a one-file change instead of a repo-wide unwind. Audit each approved command family first using existing local tests and disposable-host `conary-test` suites, add missing coverage where the evidence is thin, and only then wire the acknowledgment checks into `src/main.rs` so wrapper commands such as collection install and group update are covered at the command the user actually typed.

**Tech Stack:** Rust 2024 workspace, Clap CLI parsing, `anyhow` errors, existing `Command::new(env!("CARGO_BIN_EXE_conary"))` integration-test pattern, Markdown spec at `docs/superpowers/specs/2026-03-31-live-host-mutation-safety-design.md`.

---

## File Map

- `src/cli/mod.rs`: add the global `--allow-live-system-mutation` flag to `Cli` and extend CLI parser tests
- `src/live_host_safety.rs`: new CLI-bound policy helper for mutation classification, warning text, and acknowledgment enforcement
- `src/main.rs`: call the safety helper before dispatch for approved mutating commands, including wrapper entrypoints
- `tests/live_host_mutation_readiness.rs`: new isolated readiness smoke tests for gated commands whose real behavior can be exercised safely without touching the live host
- `tests/live_host_mutation_safety.rs`: new integration test file for end-to-end refusal behavior and dry-run bypass behavior
- `tests/workflow.rs`: existing install/remove/rollback workflow coverage; may gain additional readiness assertions
- `tests/component.rs`: existing CCS install coverage that should remain green through the hardening pass
- `tests/batch_install.rs`: existing atomic install rollback coverage that should remain green through the hardening pass
- `src/commands/system.rs`: rollback behavior and readiness audit touchpoint if local rollback coverage exposes drift
- `src/commands/generation/commands.rs`: generation switch/rollback/recover audit touchpoint if helper seams or tests need repair
- `src/commands/generation/takeover.rs`: takeover planning and dry-run behavior audit touchpoint
- `src/commands/generation/takeover_state.rs`: takeover dry-run record behavior audit touchpoint
- `tests/integration/remi/manifests/phase1-advanced.toml`: disposable-host coverage for remove, update, and takeover dry-run
- `tests/integration/remi/manifests/phase2-group-a.toml`: disposable-host coverage for CCS install and remove
- `tests/integration/remi/manifests/phase2-group-b.toml`: disposable-host coverage for generation switch and takeover generation prep
- `tests/integration/remi/manifests/phase3-group-j.toml`: disposable-host coverage for remove and autoremove
- `tests/integration/remi/manifests/phase3-group-l.toml`: disposable-host coverage for generation switch/rollback and likely home for new generation-recover coverage
- `tests/integration/remi/manifests/phase3-group-m.toml`: disposable-host coverage for update and remove
- `tests/integration/remi/manifests/phase4-group-d.toml`: disposable-host coverage for CCS install and system restore
- `tests/integration/remi/manifests/phase4-group-e.toml`: disposable-host coverage for CCS install, remove, and takeover owned-mode flows
- `docs/superpowers/specs/2026-03-31-live-host-mutation-safety-design.md`: approved design reference; implementation should stay aligned with this spec

## Command Audit Matrix

- `install` / `remove` / `update` / `autoremove`: audit existing disposable-host coverage in `tests/integration/remi/manifests/phase1-advanced.toml`, `tests/integration/remi/manifests/phase2-group-a.toml`, `tests/integration/remi/manifests/phase3-group-j.toml`, `tests/integration/remi/manifests/phase3-group-m.toml`, and `tests/integration/remi/manifests/phase4-group-e.toml`; keep `tests/workflow.rs` and `tests/batch_install.rs` green for local rollback/remove invariants
- `rollback`: audit `tests/workflow.rs`, `tests/batch_install.rs`, and `src/commands/system.rs`
- `ccs install`: audit `tests/component.rs`, `tests/workflow.rs`, and the disposable-host suites already exercising CCS installs
- `system restore`: audit `tests/integration/remi/manifests/phase4-group-d.toml`; add local dry-run readiness coverage and add disposable-host non-dry-run coverage if the manifest only proves planning mode
- `system generation switch` / `rollback`: audit `tests/integration/remi/manifests/phase2-group-b.toml`, `tests/integration/remi/manifests/phase3-group-h.toml`, and `tests/integration/remi/manifests/phase3-group-l.toml`
- `system generation recover`: there is no obvious existing meaningful coverage; add disposable-host coverage before the final gate commit or treat it as a release blocker
- `system takeover`: audit `tests/integration/remi/manifests/phase1-advanced.toml`, `tests/integration/remi/manifests/phase2-group-b.toml`, `tests/integration/remi/manifests/phase4-group-e.toml`, plus the local unit tests in `src/commands/generation/takeover.rs` and `src/commands/generation/takeover_state.rs`

If any audited command is obviously broken, insufficiently wired, or lacking meaningful passing test evidence after the audit work, stop and surface it as a release blocker instead of relying on the gate to hide it.

## Chunk 1: Guard Foundations

### Task 1: Add the Global Flag and Safety Helper Skeleton

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`
- Create: `src/live_host_safety.rs`
- Test: `src/cli/mod.rs`
- Test: `src/live_host_safety.rs`

- [ ] **Step 1: Write the failing parser test and create the helper test seam**

```rust
#[test]
fn cli_accepts_allow_live_system_mutation_as_global_flag() {
    let cli = Cli::try_parse_from([
        "conary",
        "system",
        "generation",
        "--allow-live-system-mutation",
        "switch",
        "7",
    ])
    .expect("global live-mutation flag should parse before nested commands");
    assert!(cli.allow_live_system_mutation);
}

#[test]
fn dry_run_bypasses_live_mutation_ack() {
    let request = LiveMutationRequest::new(
        "install",
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        true,
    );
    assert!(require_live_system_mutation_ack(false, &request).is_ok());
}

#[test]
fn missing_ack_mentions_early_software_rationale() {
    let request = LiveMutationRequest::new(
        "install",
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        false,
    );
    let err = require_live_system_mutation_ack(false, &request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("--allow-live-system-mutation"));
    assert!(message.contains("early software"));
}

#[test]
fn allow_live_mutation_ack_passes() {
    let request = LiveMutationRequest::new(
        "install",
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        false,
    );
    assert!(require_live_system_mutation_ack(true, &request).is_ok());
}
```

In the same step, make the helper tests discoverable in the red phase by:
- adding `mod live_host_safety;` near the top of `src/main.rs`
- creating `src/live_host_safety.rs` with the repository path comment, the test
  module above, and stubbed helper symbols that currently `todo!()`

That way the helper test commands fail for missing behavior rather than
silently matching zero tests.

- [ ] **Step 2: Run the narrow tests to confirm the new seam is red**

Run:
- `cargo test cli_accepts_allow_live_system_mutation_as_global_flag -- --exact`
- `cargo test dry_run_bypasses_live_mutation_ack -- --exact`
- `cargo test missing_ack_mentions_early_software_rationale -- --exact`
- `cargo test allow_live_mutation_ack_passes -- --exact`

Expected: FAIL because the flag is not parsed yet and the helper seam still
contains `todo!()` stubs

- [ ] **Step 3: Add the global flag to `Cli`**

Implement a new global field on `Cli` in `src/cli/mod.rs`:

```rust
/// Acknowledge that this command may mutate the active host.
#[arg(long, global = true)]
pub allow_live_system_mutation: bool,
```

Keep the wording direct and consistent with the approved spec.

- [ ] **Step 4: Replace the stub helper with the minimal policy types**

Replace the red-phase stub with a real helper module:

```rust
// src/live_host_safety.rs
pub enum LiveMutationClass {
    AlwaysLive,
    CurrentlyLiveEvenWithRootArguments,
}

pub struct LiveMutationRequest {
    pub command_name: &'static str,
    pub class: LiveMutationClass,
    pub dry_run: bool,
}

impl LiveMutationRequest {
    pub fn new(
        command_name: &'static str,
        class: LiveMutationClass,
        dry_run: bool,
    ) -> Self {
        Self {
            command_name,
            class,
            dry_run,
        }
    }
}

pub fn require_live_system_mutation_ack(
    allow_live_system_mutation: bool,
    request: &LiveMutationRequest,
) -> anyhow::Result<()> {
    // single retirement seam for the whole feature
    // if !live_mutation_ack_enforced() || request.dry_run { return Ok(()); }
    // otherwise return a refusal that mentions:
    // - active host mutation
    // - early software rationale
    // - generation rebuild / /usr remount / /etc overlay / scriptlets / takeover ownership
    // - explicit opt-in flag
}
```

Do not add root-awareness logic yet. The approved design says these classes
both require the flag in this slice.

Make the helper own a tiny `live_mutation_ack_enforced() -> bool` decision that
returns `true` for now. Future removal of this feature should happen by changing
that one helper-owned seam, not by editing every dispatch arm.

- [ ] **Step 5: Run the narrow tests again**

Run:
- `cargo test cli_accepts_allow_live_system_mutation_as_global_flag -- --exact`
- `cargo test dry_run_bypasses_live_mutation_ack -- --exact`
- `cargo test missing_ack_mentions_early_software_rationale -- --exact`
- `cargo test allow_live_mutation_ack_passes -- --exact`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/cli/mod.rs src/main.rs src/live_host_safety.rs
git commit -m "feat(cli): add live host mutation acknowledgment guard"
```

### Task 2: Keep the Warning Text Tight and Truthful

**Files:**
- Modify: `src/live_host_safety.rs`
- Test: `src/live_host_safety.rs`

- [ ] **Step 1: Add a failing message-shape test for the full refusal text**

```rust
#[test]
fn refusal_lists_live_host_risks() {
    let request = LiveMutationRequest::new(
        "system generation switch",
        LiveMutationClass::AlwaysLive,
        false,
    );
    let err = require_live_system_mutation_ack(false, &request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("system generation switch"));
    assert!(message.contains("mutate the active host"));
    assert!(message.contains("/usr"));
    assert!(message.contains("/etc"));
    assert!(message.contains("scriptlet"));
}

#[test]
fn currently_live_root_command_mentions_root_is_not_isolation_yet() {
    let request = LiveMutationRequest::new(
        "ccs install",
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        false,
    );
    let err = require_live_system_mutation_ack(false, &request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("ccs install"));
    assert!(message.contains("--root"));
    assert!(message.contains("not sufficient isolation"));
}
```

- [ ] **Step 2: Run the message-shape test**

Run:
- `cargo test refusal_lists_live_host_risks -- --exact`
- `cargo test currently_live_root_command_mentions_root_is_not_isolation_yet -- --exact`
Expected: FAIL until the refusal text is complete

- [ ] **Step 3: Expand the refusal builder**

Make the helper produce one consistent refusal message that:
- names the triggering command
- explains that Conary is still early software
- describes the concrete risks from the approved spec
- includes the Class 2 statement that `--root` is not sufficient isolation yet
  for commands that still route into live generation paths
- ends with the rerun guidance using `--allow-live-system-mutation`

Keep it as one message constructor, not inline string assembly in `main.rs`.

- [ ] **Step 4: Re-run the focused helper tests**

Run: `cargo test live_host_safety -- --nocapture`
Expected: PASS for the new helper module tests

- [ ] **Step 5: Commit**

```bash
git add src/live_host_safety.rs
git commit -m "feat(cli): explain live host mutation risks in refusal text"
```

## Chunk 2: Command Readiness Audit

### Task 3: Audit Local and Offline Mutation Behavior Before Gating

**Files:**
- Create: `tests/live_host_mutation_readiness.rs`
- Modify: `tests/workflow.rs`
- Modify: `tests/component.rs`
- Modify: `tests/batch_install.rs`
- Modify: `src/commands/restore.rs`
- Modify: `src/commands/system.rs`
- Modify: `src/commands/generation/commands.rs`
- Modify: `src/commands/generation/takeover.rs`
- Modify: `src/commands/generation/takeover_state.rs`

- [ ] **Step 1: Add any missing local readiness smoke tests for safe, isolated command behavior**

At minimum, add a new isolated test in `tests/live_host_mutation_readiness.rs`
for `system restore all --dry-run` that uses a temp database and temp CAS root,
asserts the command stays in planning mode, and proves we can exercise restore
readiness without touching the live host.

If the audit shows another gated command has only helper-level coverage but no
real isolated behavior test, add one here as well. Good candidates are small
offline seams, not live remounts.

```rust
#[test]
fn system_restore_all_dry_run_reports_missing_cas_without_live_mounting() {
    use conary_core::db;
    use conary_core::db::models::{Changeset, ChangesetStatus, FileEntry, Trove, TroveType};
    use std::process::Command;

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    db::init(db_path.to_str().unwrap()).unwrap();
    let mut conn = db::open(db_path.to_str().unwrap()).unwrap();

    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("seed restore fixture".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new(
            "restore-fixture".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        let mut file = FileEntry::new(
            "/usr/share/restore-fixture.txt".to_string(),
            "deadbeef".repeat(8),
            12,
            0o100644,
            trove_id,
        );
        file.insert(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "restore", "all", "--dry-run", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "restore dry-run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("restore-fixture"));
    assert!(stdout.contains("MISSING: /usr/share/restore-fixture.txt"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
```

- [ ] **Step 2: Run the local readiness audit set**

Run:
- `cargo test test_install_and_remove_workflow -- --exact`
- `cargo test test_install_and_rollback -- --exact`
- `cargo test test_batch_install_rollback_removes_all -- --exact`
- `cargo test test_ccs_install_components_only_installs_requested_component -- --exact`
- `cargo test test_capability_run_uses_installed_package_declaration -- --exact`
- `cargo test rollback_claim_statuses_include_post_hooks_failed -- --exact`
- `cargo test classify_side_effect_reasons_detects_all_requested_categories -- --exact`
- `cargo test test_takeover_plan_empty -- --exact`
- `cargo test test_takeover_dry_run_writes_planning_record_without_mutation -- --exact`
- `cargo test system_restore_all_dry_run_reports_missing_cas_without_live_mounting -- --exact`

Expected: PASS, or FAIL only if the audit reveals a real readiness problem we
must fix before the gate lands

- [ ] **Step 3: Fix any local readiness breakage instead of deferring behind the flag**

If any command family above fails, repair the actual command or its regression
coverage now. Do not proceed to dispatch gating while a command remains red or
obviously under-tested.

- [ ] **Step 4: Re-run the same local readiness audit set**

Run the same commands from Step 2.
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add tests/live_host_mutation_readiness.rs tests/workflow.rs tests/component.rs tests/batch_install.rs src/commands/restore.rs src/commands/system.rs src/commands/generation/commands.rs src/commands/generation/takeover.rs src/commands/generation/takeover_state.rs
git commit -m "test(cli): audit live-mutation command readiness"
```

### Task 4: Audit Disposable-Host Mutation Behavior With `conary-test`

**Files:**
- Modify: `tests/integration/remi/manifests/phase1-advanced.toml`
- Modify: `tests/integration/remi/manifests/phase2-group-a.toml`
- Modify: `tests/integration/remi/manifests/phase2-group-b.toml`
- Modify: `tests/integration/remi/manifests/phase3-group-h.toml`
- Modify: `tests/integration/remi/manifests/phase3-group-j.toml`
- Modify: `tests/integration/remi/manifests/phase3-group-l.toml`
- Modify: `tests/integration/remi/manifests/phase3-group-m.toml`
- Modify: `tests/integration/remi/manifests/phase4-group-d.toml`
- Modify: `tests/integration/remi/manifests/phase4-group-e.toml`
- Modify: `src/commands/update.rs`
- Modify: `src/commands/state.rs`
- Modify: `src/commands/restore.rs`
- Modify: `src/commands/remove.rs`
- Modify: `src/commands/install/mod.rs`
- Modify: `src/commands/ccs/install.rs`
- Modify: `src/commands/system.rs`
- Modify: `src/commands/generation/commands.rs`
- Modify: `src/commands/generation/takeover.rs`

- [ ] **Step 1: Fill the disposable-host coverage gaps before any gate commit**

Inventory the relevant manifest cases and add coverage wherever a gated command
still lacks meaningful execution evidence.

Minimum expectations:
- keep existing manifest coverage for `install`, `remove`, `update`, `autoremove`, `ccs install`, `generation switch`, and `takeover`
- make `system generation rollback` explicitly part of the disposable-host audit, not just an implied passenger in the generation suites
- add a disposable-host `system state rollback` case if no existing manifest already exercises it
- add or strengthen a `system restore` non-dry-run case if current coverage only proves `--dry-run`
- add a `system generation recover` case in `tests/integration/remi/manifests/phase3-group-h.toml`, where the interruption tests already live, so recover is not release-blocking due to zero evidence

If a command cannot be covered safely in the disposable-host harness yet, stop
and treat that as a release blocker for the gated-command set.

Wrapper entrypoints like `install @collection` and `update @group` inherit
their command-readiness evidence from `install` and `update` in this chunk; the
wrapper-boundary refusal behavior itself is verified separately in Chunk 3.

Use concrete manifest additions rather than vague TODOs. For example:

```toml
[[test]]
id = "T256"
name = "system_restore_live_path_rebuilds_generation"
description = "system restore should rebuild the generation on a disposable host without panicking"
timeout = 60
group = "D"

[[test.step]]
conary = "system restore ${FIXTURE_PKG_NAME}"

[test.step.assert]
stdout_contains_any = ["Restore complete", "generation"]
stdout_not_contains = "panic"
```

```toml
[[test]]
id = "T97"
name = "generation_recover_after_interrupted_mutation"
description = "generation recover should finish cleanup after an interrupted mutation on a disposable host"
timeout = 120
group = "H"

[[test.step]]
run = "env CONARY_TEST_HOLD_AFTER_DB_UPDATE_MS=1500 ${CONARY_BIN} ccs install ${FIXTURE_V2_CCS} --allow-unsigned --sandbox never >/tmp/recover-install.log 2>&1 & PID=$!; sleep 0.3; kill -TERM $PID || true; wait $PID || true"

[test.step.assert]
exit_code = 0

[[test.step]]
conary = "system generation recover"

[test.step.assert]
stdout_contains_any = ["Recovery complete", "Recovered", "No interrupted transaction"]
stdout_not_contains = "panic"
```

- [ ] **Step 2: Run the targeted disposable-host suites**

Run:
- `cargo run -p conary-test -- list`
- `cargo run -p conary-test -- run --suite phase1-advanced.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase2-group-a.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase2-group-b.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-h.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-j.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-l.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-m.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase4-group-d.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase4-group-e.toml --distro fedora43`

Expected: PASS, or actionable red results that must be fixed before the gate is
considered release-ready

- [ ] **Step 3: Fix real command breakage or missing harness coverage**

Repair failing command behavior, tighten flaky assertions, or add the missing
manifest cases. Do not downgrade a real command failure into “gated anyway.”

- [ ] **Step 4: Re-run the affected disposable-host suites**

Run the suites from Step 2 that were red or that gained new manifest coverage.
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add tests/integration/remi/manifests/phase1-advanced.toml tests/integration/remi/manifests/phase2-group-a.toml tests/integration/remi/manifests/phase2-group-b.toml tests/integration/remi/manifests/phase3-group-h.toml tests/integration/remi/manifests/phase3-group-j.toml tests/integration/remi/manifests/phase3-group-l.toml tests/integration/remi/manifests/phase3-group-m.toml tests/integration/remi/manifests/phase4-group-d.toml tests/integration/remi/manifests/phase4-group-e.toml src/commands/update.rs src/commands/state.rs src/commands/system.rs src/commands/generation/commands.rs src/commands/generation/takeover.rs src/commands/restore.rs src/commands/remove.rs src/commands/install/mod.rs src/commands/ccs/install.rs
git commit -m "test(cli): verify live-mutation commands on disposable hosts"
```

## Chunk 3: Dispatch Wiring

### Task 5: Gate the Root Command Entry Points in `main.rs`

**Files:**
- Modify: `src/main.rs`
- Modify: `src/live_host_safety.rs`
- Test: `tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Write failing integration tests for representative root commands**

```rust
#[test]
fn install_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["install", "bash", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn remove_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["remove", "bash", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn update_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["update", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("early software"));
}

#[test]
fn autoremove_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["autoremove", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn install_collection_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["install", "@demo", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn update_group_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["update", "@core", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}
```

- [ ] **Step 2: Run the new integration file**

Run: `cargo test --test live_host_mutation_safety`
Expected: FAIL because `main.rs` still dispatches these commands directly

- [ ] **Step 3: Wire the helper into root-command dispatch**

In `src/main.rs`:
- add `mod live_host_safety;`
- import the helper types
- call the helper before dispatch for:
  - `Commands::Install`
  - `Commands::Remove`
  - `Commands::Update`
  - `Commands::Autoremove`

Use `LiveMutationClass::CurrentlyLiveEvenWithRootArguments` for all four
root-command arms in this task.

Do the check before smart dispatch branches so:
- `install @collection` is covered at the `install` command boundary
- `update @group` is covered at the `update` command boundary

Use an explicit root-command matrix so the wiring is mechanical:
- `install` -> label `install`, class `CurrentlyLiveEvenWithRootArguments`, `dry_run` from the parsed command
- `remove` -> label `remove`, class `CurrentlyLiveEvenWithRootArguments`, `dry_run = false`
- `update` -> label `update`, class `CurrentlyLiveEvenWithRootArguments`, `dry_run = false`
- `autoremove` -> label `autoremove`, class `CurrentlyLiveEvenWithRootArguments`, `dry_run` from the parsed command

Keep `install @collection` and `update @group` under the same command-boundary
check instead of adding separate nested gates.

- [ ] **Step 4: Extend the integration file with dry-run bypass coverage**

Add a test that proves the safety gate is skipped for dry runs:

```rust
#[test]
fn install_dry_run_does_not_require_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["install", "bash", "--dry-run", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
```

It is fine if the command later fails for another reason such as database setup;
the assertion here is specifically that the safety refusal did not fire.

- [ ] **Step 5: Run focused verification**

Run:
- `cargo test --test live_host_mutation_safety install_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety remove_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety update_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety autoremove_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety install_collection_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety update_group_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety install_dry_run_does_not_require_live_mutation_ack -- --exact`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/live_host_safety.rs tests/live_host_mutation_safety.rs
git commit -m "feat(cli): gate root mutation commands behind live host ack"
```

### Task 6: Gate Nested System and CCS Entry Points

**Files:**
- Modify: `src/main.rs`
- Modify: `src/live_host_safety.rs`
- Test: `tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Add failing integration tests for nested commands**

```rust
#[test]
fn generation_switch_refuses_without_live_mutation_ack() {
    // Use an intentionally impossible generation number so the pre-gate red
    // phase cannot accidentally hit a real live-switch path on the worker.
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "generation", "switch", "-999999"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn ccs_install_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let target_root = temp_dir.path().join("target-root");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "ccs",
            "install",
            "missing.ccs",
            "--db-path",
        ])
        .arg(&db_path)
        .args(["--root"])
        .arg(&target_root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("active host"));
}

#[test]
fn system_restore_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "restore", "all", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn state_rollback_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "state", "rollback", "1", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn generation_recover_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "generation", "recover", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn takeover_refuses_without_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "takeover", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}
```

- [ ] **Step 2: Run the failing nested-command tests**

Run:
- `cargo test --test live_host_mutation_safety generation_switch_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety ccs_install_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety system_restore_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety state_rollback_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety generation_recover_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety takeover_refuses_without_live_mutation_ack -- --exact`

Expected: FAIL until nested dispatch is gated

- [ ] **Step 3: Wire the remaining approved commands in `main.rs`**

Add helper calls before dispatch for:
- `cli::SystemCommands::Restore`
- `cli::StateCommands::Rollback`
- `cli::GenerationCommands::Switch`
- `cli::GenerationCommands::Rollback`
- `cli::GenerationCommands::Recover`
- `cli::SystemCommands::Takeover`
- `cli::CcsCommands::Install`

Use the approved classes:
- `AlwaysLive` for generation switch/rollback/recover and takeover
- `CurrentlyLiveEvenWithRootArguments` for restore, state rollback, and CCS install

Keep the dispatch changes small and explicit. Do not add macros.

Use an explicit nested-command matrix so every gated arm is mechanically wired
and independently testable:
- `system restore` -> label `system restore`, class `CurrentlyLiveEvenWithRootArguments`, `dry_run` from the parsed command
- `system state rollback` -> label `system state rollback`, class `CurrentlyLiveEvenWithRootArguments`, `dry_run = false`
- `system generation switch` -> label `system generation switch`, class `AlwaysLive`, `dry_run = false`
- `system generation rollback` -> label `system generation rollback`, class `AlwaysLive`, `dry_run = false`
- `system generation recover` -> label `system generation recover`, class `AlwaysLive`, `dry_run = false`
- `system takeover` -> label `system takeover`, class `AlwaysLive`, `dry_run` from the parsed command
- `ccs install` -> label `ccs install`, class `CurrentlyLiveEvenWithRootArguments`, `dry_run` from the parsed command

- [ ] **Step 4: Add the post-wire unsafe-arm proof and nested dry-run bypass tests**

After wiring, it is safe to add the `generation rollback` refusal test because
the new gate will intercept before rollback mechanics reach live `/conary`
state. Add that proof now, together with the dry-run bypass cases:

```rust
#[test]
fn generation_rollback_refuses_without_live_mutation_ack() {
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "generation", "rollback"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn takeover_dry_run_does_not_require_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["system", "takeover", "--dry-run", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn ccs_install_dry_run_does_not_require_live_mutation_ack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let target_root = temp_dir.path().join("target-root");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "ccs",
            "install",
            "missing.ccs",
            "--dry-run",
            "--db-path",
        ])
        .arg(&db_path)
        .args(["--root"])
        .arg(&target_root)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
```

- [ ] **Step 5: Run focused verification**

Run:
- `cargo test --test live_host_mutation_safety generation_switch_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety ccs_install_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety system_restore_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety state_rollback_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety generation_rollback_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety generation_recover_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety takeover_refuses_without_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety takeover_dry_run_does_not_require_live_mutation_ack -- --exact`
- `cargo test --test live_host_mutation_safety ccs_install_dry_run_does_not_require_live_mutation_ack -- --exact`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/live_host_safety.rs tests/live_host_mutation_safety.rs
git commit -m "feat(cli): gate nested host mutation commands behind ack"
```

## Chunk 4: Final Verification And Polish

### Task 7: Tighten Parser Coverage and Regression-Proof the Command Matrix

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Add parser tests for representative command families**

```rust
#[test]
fn global_live_mutation_flag_parses_for_install() {
    Cli::try_parse_from(["conary", "--allow-live-system-mutation", "install", "bash"]).unwrap();
}

#[test]
fn global_live_mutation_flag_parses_for_ccs_install() {
    Cli::try_parse_from([
        "conary",
        "--allow-live-system-mutation",
        "ccs",
        "install",
        "pkg.ccs",
    ])
    .unwrap();
}
```

- [ ] **Step 2: Add integration assertions for the full warning contract**

Extend `ccs_install_refuses_without_live_mutation_ack` so the full stderr
checks for all of:
- `--allow-live-system-mutation`
- `early software`
- `active host`
- `--root`
- `not sufficient isolation`
- `generation`
- `/usr`
- `/etc`
- `scriptlet`
- `ownership`

Also add a read-only regression test such as:

```rust
#[test]
fn list_is_unaffected_by_live_mutation_gate() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["list", "--db-path"])
        .arg(&db_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
```

And add a positive-path integration test such as:

```rust
#[test]
fn ccs_install_with_ack_does_not_hit_safety_refusal() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let target_root = temp_dir.path().join("target-root");
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "--allow-live-system-mutation",
            "ccs",
            "install",
            "missing.ccs",
            "--db-path",
        ])
        .arg(&db_path)
        .args(["--root"])
        .arg(&target_root)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
```

- [ ] **Step 3: Run the narrow parser and integration tests**

Run:
- `cargo test cli_accepts_allow_live_system_mutation_as_global_flag -- --exact`
- `cargo test global_live_mutation_flag_parses_for_install -- --exact`
- `cargo test global_live_mutation_flag_parses_for_ccs_install -- --exact`
- `cargo test --test live_host_mutation_safety`

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs tests/live_host_mutation_safety.rs
git commit -m "test(cli): cover live host mutation flag parsing and warnings"
```

### Task 8: Run the Full Verification Set

**Files:**
- Modify: none
- Test: `src/cli/mod.rs`
- Test: `src/live_host_safety.rs`
- Test: `tests/live_host_mutation_readiness.rs`
- Test: `tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Run the focused binary and unit coverage**

Run:
- `cargo test --test live_host_mutation_readiness`
- `cargo test --test live_host_mutation_safety`
- `cargo test live_host_safety -- --nocapture`
- `cargo test cli_accepts_allow_live_system_mutation_as_global_flag -- --exact`
- `cargo test allow_live_mutation_ack_passes -- --exact`

Expected: PASS

- [ ] **Step 2: Run the main crate test suite**

Run: `cargo test`
Expected: PASS

- [ ] **Step 3: Run lint verification**

Run: `cargo clippy -- -D warnings`
Expected: PASS

- [ ] **Step 4: Re-run the disposable-host audit suites after the gate is wired**

Run:
- `cargo run -p conary-test -- run --suite phase1-advanced.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase2-group-a.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase2-group-b.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-h.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-j.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-l.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase3-group-m.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase4-group-d.toml --distro fedora43`
- `cargo run -p conary-test -- run --suite phase4-group-e.toml --distro fedora43`

Expected: PASS

- [ ] **Step 5: Record the final change summary in the final assistant handoff message**

Use the final assistant response that closes plan execution as the artifact.
It should be possible to diff the response against this checklist and verify
that every item below was covered.

Capture:
- commands gated
- how the gate can be retired later
- dry-run bypass cases
- refusal wording rationale
- readiness gaps found and fixed
- tests added and commands run

- [ ] **Step 6: Final commit if needed**

```bash
git add src/cli/mod.rs src/live_host_safety.rs src/main.rs tests/live_host_mutation_readiness.rs tests/live_host_mutation_safety.rs tests/workflow.rs tests/component.rs tests/batch_install.rs tests/integration/remi/manifests/phase1-advanced.toml tests/integration/remi/manifests/phase2-group-a.toml tests/integration/remi/manifests/phase2-group-b.toml tests/integration/remi/manifests/phase3-group-h.toml tests/integration/remi/manifests/phase3-group-j.toml tests/integration/remi/manifests/phase3-group-l.toml tests/integration/remi/manifests/phase3-group-m.toml tests/integration/remi/manifests/phase4-group-d.toml tests/integration/remi/manifests/phase4-group-e.toml src/commands/update.rs src/commands/state.rs src/commands/system.rs src/commands/generation/commands.rs src/commands/generation/takeover.rs src/commands/generation/takeover_state.rs src/commands/restore.rs src/commands/remove.rs src/commands/install/mod.rs src/commands/ccs/install.rs
git commit -m "feat(cli): require explicit ack for live host mutations"
```
