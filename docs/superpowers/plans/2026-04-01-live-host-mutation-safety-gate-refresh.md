# Live Host Mutation Safety Gate Refresh Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the refreshed `--allow-live-system-mutation` CLI gate for the 13 covered command families, prove the gate through CLI-facing tests, and either add or honestly surface any missing readiness evidence for covered commands such as `system restore` and `system generation recover`.

**Architecture:** Keep the policy in the `apps/conary` CLI boundary: Clap parses one global flag, `dispatch.rs` constructs a `LiveMutationRequest` for each covered command arm, and `live_host_safety.rs` owns classification plus refusal text. Use explicit manifest and documentation edits instead of `conary-test` harness auto-injection so every intentional covered-command invocation visibly opts in to live host mutation. Preserve the spec's readiness bar: disposable-host or CLI-facing proof is primary, local seam tests are supporting evidence only.

**Tech Stack:** Rust 2024 workspace, Clap, `anyhow`, cargo unit/integration tests, `Command::new(env!("CARGO_BIN_EXE_conary"))` CLI tests, `conary-test` manifest runner, Markdown docs under `docs/`.

---

## Preconditions

- If you execute this plan in a git worktree, do not reuse the stale March
  worktree at `.worktrees/live-host-mutation-safety-gate/`. Either remove it
  first or create a fresh sibling such as
  `.worktrees/live-host-mutation-safety-gate-refresh/`.
- `conary-test run --suite` accepts either a repository-relative manifest path
  or a suite name/stem. This plan uses repository-relative manifest paths to
  keep the target unambiguous.

## File Map

- `apps/conary/src/main.rs`: register the new `live_host_safety` module in the binary crate
- `apps/conary/src/cli/mod.rs`: add the global `--allow-live-system-mutation` flag and parser tests
- `apps/conary/src/dispatch.rs`: wire the gate into the 13 covered command families with explicit user-facing labels and `--yes`-independent enforcement
- `apps/conary/src/live_host_safety.rs`: new helper module for command classes, request shape, refusal text, and dry-run bypass logic
- `apps/conary/tests/live_host_mutation_safety.rs`: new CLI-facing refusal/bypass/allow integration tests
- `apps/conary/tests/live_host_mutation_readiness.rs`: new local readiness smoke tests for safe, non-live seams
- `apps/conary/tests/component.rs`: existing CLI-facing `ccs install` success coverage that must opt in explicitly
- `apps/conary/tests/workflow.rs`: update intentional covered-command CLI invocations to pass the new flag
- `apps/conary/tests/integration/remi/manifests/phase1-core.toml`: baseline install coverage, including an expected underlying install failure that should still reach the real command path
- `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`: generation GC and takeover dry-run coverage
- `apps/conary/tests/integration/remi/manifests/phase2-group-a.toml`: CCS install and install/remove coverage
- `apps/conary/tests/integration/remi/manifests/phase2-group-b.toml`: generation build/switch/gc and takeover generation-ready coverage
- `apps/conary/tests/integration/remi/manifests/phase2-group-e.toml`: install coverage that currently uses `--yes`
- `apps/conary/tests/integration/remi/manifests/phase3-group-g.toml`: native/CCS install coverage
- `apps/conary/tests/integration/remi/manifests/phase3-group-h.toml`: generation build/switch and rollback concurrency coverage
- `apps/conary/tests/integration/remi/manifests/phase3-group-i.toml`: CCS install adversarial coverage
- `apps/conary/tests/integration/remi/manifests/phase3-group-j.toml`: CCS install dependency coverage
- `apps/conary/tests/integration/remi/manifests/phase3-group-l.toml`: generation build/gc/switch/rollback coverage and home for generation recover additions
- `apps/conary/tests/integration/remi/manifests/phase3-group-m.toml`: install/update/CCS/install-from-repo coverage
- `apps/conary/tests/integration/remi/manifests/phase3-group-n-container.toml`: generation build coverage
- `apps/conary/tests/integration/remi/manifests/phase3-group-n-qemu.toml`: kernel install and generation rollback coverage
- `apps/conary/tests/integration/remi/manifests/phase4-group-a.toml`: CCS install coverage
- `apps/conary/tests/integration/remi/manifests/phase4-group-b.toml`: CCS install coverage
- `apps/conary/tests/integration/remi/manifests/phase4-group-c.toml`: CCS install coverage
- `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`: CCS install and `system restore` coverage; add non-dry-run restore evidence here
- `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`: install, generation build, takeover dry-run, takeover owned, and model/apply cross-distro coverage
- `docs/conaryopedia-v2.md`: update runnable examples for covered commands
- `docs/modules/ccs.md`: update `ccs install` examples
- `docs/superpowers/specs/2026-04-01-live-host-mutation-safety-gate-refresh-design.md`: approved spec reference
- `docs/superpowers/specs/2026-03-31-live-host-mutation-safety-design.md`: superseded spec to archive once the replacement plan lands
- `docs/superpowers/plans/2026-03-31-live-host-mutation-safety-gate.md`: superseded plan to archive once the replacement plan lands

## Covered Command Map

- `AlwaysLive`
  - `conary system generation build`
  - `conary system generation gc`
    because it deletes generation directories, `etc-state` overlays, and BLS
    boot entries before running CAS cleanup
  - `conary system generation switch`
  - `conary system generation rollback`
  - `conary system generation recover`
  - `conary system takeover`
- `CurrentlyLiveEvenWithRootArguments`
  - `conary install`
  - `conary install @collection`
  - `conary remove`
  - `conary update`
  - `conary update @collection`
  - `conary autoremove`
  - `conary ccs install`
  - `conary system restore`
  - `conary system state rollback`

## Verification Principles

- Treat `cargo test -p conary --test ...` and helper unit tests as supporting seam coverage, not as sole readiness proof for mutating commands.
- Treat `cargo run -p conary-test -- run --distro fedora43 --phase ... --suite ...` as the primary disposable-host proof where the repo already has manifest coverage.
- Update manifests and docs directly; do not hide the new flag behind `conary-test` harness injection.
- Keep `system generation gc` in scope even though plain `system gc` stays
  excluded: generation GC deletes generation artifacts, `etc-state`
  directories, and boot-loader entries, so it mutates generation state rather
  than only pruning detached CAS objects.
- Keep refusal-path tests focused on the durable contract:
  - command-specific refusal text
  - normal `app.rs` error-path reporting
  - nonzero exit
  - no brittle exact-exit-code or full-message matching
- If `system restore` non-dry-run or `system generation recover` cannot be covered honestly, stop and surface a release blocker instead of weakening the plan.

## Chunk 1: Guard Contract

### Task 1: Add the global flag and helper seam

**Files:**
- Modify: `apps/conary/src/main.rs`
- Modify: `apps/conary/src/cli/mod.rs`
- Create: `apps/conary/src/live_host_safety.rs`
- Test: `apps/conary/src/cli/mod.rs`
- Test: `apps/conary/src/live_host_safety.rs`

- [ ] **Step 1: Write the failing parser and helper tests**

Add a parser test in `apps/conary/src/cli/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn cli_accepts_allow_live_system_mutation_as_global_flag() {
        let cli = Cli::try_parse_from([
            "conary",
            "--allow-live-system-mutation",
            "system",
            "generation",
            "switch",
            "7",
        ])
        .expect("global live-mutation flag should parse before nested commands");

        assert!(cli.allow_live_system_mutation);
    }
}
```

Create `apps/conary/src/live_host_safety.rs` with the repository path comment,
a red-phase test module, and `todo!()` stubs:

```rust
// apps/conary/src/live_host_safety.rs

use std::borrow::Cow;

pub enum LiveMutationClass {
    AlwaysLive,
    CurrentlyLiveEvenWithRootArguments,
}

pub struct LiveMutationRequest {
    pub command_label: Cow<'static, str>,
    pub class: LiveMutationClass,
    pub dry_run: bool,
}

pub fn require_live_system_mutation_ack(
    allow_live_system_mutation: bool,
    request: &LiveMutationRequest,
) -> anyhow::Result<()> {
    todo!("implement live mutation gate")
}

#[cfg(test)]
mod tests {
    use super::{LiveMutationClass, LiveMutationRequest, require_live_system_mutation_ack};
    use std::borrow::Cow;

    #[test]
    fn dry_run_bypasses_live_mutation_ack() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: true,
        };
        assert!(require_live_system_mutation_ack(false, &request).is_ok());
    }

    #[test]
    fn missing_ack_mentions_early_software_rationale() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
        };
        let err = require_live_system_mutation_ack(false, &request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("--allow-live-system-mutation"));
        assert!(message.contains("early software"));
    }

    #[test]
    fn allow_live_mutation_ack_passes() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
        };
        assert!(require_live_system_mutation_ack(true, &request).is_ok());
    }
}
```

Also add `mod live_host_safety;` in `apps/conary/src/main.rs`.

- [ ] **Step 2: Run the red-phase tests**

Run:
- `cargo test -p conary cli_accepts_allow_live_system_mutation_as_global_flag -- --exact`
- `cargo test -p conary dry_run_bypasses_live_mutation_ack -- --exact`
- `cargo test -p conary missing_ack_mentions_early_software_rationale -- --exact`
- `cargo test -p conary allow_live_mutation_ack_passes -- --exact`

Expected: FAIL because the flag is not parsed yet and the helper still contains
`todo!()`.

- [ ] **Step 3: Add the flag and minimal helper implementation**

Implement the new field on `Cli` in `apps/conary/src/cli/mod.rs`:

```rust
/// Acknowledge that this command may mutate the active host.
#[arg(long, global = true)]
pub allow_live_system_mutation: bool,
```

Replace the helper stubs with a minimal implementation in
`apps/conary/src/live_host_safety.rs`:

```rust
use anyhow::bail;
use std::borrow::Cow;

pub enum LiveMutationClass {
    AlwaysLive,
    CurrentlyLiveEvenWithRootArguments,
}

pub struct LiveMutationRequest {
    pub command_label: Cow<'static, str>,
    pub class: LiveMutationClass,
    pub dry_run: bool,
}

pub fn require_live_system_mutation_ack(
    allow_live_system_mutation: bool,
    request: &LiveMutationRequest,
) -> anyhow::Result<()> {
    if request.dry_run || allow_live_system_mutation {
        return Ok(());
    }

    bail!(
        "command '{}' may mutate the active host; Conary is still early software, so \
         rerun with --allow-live-system-mutation only if you intend to modify the real machine.",
        request.command_label
    )
}
```

- [ ] **Step 4: Re-run the focused tests**

Run:
- `cargo test -p conary cli_accepts_allow_live_system_mutation_as_global_flag -- --exact`
- `cargo test -p conary dry_run_bypasses_live_mutation_ack -- --exact`
- `cargo test -p conary missing_ack_mentions_early_software_rationale -- --exact`
- `cargo test -p conary allow_live_mutation_ack_passes -- --exact`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/main.rs apps/conary/src/cli/mod.rs apps/conary/src/live_host_safety.rs
git commit -m "feat(cli): add live host mutation gate foundation"
```

### Task 2: Lock the message contract and command classes

**Files:**
- Modify: `apps/conary/src/live_host_safety.rs`
- Test: `apps/conary/src/live_host_safety.rs`

- [ ] **Step 1: Add failing message and class tests**

Extend the helper test module with:

```rust
#[test]
fn refusal_lists_live_host_risks() {
    let request = LiveMutationRequest {
        command_label: Cow::Borrowed("conary system generation switch"),
        class: LiveMutationClass::AlwaysLive,
        dry_run: false,
    };
    let err = require_live_system_mutation_ack(false, &request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("conary system generation switch"));
    assert!(message.contains("mutate the active host"));
    assert!(message.contains("/usr"));
    assert!(message.contains("/etc"));
    assert!(message.contains("scriptlet"));
}

#[test]
fn currently_live_root_command_mentions_root_is_not_isolation_yet() {
    let request = LiveMutationRequest {
        command_label: Cow::Borrowed("conary ccs install"),
        class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        dry_run: false,
    };
    let err = require_live_system_mutation_ack(false, &request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("conary ccs install"));
    assert!(message.contains("--root"));
    assert!(message.contains("not sufficient isolation"));
}
```

- [ ] **Step 2: Run the focused helper tests**

Run:
- `cargo test -p conary refusal_lists_live_host_risks -- --exact`
- `cargo test -p conary currently_live_root_command_mentions_root_is_not_isolation_yet -- --exact`

Expected: FAIL until the refusal text is expanded.

- [ ] **Step 3: Expand the refusal builder**

Make `require_live_system_mutation_ack()` produce one shared refusal message
that includes:

- the command label
- the "Conary is still early software" rationale
- generation rebuild / activation work
- `/usr` remounts
- live `/etc` overlay changes
- scriptlet execution
- takeover or rollback ownership changes
- the extra `--root` limitation line for
  `CurrentlyLiveEvenWithRootArguments`
- the final rerun guidance with `--allow-live-system-mutation`

Do not add root-aware allow logic in this slice.

- [ ] **Step 4: Re-run the helper module tests**

Run:
- `cargo test -p conary live_host_safety -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/live_host_safety.rs
git commit -m "feat(cli): tighten live host mutation refusal contract"
```

## Chunk 2: Dispatch Wiring And Gate Tests

### Task 3: Add CLI-facing refusal, bypass, and allow tests

**Files:**
- Create: `apps/conary/tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `apps/conary/tests/live_host_mutation_safety.rs`:

```rust
// apps/conary/tests/live_host_mutation_safety.rs

mod common;

use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

#[test]
fn install_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "install",
        "nginx",
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
    assert!(stderr.contains("Error:"));
    assert!(stderr.contains("conary install"));
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn collection_install_refusal_uses_collection_label() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "install",
        "@web-stack",
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
    assert!(stderr.contains("conary install @collection"));
}

#[test]
fn system_restore_dry_run_bypasses_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "system",
        "restore",
        "all",
        "--dry-run",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
    ]);

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn allow_flag_reaches_underlying_restore_error() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "system",
        "restore",
        "missing-package",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));
    assert!(!stderr.contains("allow-live-system-mutation only if"));
}

#[test]
fn excluded_system_gc_is_not_gated() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let missing_objects = tempfile::tempdir().unwrap().path().join("objects");

    let output = run_conary(&[
        "system",
        "gc",
        "--db-path",
        &db_path,
        "--objects-dir",
        missing_objects.to_str().unwrap(),
        "--dry-run",
    ]);

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
```

- [ ] **Step 2: Run the new integration tests**

Run:
- `cargo test -p conary --test live_host_mutation_safety -- --nocapture`

Expected: FAIL before dispatch wiring exists.

### Task 4: Wire the 13 covered command families in `dispatch.rs`

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Test: `apps/conary/tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Add a local dispatch helper**

Near the top of `apps/conary/src/dispatch.rs`, add imports and a small helper:

```rust
use std::borrow::Cow;

use crate::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, require_live_system_mutation_ack,
};

fn require_live_mutation(
    allow_live_system_mutation: bool,
    command_label: Cow<'static, str>,
    class: LiveMutationClass,
    dry_run: bool,
) -> Result<()> {
    require_live_system_mutation_ack(
        allow_live_system_mutation,
        &LiveMutationRequest {
            command_label,
            class,
            dry_run,
        },
    )
}
```

- [ ] **Step 2: Capture the global allow flag before matching**

At the start of `dispatch(cli: Cli)`:

```rust
pub async fn dispatch(cli: Cli) -> Result<()> {
    let allow_live_system_mutation = cli.allow_live_system_mutation;

    match cli.command {
        // ...
    }
}
```

- [ ] **Step 3: Gate every covered dispatch arm with the explicit labels**

Add `require_live_mutation(...) ?;` before the implementation call in these
arms:

- `Install` package path -> label `conary install`
- `Install` collection wrapper -> label `conary install @collection`
- `Remove` -> label `conary remove`
- `Update` package path -> label `conary update`
- `Update` collection wrapper -> label `conary update @collection`
- `Autoremove` -> label `conary autoremove`
- `SystemCommands::Restore` -> label `conary system restore`
- `StateCommands::Rollback` -> label `conary system state rollback`
- `GenerationCommands::Build` -> label `conary system generation build`
- `GenerationCommands::Gc` -> label `conary system generation gc`
  because it removes generation directories, overlay state, and BLS entries
- `GenerationCommands::Switch` -> label `conary system generation switch`
- `GenerationCommands::Rollback` -> label `conary system generation rollback`
- `GenerationCommands::Recover` -> label `conary system generation recover`
- `SystemCommands::Takeover` -> label `conary system takeover`
- `CcsCommands::Install` -> label `conary ccs install`

Use the spec's class map exactly. `--yes` must not bypass the gate.
`system gc` must stay untouched.
For any covered arm that does not destructure a `dry_run` field
(`Remove`, both `Update` paths, `StateCommands::Rollback`, and the listed
`GenerationCommands` arms), pass `false` as the helper's `dry_run` argument.

- [ ] **Step 4: Re-run the new safety integration tests**

Run:
- `cargo test -p conary --test live_host_mutation_safety -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Run formatting and linting on the Rust changes**

Run:
- `cargo fmt --check`
- `cargo clippy -p conary --all-targets -- -D warnings`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/dispatch.rs apps/conary/tests/live_host_mutation_safety.rs
git commit -m "feat(cli): gate live host mutation commands in dispatch"
```

## Chunk 3: Readiness Proof And Coverage Gaps

### Task 5: Add the local readiness smoke file

**Files:**
- Create: `apps/conary/tests/live_host_mutation_readiness.rs`

- [ ] **Step 1: Write the local readiness smoke tests**

Create `apps/conary/tests/live_host_mutation_readiness.rs`:

```rust
// apps/conary/tests/live_host_mutation_readiness.rs

mod common;

use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

#[test]
fn system_restore_all_dry_run_reports_missing_cas_without_live_mounting() {
    let (_temp_dir, db_path) = common::setup_command_test_db();
    let root_dir = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "system",
        "restore",
        "all",
        "--dry-run",
        "--db-path",
        &db_path,
        "--root",
        root_dir.path().to_str().unwrap(),
    ]);

    assert!(
        output.status.success(),
        "restore dry-run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}
```

If the current fixture helper makes a second safe smoke obvious, add it here.
Do not use this file to fake live mutation; keep it isolated and host-safe.

- [ ] **Step 2: Run the readiness smoke tests**

Run:
- `cargo test -p conary --test live_host_mutation_readiness -- --nocapture`

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add apps/conary/tests/live_host_mutation_readiness.rs
git commit -m "test(cli): add local live mutation readiness smoke coverage"
```

### Task 6: Add missing disposable-host proof for `system restore` and make `generation recover` pass or block the feature

**Files:**
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-l.toml`

- [ ] **Step 1: Add a non-dry-run `system restore` manifest case**

Extend `phase4-group-d.toml` with a real restore case that:

1. installs the phase4 runtime fixture
2. inspects the fixture's tracked files and chooses a real restore target
3. perturbs or removes that tracked file
4. runs `conary system restore <package> --allow-live-system-mutation`
5. asserts either the restored file content or the expected generation output

Use the same explicit flag in the manifest command string. A representative
step block should look like. Treat the path below as illustrative only; verify
the actual phase 4 fixture layout before choosing the file you mutate:

```toml
[[test]]
id = "T239b"
name = "system_restore_non_dry_run"
description = "Restore package files from CAS with live mutation explicitly allowed"
timeout = 180
group = "D"

[[test.step]]
run = "printf 'broken\n' > /etc/phase4-runtime-fixture/app.conf"

[[test.step]]
conary = "system restore ${FIXTURE_PKG_NAME} --allow-live-system-mutation"

[test.step.assert]
exit_code = 0
stdout_contains_any = ["Restore complete", "generation"]
```

- [ ] **Step 2: Run the targeted phase 4 suite**

Run:
- `cargo run -p conary-test -- run --distro fedora43 --phase 4 --suite apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`

Expected: PASS with the new restore case included.

- [ ] **Step 3: Audit whether the harness can exercise honest recover scenarios**

Before writing any recover manifest case, inspect
`apps/conary/src/commands/generation/commands.rs` and verify the disposable-host
harness can actually satisfy the command's prerequisites:

- a database whose parent directory is the synthetic Conary root used by recover
- a usable `<root>/current` generation reference plus generation directories
- at least one recoverable or intentionally broken generation image/state
- a staging mount path under that root without depending on the developer's
  real `/conary`

Only continue if you can describe one successful recovery path and one clean
failure-or-fallback path that exercise the documented CLI behavior rather than
argument parsing or unrelated setup errors.

- [ ] **Step 4: If the audit passes, add one successful and one clean failure/fallback recover case**

Extend `phase3-group-l.toml` so `system generation recover` gets:

- one scenario that can complete with `Recovery complete.`
- one scenario that fails or falls back cleanly without panic

Keep the assertions tied to the CLI contract, not internal implementation
details. Do not count missing-db, missing-directory, or parser-only failures as
readiness evidence. A representative failure-path step should look like:

```toml
[[test]]
id = "T135b"
name = "generation_recover_fails_cleanly"
description = "Generation recover fails or falls back cleanly without panic"
timeout = 120
group = "L"

[[test.step]]
run = "${CONARY_BIN} system generation recover --allow-live-system-mutation 2>&1 || true"

[test.step.assert]
stdout_contains_any = ["Recovery complete.", "recover", "generation", "failed"]
stdout_not_contains = "panic"
```

If the audit or manifest work shows the harness cannot automate both scenarios
honestly, stop here and surface `system generation recover` as a blocker
instead of pretending the coverage exists.

- [ ] **Step 5: Run the targeted phase 3 lifecycle suite or stop with a blocker**

Run:
- `cargo run -p conary-test -- run --distro fedora43 --phase 3 --suite apps/conary/tests/integration/remi/manifests/phase3-group-l.toml`

Expected:
- PASS if both recover scenarios were added honestly
- otherwise stop the feature work at this point and hand back a blocker report
  that names the missing recover-harness prerequisite(s)

- [ ] **Step 6: Commit**

If both restore and recover evidence land:

```bash
git add apps/conary/tests/integration/remi/manifests/phase4-group-d.toml apps/conary/tests/integration/remi/manifests/phase3-group-l.toml
git commit -m "test(cli): expand live mutation readiness evidence"
```

If `generation recover` remains a blocker, commit only the restore work plus
any recover-audit notes/tests that were useful, then stop the implementation
flow without shipping the final feature commit.

## Chunk 4: Flag Injection In Existing Suites, Docs, And Final Verification

### Task 7: Update existing intentional invocations to opt in explicitly

**Files:**
- Modify: `apps/conary/tests/component.rs`
- Modify: `apps/conary/tests/workflow.rs`
- Modify: `apps/conary/tests/integration/remi/manifests/phase1-core.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase2-group-a.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase2-group-b.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase2-group-e.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-g.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-h.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-i.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-j.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-l.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-m.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-n-container.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-n-qemu.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-a.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-b.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-c.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/modules/ccs.md`

- [ ] **Step 1: Update local CLI tests that intentionally mutate**

In `apps/conary/tests/workflow.rs` and `apps/conary/tests/component.rs`, add
the new flag anywhere a covered command is expected to succeed. For example:

```rust
let install_output = Command::new(env!("CARGO_BIN_EXE_conary"))
    .arg("--allow-live-system-mutation")
    .arg("install")
    .arg(package_path.to_str().unwrap())
    .arg("--db-path")
    .arg(&db_path)
    .arg("--root")
    .arg(install_root.to_str().unwrap())
    .arg("--sandbox")
    .arg("never")
    .arg("--yes")
    .output()
    .unwrap();

let ccs_output = Command::new(env!("CARGO_BIN_EXE_conary"))
    .arg("--allow-live-system-mutation")
    .arg("ccs")
    .arg("install")
    .arg(package_path.to_str().unwrap())
    .arg("--components")
    .arg("devel")
    .arg("--allow-unsigned")
    .arg("--sandbox")
    .arg("never")
    .arg("--db-path")
    .arg(db_path.to_str().unwrap())
    .arg("--root")
    .arg(install_root.to_str().unwrap())
    .output()
    .unwrap();
```

- [ ] **Step 2: Update manifest commands directly instead of using harness injection**

Start by running the audit commands in Step 4 and turn the matches into an edit
queue grouped by file. On current `main`, expect candidate matches in:
`phase1-core`, `phase1-advanced`, `phase2-group-a`, `phase2-group-b`,
`phase2-group-e`, `phase3-group-g`, `phase3-group-h`, `phase3-group-i`,
`phase3-group-j`, `phase3-group-l`, `phase3-group-m`,
`phase3-group-n-container`, `phase4-group-a`, `phase4-group-b`,
`phase4-group-c`, `phase4-group-d`, and `phase4-group-e`.

For every intentional covered-command invocation in the listed manifest files:

- add `--allow-live-system-mutation` to `conary = "..."` forms
- add `--allow-live-system-mutation` to raw `run = "${CONARY_BIN} ..."` forms
- add the flag to expected underlying command-failure cases that should still
  reach the real implementation, such as
  `install zzz-nonexistent-pkg-12345` in `phase1-core.toml`
- leave `--dry-run` takeover and restore probes alone when the spec says they
  bypass the gate
- leave the dedicated refusal-path coverage in
  `apps/conary/tests/live_host_mutation_safety.rs` as the only intentional
  no-flag coverage for covered commands
- do not add the flag to excluded commands such as `system gc`, `system adopt`,
  `automation apply`, or `model apply`

Representative manifest edits:

```toml
conary = "install ${FIXTURE_PKG_NAME} --repo ${REPO_NAME} --dep-mode takeover --allow-live-system-mutation --yes --sandbox never"
conary = "ccs install ${FIXTURE_V1_CCS} --allow-live-system-mutation --allow-unsigned --sandbox never"
run = "${CONARY_BIN} system generation build --allow-live-system-mutation 2>&1"
conary = "system takeover --allow-live-system-mutation --up-to owned --yes"
```

- [ ] **Step 3: Update user-facing docs/examples**

In `docs/conaryopedia-v2.md` and `docs/modules/ccs.md`, update intentional
covered-command examples to include the new flag while preserving dry-run
examples. Representative edits:

```bash
conary ccs install package.ccs --allow-live-system-mutation
conary install @web-stack --allow-live-system-mutation
conary system takeover --allow-live-system-mutation --up-to owned
conary system generation switch 1 --allow-live-system-mutation
```

Also add one short note near the takeover examples that `--yes` skips prompts
but does not replace `--allow-live-system-mutation`.

- [ ] **Step 4: Audit for missing explicit opt-ins**

Run:
- `rg -n 'conary = "(install|update|autoremove|ccs install|system restore|system takeover)' apps/conary/tests/integration/remi/manifests`
- `rg -n 'run = ".*\\$\\{CONARY_BIN\\} (install|update|autoremove|ccs install|system generation (build|gc|switch|rollback|recover)|system restore|system takeover)' apps/conary/tests/integration/remi/manifests`
- `rg -n 'Command::new\\(env!\\("CARGO_BIN_EXE_conary"\\)\\)' apps/conary/tests/component.rs apps/conary/tests/workflow.rs`
- `rg -n 'conary (install @|install |update @|ccs install|system takeover|system generation switch)' docs/conaryopedia-v2.md docs/modules/ccs.md`

Expected: every intentional covered-command invocation that should reach the
underlying command behavior includes `--allow-live-system-mutation`, while
excluded commands remain unchanged.

- [ ] **Step 5: Commit**

```bash
git add \
  apps/conary/tests/component.rs \
  apps/conary/tests/workflow.rs \
  apps/conary/tests/integration/remi/manifests/phase1-core.toml \
  apps/conary/tests/integration/remi/manifests/phase1-advanced.toml \
  apps/conary/tests/integration/remi/manifests/phase2-group-a.toml \
  apps/conary/tests/integration/remi/manifests/phase2-group-b.toml \
  apps/conary/tests/integration/remi/manifests/phase2-group-e.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-g.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-h.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-i.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-j.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-l.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-m.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-n-container.toml \
  apps/conary/tests/integration/remi/manifests/phase3-group-n-qemu.toml \
  apps/conary/tests/integration/remi/manifests/phase4-group-a.toml \
  apps/conary/tests/integration/remi/manifests/phase4-group-b.toml \
  apps/conary/tests/integration/remi/manifests/phase4-group-c.toml \
  apps/conary/tests/integration/remi/manifests/phase4-group-d.toml \
  apps/conary/tests/integration/remi/manifests/phase4-group-e.toml \
  docs/conaryopedia-v2.md \
  docs/modules/ccs.md
git commit -m "chore(cli): add explicit live mutation acknowledgment to tests and docs"
```

### Task 8: Run the final matrix and archive superseded March docs

**Files:**
- Move to archive: `docs/superpowers/specs/2026-03-31-live-host-mutation-safety-design.md`
- Move to archive: `docs/superpowers/plans/2026-03-31-live-host-mutation-safety-gate.md`

- [ ] **Step 1: Run the focused local test matrix**

Run:
- `cargo fmt --check`
- `cargo clippy -p conary --all-targets -- -D warnings`
- `cargo test -p conary cli_accepts_allow_live_system_mutation_as_global_flag -- --exact`
- `cargo test -p conary live_host_safety -- --nocapture`
- `cargo test -p conary --test live_host_mutation_safety -- --nocapture`
- `cargo test -p conary --test live_host_mutation_readiness -- --nocapture`
- `cargo test -p conary --test component -- --nocapture`
- `cargo test -p conary --test workflow -- --nocapture`

Expected: PASS.

- [ ] **Step 2: Run the targeted disposable-host suites**

Run:
- `cargo run -p conary-test -- run --distro fedora43 --phase 2 --suite apps/conary/tests/integration/remi/manifests/phase2-group-b.toml`
- `cargo run -p conary-test -- run --distro fedora43 --phase 3 --suite apps/conary/tests/integration/remi/manifests/phase3-group-h.toml`
- `cargo run -p conary-test -- run --distro fedora43 --phase 3 --suite apps/conary/tests/integration/remi/manifests/phase3-group-l.toml`
- `cargo run -p conary-test -- run --distro fedora43 --phase 4 --suite apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
- `cargo run -p conary-test -- run --distro fedora43 --phase 4 --suite apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`

Expected: PASS, or an explicitly surfaced blocker for `system generation recover`
if honest coverage still cannot be achieved.

- [ ] **Step 3: Archive the superseded March design and plan**

Run:

```bash
mv docs/superpowers/specs/2026-03-31-live-host-mutation-safety-design.md docs/superpowers/specs/archive/
mv docs/superpowers/plans/2026-03-31-live-host-mutation-safety-gate.md docs/superpowers/plans/archive/
git add -u docs/superpowers/specs docs/superpowers/plans
git add -f \
  docs/superpowers/specs/archive/2026-03-31-live-host-mutation-safety-design.md \
  docs/superpowers/plans/archive/2026-03-31-live-host-mutation-safety-gate.md
```

- [ ] **Step 4: Commit the archive move**

```bash
git add docs/superpowers/specs/archive/2026-03-31-live-host-mutation-safety-design.md docs/superpowers/plans/archive/2026-03-31-live-host-mutation-safety-gate.md
git commit -m "docs(superpowers): archive superseded March live mutation docs"
```

If `system restore` or `system generation recover` remains unproven, do not
ship any further feature commits beyond the earlier task commits. At that
point, only the archive move above should remain optional doc cleanup.
