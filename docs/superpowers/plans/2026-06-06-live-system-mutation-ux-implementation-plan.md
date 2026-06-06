# Live-System Mutation UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broad `--allow-live-system-mutation` user experience with
risk-tiered apply intent while preserving refusal-before-mutation safety and
compatibility for old persisted retry commands.

**Architecture:** Keep the old global flag accepted as a hidden compatibility
alias, introduce explicit mutation intent in the shared live-host helper, and
route CLI/daemon callers through the helper using command-specific apply
signals such as `--yes`. Then migrate active tests, manifests, source hints,
docs, and generated manpages so new guidance points at dry-run-first and
apply-intent flows instead of the old global phrase.

**Tech Stack:** Rust, Clap, existing Conary CLI/daemon dispatch, conary-test
manifest validation, existing Markdown docs, generated manpages, docs-audit
scripts.

---

## Status

Draft implementation plan for review.

## Read First

- `docs/superpowers/specs/2026-06-06-live-system-mutation-ux-redesign.md`
- `apps/conary/src/cli/mod.rs`
- `apps/conary/src/cli/system.rs`
- `apps/conary/src/cli/model.rs`
- `apps/conary/src/cli/ccs.rs`
- `apps/conary/src/command_risk.rs`
- `apps/conary/src/live_host_safety.rs`
- `apps/conary/src/dispatch.rs`
- `apps/conaryd/src/daemon/routes.rs`
- `apps/conaryd/src/daemon/routes/transactions.rs`
- `apps/conaryd/src/daemon/package_ops.rs`
- `apps/conaryd/src/daemon/client.rs`
- `apps/conary-test/src/config/mod.rs`
- `apps/conary/tests/live_host_mutation_safety.rs`
- `apps/conary/tests/cli_daily_ux.rs`
- `apps/conary/tests/integration/remi/manifests/`
- `docs/operations/live-mutation-backup-inventory.md`
- `docs/operations/daily-driver-ux-matrix.md`

## Design Decisions Locked By This Plan

- The old global `--allow-live-system-mutation` remains parseable in this
  implementation and acts as a compatibility alias for apply intent. Hide it
  from active help output, but do not remove it from Clap yet.
- `DbMutation` commands, including mutating `system adopt` variants, no longer
  require the live-system acknowledgement. Their safety signal is the command
  name and existing scoped options such as `--dry-run`, `--system`, `--refresh`,
  and `--convert`.
- Tier 2 active-host commands require apply intent. Existing `--yes` flags
  satisfy that intent. Commands without `--yes` gain `--yes` where the meaning
  is "apply this command's plan."
- Tier 3 always-live commands require command-specific apply intent. In this
  slice, `--yes` is the non-interactive command-specific signal for generation
  build, publish, switch, rollback, garbage collection, recover, and takeover
  flows. Broad generation operations must still print or name the concrete
  generation, boot, publication, or recovery risk before accepting `--yes`;
  refusal messages must name that exact risk.
- `self-update` keeps its existing `--force` apply intent in this slice. Do not
  add `--yes` to `self-update` unless implementation proves `--force` cannot
  cleanly satisfy the active-host policy without changing meaning.
- Spec correction: `model apply` currently uses `--strict` for
  model-convergence scope, not `--force`. Preserve `--strict` and add `--yes`
  only for active-system apply intent.
- `system restore` keeps `--force` for overwrite semantics and gains `--yes`
  for active-system apply intent. These flags are intentionally orthogonal:
  `--force` means overwrite, while `--yes` means apply the restore operation.
- conaryd keeps accepting `allow_live_system_mutation` in request bodies while
  adding preferred `apply_intent` request fields. Existing clients continue to
  work while active docs/tests move to the new field.
- Active docs and tester-facing copy migrate to new wording. Archived
  historical docs may retain old command text.

## Implementation Plan

### Task 0: Lock The Reviewed Implementation Plan And Docs-Audit Row

**Files:**
- Add: `docs/superpowers/plans/2026-06-06-live-system-mutation-ux-implementation-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the reviewed plan before regenerating docs inventory**

```bash
git add docs/superpowers/plans/2026-06-06-live-system-mutation-ux-implementation-plan.md
```

- [ ] **Step 2: Regenerate docs-audit inventory**

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected on the current baseline: tracked doc-like files grow from 150 to 151
data rows, excluding the inventory header, with this plan file added as
`planning` / `maintainer`. If another docs file lands first, use the
regenerated inventory as source of truth and update counts accordingly.

- [ ] **Step 3: Add the plan ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the other active
maintainability and live-mutation rows:

```text
docs/superpowers/plans/2026-06-06-live-system-mutation-ux-implementation-plan.md	docs/superpowers/plans/2026-06-06-live-system-mutation-ux-implementation-plan.md	planning	maintainer	live-mutation; command-risk; contributor-ux; conaryd; implementation-plan	docs/superpowers/specs/2026-06-06-live-system-mutation-ux-redesign.md; apps/conary/src/command_risk.rs; apps/conary/src/live_host_safety.rs; apps/conary/src/cli/mod.rs; apps/conary/src/dispatch.rs; apps/conary/tests/live_host_mutation_safety.rs; apps/conary/tests/cli_daily_ux.rs; apps/conaryd/src/daemon/package_ops.rs; apps/conaryd/src/daemon/routes.rs; apps/conary-test/src/config/mod.rs; scripts/bootstrap-vm/guest-validate.sh; site/src/routes/install/+page.svelte; docs/operations/live-mutation-backup-inventory.md	verified	corrected	Added the reviewed implementation plan for the live-system mutation UX redesign: hidden compatibility for the old global flag, risk-tiered apply intent, conaryd request compatibility, expanded manifest validation, active script/site/docs migration, and refusal-before-mutation verification gates.
```

- [ ] **Step 4: Update the audit summary**

Append this paragraph to the existing
`### 2026-06-06 Maintainability Planning` section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The live-system mutation UX implementation plan now turns that reviewed design
into a task-by-task behavior packet. It keeps the old global flag parseable as
a hidden compatibility alias, removes the live-system gate from DB/CAS-only
adoption paths, adds command-scoped apply intent for active-host and
always-live flows, preserves conaryd request compatibility, expands manifest
validation coverage, and inventories the active source, script, site, docs,
manpage, and test surfaces that must migrate together.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 151
- `verified-no-change`: 13
- `corrected`: 51
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes include this implementation plan.

- [ ] **Step 5: Verify docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --cached --check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit the reviewed plan lock-in**

```bash
git add docs/superpowers/plans/2026-06-06-live-system-mutation-ux-implementation-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan live mutation ux implementation"
```

### Task 1: Introduce Stub Types And Add Failing Tests For The New Intent Model

**Files:**
- Modify: `apps/conary/src/live_host_safety.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/cli/system.rs`
- Modify: `apps/conary/src/cli/model.rs`
- Modify: `apps/conary/src/cli/ccs.rs`
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`
- Modify: `apps/conary/tests/cli_daily_ux.rs`
- Modify: `apps/conaryd/src/daemon/package_ops.rs`

- [ ] **Step 0: Stub the required structures and fields so red tests compile**

Add the compile scaffolding that later tasks will wire behavior through:

- Define `MutationIntent` in `apps/conary/src/live_host_safety.rs`.
- Add `intent: MutationIntent` to `LiveMutationRequest`.
- Add a temporary `require_mutation_intent` function that preserves the old
  refusal behavior until Task 2 replaces the helper body.
- Add `apply_intent: bool` and `requires_apply_intent()` to
  `CommandRiskPolicy`; default constructors may set `apply_intent: false`.
- Add the new `yes: bool` Clap fields to the Tier 2 and Tier 3 command
  structs listed in Task 3, with default `false` behavior.
- Add `apply_intent` to conaryd request options and transaction operation
  variants with serde defaults.
- Adjust struct literals and match arms only enough for the workspace to
  compile.

Do not change policy behavior in this step except where needed to keep the code
compilable. The behavior assertions added below should still fail until Task 2
and Task 3 wire the helper, classifier, and dispatch branches.

- [ ] **Step 1: Add live-host helper unit tests for apply intent and compatibility**

In `apps/conary/src/live_host_safety.rs`, replace the current tests that assert
on `--allow-live-system-mutation` wording with tests for these cases:

```rust
#[test]
fn apply_intent_passes_active_host_mutation() {
    let request = LiveMutationRequest {
        command_label: Cow::Borrowed("conary install"),
        class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        dry_run: false,
        intent: MutationIntent::Apply,
    };

    assert!(require_mutation_intent(&request).is_ok());
}

#[test]
fn deprecated_global_flag_still_passes_for_persisted_retry_commands() {
    let request = LiveMutationRequest {
        command_label: Cow::Borrowed("conary system generation publish"),
        class: LiveMutationClass::AlwaysLive,
        dry_run: false,
        intent: MutationIntent::DeprecatedLiveSystemMutationFlag,
    };

    assert!(require_mutation_intent(&request).is_ok());
}

#[test]
fn missing_apply_intent_mentions_dry_run_and_yes_not_old_global_flag() {
    let request = LiveMutationRequest {
        command_label: Cow::Borrowed("conary install"),
        class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        dry_run: false,
        intent: MutationIntent::Missing,
    };

    let err = require_mutation_intent(&request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("conary install"));
    assert!(message.contains("--dry-run"));
    assert!(message.contains("--yes"));
    assert!(!message.contains("--allow-live-system-mutation"));
    assert!(!message.contains("early software"));
}

#[test]
fn always_live_refusal_names_generation_risk() {
    let request = LiveMutationRequest {
        command_label: Cow::Borrowed("conary system generation switch"),
        class: LiveMutationClass::AlwaysLive,
        dry_run: false,
        intent: MutationIntent::Missing,
    };

    let err = require_mutation_intent(&request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("generation"));
    assert!(message.contains("boot selection"));
    assert!(message.contains("--yes"));
}
```

- [ ] **Step 2: Run helper tests and confirm they fail**

```bash
cargo test -p conary --lib live_host_safety
```

Expected: tests compile and fail on behavior because the temporary helper still
uses old refusal wording or does not yet treat `MutationIntent::Apply` as
apply intent.

- [ ] **Step 3: Add command-risk tests for DbMutation no-gate behavior**

In `apps/conary/src/command_risk.rs`, add these tests inside the existing test
module:

```rust
#[test]
fn db_mutation_adopt_no_longer_requires_live_ack() {
    let policy = policy(&["conary", "system", "adopt", "curl"]);
    assert_eq!(policy.risk, CommandRisk::DbMutation);
    assert!(!policy.requires_apply_intent());
}

#[test]
fn active_host_install_requires_apply_intent() {
    let policy = policy(&["conary", "install", "nginx"]);
    assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
    assert!(policy.requires_apply_intent());
}
```

- [ ] **Step 4: Run command-risk tests and confirm they fail**

```bash
cargo test -p conary --lib command_risk
```

Expected: tests compile and fail until the classifier treats `DbMutation` as
not requiring apply intent while active-host commands still require it.

- [ ] **Step 5: Add CLI parse tests for hidden compatibility and new `--yes` flags**

In `apps/conary/src/cli/mod.rs`, keep
`cli_accepts_allow_live_system_mutation_as_global_flag`, then add parse tests
for new apply-intent flags:

```rust
#[test]
fn cli_accepts_yes_for_remove_autoremove_and_ccs_install() {
    let remove = Cli::try_parse_from(["conary", "remove", "nginx", "--yes"]).unwrap();
    assert!(matches!(
        remove.command,
        Some(Commands::Remove { yes: true, .. })
    ));

    let autoremove = Cli::try_parse_from(["conary", "autoremove", "--yes"]).unwrap();
    assert!(matches!(
        autoremove.command,
        Some(Commands::Autoremove { yes: true, .. })
    ));

    let ccs = Cli::try_parse_from(["conary", "ccs", "install", "pkg.ccs", "--yes"]).unwrap();
    assert!(matches!(
        ccs.command,
        Some(Commands::Ccs(crate::cli::CcsCommands::Install { yes: true, .. }))
    ));
}

#[test]
fn cli_accepts_yes_for_state_and_generation_apply_commands() {
    let revert = Cli::try_parse_from(["conary", "system", "state", "revert", "1", "--yes"]).unwrap();
    assert!(matches!(
        revert.command,
        Some(Commands::System(crate::cli::SystemCommands::State(
            crate::cli::StateCommands::Revert { yes: true, .. }
        )))
    ));

    let build = Cli::try_parse_from([
        "conary",
        "system",
        "generation",
        "build",
        "--summary",
        "after install",
        "--yes",
    ])
    .unwrap();
    assert!(matches!(
        build.command,
        Some(Commands::System(crate::cli::SystemCommands::Generation(
            crate::cli::GenerationCommands::Build { yes: true, .. }
        )))
    ));
}
```

- [ ] **Step 6: Run CLI parse tests and confirm they fail**

```bash
cargo test -p conary --lib cli_accepts_yes_for_remove_autoremove_and_ccs_install
cargo test -p conary --lib cli_accepts_yes_for_state_and_generation_apply_commands
```

Expected: tests pass once Step 0 added the parse-only `yes` fields. These
tests protect the CLI shape before Task 3 wires the fields into policy.

- [ ] **Step 7: Add integration tests for user-visible refusal behavior**

In `apps/conary/tests/live_host_mutation_safety.rs`, update old refusal tests
and add these new tests:

```rust
#[test]
fn install_refuses_without_apply_intent_and_mentions_yes() {
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
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary install"));
    assert!(stderr.contains("--dry-run"));
    assert!(stderr.contains("--yes"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn install_with_yes_reaches_underlying_package_resolution() {
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
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may mutate"));
}

#[test]
fn deprecated_global_flag_still_reaches_underlying_package_resolution() {
    let (_tmp, db_path) = common::setup_command_test_db();
    let root = tempfile::tempdir().unwrap();

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "install",
        "nginx",
        "--db-path",
        &db_path,
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("unrecognized option"));
    assert!(!stderr.contains("may mutate"));
}

#[test]
fn system_adopt_package_no_longer_requires_live_mutation_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "curl", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
    assert!(!stderr.contains("may mutate"));
}
```

- [ ] **Step 8: Run integration tests and confirm they fail**

```bash
cargo test -p conary --test live_host_mutation_safety
```

Expected: tests fail because current refusal wording still requires the old
global flag and `install --yes` still needs that flag.

- [ ] **Step 9: Add conaryd compatibility tests**

In `apps/conaryd/src/daemon/package_ops.rs`, add tests next to
`package_executor_refuses_live_mutation_without_ack`:

```rust
#[test]
fn package_executor_accepts_apply_intent_without_old_ack() {
    assert!(require_live_ack("conaryd install", false, MutationIntent::Apply).is_ok());
}

#[test]
fn package_executor_accepts_old_ack_as_compatibility_alias() {
    assert!(
        require_live_ack(
            "conaryd install",
            false,
            MutationIntent::DeprecatedLiveSystemMutationFlag
        )
        .is_ok()
    );
}
```

- [ ] **Step 10: Run conaryd package-op tests and confirm they fail**

```bash
cargo test -p conaryd package_executor
```

Expected: tests compile and fail until the daemon helper treats
`MutationIntent::Apply` and `MutationIntent::DeprecatedLiveSystemMutationFlag`
as accepted intent.

- [ ] **Step 11: Keep the red checkpoint local**

```bash
cargo test -p conary --lib live_host_safety
cargo test -p conary --lib command_risk
cargo test -p conary --test live_host_mutation_safety
cargo test -p conaryd package_executor
```

Expected: at least one behavior assertion fails. Do not commit this red
checkpoint. Continue directly to Task 2, then commit once the shared helper and
classifier tests pass.

### Task 2: Implement The Shared Mutation Intent Helper

**Files:**
- Modify: `apps/conary/src/live_host_safety.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/cli/system.rs`
- Modify: `apps/conary/src/cli/model.rs`
- Modify: `apps/conary/src/cli/ccs.rs`
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conaryd/src/daemon/package_ops.rs`
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`
- Modify: `apps/conary/tests/cli_daily_ux.rs`

- [ ] **Step 1: Replace the `MutationIntent` stub with the final shared helper type**

In `apps/conary/src/live_host_safety.rs`, keep the enum added in Task 1 Step 0
and make sure it has this final shape above `LiveMutationClass`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationIntent {
    Missing,
    Apply,
    DeprecatedLiveSystemMutationFlag,
}

impl MutationIntent {
    pub fn from_apply_intent(apply_intent: bool, deprecated_live_ack: bool) -> Self {
        if apply_intent {
            Self::Apply
        } else if deprecated_live_ack {
            Self::DeprecatedLiveSystemMutationFlag
        } else {
            Self::Missing
        }
    }

    pub fn is_present(self) -> bool {
        !matches!(self, Self::Missing)
    }
}
```

- [ ] **Step 2: Keep `intent` on `LiveMutationRequest`**

Make sure the Task 1 Step 0 stub field has this final shape:

```rust
pub struct LiveMutationRequest {
    pub command_label: Cow<'static, str>,
    pub class: LiveMutationClass,
    pub dry_run: bool,
    pub intent: MutationIntent,
}
```

- [ ] **Step 3: Replace the helper body**

Replace `require_live_system_mutation_ack` with this compatibility wrapper and
new helper:

```rust
pub fn require_live_system_mutation_ack(
    allow_live_system_mutation: bool,
    request: &LiveMutationRequest,
) -> anyhow::Result<()> {
    let intent = if request.intent.is_present() {
        request.intent
    } else {
        MutationIntent::from_apply_intent(false, allow_live_system_mutation)
    };

    let request = LiveMutationRequest {
        command_label: request.command_label.clone(),
        class: request.class,
        dry_run: request.dry_run,
        intent,
    };
    require_mutation_intent(&request)
}

pub fn require_mutation_intent(request: &LiveMutationRequest) -> anyhow::Result<()> {
    if request.dry_run || request.intent.is_present() {
        return Ok(());
    }

    let mut message = match request.class {
        LiveMutationClass::LiveConaryState => format!(
            "command '{}' may update Conary DB or CAS metadata for this machine.",
            request.command_label
        ),
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments => format!(
            "command '{}' may change packages, files, scriptlets, ownership, or the live Conary database.",
            request.command_label
        ),
        LiveMutationClass::AlwaysLive => format!(
            "command '{}' may change generation state, boot selection, publication debt, or recovery state.",
            request.command_label
        ),
    };

    if matches!(
        request.class,
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments
    ) {
        message.push_str(
            " Current --root or similar arguments are not sufficient isolation for this command yet.",
        );
    }

    message.push_str(" Use --dry-run when available to preview first.");
    message.push_str(" Rerun with --yes when you intend to apply this command.");
    if matches!(request.class, LiveMutationClass::AlwaysLive) {
        message.push_str(
            " For generation and recovery operations, verify the concrete generation, boot, or recovery target before applying.",
        );
    }

    bail!("{message}")
}
```

After this step, update every test request literal in the same file to include
`intent: MutationIntent::Missing`, `MutationIntent::Apply`, or
`MutationIntent::DeprecatedLiveSystemMutationFlag`.

- [ ] **Step 4: Update `CommandRiskPolicy` for apply intent**

In `apps/conary/src/command_risk.rs`, add an `apply_intent` field:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRiskPolicy {
    pub command_label: Cow<'static, str>,
    pub risk: CommandRisk,
    pub dry_run: bool,
    pub apply_intent: bool,
}
```

Update the local constructors so read-only/local-state policies use
`apply_intent: false`, and active policies can pass an explicit value without
rewriting the entire classifier at once:

```rust
fn policy(
    command_label: &'static str,
    risk: CommandRisk,
    dry_run: bool,
) -> CommandRiskPolicy {
    policy_with_intent(command_label, risk, dry_run, false)
}

fn policy_with_intent(
    command_label: &'static str,
    risk: CommandRisk,
    dry_run: bool,
    apply_intent: bool,
) -> CommandRiskPolicy {
    CommandRiskPolicy {
        command_label: Cow::Borrowed(command_label),
        risk,
        dry_run,
        apply_intent,
    }
}
```

Add this method:

```rust
impl CommandRiskPolicy {
    pub fn requires_apply_intent(&self) -> bool {
        matches!(
            self.risk,
            CommandRisk::ActiveHostMutation | CommandRisk::AlwaysLive
        ) && !self.dry_run
    }
}
```

Keep `DbMutation` classified as `CommandRisk::DbMutation`, but make it return
`false` from `requires_apply_intent`.

- [ ] **Step 5: Update `enforce_cli_policy` to use `MutationIntent`**

Change the imports to use the new helper:

```rust
use crate::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, MutationIntent, require_mutation_intent,
};
```

Then change `enforce_cli_policy` so it only calls the helper when
`policy.requires_apply_intent()` is true:

```rust
if !policy.requires_apply_intent() {
    return Ok(());
}

let Some(class) = policy.mutation_class() else {
    return Ok(());
};

require_mutation_intent(&LiveMutationRequest {
    command_label: policy.command_label,
    class,
    dry_run: policy.dry_run,
    intent: MutationIntent::from_apply_intent(
        policy.apply_intent,
        allow_live_system_mutation,
    ),
})
```

- [ ] **Step 6: Update dispatch's local helper**

In `apps/conary/src/dispatch.rs`, change `require_live_mutation` to accept
`MutationIntent` instead of `allow_live_system_mutation: bool`, then call
`require_mutation_intent`.

```rust
fn require_live_mutation(
    intent: MutationIntent,
    command_label: Cow<'static, str>,
    class: LiveMutationClass,
    dry_run: bool,
) -> Result<()> {
    require_mutation_intent(&LiveMutationRequest {
        command_label,
        class,
        dry_run,
        intent,
    })
}
```

In each dispatch branch that already has `yes`, compute intent with
`MutationIntent::from_apply_intent(yes, allow_live_system_mutation)`. In
branches that do not gain `yes` until Task 3, temporarily use
`MutationIntent::from_apply_intent(false, allow_live_system_mutation)` so Task
2 can still compile and preserve the old compatibility alias.

- [ ] **Step 7: Update conaryd package-op helper**

In `apps/conaryd/src/daemon/package_ops.rs`, update `require_live_ack` to
accept `MutationIntent` and call `require_mutation_intent`. Use
`MutationIntent::from_apply_intent(apply_intent, allow_live_system_mutation)` at
the call sites after Task 4 adds request fields.

- [ ] **Step 8: Run the helper and risk tests**

```bash
cargo test -p conary --lib live_host_safety
cargo test -p conary --lib command_risk
```

Expected: helper and classification tests pass.

- [ ] **Step 9: Commit the helper refactor**

```bash
git add apps/conary/src/live_host_safety.rs apps/conary/src/command_risk.rs apps/conary/src/dispatch.rs apps/conary/src/cli/mod.rs apps/conary/src/cli/system.rs apps/conary/src/cli/model.rs apps/conary/src/cli/ccs.rs apps/conary/src/cli/generation.rs apps/conaryd/src/daemon/package_ops.rs apps/conaryd/src/daemon/routes.rs apps/conary/tests/live_host_mutation_safety.rs apps/conary/tests/cli_daily_ux.rs
git commit -m "refactor: add mutation intent helper"
```

### Task 3: Add CLI Apply Intent Flags And Wire CLI Policy

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/cli/system.rs`
- Modify: `apps/conary/src/cli/model.rs`
- Modify: `apps/conary/src/cli/ccs.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`

- [ ] **Step 1: Hide the old global flag from help while keeping it parseable**

In `apps/conary/src/cli/mod.rs`, change the global flag attribute to:

```rust
/// Deprecated compatibility alias for old persisted retry commands.
#[arg(long, global = true, hide = true)]
pub allow_live_system_mutation: bool,
```

Keep `cli_accepts_allow_live_system_mutation_as_global_flag`.

- [ ] **Step 2: Update root help examples**

Replace the `after_help` string with:

```rust
after_help = "Daily workflow examples:\n  conary install nginx --dry-run\n  conary install nginx --yes\n  conary update --dry-run\n  conary system adopt --refresh\n  conary system completions bash > /tmp/conary-completion.bash\n  conary system generation export --path /conary/generations/1 --format qcow2 --output gen1.qcow2\n  conaryd handles durable package jobs with the same apply-intent boundary"
```

- [ ] **Step 3: Finalize Tier 2 `--yes` fields added by the scaffold**

Confirm the `yes: bool` fields from Task 1 Step 0 exist with
`#[arg(short = 'y', long)]` where `-y` is not already used, otherwise use
`#[arg(long)]`:

- `Commands::Remove`
- `Commands::Autoremove`
- `SystemCommands::Restore`
- `SystemCommands::Unadopt`
- `StateCommands::Revert`
- `StateCommands::Rollback`
- `CcsCommands::Install`
- `ModelCommands::Apply`

Use help text:

```rust
/// Confirm applying this command's active-system changes
#[arg(short = 'y', long)]
yes: bool,
```

For `model apply`, preserve the existing `--strict` flag as model-convergence
scope and add `--yes` only for active-system apply intent.

- [ ] **Step 4: Finalize Tier 3 `--yes` fields added by the scaffold**

Confirm the `yes: bool` fields from Task 1 Step 0 exist on:

- `GenerationCommands::Build`
- `GenerationCommands::Publish`
- `GenerationCommands::Switch`
- `GenerationCommands::Rollback`; this is currently a unit variant and must
  become `Rollback { yes: bool }`. The conversion remains parse-compatible
  because `yes: bool` defaults to `false`; old commands such as
  `conary --allow-live-system-mutation system generation rollback` still parse
  and pass through the deprecated compatibility alias.
- `GenerationCommands::Gc`
- `GenerationCommands::Recover`

Use help text:

```rust
/// Confirm applying this generation, boot, publication, or recovery change
#[arg(short = 'y', long)]
yes: bool,
```

For commands where `-y` conflicts, use `#[arg(long)]`.

- [ ] **Step 5: Wire `apply_intent` in `classify_cli`**

Update `classify_cli` and nested classifiers so Tier 2/Tier 3 policies carry
the command's `yes` value. Examples:

```rust
Commands::Install { package, dry_run, yes, .. } => Some(policy_with_intent(
    if package.starts_with('@') {
        "conary install @collection"
    } else {
        "conary install"
    },
    CommandRisk::ActiveHostMutation,
    *dry_run,
    *yes,
)),
```

For `system db-backup recover`, use its existing `yes`.
For `system generation gc`, wire the new `yes` field through
`classify_generation`; it remains `AlwaysLive` because it can remove generation
and CAS assets.
For `self-update`, map `force` to apply intent for non-read-only self-update
paths and keep `--force` documented as the scoped confirmation flag.
For `DbMutation` adopt variants, pass `false`; they do not require apply
intent.

- [ ] **Step 6: Wire dispatch branches**

Update each active mutation dispatch branch to pass the new command-specific
intent to `require_live_mutation`. Example:

```rust
let intent = MutationIntent::from_apply_intent(yes, allow_live_system_mutation);
require_live_mutation(
    intent,
    Cow::Borrowed("conary install"),
    LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
    dry_run,
)?;
```

For `DbMutation` adopt variants, do not add a secondary live-mutation check.
Keep `system adopt --sync-hook` and `--remove-hook` as active-host mutations
because they install or remove native package-manager hooks.

- [ ] **Step 7: Update live-host integration tests**

Update old tests in `apps/conary/tests/live_host_mutation_safety.rs` so:

- Tier 1 adopt package/system/refresh/convert tests assert the old global flag
  is not mentioned.
- Tier 2/Tier 3 missing-intent tests assert `--yes`, `--dry-run` when
  available, and no old global flag.
- Old global flag compatibility tests assert the command parses and reaches the
  underlying command behavior.

Also update `apps/conary/tests/cli_daily_ux.rs` before running the Task 3
checkpoint. In `live_mutation_refusal_routes_to_preview_ack_and_daemon_jobs`,
`install --yes` now satisfies apply intent and should proceed past the
live-host gate. Replace the old refusal assertions with assertions that the
command still fails for underlying package-resolution/setup reasons and that
stderr does not mention `--allow-live-system-mutation` or the old conaryd
acknowledgement wording.

Leave `adopted_install_refusal_routes_to_refresh_and_takeover` on the old
source-hint wording until Task 6 migrates the install/update adopted-package
hints. Task 6 must update that test in the same commit as the source hint
changes.

- [ ] **Step 8: Run focused CLI tests**

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk
cargo test -p conary --test live_host_mutation_safety
cargo test -p conary --test cli_daily_ux live_mutation
```

Expected: all pass.

- [ ] **Step 9: Commit CLI intent wiring**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/cli/system.rs apps/conary/src/cli/model.rs apps/conary/src/cli/ccs.rs apps/conary/src/command_risk.rs apps/conary/src/dispatch.rs apps/conary/tests/live_host_mutation_safety.rs apps/conary/tests/cli_daily_ux.rs
git commit -m "feat: add risk-tiered cli apply intent"
```

### Task 4: Add conaryd Apply Intent Compatibility

**Files:**
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
- Modify: `apps/conaryd/src/daemon/package_ops.rs`
- Modify: `apps/conaryd/src/daemon/client.rs`
- Modify: `docs/modules/conaryd.md`

- [ ] **Step 1: Add `apply_intent` to daemon request structs**

In `apps/conaryd/src/daemon/routes.rs`, add this field next to each
`allow_live_system_mutation` field in `TransactionOperation` and
`PackageOperationOptions`:

```rust
/// Confirm applying this package operation.
#[serde(default, skip_serializing_if = "is_false")]
apply_intent: bool,
```

For `PackageOperationOptions`, make the field public:

```rust
/// Confirm applying this package operation.
#[serde(default)]
pub apply_intent: bool,
```

Keep `allow_live_system_mutation` with its existing serde defaults.

- [ ] **Step 2: Forward both fields into package operations**

In `apps/conaryd/src/daemon/routes/transactions.rs`, pass both fields into the
package operation enum or options. In `apps/conaryd/src/daemon/package_ops.rs`,
add `apply_intent` to the internal `PackageCommand::{Install,Remove,Update}`
variants and to `impl From<&TransactionOperation> for PackageCommand` so the
executor receives the value that serde parsed. The effective intent is:

```rust
let intent = MutationIntent::from_apply_intent(apply_intent, allow_live_system_mutation);
```

- [ ] **Step 3: Update package executor checks**

In `apps/conaryd/src/daemon/package_ops.rs`, replace boolean live-ack calls
with `MutationIntent`. Dry-run install/update still bypasses. Remove remains
non-dry-run and requires intent.

- [ ] **Step 4: Update daemon client options**

In `apps/conaryd/src/daemon/client.rs`, add `apply_intent: bool` to
`InstallOptions`, `RemoveOptions`, and `UpdateOptions`. Keep
`allow_live_system_mutation` for compatibility.

- [ ] **Step 5: Update daemon tests**

Add or update tests so:

- request bodies with `apply_intent: true` queue package jobs;
- old request bodies with `allow_live_system_mutation: true` still queue jobs;
- request bodies with neither field still refuse before package mutation.

- [ ] **Step 6: Run daemon focused tests**

```bash
cargo test -p conaryd package_executor
cargo test -p conaryd daemon::routes
```

Expected: all pass.

- [ ] **Step 7: Commit daemon compatibility**

```bash
git add apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/transactions.rs apps/conaryd/src/daemon/package_ops.rs apps/conaryd/src/daemon/client.rs docs/modules/conaryd.md
git commit -m "feat: add conaryd apply intent compatibility"
```

### Task 5: Migrate Manifests And Manifest Validation

**Files:**
- Modify: `apps/conary-test/src/config/mod.rs`
- Modify: `apps/conary/tests/integration/remi/manifests/*.toml`

- [ ] **Step 1: Update manifest validator to accept old and new intent forms**

In `apps/conary-test/src/config/mod.rs`, first expand the segment discovery so
the validator covers the whole active mutation surface:

```rust
fn segment_matches_command(segment: &str, command: &str) -> bool {
    segment.starts_with(command)
        || segment.contains(&format!(" {command} "))
        || segment.contains(&format!("${{CONARY_BIN}} {command}"))
}

fn is_package_apply_segment(segment: &str) -> bool {
    ["install", "update", "ccs install"]
        .iter()
        .any(|command| segment_matches_command(segment, command))
}

fn is_system_mutation_segment(segment: &str) -> bool {
    [
        "system adopt",
        "system restore",
        "system native-handoff",
        "system state revert",
        "system state rollback",
        "system db-backup recover",
        "system generation build",
        "system generation publish",
        "system generation switch",
        "system generation gc",
        "system generation rollback",
        "system generation recover",
        "system generation recover-db",
        "system takeover",
        "system unadopt",
        "model apply",
        "automation apply",
    ]
    .iter()
    .any(|command| segment_matches_command(segment, command))
}
```

Then update `live_mutation_segments` so it includes package apply segments:

```rust
fn live_mutation_segments(command: &str) -> impl Iterator<Item = &str> {
    command.split(';').filter_map(|segment| {
        let segment = segment.trim();
        (is_package_apply_segment(segment)
            || is_package_remove_segment(segment)
            || is_system_mutation_segment(segment))
        .then_some(segment)
    })
}
```

Finally, change
`active_manifest_live_mutation_commands_acknowledge_live_mutation` so
non-dry-run mutation segments pass when they contain either the deprecated old
flag or the command-specific apply intent:

```rust
fn has_apply_intent(segment: &str) -> bool {
    segment.contains("--yes") || segment.contains("--allow-live-system-mutation")
}
```

Use `has_apply_intent(segment)` in the assertion message:

```rust
assert!(
    has_apply_intent(segment),
    "{}:{} step {} mutation command must include apply intent: {}",
    path.display(),
    test.id,
    index + 1,
    segment
);
```

- [ ] **Step 2: Run validator test**

```bash
cargo test -p conary-test config::tests::active_manifest_live_mutation_commands_acknowledge_live_mutation
```

Expected: pass before manifest migration because old flags are still accepted.

- [ ] **Step 3: Replace active manifest commands mechanically**

For commands that can now use `--yes`, replace old global flag instances with
the command-specific apply flag. Examples:

```text
conary --allow-live-system-mutation install tree --sandbox never
```

becomes:

```text
conary install tree --sandbox never --yes
```

```text
${CONARY_BIN} system generation build --allow-live-system-mutation
```

becomes:

```text
${CONARY_BIN} system generation build --yes
```

```text
${CONARY_BIN} ccs install --allow-live-system-mutation "$CCS_FILE" --allow-unsigned
```

becomes:

```text
${CONARY_BIN} ccs install "$CCS_FILE" --allow-unsigned --yes
```

For Tier 1 adopt commands, remove the old global flag without adding `--yes`:

```text
conary --allow-live-system-mutation system adopt --system --full
```

becomes:

```text
conary system adopt --system --full
```

- [ ] **Step 4: Run suite inventory checks**

```bash
cargo run -p conary-test -- list
cargo test -p conary-test suite_inventory
cargo test -p conary-test config::tests::active_manifest_live_mutation_commands_acknowledge_live_mutation
```

Expected: all pass.

- [ ] **Step 5: Commit manifest migration**

```bash
git add apps/conary-test/src/config/mod.rs apps/conary/tests/integration/remi/manifests
git commit -m "test: migrate manifests to apply intent"
```

### Task 6: Migrate Active Source Hints, Tests, Docs, And Manpages

**Files:**
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/commands/adopt/refresh.rs`
- Modify: `apps/conary/src/commands/generation/builder.rs`
- Modify: `apps/conary/src/commands/generation/publication.rs`
- Modify: `apps/conary/src/commands/changeset_metadata.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`
- Modify: `apps/conary/src/commands/query/history.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `apps/conary/tests/component.rs`
- Modify: `apps/conary/tests/live_host_mutation_readiness.rs`
- Modify: `apps/conary/tests/model_apply.rs`
- Modify: `apps/conary/tests/native_pm_daily_driver.rs`
- Modify: `apps/conary/tests/native_pm_live_root.rs`
- Modify: `apps/conary/tests/bundle_replay.rs`
- Modify: `apps/conary/tests/query.rs`
- Modify: `apps/conary/tests/workflow.rs`
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/operations/daily-driver-ux-matrix.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/operations/bootstrap-selfhosting-vm.md`
- Modify: `docs/operations/live-mutation-backup-inventory.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`
- Modify: `docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`
- Modify: `apps/conary/man/conary.1`
- Modify: `man/conary.1`
- Modify: `scripts/bootstrap-vm/guest-validate.sh`
- Modify: `site/src/routes/install/+page.svelte`

Note: `docs/modules/feature-ownership.md` is listed in the design spec's doc
surface inventory but currently contains no old live-mutation flag examples; it
is intentionally omitted from this migration unless implementation adds or
finds active wording there. `docs/operations/release-artifact-matrix.md` also
currently has no old-flag examples; inspect it during the active docs sweep and
leave it unstaged if no apply-intent wording needs to change.

- [ ] **Step 1: Update persisted follow-up generators**

Change generated retry commands to the new preferred form:

```rust
pub(crate) const DEFAULT_PUBLICATION_RETRY_COMMAND: &str =
    "conary system generation publish --yes";
```

Update `publication_deferred_follow_up`, publication tests,
`install/batch.rs` tests, and query history tests to expect the new retry
command. Keep old retry commands parseable through the hidden compatibility
alias from Task 3.

- [ ] **Step 2: Update active source hints**

Replace source hints with the new command forms:

- adopt refresh hint:
  `conary system adopt --system`
- generation builder full adoption hint:
  `conary system adopt --system --full`
- install/update adopted package refresh hints:
  `conary system adopt --refresh`
- sync-hook context hint:
  `conary system adopt --refresh`
- generation publish retry hint:
  `conary system generation publish --yes`

- [ ] **Step 3: Update active Rust tests that invoke the old flag**

Run:

```bash
rg -n -- "--allow-live-system-mutation" apps/conary/tests apps/conary/src apps/conaryd/src apps/conary-test/src
```

For active tests, migrate commands using these rules:

- Tier 1 adopt: remove old flag.
- Tier 2 install/update/remove/autoremove/ccs/model/automation/state/db-backup:
  use `--yes`.
- Tier 3 generation/takeover/recovery: use `--yes`.
- Compatibility tests may keep the old flag intentionally and should name that
  purpose in the test name.
- `apps/conary/tests/live_host_mutation_readiness.rs` is expected to keep
  passing as a dry-run bypass proof; only edit it if the command text in that
  test still mentions the old flag.

- [ ] **Step 4: Update active docs, validation scripts, site page, and generated manpages**

Run:

```bash
rg -n --glob '!target/**' -- "--allow-live-system-mutation|allow-live-system-mutation|live-system-mutation" README.md ROADMAP.md docs/ARCHITECTURE.md docs/conaryopedia-v2.md docs/operations docs/modules apps/conary/man man scripts site
```

Update active docs to prefer:

- preview: `conary install nginx --dry-run`
- apply package work: `conary install nginx --yes`
- metadata adoption: `conary system adopt --system`
- generation apply: `conary system generation build --summary "..." --yes`
- generation switch: `conary system generation switch 2 --yes`
- generation publish follow-up: `conary system generation publish --yes`
- native handoff: `conary system native-handoff --dry-run`, then
  `conary system native-handoff --yes`

Update `scripts/bootstrap-vm/guest-validate.sh` to remove the deprecated global
flag from install/ccs-install invocations and to use `--yes` for remove
invocations. Update `site/src/routes/install/+page.svelte` so setup and
teardown snippets use the new adoption/unadoption command forms.

Do not rewrite archived historical docs solely for old examples.

- [ ] **Step 5: Update manpages where generated help or examples mention the old flag**

The root `man/conary.1` and `apps/conary/man/conary.1` are currently stub
manpages and may not contain the old flag. Verify that `hide = true` removes
the old global flag from generated help output. If future or generated
sub-manpages such as `man/conary-install.1` exist and contain old-flag
examples, update those examples to use `--dry-run` and `--yes`. Leave the root
stubs unchanged if they contain no affected wording.

- [ ] **Step 6: Run source/docs migration checks**

```bash
cargo test -p conary --test component
cargo test -p conary --test live_host_mutation_readiness
cargo test -p conary --test model_apply
cargo test -p conary --test native_pm_daily_driver
cargo test -p conary --test native_pm_live_root
cargo test -p conary --test bundle_replay
cargo test -p conary --test query
cargo test -p conary --test workflow
```

Expected: all pass.

- [ ] **Step 7: Commit source/docs migration**

```bash
git add apps/conary/src/command_risk.rs apps/conary/src/commands apps/conary/tests README.md ROADMAP.md docs/ARCHITECTURE.md docs/conaryopedia-v2.md docs/operations docs/modules apps/conary/man man scripts/bootstrap-vm/guest-validate.sh site/src/routes/install/+page.svelte
git commit -m "docs: migrate live mutation guidance"
```

### Task 7: Final Verification And Drift Sweep

**Files:**
- Modify as needed from verification failures only.

- [ ] **Step 1: Run focused behavior gates**

```bash
cargo test -p conary --lib command_risk
cargo test -p conary --lib live_host_safety
cargo test -p conary --test live_host_mutation_safety
cargo test -p conary --test cli_daily_ux live_mutation
cargo test -p conaryd package_executor
cargo test -p conaryd daemon::routes
```

Expected: all pass.

- [ ] **Step 2: Run medium integration-adjacent gates**

```bash
cargo run -p conary-test -- list
cargo test -p conary-test suite_inventory
cargo test -p conary-test config::tests::active_manifest_live_mutation_commands_acknowledge_live_mutation
cargo test -p conary --test component
cargo test -p conary --test live_host_mutation_readiness
cargo test -p conary --test model_apply
cargo test -p conary --test native_pm_daily_driver
cargo test -p conary --test native_pm_live_root
cargo test -p conary --test bundle_replay
cargo test -p conary --test query
cargo test -p conary --test workflow
```

Expected: all pass.

- [ ] **Step 3: Run formatting and lint gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both pass.

- [ ] **Step 4: Run docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
git diff --cached --check
```

Expected: all pass.

- [ ] **Step 5: Run the active old-phrase sweep**

This sweep intentionally excludes archived historical docs and permits
compatibility code/tests that still parse the old flag:

```bash
rg -n --glob '!target/**' -- "--allow-live-system-mutation|allow-live-system-mutation|live-system-mutation" \
  README.md ROADMAP.md docs/ARCHITECTURE.md docs/conaryopedia-v2.md docs/operations docs/modules \
  apps/conary/man man \
  apps/conary/src apps/conary/tests apps/conaryd/src apps/conary-test/src scripts site
```

Expected remaining matches are limited to:

- hidden compatibility parsing;
- compatibility tests that prove old retry commands still parse;
- implementation comments explaining the compatibility window.

If active user-facing guidance still tells users to use the old global flag,
fix it before final commit.

- [ ] **Step 6: Commit final verification fixes**

If verification required changes:

```bash
git add <changed-files>
git commit -m "fix: complete live mutation ux migration"
```

If no changes were required, record the verification commands in the final
response and do not create an empty commit.

## Review Checklist Before Execution

- The plan keeps the old global flag parseable during the compatibility window.
- `DbMutation` adoption work is no longer blocked by the live-system guard.
- Active-host and always-live commands still refuse before mutation when apply
  intent is missing.
- conaryd accepts both old and new request intent fields.
- Integration manifests and their validator migrate together.
- Active docs and source hints migrate together.
- Archived historical docs remain historical.
- Final sweeps distinguish compatibility leftovers from active guidance.
