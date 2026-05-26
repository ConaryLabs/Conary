# Adoption Safety And Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the Plan A hardening slice from the preview invariant spec: adoption commands must cross the live-system acknowledgement boundary consistently, full adoption must store private immutable CAS objects, and adoption DB state must not persist ghost or hollow package metadata.

**Architecture:** Keep the first implementation slice narrow. Add a command-risk policy surface for the CLI paths that currently perform or bypass live mutation checks, route adoption through that surface, give installed sync hooks a constrained non-interactive refresh path, switch full-adoption CAS capture from hardlinking to private CAS storage that repairs touched legacy shared objects, and make adoption metadata writes all-or-clean using the existing changeset metadata envelope. Durable generation publication, broad docs truth automation, and `conary-core` facade decisions remain outside this plan.

**Tech Stack:** Rust, clap, rusqlite transactions/savepoints, tempfile-based tests, existing `conary_core::filesystem::CasStore`, existing `apps/conary/tests/live_host_mutation_safety.rs` integration tests.

---

## Scope

This plan implements Tracks 1 and 2 from `docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md`.

It intentionally does not implement Track 3 generation publication durability or Track 4 CI truth checks, except for documentation and app-string touchups directly caused by adoption behavior changes.

## File Structure

- Create `apps/conary/src/command_risk.rs`: CLI command-risk policy definitions and enforcement helpers. This keeps acknowledgement routing visible instead of burying it in scattered dispatch arms.
- Modify `apps/conary/src/lib.rs`: export the new `command_risk` module for tests and dispatch.
- Modify `apps/conary/src/live_host_safety.rs`: add a DB/CAS-only live Conary state mutation class with accurate refusal text.
- Modify `apps/conary/src/dispatch.rs`: call command-risk enforcement once before command execution, then remove the duplicate adoption-specific risk logic from the dispatch arm.
- Modify `apps/conary/src/cli/system.rs`: clarify that single-package `system adopt --dry-run` is currently rejected until a true preview path exists, and add the hidden constrained `--from-sync-hook` refresh flag.
- Modify `apps/conary/src/commands/adopt/hooks.rs`: generated hook scripts pass `--from-sync-hook`, hook comments/tests document the constrained quiet-refresh contract, and hook install/remove remains gated.
- Modify `apps/conary/tests/live_host_mutation_safety.rs`: add regression coverage for every adoption mode in the Plan A table.
- Modify `apps/conary/src/commands/adopt/convert.rs`: move source-identity backfill behind the dry-run return.
- Modify `crates/conary-core/src/filesystem/cas.rs`: add a private-copy storage API for mutable source files and rename/document hardlink APIs as sealed-source-only.
- Modify `apps/conary/src/commands/adopt/cas_capture.rs`: use private CAS storage for full adoption regular files.
- Modify `apps/conary/src/commands/adopt/system.rs`: use private CAS storage in the legacy single-package helper branch and delete all-failed bulk-adoption troves.
- Modify `apps/conary/src/commands/adopt/packages.rs`: share metadata outcome logic and persist degraded partial-insert warnings.
- Create `apps/conary/src/commands/adopt/outcome.rs`: adoption outcome helpers, adoption warning types, and tests.
- Modify `apps/conary/src/commands/changeset_metadata.rs`: extend the existing versioned metadata envelope with adoption warning records and helper accessors.
- Modify `apps/conary/src/commands/adopt/refresh.rs`: replace child metadata through a production savepoint helper, preserve old children on per-package failure, and persist degraded refresh warnings.
- Modify focused docs only if command help or operator-facing text changes.

## Review-Tightened Decisions

- `None` from `classify_cli` means "no subcommand was supplied"; it does not mean read-only. Every parsed Clap command variant must return a `CommandRiskPolicy`, including explicit `ReadOnly` policies.
- Plan A uses a constrained hidden hook-refresh path rather than embedding the global acknowledgement flag in installed hook scripts. Hook installation remains gated, and generated hook scripts call `system adopt --refresh --quiet --from-sync-hook`.
- `store_file_copy_from_existing` must repair a touched legacy shared CAS object when the hash path already exists with a shared inode. A full historical CAS sweep is deferred.
- Adoption warnings must live inside the existing `conary.changeset.metadata.v1` envelope. The plan must not overwrite rollback snapshots or deferred follow-up metadata with a new top-level JSON shape.
- Refresh replacement failures skip/degrade the affected package and preserve old child metadata through a production savepoint helper; tests must exercise that production helper rather than only a test-only savepoint wrapper.
- Final commits stage exact paths only after reviewing `git diff --name-only`; broad directory staging is not allowed in this plan.

---

### Task 1: Command Risk Policy Surface

**Files:**
- Create: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/lib.rs`
- Modify: `apps/conary/src/live_host_safety.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Test: `apps/conary/src/command_risk.rs`
- Test: `apps/conary/src/live_host_safety.rs`

- [ ] **Step 1: Write failing unit tests for risk classes and adoption policy**

Add this test module to the new `apps/conary/src/command_risk.rs` file while implementing the file in the same task:

```rust
#[cfg(test)]
mod tests {
    use super::{CommandRisk, classify_cli};
    use crate::cli::Cli;
    use clap::Parser;

    fn policy(args: &[&str]) -> super::CommandRiskPolicy {
        let cli = Cli::try_parse_from(args).unwrap();
        classify_cli(&cli).expect("command should be classified")
    }

    #[test]
    fn classify_system_adopt_status_as_read_only() {
        let policy = policy(&["conary", "system", "adopt", "--status"]);
        assert_eq!(policy.risk, CommandRisk::ReadOnly);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_system_dry_run_as_dry_run_only() {
        let policy = policy(&["conary", "system", "adopt", "--system", "--dry-run"]);
        assert_eq!(policy.risk, CommandRisk::DryRunOnly);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_package_as_live_db_mutation() {
        let policy = policy(&["conary", "system", "adopt", "curl"]);
        assert_eq!(policy.risk, CommandRisk::DbMutation);
        assert!(policy.requires_ack());
        assert_eq!(policy.command_label.as_ref(), "conary system adopt <pkg>");
    }

    #[test]
    fn classify_system_adopt_full_package_as_live_db_mutation() {
        let policy = policy(&["conary", "system", "adopt", "curl", "--full"]);
        assert_eq!(policy.risk, CommandRisk::DbMutation);
        assert!(policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_convert_dry_run_as_dry_run_only() {
        let policy = policy(&["conary", "system", "adopt", "--convert", "--dry-run"]);
        assert_eq!(policy.risk, CommandRisk::DryRunOnly);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_installed_sync_hook_refresh_as_narrow_hook_refresh() {
        let policy = policy(&[
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--quiet",
            "--from-sync-hook",
        ]);
        assert_eq!(policy.risk, CommandRisk::HookRefreshDbMutation);
        assert!(!policy.requires_ack());
        assert_eq!(
            policy.command_label.as_ref(),
            "conary system adopt --refresh --quiet --from-sync-hook"
        );
    }

    #[test]
    fn from_sync_hook_requires_quiet_refresh_and_rejects_full_capture() {
        assert!(Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--from-sync-hook",
        ])
        .is_err());

        assert!(Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--quiet",
            "--full",
            "--from-sync-hook",
        ])
        .is_err());
    }

    #[test]
    fn classify_system_adopt_sync_hook_as_active_host_mutation() {
        let policy = policy(&["conary", "system", "adopt", "--sync-hook"]);
        assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
        assert!(policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_remove_hook_as_active_host_mutation() {
        let policy = policy(&["conary", "system", "adopt", "--sync-hook", "--remove-hook"]);
        assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
        assert!(policy.requires_ack());
    }

    #[test]
    fn classify_pin_and_unpin_as_live_db_mutations() {
        for args in [
            ["conary", "pin", "curl"].as_slice(),
            ["conary", "unpin", "curl"].as_slice(),
        ] {
            let policy = policy(args);
            assert_eq!(policy.risk, CommandRisk::DbMutation);
            assert!(policy.requires_ack());
        }
    }

    #[test]
    fn classify_adoption_dry_runs_with_precise_labels() {
        let system = policy(&["conary", "system", "adopt", "--system", "--dry-run"]);
        assert_eq!(system.command_label.as_ref(), "conary system adopt --system --dry-run");

        let refresh = policy(&["conary", "system", "adopt", "--refresh", "--dry-run"]);
        assert_eq!(refresh.command_label.as_ref(), "conary system adopt --refresh --dry-run");

        let convert = policy(&["conary", "system", "adopt", "--convert", "--dry-run"]);
        assert_eq!(convert.command_label.as_ref(), "conary system adopt --convert --dry-run");
    }
}
```

Extend `apps/conary/src/live_host_safety.rs` tests with this DB-only refusal check:

```rust
#[test]
fn live_conary_state_refusal_describes_db_and_cas_not_scriptlets() {
    let request = LiveMutationRequest {
        command_label: Cow::Borrowed("conary system adopt <pkg>"),
        class: LiveMutationClass::LiveConaryState,
        dry_run: false,
    };

    let err = require_live_system_mutation_ack(false, &request).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("Conary DB"));
    assert!(message.contains("CAS"));
    assert!(message.contains("--allow-live-system-mutation"));
    assert!(!message.contains("scriptlet hooks"));
    assert!(!message.contains("remount /usr"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p conary command_risk -- --nocapture
cargo test -p conary live_host_safety -- --nocapture
```

Expected before implementation: the command-risk module is missing or `classify_cli` is missing, and `LiveMutationClass::LiveConaryState` is missing.

- [ ] **Step 3: Add the DB/CAS-only live mutation class**

Modify `apps/conary/src/live_host_safety.rs`:

```rust
pub enum LiveMutationClass {
    AlwaysLive,
    CurrentlyLiveEvenWithRootArguments,
    LiveConaryState,
}
```

Change `require_live_system_mutation_ack` so the message is class-specific:

```rust
let mut message = match request.class {
    LiveMutationClass::LiveConaryState => format!(
        "command '{}' mutates Conary DB and/or CAS state for this live system. \
         Conary is still early software, and live adoption state affects future \
         package resolution, restore, unadoption, and generation builds.",
        request.command_label
    ),
    LiveMutationClass::AlwaysLive | LiveMutationClass::CurrentlyLiveEvenWithRootArguments => {
        format!(
            "command '{}' may mutate the active host. Conary is still early software, \
             and this command can perform generation rebuild or activation work, \
             remount /usr, rewrite the live /etc overlay, execute scriptlet hooks, \
             or change package ownership during takeover or rollback.",
            request.command_label
        )
    }
};
```

Keep the existing `--root` suffix only for `CurrentlyLiveEvenWithRootArguments`.

- [ ] **Step 4: Implement the command-risk module**

Create `apps/conary/src/command_risk.rs`:

```rust
// apps/conary/src/command_risk.rs

use anyhow::Result;
use std::borrow::Cow;

use crate::cli::{self, Cli, Commands};
use crate::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, require_live_system_mutation_ack,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRisk {
    ReadOnly,
    LocalStateMutation,
    DryRunOnly,
    HookRefreshDbMutation,
    DbMutation,
    ActiveHostMutation,
    AlwaysLive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRiskPolicy {
    pub command_label: Cow<'static, str>,
    pub risk: CommandRisk,
    pub dry_run: bool,
}

impl CommandRiskPolicy {
    pub fn requires_ack(&self) -> bool {
        !self.dry_run
            && matches!(
                self.risk,
                CommandRisk::DbMutation
                    | CommandRisk::ActiveHostMutation
                    | CommandRisk::AlwaysLive
            )
    }

    fn mutation_class(&self) -> Option<LiveMutationClass> {
        match self.risk {
            CommandRisk::ReadOnly
            | CommandRisk::LocalStateMutation
            | CommandRisk::DryRunOnly
            | CommandRisk::HookRefreshDbMutation => None,
            CommandRisk::DbMutation => Some(LiveMutationClass::LiveConaryState),
            CommandRisk::ActiveHostMutation => {
                Some(LiveMutationClass::CurrentlyLiveEvenWithRootArguments)
            }
            CommandRisk::AlwaysLive => Some(LiveMutationClass::AlwaysLive),
        }
    }
}

pub fn enforce_cli_policy(allow_live_system_mutation: bool, cli: &Cli) -> Result<()> {
    let Some(policy) = classify_cli(cli) else {
        return Ok(());
    };

    if policy.risk == CommandRisk::HookRefreshDbMutation {
        return require_sync_hook_context(&policy);
    }

    let Some(class) = policy.mutation_class() else {
        return Ok(());
    };

    require_live_system_mutation_ack(
        allow_live_system_mutation,
        &LiveMutationRequest {
            command_label: policy.command_label,
            class,
            dry_run: policy.dry_run,
        },
    )
}

fn require_sync_hook_context(policy: &CommandRiskPolicy) -> Result<()> {
    #[cfg(unix)]
    {
        if unsafe { libc::geteuid() } != 0 {
            anyhow::bail!(
                "command '{}' is reserved for installed native package-manager sync hooks; run `conary --allow-live-system-mutation system adopt --refresh` for an interactive refresh",
                policy.command_label
            );
        }
    }

    #[cfg(not(unix))]
    {
        anyhow::bail!(
            "command '{}' is reserved for Unix native package-manager sync hooks",
            policy.command_label
        );
    }

    Ok(())
}

pub fn classify_cli(cli: &Cli) -> Option<CommandRiskPolicy> {
    match cli.command.as_ref()? {
        Commands::Install { package, dry_run, .. } if package.starts_with('@') => {
            Some(policy("conary install @collection", CommandRisk::ActiveHostMutation, *dry_run))
        }
        Commands::Install { dry_run, .. } => {
            Some(policy("conary install", CommandRisk::ActiveHostMutation, *dry_run))
        }
        Commands::Remove { .. } => Some(policy(
            "conary remove",
            CommandRisk::ActiveHostMutation,
            false,
        )),
        Commands::Update {
            package, dry_run, ..
        } if package.as_deref().is_some_and(|name| name.starts_with('@')) => Some(policy(
            "conary update @collection",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        Commands::Update { dry_run, .. } => {
            Some(policy("conary update", CommandRisk::ActiveHostMutation, *dry_run))
        }
        Commands::Autoremove { dry_run, .. } => Some(policy(
            "conary autoremove",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        Commands::System(system) => classify_system(system),
        Commands::Ccs(cli::CcsCommands::Install { dry_run, .. }) => Some(policy(
            "conary ccs install",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        Commands::Model(cli::ModelCommands::Apply { dry_run, .. }) => Some(policy(
            "conary model apply",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        Commands::Automation(cli::AutomationCommands::Apply { dry_run, .. }) => Some(policy(
            "conary automation apply",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        Commands::Pin { .. } => Some(local_state("conary pin")),
        Commands::Unpin { .. } => Some(local_state("conary unpin")),
        Commands::Search { .. }
        | Commands::List { .. }
        | Commands::Cook { .. }
        | Commands::ConvertPkgbuild { .. }
        | Commands::RecipeAudit { .. }
        | Commands::Repo(_)
        | Commands::Config(_)
        | Commands::Distro(_)
        | Commands::Canonical(_)
        | Commands::Groups(_)
        | Commands::Registry(_)
        | Commands::Query(_)
        | Commands::Ccs(_)
        | Commands::Derive(_)
        | Commands::Model(_)
        | Commands::Collection(_)
        | Commands::Automation(_)
        | Commands::Bootstrap(_)
        | Commands::Cache(_)
        | Commands::Derivation(_)
        | Commands::Profile(_)
        | Commands::SelfUpdate { .. }
        | Commands::Provenance(_)
        | Commands::Capability(_)
        | Commands::Trust(_)
        | Commands::VerifyDerivation(_)
        | Commands::Sbom { .. }
        | Commands::Federation(_)
        | Commands::Export { .. } => Some(policy(
            "conary read-only or non-host command",
            CommandRisk::ReadOnly,
            false,
        )),
    }
}

// Do not leave the aggregate `read-only or non-host command` arm in the final
// patch. Replace it with one classifier per nested enum: classify_repo,
// classify_config, classify_ccs, classify_model, classify_automation, and
// equivalent helpers for every Commands::*(_) group. Each helper must match
// every current variant without wildcard arms. Use LocalStateMutation for
// non-host config/metadata mutations such as repository add/remove, trigger
// enable/disable/add/remove, redirect add/remove, collection
// create/add/remove/delete, model snapshot/update/publish, automation
// configure, federation peer add/remove/enable/disable, distro
// set/remove/mixing/selection-mode, trust init/enable/disable, and
// update-channel set/reset. Reserve DbMutation for live package/CAS state that
// must cross the acknowledgement boundary, such as mutating adoption modes. Use
// ReadOnly for pure queries, inspections, local builds, or local output
// generation. A new nested enum variant should break compilation until it is
// classified.

fn classify_system(command: &cli::SystemCommands) -> Option<CommandRiskPolicy> {
    match command {
        cli::SystemCommands::Restore { dry_run, .. } => Some(policy(
            "conary system restore",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        cli::SystemCommands::Adopt {
            system,
            status,
            dry_run,
            refresh,
            convert,
            sync_hook,
            from_sync_hook,
            remove_hook: _,
            packages: _,
            full: _,
            db: _,
            pattern: _,
            exclude: _,
            explicit_only: _,
            jobs: _,
            no_chunking: _,
            quiet,
        } => classify_adopt(
            *system,
            *status,
            *dry_run,
            *refresh,
            *convert,
            *sync_hook,
            *quiet,
            *from_sync_hook,
        ),
        cli::SystemCommands::Unadopt { dry_run, .. } => Some(policy(
            "conary system unadopt",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        cli::SystemCommands::NativeHandoff { dry_run, .. } => Some(policy(
            "conary system native-handoff",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        cli::SystemCommands::State(cli::StateCommands::Revert { dry_run, .. }) => Some(policy(
            "conary system state revert",
            CommandRisk::ActiveHostMutation,
            *dry_run,
        )),
        cli::SystemCommands::State(cli::StateCommands::Rollback { .. }) => Some(policy(
            "conary system state rollback",
            CommandRisk::ActiveHostMutation,
            false,
        )),
        cli::SystemCommands::Generation(cli::GenerationCommands::Build { .. }) => Some(policy(
            "conary system generation build",
            CommandRisk::AlwaysLive,
            false,
        )),
        cli::SystemCommands::Generation(cli::GenerationCommands::Switch { .. }) => Some(policy(
            "conary system generation switch",
            CommandRisk::AlwaysLive,
            false,
        )),
        cli::SystemCommands::Generation(cli::GenerationCommands::Rollback) => Some(policy(
            "conary system generation rollback",
            CommandRisk::AlwaysLive,
            false,
        )),
        cli::SystemCommands::Generation(cli::GenerationCommands::Gc { .. }) => Some(policy(
            "conary system generation gc",
            CommandRisk::AlwaysLive,
            false,
        )),
        cli::SystemCommands::Generation(cli::GenerationCommands::Recover { .. }) => Some(policy(
            "conary system generation recover",
            CommandRisk::AlwaysLive,
            false,
        )),
        cli::SystemCommands::Takeover { dry_run, .. } => Some(policy(
            "conary system takeover",
            CommandRisk::AlwaysLive,
            *dry_run,
        )),
        cli::SystemCommands::Init { .. }
        | cli::SystemCommands::Completions { .. }
        | cli::SystemCommands::History { .. }
        | cli::SystemCommands::Verify { .. }
        | cli::SystemCommands::Gc { .. }
        | cli::SystemCommands::Sbom { .. }
        | cli::SystemCommands::State(_)
        | cli::SystemCommands::Generation(_)
        | cli::SystemCommands::Trigger(_)
        | cli::SystemCommands::Redirect(_)
        | cli::SystemCommands::UpdateChannel { .. } => Some(policy(
            "conary system read-only or non-host command",
            CommandRisk::ReadOnly,
            false,
        )),
    }
}

fn classify_adopt(
    system: bool,
    status: bool,
    dry_run: bool,
    refresh: bool,
    convert: bool,
    sync_hook: bool,
    quiet: bool,
    from_sync_hook: bool,
) -> Option<CommandRiskPolicy> {
    if status {
        Some(policy("conary system adopt --status", CommandRisk::ReadOnly, false))
    } else if sync_hook {
        Some(policy(
            "conary system adopt --sync-hook",
            CommandRisk::ActiveHostMutation,
            false,
        ))
    } else if from_sync_hook && refresh && quiet {
        Some(policy(
            "conary system adopt --refresh --quiet --from-sync-hook",
            CommandRisk::HookRefreshDbMutation,
            false,
        ))
    } else if dry_run && system {
        Some(policy(
            "conary system adopt --system --dry-run",
            CommandRisk::DryRunOnly,
            true,
        ))
    } else if dry_run && refresh {
        Some(policy(
            "conary system adopt --refresh --dry-run",
            CommandRisk::DryRunOnly,
            true,
        ))
    } else if dry_run && convert {
        Some(policy(
            "conary system adopt --convert --dry-run",
            CommandRisk::DryRunOnly,
            true,
        ))
    } else if convert {
        Some(policy("conary system adopt --convert", CommandRisk::DbMutation, false))
    } else if refresh {
        Some(policy("conary system adopt --refresh", CommandRisk::DbMutation, false))
    } else if system {
        Some(policy("conary system adopt --system", CommandRisk::DbMutation, false))
    } else if dry_run {
        Some(policy("conary system adopt <pkg> --dry-run", CommandRisk::DryRunOnly, true))
    } else {
        Some(policy("conary system adopt <pkg>", CommandRisk::DbMutation, false))
    }
}

fn policy(
    command_label: &'static str,
    risk: CommandRisk,
    dry_run: bool,
) -> CommandRiskPolicy {
    CommandRiskPolicy {
        command_label: Cow::Borrowed(command_label),
        risk,
        dry_run,
    }
}
```

This match must compile without wildcard arms for `Commands` and `SystemCommands`. If the compiler reports a missing enum variant, classify that variant explicitly.

- [ ] **Step 5: Export and wire the policy once**

Modify `apps/conary/src/lib.rs`:

```rust
pub mod command_risk;
```

Modify the start of `dispatch` in `apps/conary/src/dispatch.rs`:

```rust
pub async fn dispatch(cli: Cli) -> Result<()> {
    let allow_live_system_mutation = cli.allow_live_system_mutation;
    crate::command_risk::enforce_cli_policy(allow_live_system_mutation, &cli)?;

    match cli.command {
```

Then remove the existing `require_live_mutation` calls from branches now covered
by `command_risk`. Remove the now-unused `allow_live_system_mutation`
parameters from nested dispatch helpers and their call sites, including
`dispatch_system_command`, `dispatch_ccs_command`, `dispatch_model_command`, and
`dispatch_automation_command`. Delete the old local `require_live_mutation`
helper and unused imports once pre-dispatch policy owns the acknowledgement
gate. Keep command behavior unchanged after the pre-dispatch policy check.

- [ ] **Step 6: Run unit tests**

Run:

```bash
cargo test -p conary command_risk -- --nocapture
cargo test -p conary live_host_safety -- --nocapture
```

Expected after implementation: both commands pass.

- [ ] **Step 7: Commit**

```bash
git add apps/conary/src/command_risk.rs apps/conary/src/lib.rs apps/conary/src/live_host_safety.rs apps/conary/src/dispatch.rs
git commit -m "security(cli): centralize live mutation policy"
```

---

### Task 2: Adoption Gate And Dry-Run Regression Coverage

**Files:**
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/commands/adopt/convert.rs`
- Modify: `apps/conary/src/commands/adopt/hooks.rs`
- Modify: `apps/conary/src/cli/system.rs`

- [ ] **Step 1: Add failing integration tests for adoption gate behavior**

First add this hidden flag to the `SystemCommands::Adopt` variant in
`apps/conary/src/cli/system.rs` so the tests can parse the installed-hook
command line:

```rust
/// Internal path used by installed native package-manager sync hooks.
///
/// Requires --refresh --quiet and cannot be combined with --full; hook
/// install/remove remains the explicit consent point.
#[arg(long, hide = true, requires_all = ["refresh", "quiet"], conflicts_with_all = ["system", "status", "convert", "sync_hook", "full"])]
from_sync_hook: bool,
```

Append these helpers and tests to `apps/conary/tests/live_host_mutation_safety.rs`:

```rust
fn seed_adopted_trove_without_source_identity(db_path: &str, name: &str) {
    use conary_core::db;
    use conary_core::db::models::{
        Changeset, ChangesetStatus, InstallSource, Trove, TroveType,
    };

    let mut conn = db::open(db_path).unwrap();
    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new(format!("Seed adopted {name}"));
        let changeset_id = changeset.insert(tx)?;
        let mut trove = Trove::new_with_source(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.source_distro = None;
        trove.version_scheme = None;
        trove.insert(tx)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();
}

fn source_identity_for(db_path: &str, name: &str) -> (Option<String>, Option<String>) {
    let conn = conary_core::db::open(db_path).unwrap();
    conn.query_row(
        "SELECT source_distro, version_scheme FROM troves WHERE name = ?1",
        [name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

#[test]
fn system_adopt_package_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "curl", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system adopt <pkg>"));
    assert!(stderr.contains("--allow-live-system-mutation"));
    assert!(stderr.contains("Conary DB"));
}

#[test]
fn system_adopt_system_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "--system", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system adopt --system"));
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_refresh_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "--refresh", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system adopt --refresh"));
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_convert_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();
    seed_adopted_trove_without_source_identity(&db_path, "curl");

    let output = run_conary(&["system", "adopt", "--convert", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system adopt --convert"));
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_sync_hook_refuses_without_live_mutation_flag() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "--sync-hook", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conary system adopt --sync-hook"));
    assert!(stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_status_bypasses_gate() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "--status", "--db-path", &db_path]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_package_dry_run_is_rejected_without_ack_prompt() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let output = run_conary(&["system", "adopt", "curl", "--dry-run", "--db-path", &db_path]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("single-package adoption dry-run is not implemented"));
    assert!(!stderr.contains("--allow-live-system-mutation"));
}

#[test]
fn system_adopt_convert_dry_run_does_not_backfill_source_identity() {
    let (_tmp, db_path) = common::setup_command_test_db();
    seed_adopted_trove_without_source_identity(&db_path, "curl");

    let output = run_conary(&[
        "system",
        "adopt",
        "--convert",
        "--dry-run",
        "--db-path",
        &db_path,
    ]);

    assert!(output.status.success());
    assert_eq!(source_identity_for(&db_path, "curl"), (None, None));
}
```

Update the existing `apps/conary/src/commands/adopt/hooks.rs` tests so the
generated hook templates assert the constrained hook-only refresh path:

```rust
#[test]
fn test_rpm_hook_content_format() {
    assert!(RPM_SCRIPT_CONTENT.contains(
        "/usr/bin/conary system adopt --refresh --quiet --from-sync-hook"
    ));
    assert!(RPM_SCRIPT_CONTENT.starts_with("#!/bin/sh"));
}

#[test]
fn test_apt_hook_content_format() {
    assert!(APT_HOOK_CONTENT.contains("DPkg::Post-Invoke"));
    assert!(APT_HOOK_CONTENT.contains(
        "/usr/bin/conary system adopt --refresh --quiet --from-sync-hook"
    ));
}

#[test]
fn test_pacman_hook_content_format() {
    assert!(PACMAN_HOOK_CONTENT.contains("[Trigger]"));
    assert!(PACMAN_HOOK_CONTENT.contains("[Action]"));
    assert!(PACMAN_HOOK_CONTENT.contains("PostTransaction"));
    assert!(PACMAN_HOOK_CONTENT.contains(
        "Exec = /usr/bin/conary system adopt --refresh --quiet --from-sync-hook"
    ));
}
```

- [ ] **Step 2: Run the failing adoption gate tests**

Run:

```bash
cargo test -p conary --test live_host_mutation_safety system_adopt -- --nocapture
cargo test -p conary adopt::hooks -- --nocapture
```

Expected before implementation: mutating adoption commands reach their handlers instead of the acknowledgement error, and `system_adopt_convert_dry_run_does_not_backfill_source_identity` fails because the dry-run path writes source identity.

- [ ] **Step 3: Reject unsupported single-package dry-run before mutation**

In `apps/conary/src/dispatch.rs`, inside the `SystemCommands::Adopt` package branch, add this guard before `commands::cmd_adopt`:

```rust
} else {
    if dry_run {
        anyhow::bail!(
            "single-package adoption dry-run is not implemented yet; use `conary system adopt --system --dry-run` for a system-wide preview or rerun without --dry-run and with --allow-live-system-mutation to adopt package(s)"
        );
    }
    commands::cmd_adopt(&packages, &db.db_path, full).await
}
```

Update `apps/conary/src/cli/system.rs` dry-run help for `Adopt`:

```rust
/// Show what would be adopted without making changes
/// Used by: --system, --convert, --refresh. Single-package dry-run is rejected
/// until it has a true non-mutating preview path.
```

- [ ] **Step 4: Move convert backfill behind dry-run**

In `apps/conary/src/commands/adopt/convert.rs`, keep package-manager identity detection but move the DB write below the dry-run return:

```rust
let mut conn = open_db(db_path)?;
let source_identity =
    conary_core::packages::SystemPackageManager::detect().detect_source_identity();

let troves = query_unconverted_adopted(&conn)?;

if troves.is_empty() {
    println!("No unconverted adopted packages found.");
    return Ok(());
}

println!(
    "Found {} adopted packages to convert to CCS format.",
    troves.len()
);

if dry_run {
    for t in &troves {
        println!("  {} {}", t.name, t.version);
    }
    println!("\nDry run: no packages converted.");
    return Ok(());
}

backfill_adopted_source_identity(
    &conn,
    source_identity.source_distro.as_deref(),
    source_identity.version_scheme.as_deref(),
)?;
```

If the regression can pass on a host where package-manager detection returns no
source identity, extract the dry-run ordering into a small helper that accepts an
explicit `InstalledSourceIdentity` in tests. The test must prove that a
non-`None` identity is not written during dry-run; it must not depend on the
CI host having RPM, dpkg, or pacman identity metadata.

- [ ] **Step 5: Encode the constrained installed-hook refresh contract**

In `apps/conary/src/commands/adopt/hooks.rs`, update the module doc comment,
template comments, and generated commands from:

```text
conary system adopt --refresh --quiet
```

to:

```text
conary system adopt --refresh --quiet --from-sync-hook
```

Also update the post-install message:

```rust
println!(
    "The system PM will now auto-refresh Conary adopted-package metadata after package operations through a constrained installed-hook refresh path."
);
```

Do not put the global `--allow-live-system-mutation` flag in generated hook
scripts. The hidden `--from-sync-hook` flag is the narrow installed-hook
contract: it requires `--refresh --quiet`, conflicts with `--full`, is enforced
by `command_risk`, and should not be documented as a general user command.

- [ ] **Step 6: Run adoption gate tests**

Run:

```bash
cargo test -p conary --test live_host_mutation_safety system_adopt -- --nocapture
cargo test -p conary adopt::hooks -- --nocapture
```

Expected after implementation: all `system_adopt*` tests pass and hook-template
tests prove generated hooks include `--from-sync-hook` without
`--allow-live-system-mutation`.

- [ ] **Step 7: Commit**

```bash
git add apps/conary/tests/live_host_mutation_safety.rs apps/conary/src/dispatch.rs apps/conary/src/commands/adopt/convert.rs apps/conary/src/commands/adopt/hooks.rs apps/conary/src/cli/system.rs
git commit -m "security(adopt): gate live adoption mutations"
```

---

### Task 3: Private CAS Storage For Full Adoption

**Files:**
- Modify: `crates/conary-core/src/filesystem/cas.rs`
- Modify: `apps/conary/src/commands/adopt/cas_capture.rs`
- Modify: `apps/conary/src/commands/adopt/system.rs`

- [ ] **Step 1: Add failing CAS mutation and inode tests**

In `apps/conary/src/commands/adopt/cas_capture.rs`, add these tests to the existing test module:

```rust
#[test]
fn full_adoption_cas_survives_in_place_source_mutation() {
    let tmp = tempdir_in_target();
    let source = tmp.path().join("mutable-source");
    std::fs::write(&source, b"original bytes").unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();

    let hash = compute_cas_backed_file_hash(
        source.to_str().unwrap(),
        0o100644,
        Some("package-manager-digest"),
        None,
        &cas,
    )
    .unwrap();

    std::fs::write(&source, b"mutated bytes").unwrap();

    assert_eq!(cas.retrieve(&hash).unwrap(), b"original bytes");
}

#[test]
#[cfg(unix)]
fn full_adoption_regular_file_uses_private_cas_inode() {
    use std::os::unix::fs::MetadataExt;

    let tmp = tempdir_in_target();
    let source = tmp.path().join("private-inode-source");
    std::fs::write(&source, b"private inode bytes").unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();

    let hash = compute_cas_backed_file_hash(
        source.to_str().unwrap(),
        0o100644,
        Some("package-manager-digest"),
        None,
        &cas,
    )
    .unwrap();
    let cas_path = cas.hash_to_path(&hash).unwrap();

    assert_ne!(
        std::fs::metadata(&source).unwrap().ino(),
        std::fs::metadata(&cas_path).unwrap().ino(),
        "live full adoption must not share an inode with mutable source files"
    );
}
```

In `crates/conary-core/src/filesystem/cas.rs`, replace the shared-inode assertion test with a sealed-source-specific name:

```rust
#[test]
#[cfg(unix)]
fn test_hardlink_from_immutable_root_shares_inode() {
    use std::os::unix::fs::MetadataExt;

    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();
    let existing_file = temp_dir.path().join("sealed_inode.txt");
    let content = b"This sealed-source helper intentionally shares an inode";
    fs::write(&existing_file, content).unwrap();
    let original_inode = fs::metadata(&existing_file).unwrap().ino();

    let hash = cas.hardlink_from_immutable_root(&existing_file).unwrap();
    let cas_path = cas.hash_to_path(&hash).unwrap();

    assert_eq!(original_inode, fs::metadata(&cas_path).unwrap().ino());
}

#[test]
#[cfg(unix)]
fn test_store_file_copy_repairs_existing_shared_cas_object() {
    use std::os::unix::fs::MetadataExt;

    let temp_dir = TempDir::new().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let cas = CasStore::new(&cas_dir).unwrap();
    let sealed_file = temp_dir.path().join("sealed.txt");
    let mutable_file = temp_dir.path().join("mutable.txt");
    let content = b"same content through two capture paths";
    fs::write(&sealed_file, content).unwrap();
    fs::write(&mutable_file, content).unwrap();

    let hash = cas.hardlink_from_immutable_root(&sealed_file).unwrap();
    let shared_path = cas.hash_to_path(&hash).unwrap();
    assert_eq!(
        fs::metadata(&sealed_file).unwrap().ino(),
        fs::metadata(&shared_path).unwrap().ino()
    );

    let copied_hash = cas.store_file_copy_from_existing(&mutable_file).unwrap();
    assert_eq!(copied_hash, hash);
    assert_ne!(
        fs::metadata(&sealed_file).unwrap().ino(),
        fs::metadata(&shared_path).unwrap().ino(),
        "mutable-source copy must break a touched legacy shared CAS object"
    );
    assert_eq!(cas.retrieve(&hash).unwrap(), content);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p conary adopt::cas_capture -- --nocapture
cargo test -p conary-core filesystem::cas::tests::test_hardlink_from_immutable_root_shares_inode -- --nocapture
cargo test -p conary-core filesystem::cas::tests::test_store_file_copy_repairs_existing_shared_cas_object -- --nocapture
```

Expected before implementation: the mutation and private inode tests fail, the
shared-object repair test is missing or fails, and `hardlink_from_immutable_root`
is missing.

- [ ] **Step 3: Add private copy storage API and rename hardlink API**

In `crates/conary-core/src/filesystem/cas.rs`, add:

```rust
/// Store an existing mutable file into CAS by copying bytes into a private inode.
///
/// Use this for live adoption and any path whose source can be modified outside
/// Conary after capture.
pub fn store_file_copy_from_existing<P: AsRef<Path>>(&self, existing_path: P) -> Result<String> {
    let content = fs::read(existing_path)?;
    let hash = self.compute_hash(&content);
    self.atomic_store_private_copy(&hash, &content)?;
    Ok(hash)
}

/// Atomically store content into a private CAS inode.
///
/// Unlike `store`, this helper also repairs a touched legacy shared hardlink
/// object. If the hash already exists and appears private, it is left alone for
/// deduplication. If it is shared on Unix, the CAS directory entry is replaced
/// with a fresh inode containing the same content.
fn atomic_store_private_copy(&self, hash: &str, content: &[u8]) -> Result<bool> {
    let path = self.hash_to_path(hash)?;
    if path.exists() && !cas_object_appears_shared(&path)? {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_ext = format!("tmp.{}.{}.private", std::process::id(), Self::next_temp_id());
    let temp_path = path.with_extension(temp_ext);
    let mut file = fs::File::create(&temp_path)?;
    file.write_all(content)?;
    file.sync_all()?;
    fs::rename(&temp_path, &path)?;
    sync_parent_dir(&path)?;
    Ok(true)
}

/// Hardlink an existing file into CAS when the caller proves the source root is sealed.
///
/// Do not use this for live native package-manager files. A hardlink shares the
/// inode with the source and therefore shares future in-place mutations.
pub fn hardlink_from_immutable_root<P: AsRef<Path>>(&self, existing_path: P) -> Result<String> {
    self.hardlink_from_existing_inner(existing_path)
}
```

Add these helpers near `atomic_store` and use `sync_parent_dir` from both
`atomic_store` and `atomic_store_private_copy`:

```rust
#[cfg(unix)]
fn cas_object_appears_shared(path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    Ok(fs::metadata(path)?.nlink() > 1)
}

#[cfg(not(unix))]
fn cas_object_appears_shared(_path: &Path) -> Result<bool> {
    Ok(false)
}

fn sync_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        let dir = fs::File::open(parent)?;
        dir.sync_all()?;
    }
    Ok(())
}
```

Do not leave `hardlink_from_existing` as a public compatibility shim. Move the
current hardlink implementation into a private helper named
`hardlink_from_existing_inner`, and make the only public hardlink entrypoint the
sealed-source name `hardlink_from_immutable_root`. This intentionally breaks any
workspace call site that still tries to hardlink mutable live files.

Apply the same sealed-source naming to the known-hash helper:

```rust
pub fn hardlink_from_immutable_root_with_hash<P: AsRef<Path>>(
    &self,
    existing_path: P,
    expected_hash: &str,
    verify_hash: bool,
) -> Result<String> {
    self.hardlink_from_existing_with_hash_inner(existing_path, expected_hash, verify_hash)
}

```

Move the current known-hash hardlink body into a private helper named
`hardlink_from_existing_with_hash_inner`, and do not leave
`hardlink_from_existing_with_hash` public.

- [ ] **Step 4: Switch live adoption to private CAS storage**

In `apps/conary/src/commands/adopt/cas_capture.rs`, replace:

```rust
cas.hardlink_from_existing(path)
```

with:

```rust
cas.store_file_copy_from_existing(path)
```

In `apps/conary/src/commands/adopt/system.rs`, replace the legacy helper branch:

```rust
match cas_store.hardlink_from_existing(file_path) {
```

with:

```rust
match cas_store.store_file_copy_from_existing(file_path) {
```

Update the nearby comment from `Regular file - use hardlink_from_existing` to:

```rust
// Regular live file - copy into a private CAS inode.
```

Plan A does not scan the entire existing CAS for historical shared inodes. The
repair guarantee is bounded to objects touched through
`store_file_copy_from_existing`; a future audit/repair plan can add a full CAS
sweep if preview operators need it.

- [ ] **Step 5: Run focused CAS tests**

Run:

```bash
cargo test -p conary adopt::cas_capture -- --nocapture
cargo test -p conary-core filesystem::cas -- --nocapture
```

Expected after implementation: CAS capture tests pass, mutable-source capture
does not share source inodes, a touched legacy shared object is repaired into a
private CAS inode, and remaining hardlink tests use the sealed-source helper
name.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/filesystem/cas.rs apps/conary/src/commands/adopt/cas_capture.rs apps/conary/src/commands/adopt/system.rs
git commit -m "security(cas): copy live adoption content into private objects"
```

---

### Task 4: Bulk Adoption Ghost-Trove Cleanup And Degraded Metadata

**Files:**
- Create: `apps/conary/src/commands/adopt/outcome.rs`
- Modify: `apps/conary/src/commands/adopt/mod.rs`
- Modify: `apps/conary/src/commands/adopt/packages.rs`
- Modify: `apps/conary/src/commands/adopt/system.rs`
- Modify: `apps/conary/src/commands/changeset_metadata.rs`

- [ ] **Step 1: Add helper tests for metadata outcomes**

Create `apps/conary/src/commands/adopt/outcome.rs` with tests first:

```rust
// apps/conary/src/commands/adopt/outcome.rs

#[cfg(test)]
mod tests {
    use super::metadata_insert_succeeded;
    use crate::commands::{
        AdoptionWarning, adoption_warnings, metadata_with_adoption_warnings,
        parse_rollback_snapshots,
    };

    #[test]
    fn metadata_insert_succeeded_rejects_all_failed_non_empty_metadata() {
        assert!(!metadata_insert_succeeded(3, 3));
    }

    #[test]
    fn metadata_insert_succeeded_allows_partial_success_and_empty_real_metadata() {
        assert!(metadata_insert_succeeded(3, 2));
        assert!(metadata_insert_succeeded(0, 0));
    }

    #[test]
    fn adoption_warning_metadata_preserves_versioned_envelope() {
        let json = metadata_with_adoption_warnings(vec![], vec![], vec![
            AdoptionWarning::partial_insert_failure("curl", 4, 1),
            AdoptionWarning::all_insert_failure("bash", 3),
        ])
        .unwrap();

        assert!(json.contains("\"schema\":\"conary.changeset.metadata.v1\""));
        assert!(json.contains("\"package\":\"curl\""));
        assert!(json.contains("\"reason\":\"partial_metadata_insert_failure\""));
        assert!(json.contains("\"package\":\"bash\""));
        assert!(json.contains("\"reason\":\"all_metadata_inserts_failed\""));
        assert!(parse_rollback_snapshots(&json).unwrap().is_empty());
        assert_eq!(adoption_warnings(Some(&json)).len(), 2);
    }
}
```

- [ ] **Step 2: Run helper tests to verify failure**

Run:

```bash
cargo test -p conary adopt::outcome -- --nocapture
```

Expected before implementation: `adopt::outcome` does not exist.

- [ ] **Step 3: Implement shared outcome helpers**

First extend `apps/conary/src/commands/changeset_metadata.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AdoptionWarning {
    pub package: String,
    pub reason: String,
    pub total_inserts: usize,
    pub failed_inserts: usize,
}

impl AdoptionWarning {
    pub(crate) fn partial_insert_failure(
        package: impl Into<String>,
        total_inserts: usize,
        failed_inserts: usize,
    ) -> Self {
        Self {
            package: package.into(),
            reason: "partial_metadata_insert_failure".to_string(),
            total_inserts,
            failed_inserts,
        }
    }

    pub(crate) fn all_insert_failure(package: impl Into<String>, total_inserts: usize) -> Self {
        Self {
            package: package.into(),
            reason: "all_metadata_inserts_failed".to_string(),
            total_inserts,
            failed_inserts: total_inserts,
        }
    }

    pub(crate) fn refresh_replacement_failure(
        package: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            package: package.into(),
            reason: format!("refresh_replacement_failed: {}", message.into()),
            total_inserts: 0,
            failed_inserts: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChangesetMetadataEnvelope {
    pub schema: String,
    #[serde(default)]
    pub removed_troves: Vec<TroveSnapshot>,
    #[serde(default)]
    pub deferred_follow_up: Vec<DeferredFollowUp>,
    #[serde(default)]
    pub adoption_warnings: Vec<AdoptionWarning>,
}
```

Update `metadata_with_removed_troves` and `metadata_with_deferred_follow_up` so
they initialize `adoption_warnings: Vec::new()`. Add:

```rust
pub(crate) fn metadata_with_adoption_warnings(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
    adoption_warnings: Vec<AdoptionWarning>,
) -> Result<String> {
    serde_json::to_string(&ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: snapshots,
        deferred_follow_up,
        adoption_warnings,
    })
    .map_err(Into::into)
}

pub(crate) fn adoption_warnings(snapshot_json: Option<&str>) -> Vec<AdoptionWarning> {
    snapshot_json
        .and_then(|raw| serde_json::from_str::<ChangesetMetadataEnvelope>(raw).ok())
        .filter(|envelope| envelope.schema == CHANGESET_METADATA_SCHEMA)
        .map(|envelope| envelope.adoption_warnings)
        .unwrap_or_default()
}

pub(crate) fn append_adoption_warning_metadata(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    warnings: Vec<AdoptionWarning>,
) -> Result<()> {
    if warnings.is_empty() {
        return Ok(());
    }

    let existing: Option<String> = conn.query_row(
        "SELECT metadata FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get(0),
    )?;
    let removed_troves = existing
        .as_deref()
        .map(parse_rollback_snapshots)
        .transpose()?
        .unwrap_or_default();
    let deferred = deferred_follow_up(existing.as_deref());
    let mut existing_warnings = adoption_warnings(existing.as_deref());
    existing_warnings.extend(warnings);

    let metadata =
        metadata_with_adoption_warnings(removed_troves, deferred, existing_warnings)?;
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![metadata, changeset_id],
    )?;
    Ok(())
}
```

Update `apps/conary/src/commands/mod.rs` re-exports to include
`AdoptionWarning`, `adoption_warnings`, `append_adoption_warning_metadata`, and
`metadata_with_adoption_warnings`.

Then implement `apps/conary/src/commands/adopt/outcome.rs`:

```rust
// apps/conary/src/commands/adopt/outcome.rs

use anyhow::Result;
use crate::commands::{AdoptionWarning, append_adoption_warning_metadata};

pub(crate) fn metadata_insert_succeeded(total_inserts: usize, insert_failures: usize) -> bool {
    total_inserts == 0 || insert_failures < total_inserts
}

pub(crate) fn write_warning_metadata(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    warnings: Vec<AdoptionWarning>,
) -> Result<()> {
    append_adoption_warning_metadata(conn, changeset_id, warnings)
}
```

Modify `apps/conary/src/commands/adopt/mod.rs`:

```rust
mod outcome;
```

Remove the local `metadata_insert_succeeded` helper and tests from `packages.rs`, then import the shared helper:

```rust
use crate::commands::AdoptionWarning;
use super::outcome::{metadata_insert_succeeded, write_warning_metadata};
```

- [ ] **Step 4: Persist partial-insert warnings in single-package adoption**

In `apps/conary/src/commands/adopt/packages.rs`, inside the DB transaction after metadata inserts and before `changeset.update_status`, add:

```rust
let warnings = if total_inserts > 0 && insert_failures > 0 {
    vec![AdoptionWarning::partial_insert_failure(
        pkg_name.clone(),
        total_inserts,
        insert_failures,
    )]
} else {
    Vec::new()
};

write_warning_metadata(tx, changeset_id, warnings)?;
```

Keep the existing all-failed branch that deletes the trove and marks the changeset rolled back.

- [ ] **Step 5: Delete ghost troves in bulk adoption and persist warnings**

In `apps/conary/src/commands/adopt/system.rs`, import:

```rust
use crate::commands::AdoptionWarning;
use super::outcome::{metadata_insert_succeeded, write_warning_metadata};
```

Before the `for pkg in packages` loop inside the transaction, add:

```rust
let mut adoption_warnings = Vec::new();
```

Replace the all-failed branch:

```rust
if total_inserts > 0 && insert_failures == total_inserts {
    warn!(
        "All {} insert(s) failed for '{}'; skipping trove",
        total_inserts, pkg.name
    );
    error_count += 1;
    continue;
}
```

with:

```rust
if !metadata_insert_succeeded(total_inserts, insert_failures) {
    warn!(
        "All {} insert(s) failed for '{}'; removing empty adopted trove",
        total_inserts, pkg.name
    );
    Trove::delete(tx, trove_id)?;
    adoption_warnings.push(AdoptionWarning::all_insert_failure(
        pkg.name.clone(),
        total_inserts,
    ));
    error_count += 1;
    continue;
}

if total_inserts > 0 && insert_failures > 0 {
    adoption_warnings.push(AdoptionWarning::partial_insert_failure(
        pkg.name.clone(),
        total_inserts,
        insert_failures,
    ));
}
```

Before `changeset.update_status(tx, ChangesetStatus::Applied)?;`, add:

```rust
write_warning_metadata(tx, changeset_id, adoption_warnings)?;
```

Track a `degraded_count` alongside `adopted_count` and `error_count`. Increment
it for packages with partial metadata insert failures and print an operator
summary after the transaction:

```rust
if degraded_count > 0 {
    println!(
        "Adopted with warnings: {degraded_count} package(s). Run `conary system history` to inspect adoption warning metadata."
    );
}
```

Single-package adoption should print the same warning when `warnings` is not
empty. Do not count an all-failed package as adopted; it is skipped/failed and
the trove is deleted.

- [ ] **Step 6: Add ghost-trove production-helper regression**

Do not test ghost-trove cleanup by calling `Trove::delete` directly in the test.
Extract the bulk-adoption all-failed branch into a helper in
`apps/conary/src/commands/adopt/system.rs`, and call that helper from the real
bulk-adoption loop:

```rust
fn finalize_bulk_metadata_insert_outcome(
    tx: &rusqlite::Connection,
    trove_id: i64,
    package_name: &str,
    total_inserts: usize,
    insert_failures: usize,
    adoption_warnings: &mut Vec<AdoptionWarning>,
) -> Result<bool> {
    if metadata_insert_succeeded(total_inserts, insert_failures) {
        if total_inserts > 0 && insert_failures > 0 {
            adoption_warnings.push(AdoptionWarning::partial_insert_failure(
                package_name.to_string(),
                total_inserts,
                insert_failures,
            ));
        }
        return Ok(true);
    }

    warn!(
        "All {} insert(s) failed for '{}'; removing empty adopted trove",
        total_inserts, package_name
    );
    Trove::delete(tx, trove_id)?;
    adoption_warnings.push(AdoptionWarning::all_insert_failure(
        package_name.to_string(),
        total_inserts,
    ));
    Ok(false)
}
```

Then add a unit test in `apps/conary/src/commands/adopt/system.rs` that proves
the production helper deletes the trove and records the warning:

```rust
#[test]
fn all_failed_bulk_outcome_helper_deletes_seeded_trove() {
    use conary_core::db;
    use conary_core::db::models::{Trove, TroveType};

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db").to_string_lossy().into_owned();
    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        let mut trove = Trove::new(
            "ghost".to_string(),
            "1.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(tx)?;

        let mut warnings = Vec::new();
        let keep_trove =
            finalize_bulk_metadata_insert_outcome(tx, trove_id, "ghost", 3, 3, &mut warnings)?;
        assert!(!keep_trove);
        assert_eq!(warnings.len(), 1);

        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM troves WHERE id = ?1",
            [trove_id],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
}
```

- [ ] **Step 7: Run adoption outcome tests**

Run:

```bash
cargo test -p conary adopt::outcome -- --nocapture
cargo test -p conary adopt::packages -- --nocapture
cargo test -p conary adopt::system::tests::all_failed_bulk_outcome_helper_deletes_seeded_trove -- --nocapture
cargo test -p conary changeset_metadata -- --nocapture
```

Expected after implementation: outcome tests pass, changeset metadata tests prove
adoption warnings preserve rollback/deferred metadata, the bulk-adoption helper
test proves the production cleanup helper deletes ghost troves, and package
adoption tests still pass.

- [ ] **Step 8: Commit**

```bash
git add apps/conary/src/commands/adopt/outcome.rs apps/conary/src/commands/adopt/mod.rs apps/conary/src/commands/adopt/packages.rs apps/conary/src/commands/adopt/system.rs apps/conary/src/commands/changeset_metadata.rs apps/conary/src/commands/mod.rs
git commit -m "fix(adopt): remove ghost troves on metadata failure"
```

---

### Task 5: Refresh Metadata Replacement Savepoints

**Files:**
- Modify: `apps/conary/src/commands/adopt/refresh.rs`
- Test: `apps/conary/src/commands/adopt/refresh.rs`

- [ ] **Step 1: Add a failing savepoint rollback test**

At the bottom of `apps/conary/src/commands/adopt/refresh.rs`, add a test module if one does not exist:

```rust
#[cfg(test)]
mod tests {
    use conary_core::db;
    use conary_core::db::models::{
        Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, ProvideEntry,
        Trove, TroveType,
    };

    fn create_refresh_test_db() -> (tempfile::TempDir, String, rusqlite::Connection, i64) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db").to_string_lossy().into_owned();
        db::init(&db_path).unwrap();
        let mut conn = db::open(&db_path).unwrap();
        let trove_id = db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new("seed adopted".to_string());
            let changeset_id = changeset.insert(tx)?;
            let mut trove = Trove::new_with_source(
                "curl".to_string(),
                "8.8.0".to_string(),
                TroveType::Package,
                InstallSource::AdoptedFull,
            );
            trove.installed_by_changeset_id = Some(changeset_id);
            let trove_id = trove.insert(tx)?;
            let mut file = FileEntry::new(
                "/usr/bin/curl".to_string(),
                "old-hash".to_string(),
                4,
                0o100755,
                trove_id,
            );
            file.insert(tx)?;
            let mut dep = DependencyEntry::new(
                trove_id,
                "openssl".to_string(),
                None,
                "runtime".to_string(),
                None,
            );
            dep.insert(tx)?;
            let mut provide = ProvideEntry::new(trove_id, "curl".to_string(), None);
            provide.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(trove_id)
        })
        .unwrap();
        (tmp, db_path, conn, trove_id)
    }

    #[test]
    fn refresh_savepoint_preserves_old_children_when_replacement_fails() {
        let (_tmp, _db_path, mut conn, trove_id) = create_refresh_test_db();
        let result = db::transaction(&mut conn, |tx| {
            let err = replace_refresh_children_for_package_for_test(tx, trove_id, true)
                .expect_err("injected replacement failure should be isolated to savepoint");
            assert!(err.to_string().contains("injected refresh child replacement failure"));

            tx.execute(
                "UPDATE troves SET description = ?1 WHERE id = ?2",
                ("outer transaction committed", trove_id),
            )?;
            Ok(())
        });

        assert!(result.is_ok());

        let file_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files WHERE trove_id = ?1", [trove_id], |row| {
                row.get(0)
            })
            .unwrap();
        let dep_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dependencies WHERE trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();
        let provide_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provides WHERE trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(file_count, 1);
        assert_eq!(dep_count, 1);
        assert_eq!(provide_count, 1);
        let description: String = conn
            .query_row("SELECT description FROM troves WHERE id = ?1", [trove_id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(description, "outer transaction committed");
    }
}
```

- [ ] **Step 2: Run the failing refresh test**

Run:

```bash
cargo test -p conary adopt::refresh::tests::refresh_savepoint_preserves_old_children_when_replacement_fails -- --nocapture
```

Expected before implementation: `replace_refresh_children_for_package_for_test`
is missing.

- [ ] **Step 3: Extract child replacement into a savepoint helper**

In `apps/conary/src/commands/adopt/refresh.rs`, add a helper near the refresh transaction code:

```rust
fn with_refresh_savepoint<T>(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let savepoint = format!("refresh_trove_{trove_id}");
    tx.execute_batch(&format!("SAVEPOINT {savepoint}"))?;
    match f(tx) {
        Ok(value) => {
            tx.execute_batch(&format!("RELEASE {savepoint}"))?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.execute_batch(&format!("ROLLBACK TO {savepoint}"));
            let _ = tx.execute_batch(&format!("RELEASE {savepoint}"));
            Err(error)
        }
    }
}
```

Extract the `UPDATE troves`, `DELETE FROM files/dependencies/provides`, and
replacement insert loops for `DriftOutcome::Updated` into a production helper
called by `with_refresh_savepoint`. The production `DriftOutcome::Updated` arm
must delegate to this helper; do not leave raw child-row deletes in the match
arm. Insert failures must return `Err` instead of logging and continuing:

```rust
fe.insert_or_replace(tx).map_err(|e| {
    anyhow::anyhow!("failed to insert refreshed file {file_path} for {}: {e}", trove.name)
})?;
```

Use the same pattern for dependencies and provides.

For test injection, add a `#[cfg(test)]` wrapper that calls the same production
helper used by `DriftOutcome::Updated`. It may pass fixture replacement metadata
and an injected failure flag, but it must not reimplement the delete/insert
sequence itself:

```rust
struct RefreshReplacement {
    files: Vec<(FileInfoTuple, String)>,
    deps: Vec<DependencyInfo>,
    provides: Vec<String>,
}

impl RefreshReplacement {
    #[cfg(test)]
    fn test_fixture(trove_id: i64) -> Self {
        let _ = trove_id;
        Self {
            files: Vec::new(),
            deps: Vec::new(),
            provides: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefreshFailureInjection {
    None,
    AfterDelete,
}

impl RefreshFailureInjection {
    #[cfg(test)]
    fn after_delete(enabled: bool) -> Self {
        if enabled {
            Self::AfterDelete
        } else {
            Self::None
        }
    }
}

#[cfg(test)]
fn replace_refresh_children_for_package_for_test(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    fail_after_delete: bool,
) -> Result<()> {
    let replacement = RefreshReplacement::test_fixture(trove_id);
    replace_refresh_children_for_package(
        tx,
        trove_id,
        &replacement,
        RefreshFailureInjection::after_delete(fail_after_delete),
    )
}
```

If a replacement failure is returned for one package, catch it at the package
loop, push
`AdoptionWarning::refresh_replacement_failure(pkg.name.clone(), error.to_string())`,
leave the old metadata intact through the savepoint rollback, increment a
`degraded_count`, and continue refreshing the remaining packages. Persist the
warnings with `write_warning_metadata` before the refresh changeset is marked
applied.

Add the imports:

```rust
use crate::commands::AdoptionWarning;
use super::outcome::write_warning_metadata;
```

- [ ] **Step 4: Run refresh tests**

Run:

```bash
cargo test -p conary adopt::refresh -- --nocapture
```

Expected after implementation: the injected failure test proves the outer
transaction can still commit after a per-package replacement failure, old child
rows remain intact, degraded refresh warnings are persisted, and existing
refresh tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/adopt/refresh.rs
git commit -m "fix(adopt): preserve metadata on refresh failure"
```

---

### Task 6: Final Plan A Verification And Docs Touchups

**Files:**
- Modify if needed: `README.md`
- Modify if needed: `docs/INTEGRATION-TESTING.md`
- Modify if needed: `docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md`

- [ ] **Step 1: Search for adoption command text that became stale**

Run:

```bash
rg -n "system adopt|adopt-system|--sync-hook|--convert|--full|--dry-run" README.md ROADMAP.md docs apps/conary/src apps/conary/tests
```

Expected: active docs and app strings should not claim single-package `system adopt <pkg> --dry-run` works. If they do, update the text to say single-package adoption dry-run is currently rejected until a non-mutating preview path exists.

- [ ] **Step 2: Run focused Plan A gates**

Run:

```bash
cargo test -p conary --test live_host_mutation_safety -- --nocapture
cargo test -p conary adopt::cas_capture -- --nocapture
cargo test -p conary adopt::hooks -- --nocapture
cargo test -p conary adopt::outcome -- --nocapture
cargo test -p conary adopt::refresh -- --nocapture
cargo test -p conary changeset_metadata -- --nocapture
cargo test -p conary-core filesystem::cas -- --nocapture
```

Expected: all focused Plan A tests pass.

- [ ] **Step 3: Run repo verification gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
git diff --check
```

Expected: all commands exit 0. If docs under `docs/superpowers/` were added,
moved, or archived, update the docs-audit inventory and ledger before rerunning
the inventory diff and ledger check. There should be no deprecation warnings
from old hardlink APIs because the public old names were removed or made
private.

- [ ] **Step 4: Update the umbrella spec status note if Plan A lands**

After the final Plan A code commit exists, update
`docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md` so
Track 1 and Track 2 note that exact Plan A implementation commit SHA and current
status. This should be a follow-up docs commit. Do not archive the umbrella yet
unless Plans B and C are also landed or explicitly deferred.

- [ ] **Step 5: Commit final docs touchups**

```bash
git diff --name-only
git add <exact Plan A docs/app-string files changed in this step>
git status --short
git commit -m "docs(adopt): record adoption safety hardening"
```

Before committing, confirm `git status --short` contains only Plan A docs or
app-string files intended for this final docs commit. Do not stage broad
directories. If Step 1 found no docs changes, skip this commit and record "no
docs touchups needed" in the final implementation summary.
