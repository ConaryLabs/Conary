# Phase 21 Dispatch Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `apps/conary/src/dispatch.rs` from a 2,177-line CLI routing hotspot into a small public hub plus focused child router modules without changing any CLI route, live-mutation gate, command label, or public entrypoint.

**Architecture:** Keep `crate::dispatch::dispatch(cli)` as the only public dispatch API used by `apps/conary/src/app.rs`. Add child modules under `apps/conary/src/dispatch/` for shared dispatch helpers, the top-level command match, and each CLI command namespace. Preserve all command implementation calls in `crate::commands::*`; this phase only moves routing glue and live-host safety gate placement.

**Tech Stack:** Rust 2024 file modules, `anyhow`, `clap`, `clap_complete`, existing `crate::cli` command enums, existing `crate::commands` command implementations, `crate::command_risk`, `crate::live_host_safety`.

---

## Current Repo Facts To Preserve

- `apps/conary/src/dispatch.rs` is 2,177 lines and is currently the largest Rust hotspot:

```text
lines	path
2177	apps/conary/src/dispatch.rs
2147	crates/conary-core/src/generation/builder.rs
1990	apps/conary/src/commands/remove.rs
```

- `apps/conary/src/lib.rs` exports the module with:

```rust
pub mod dispatch;
```

- `apps/conary/src/app.rs` is the only production caller:

```rust
use crate::dispatch;

dispatch::dispatch(cli).await
```

- `dispatch.rs` currently has no direct unit tests:

```bash
cargo test -p conary --lib dispatch -- --list
# 0 tests, 0 benchmarks
```

- `dispatch.rs` owns two shared helper functions:

```text
17:fn require_live_mutation(
31:fn legacy_replay_options(
```

- `dispatch.rs` owns one public entrypoint:

```text
41:pub async fn dispatch(cli: Cli) -> Result<()>
```

- It also owns the following private subcommand dispatch functions:

```text
dispatch_cache_command
dispatch_system_command
dispatch_repo_command
dispatch_config_command
dispatch_query_command
dispatch_collection_command
dispatch_ccs_command
dispatch_derive_command
dispatch_model_command
dispatch_automation_command
dispatch_bootstrap_command
dispatch_provenance_command
dispatch_capability_command
dispatch_trust_command
dispatch_federation_command
dispatch_distro_command
dispatch_canonical_command
dispatch_groups_command
dispatch_registry_command
dispatch_derivation_command
dispatch_profile_command
dispatch_verify_derivation_command
```

- The top-level `Commands` match currently routes these variants:

```text
Install
Remove
Update
Search
List
Autoremove
Pin
Unpin
Cook
ConvertPkgbuild
RecipeAudit
Cache
System
Repo
Config
Query
Collection
Ccs
Derive
Model
Automation
Bootstrap
Provenance
Capability
Trust
Federation
Distro
Canonical
Groups
Registry
Export
Derivation
Profile
SelfUpdate
VerifyDerivation
Sbom
None
```

- The following live-mutation labels, classes, and dry-run parameters must remain byte-for-byte equivalent in behavior. Every row uses `MutationIntent::from_apply_intent(yes, allow_live_system_mutation)`:

| Command label | Final router | `LiveMutationClass` | Dry-run expression |
| --- | --- | --- | --- |
| `conary install @collection` | `root.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary install` | `root.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary remove` | `root.rs` | `CurrentlyLiveEvenWithRootArguments` | `false` |
| `conary update @collection` | `root.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary update` | `root.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary autoremove` | `root.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary system restore` | `system.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary system unadopt` | `system.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary system native-handoff` | `system.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary system db-backup recover` | `system.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary system state revert` | `system_state.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary system state rollback` | `system_state.rs` | `CurrentlyLiveEvenWithRootArguments` | `false` |
| `conary system generation build` | `system_generation.rs` | `AlwaysLive` | `false` |
| `conary system generation publish` | `system_generation.rs` | `AlwaysLive` | `false` |
| `conary system generation recover-db` | `system_generation.rs` | `AlwaysLive` | `dry_run` |
| `conary system generation switch` | `system_generation.rs` | `AlwaysLive` | `false` |
| `conary system generation rollback` | `system_generation.rs` | `AlwaysLive` | `false` |
| `conary system generation gc` | `system_generation.rs` | `AlwaysLive` | `false` |
| `conary system generation recover` | `system_generation.rs` | `AlwaysLive` | `false` |
| `conary system takeover` | `system.rs` | `AlwaysLive` | `dry_run` |
| `conary ccs install` | `ccs.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary model apply` | `model.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |
| `conary automation apply` | `automation.rs` | `CurrentlyLiveEvenWithRootArguments` | `dry_run` |

- `docs/ARCHITECTURE.md` currently names `dispatch.rs` as "Command routing and live-host safety gates".
- `docs/llms/subsystem-map.md` routes `apps/conary/` work through "argument parsing, and command dispatch".
- `docs/modules/query.md` currently mentions `apps/conary/src/dispatch.rs` for label routing and top-level `sbom` dispatch.
- Docs-audit baseline before locking this plan:
  - inventory: `164`
  - ledger counts: `archived 73`, `corrected 64`, `retained-historical 14`, `verified-no-change 13`
- After locking this plan file into git, docs-audit should move to inventory `165` and corrected rows `65`.

## Desired End State

```text
apps/conary/src/dispatch.rs
apps/conary/src/dispatch/automation.rs
apps/conary/src/dispatch/bootstrap.rs
apps/conary/src/dispatch/cache.rs
apps/conary/src/dispatch/capability.rs
apps/conary/src/dispatch/catalog.rs
apps/conary/src/dispatch/ccs.rs
apps/conary/src/dispatch/collection.rs
apps/conary/src/dispatch/config.rs
apps/conary/src/dispatch/context.rs
apps/conary/src/dispatch/derivation.rs
apps/conary/src/dispatch/derive.rs
apps/conary/src/dispatch/federation.rs
apps/conary/src/dispatch/model.rs
apps/conary/src/dispatch/profile.rs
apps/conary/src/dispatch/provenance.rs
apps/conary/src/dispatch/query.rs
apps/conary/src/dispatch/repo.rs
apps/conary/src/dispatch/root.rs
apps/conary/src/dispatch/system.rs
apps/conary/src/dispatch/system_generation.rs
apps/conary/src/dispatch/system_redirect.rs
apps/conary/src/dispatch/system_state.rs
apps/conary/src/dispatch/system_trigger.rs
apps/conary/src/dispatch/system_update_channel.rs
apps/conary/src/dispatch/trust.rs
apps/conary/src/dispatch/verify_derivation.rs
```

Final `apps/conary/src/dispatch.rs` should contain only:

- path comment and module docs,
- child module declarations,
- imports for `anyhow::Result`, `crate::cli::Cli`, and `crate::command_risk`,
- public `dispatch(cli)` wrapper that enforces `command_risk::enforce_cli_policy` before delegating.

Sketch:

```rust
// apps/conary/src/dispatch.rs
//! Conary CLI command dispatch.

mod automation;
mod bootstrap;
mod cache;
mod capability;
mod catalog;
mod ccs;
mod collection;
mod config;
mod context;
mod derivation;
mod derive;
mod federation;
mod model;
mod profile;
mod provenance;
mod query;
mod repo;
mod root;
mod system;
mod system_generation;
mod system_redirect;
mod system_state;
mod system_trigger;
mod system_update_channel;
mod trust;
mod verify_derivation;

use anyhow::Result;

use crate::cli::Cli;
use crate::command_risk;

pub async fn dispatch(cli: Cli) -> Result<()> {
    let allow_live_system_mutation = cli.allow_live_system_mutation;
    command_risk::enforce_cli_policy(allow_live_system_mutation, &cli)?;
    root::dispatch_command(cli.command, allow_live_system_mutation).await
}
```

## Design Choice

Three decomposition paths were considered:

1. **Private dispatch hub plus command namespace routers.** This is the recommended path. It preserves `crate::dispatch::dispatch`, keeps live-host policy enforcement centralized, and makes each CLI namespace easy to review without crossing into command implementation modules.
2. **Move dispatch routing into each `commands/` implementation module.** This would reduce `dispatch.rs`, but it would mix CLI argument destructuring and live-host policy gates into command implementation ownership. That makes command modules harder to test and weakens the central command-risk story.
3. **Split only the existing private helper functions and keep the top-level match in `dispatch.rs`.** This is lower risk but leaves the public dispatch file at roughly 500 lines of root-match glue and keeps unrelated singleton routes packed together.

Use option 1.

## Rust Module Resolution

The repository uses Rust 2024. Keeping `apps/conary/src/dispatch.rs` as a file module while declaring child modules under `apps/conary/src/dispatch/` is valid Rust module resolution:

```rust
// apps/conary/src/dispatch.rs
mod root;
```

resolves to:

```text
apps/conary/src/dispatch/root.rs
```

Do not create `apps/conary/src/dispatch/mod.rs`.

## Visibility Contract

- `dispatch(cli)` remains `pub async fn` in `dispatch.rs`.
- All child module dispatch functions should be `pub(super)` and called only within `crate::dispatch`.
- `context::require_live_mutation` and `context::legacy_replay_options` should be `pub(super)`.
- Child modules should import `crate::cli` and `crate::commands` directly instead of relying on parent private imports.
- No child module should re-export public API.
- No production module should use `use super::*`; import the exact sibling helpers it needs.
- `command_risk::enforce_cli_policy` stays in the public `dispatch(cli)` wrapper and must run before any command match or command implementation call.
- `root::dispatch_command` owns `None` handling and all top-level `Commands::*` destructuring.

## File Responsibility Map

| File | Responsibility |
| --- | --- |
| `dispatch.rs` | Public dispatch wrapper and module declarations only. |
| `dispatch/context.rs` | Shared live-mutation gate helper and legacy replay option construction. |
| `dispatch/root.rs` | Top-level `Option<Commands>` match, root singleton commands, and delegation to namespace routers. |
| `dispatch/cache.rs` | `CacheCommands` routes. |
| `dispatch/system.rs` | `SystemCommands` routes except nested state/generation/trigger/redirect/update-channel matches. |
| `dispatch/system_state.rs` | `StateCommands` routes. |
| `dispatch/system_generation.rs` | `GenerationCommands` routes. |
| `dispatch/system_trigger.rs` | `TriggerCommands` routes. |
| `dispatch/system_redirect.rs` | `RedirectCommands` routes. |
| `dispatch/system_update_channel.rs` | `UpdateChannelAction` routes. |
| `dispatch/repo.rs` | `RepoCommands` routes and `CliSecurityAdvisorySupport` mapping. |
| `dispatch/config.rs` | `ConfigCommands` routes. |
| `dispatch/query.rs` | `QueryCommands` routes, including label subcommands. |
| `dispatch/collection.rs` | `CollectionCommands` routes. |
| `dispatch/ccs.rs` | `CcsCommands` routes and CCS install live-mutation gate. |
| `dispatch/derive.rs` | `DeriveCommands` routes for derived package management. |
| `dispatch/model.rs` | `ModelCommands` routes and model apply live-mutation gate. |
| `dispatch/automation.rs` | `AutomationCommands` routes and automation apply live-mutation gate. |
| `dispatch/bootstrap.rs` | `BootstrapCommands` routes. |
| `dispatch/provenance.rs` | `ProvenanceCommands` routes. |
| `dispatch/capability.rs` | `CapabilityCommands` routes. |
| `dispatch/trust.rs` | `TrustCommands` routes. |
| `dispatch/federation.rs` | `FederationCommands` routes. |
| `dispatch/catalog.rs` | Distro, canonical, groups, and registry route helpers. These are grouped because each router is small and they collectively own repo/distro catalog identity surfaces. |
| `dispatch/derivation.rs` | Derivation engine `DerivationCommands` routes. |
| `dispatch/profile.rs` | `ProfileCommands` routes. |
| `dispatch/verify_derivation.rs` | `VerifyCommands` derivation verification routes. |

## Import Surfaces

### `context.rs`

```rust
// apps/conary/src/dispatch/context.rs

use std::borrow::Cow;

use anyhow::Result;

use crate::commands;
use crate::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, MutationIntent, require_mutation_intent,
};
```

### `root.rs`

```rust
// apps/conary/src/dispatch/root.rs

use std::borrow::Cow;
use std::path::Path;

use anyhow::Result;

use super::automation::dispatch_automation_command;
use super::bootstrap::dispatch_bootstrap_command;
use super::cache::dispatch_cache_command;
use super::capability::dispatch_capability_command;
use super::catalog::{
    dispatch_canonical_command, dispatch_distro_command, dispatch_groups_command,
    dispatch_registry_command,
};
use super::ccs::dispatch_ccs_command;
use super::collection::dispatch_collection_command;
use super::config::dispatch_config_command;
use super::context::{legacy_replay_options, require_live_mutation};
use super::derivation::dispatch_derivation_command;
use super::derive::dispatch_derive_command;
use super::federation::dispatch_federation_command;
use super::model::dispatch_model_command;
use super::profile::dispatch_profile_command;
use super::provenance::dispatch_provenance_command;
use super::query::dispatch_query_command;
use super::repo::dispatch_repo_command;
use super::system::dispatch_system_command;
use super::trust::dispatch_trust_command;
use super::verify_derivation::dispatch_verify_derivation_command;
use crate::cli::{self, Commands};
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
```

### Namespace Router Import Pattern

Each namespace router should follow this pattern and add only the extra imports it actually needs:

```rust
// apps/conary/src/dispatch/<namespace>.rs

use anyhow::Result;

use crate::cli;
use crate::commands;
```

Routers that call live-mutation gates also need:

```rust
use std::borrow::Cow;

use super::context::require_live_mutation;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
```

`root.rs` and `ccs.rs` also need `legacy_replay_options`; `model.rs` and `automation.rs` do not.

Routers with nested sibling delegates import them explicitly. For example, `system.rs` needs:

```rust
use clap::CommandFactory;
use clap_complete::generate;
use std::borrow::Cow;
use std::io;

use super::context::require_live_mutation;
use super::system_generation::dispatch_system_generation_command;
use super::system_redirect::dispatch_system_redirect_command;
use super::system_state::dispatch_system_state_command;
use super::system_trigger::dispatch_system_trigger_command;
use super::system_update_channel::dispatch_system_update_channel_action;
use crate::cli::{self, Cli};
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
```

`repo.rs` additionally needs:

```rust
use conary_core::db::models::SecurityAdvisorySupport;
```

and should shorten the current fully qualified mapping to:

```rust
let security_advisory_support = match security_advisories {
    cli::CliSecurityAdvisorySupport::Unknown => SecurityAdvisorySupport::Unknown,
    cli::CliSecurityAdvisorySupport::Unsupported => SecurityAdvisorySupport::Unsupported,
    cli::CliSecurityAdvisorySupport::Supported => SecurityAdvisorySupport::Supported,
};
```

`system.rs` needs `Cli::command()` for completion generation. Do not move completion generation into `root.rs`.

`bootstrap.rs` should keep the existing `anyhow::anyhow!("--from is required when not using --from-adopted")` expression fully qualified unless it imports `anyhow`.

## Task 0: Lock In The Reviewed Plan

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Add: `docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase21-dispatch-decomposition-plan.md`

- [ ] **Step 1: Confirm baseline is clean**

Run:

```bash
git status --short --branch --untracked-files=no
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected before staging this plan into the index:

```text
## main...origin/main
164
archived 73
corrected 64
retained-historical 14
verified-no-change 13
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 2: Add the Phase 21 ledger row**

Append one row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```text
docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase21-dispatch-decomposition-plan.md	docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase21-dispatch-decomposition-plan.md	planning	maintainer	maintainability; phase21; conary-dispatch; cli-routing; hotspot-decomposition	apps/conary/src/dispatch.rs; apps/conary/src/dispatch/; apps/conary/src/app.rs; apps/conary/src/lib.rs; apps/conary/src/cli/mod.rs; apps/conary/src/command_risk.rs; apps/conary/src/live_host_safety.rs; docs/ARCHITECTURE.md; docs/llms/subsystem-map.md; docs/modules/feature-ownership.md; docs/modules/query.md	verified	corrected	Added the Phase 21 dispatch decomposition plan for turning apps/conary/src/dispatch.rs into a public CLI dispatch hub plus focused child router modules while preserving command routes, live-mutation safety gates, command labels, and the public dispatch entrypoint.
```

- [ ] **Step 3: Refresh audit inventory**

Stage the plan path before regenerating inventory because `scripts/docs-audit-inventory.sh` reads from `git ls-files`:

```bash
git add -N docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase21-dispatch-decomposition-plan.md
```

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 4: Update the audit summary**

Add a concise paragraph near the active maintainability planning section of `docs/superpowers/documentation-accuracy-audit-summary.md` noting that Phase 21 opens the dispatch decomposition lane after Phase 20 and updates the docs-audit baseline to 165 tracked files / 65 corrected rows.

- [ ] **Step 5: Verify docs-audit lock-in**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected:

```text
165
archived 73
corrected 65
retained-historical 14
verified-no-change 13
Documentation audit ledger check passed (--require-complete).
```

- [ ] **Step 6: Commit plan lock-in**

Run:

```bash
git add docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase21-dispatch-decomposition-plan.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: plan dispatch decomposition"
```

## Task 1: Extract Shared Dispatch Context Helpers

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Create: `apps/conary/src/dispatch/context.rs`

- [ ] **Step 1: Create the child module directory**

Run:

```bash
mkdir -p apps/conary/src/dispatch
```

- [ ] **Step 2: Add the context module declaration**

Add this near the top of `apps/conary/src/dispatch.rs`, below the module docs:

```rust
mod context;
```

- [ ] **Step 3: Move shared helpers into `context.rs`**

Create `apps/conary/src/dispatch/context.rs` with the path comment, imports from the import surface above, and the exact existing helper bodies:

```rust
// apps/conary/src/dispatch/context.rs

use std::borrow::Cow;

use anyhow::Result;

use crate::commands;
use crate::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, MutationIntent, require_mutation_intent,
};

pub(super) fn require_live_mutation(
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

pub(super) fn legacy_replay_options(
    allow_legacy_replay: bool,
    allow_foreign_legacy_replay: bool,
) -> commands::LegacyReplayOptions {
    commands::LegacyReplayOptions {
        allow_legacy_replay,
        allow_foreign_legacy_replay,
    }
}
```

- [ ] **Step 4: Import the moved helpers in `dispatch.rs`**

Delete the old helper definitions from `dispatch.rs` and add:

```rust
use self::context::{legacy_replay_options, require_live_mutation};
```

Keep existing call sites unchanged.

- [ ] **Step 5: Trim obsolete imports**

Remove these parent imports only if they are no longer used by `dispatch.rs` after Step 4:

```rust
use crate::live_host_safety::{LiveMutationRequest, require_mutation_intent};
```

Keep `Cow`, `LiveMutationClass`, and `MutationIntent` in the parent while root command bodies still live there.

- [ ] **Step 6: Verify Task 1**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib dispatch -- --list
```

Expected:

```text
0 tests, 0 benchmarks
```

- [ ] **Step 7: Commit Task 1**

Run:

```bash
git add apps/conary/src/dispatch.rs apps/conary/src/dispatch/context.rs
git commit -m "refactor(conary): extract dispatch context helpers"
```

## Task 2: Move Top-Level Routing Into `root.rs`

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Create: `apps/conary/src/dispatch/root.rs`

- [ ] **Step 1: Add `root` module declaration**

Add to `dispatch.rs`:

```rust
mod root;
```

- [ ] **Step 2: Create `root.rs` with the top-level match**

Create `apps/conary/src/dispatch/root.rs` with this function signature:

```rust
pub(super) async fn dispatch_command(
    command: Option<Commands>,
    allow_live_system_mutation: bool,
) -> Result<()>
```

Move the entire current top-level `match cli.command` body into that function. The first arm remains the current `Install` arm and the final arm remains the existing `None` arm. The moved match arms must preserve all current command implementation calls and early returns. In the moved body, change the opening line from:

```rust
match cli.command {
```

to:

```rust
match command {
```

In the `Export` arm, change `std::path::Path::new(...)` to `Path::new(...)` because `root.rs` owns `use std::path::Path;`.

- [ ] **Step 3: Keep temporary parent-helper imports**

During this task only, `root.rs` may call the still-parented private helper dispatch functions through explicit `super::dispatch_*` imports. Add this temporary import list:

```rust
use super::{
    dispatch_automation_command, dispatch_bootstrap_command, dispatch_cache_command,
    dispatch_capability_command, dispatch_canonical_command, dispatch_ccs_command,
    dispatch_collection_command, dispatch_config_command, dispatch_derivation_command,
    dispatch_derive_command, dispatch_distro_command, dispatch_federation_command,
    dispatch_groups_command, dispatch_model_command, dispatch_profile_command,
    dispatch_provenance_command, dispatch_query_command, dispatch_registry_command,
    dispatch_repo_command, dispatch_system_command, dispatch_trust_command,
    dispatch_verify_derivation_command,
};
```

This compiles because child modules can access private items defined in ancestor modules.

- [ ] **Step 4: Update public `dispatch(cli)` wrapper**

Replace the old large `dispatch(cli)` body in `dispatch.rs` with:

```rust
pub async fn dispatch(cli: Cli) -> Result<()> {
    let allow_live_system_mutation = cli.allow_live_system_mutation;
    command_risk::enforce_cli_policy(allow_live_system_mutation, &cli)?;
    root::dispatch_command(cli.command, allow_live_system_mutation).await
}
```

- [ ] **Step 5: Move root-only imports**

Move these imports from `dispatch.rs` to `root.rs` if no longer used in the parent:

```rust
use std::borrow::Cow;
use std::path::Path;
use crate::cli::Commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
```

Keep `crate::cli::Cli` and `crate::command_risk` in `dispatch.rs`.

Do not apply the final parent import set yet: while helper routers still live in `dispatch.rs`, the parent continues to need `crate::cli::{self, Cli}` and `crate::commands`. Task 6 performs the final parent import cleanup after all helper bodies have moved.

- [ ] **Step 6: Verify Task 2**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
```

- [ ] **Step 7: Commit Task 2**

Run:

```bash
git add apps/conary/src/dispatch.rs apps/conary/src/dispatch/root.rs
git commit -m "refactor(conary): extract top-level dispatch routing"
```

## Task 3: Extract Non-Live Namespace Routers

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Create:
  - `apps/conary/src/dispatch/cache.rs`
  - `apps/conary/src/dispatch/repo.rs`
  - `apps/conary/src/dispatch/config.rs`
  - `apps/conary/src/dispatch/query.rs`
  - `apps/conary/src/dispatch/collection.rs`
  - `apps/conary/src/dispatch/derive.rs`
  - `apps/conary/src/dispatch/bootstrap.rs`
  - `apps/conary/src/dispatch/provenance.rs`
  - `apps/conary/src/dispatch/capability.rs`
  - `apps/conary/src/dispatch/trust.rs`
  - `apps/conary/src/dispatch/federation.rs`
  - `apps/conary/src/dispatch/catalog.rs`
  - `apps/conary/src/dispatch/derivation.rs`
  - `apps/conary/src/dispatch/profile.rs`
  - `apps/conary/src/dispatch/verify_derivation.rs`

- [ ] **Step 1: Add module declarations**

Add these module declarations to `dispatch.rs`:

```rust
mod bootstrap;
mod cache;
mod capability;
mod catalog;
mod collection;
mod config;
mod derivation;
mod derive;
mod federation;
mod profile;
mod provenance;
mod query;
mod repo;
mod trust;
mod verify_derivation;
```

- [ ] **Step 2: Move one existing helper function per child module**

Move each existing helper body exactly as follows and change the function visibility to `pub(super)`:

| Current helper | New file |
| --- | --- |
| `dispatch_cache_command` | `dispatch/cache.rs` |
| `dispatch_repo_command` | `dispatch/repo.rs` |
| `dispatch_config_command` | `dispatch/config.rs` |
| `dispatch_query_command` | `dispatch/query.rs` |
| `dispatch_collection_command` | `dispatch/collection.rs` |
| `dispatch_derive_command` | `dispatch/derive.rs` |
| `dispatch_bootstrap_command` | `dispatch/bootstrap.rs` |
| `dispatch_provenance_command` | `dispatch/provenance.rs` |
| `dispatch_capability_command` | `dispatch/capability.rs` |
| `dispatch_trust_command` | `dispatch/trust.rs` |
| `dispatch_federation_command` | `dispatch/federation.rs` |
| `dispatch_derivation_command` | `dispatch/derivation.rs` |
| `dispatch_profile_command` | `dispatch/profile.rs` |
| `dispatch_verify_derivation_command` | `dispatch/verify_derivation.rs` |

- [ ] **Step 3: Move catalog helpers into one file**

Move these four helpers into `dispatch/catalog.rs` and change each to `pub(super)`:

```rust
dispatch_distro_command
dispatch_canonical_command
dispatch_groups_command
dispatch_registry_command
```

Add a short module doc or comment near the top of `catalog.rs` explaining that it intentionally groups the small distro, canonical, groups, and registry routers.

- [ ] **Step 4: Replace root temporary imports with child imports**

In `root.rs`, replace the temporary `super::{dispatch_*}` import list for moved helpers with explicit module imports:

```rust
use super::bootstrap::dispatch_bootstrap_command;
use super::cache::dispatch_cache_command;
use super::capability::dispatch_capability_command;
use super::catalog::{
    dispatch_canonical_command, dispatch_distro_command, dispatch_groups_command,
    dispatch_registry_command,
};
use super::collection::dispatch_collection_command;
use super::config::dispatch_config_command;
use super::derivation::dispatch_derivation_command;
use super::derive::dispatch_derive_command;
use super::federation::dispatch_federation_command;
use super::profile::dispatch_profile_command;
use super::provenance::dispatch_provenance_command;
use super::query::dispatch_query_command;
use super::repo::dispatch_repo_command;
use super::trust::dispatch_trust_command;
use super::verify_derivation::dispatch_verify_derivation_command;
```

Leave `dispatch_system_command`, `dispatch_ccs_command`, `dispatch_model_command`, and `dispatch_automation_command` as temporary parent imports until Tasks 4 and 5 move them.

- [ ] **Step 5: Verify Task 3**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --test query
cargo test -p conary --test query_scripts
cargo test -p conary --lib commands::model
```

- [ ] **Step 6: Commit Task 3**

Run:

```bash
git add apps/conary/src/dispatch.rs apps/conary/src/dispatch/root.rs apps/conary/src/dispatch
git commit -m "refactor(conary): extract dispatch namespace routers"
```

## Task 4: Extract System Router And Nested System Routers

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Create:
  - `apps/conary/src/dispatch/system.rs`
  - `apps/conary/src/dispatch/system_state.rs`
  - `apps/conary/src/dispatch/system_generation.rs`
  - `apps/conary/src/dispatch/system_trigger.rs`
  - `apps/conary/src/dispatch/system_redirect.rs`
  - `apps/conary/src/dispatch/system_update_channel.rs`

- [ ] **Step 1: Add module declarations**

Add to `dispatch.rs`:

```rust
mod system;
mod system_generation;
mod system_redirect;
mod system_state;
mod system_trigger;
mod system_update_channel;
```

- [ ] **Step 2: Move nested matches first**

Extract these nested match bodies into sibling helpers:

```rust
// apps/conary/src/dispatch/system_state.rs
pub(super) async fn dispatch_system_state_command(
    state_cmd: cli::StateCommands,
    allow_live_system_mutation: bool,
) -> Result<()>

// apps/conary/src/dispatch/system_generation.rs
pub(super) async fn dispatch_system_generation_command(
    gen_cmd: cli::GenerationCommands,
    allow_live_system_mutation: bool,
) -> Result<()>

// apps/conary/src/dispatch/system_trigger.rs
pub(super) async fn dispatch_system_trigger_command(
    trigger_cmd: cli::TriggerCommands,
) -> Result<()>

// apps/conary/src/dispatch/system_redirect.rs
pub(super) async fn dispatch_system_redirect_command(
    redirect_cmd: cli::RedirectCommands,
) -> Result<()>

// apps/conary/src/dispatch/system_update_channel.rs
pub(super) async fn dispatch_system_update_channel_action(
    action: cli::UpdateChannelAction,
) -> Result<()>
```

Each helper should contain the exact corresponding nested `match` body from the current `dispatch_system_command`.

`system_state.rs` and `system_generation.rs` own live-mutation gates and need:

```rust
use std::borrow::Cow;

use anyhow::Result;

use super::context::require_live_mutation;
use crate::cli;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
```

`system_trigger.rs`, `system_redirect.rs`, and `system_update_channel.rs` do not own live-mutation gates and need only:

```rust
use anyhow::Result;

use crate::cli;
use crate::commands;
```

- [ ] **Step 3: Move `dispatch_system_command`**

Move `dispatch_system_command` into `system.rs`, change it to `pub(super)`, and replace nested inline matches with calls to the new sibling helpers:

```rust
cli::SystemCommands::State(state_cmd) => {
    dispatch_system_state_command(state_cmd, allow_live_system_mutation).await
}
cli::SystemCommands::Generation(gen_cmd) => {
    dispatch_system_generation_command(gen_cmd, allow_live_system_mutation).await
}
cli::SystemCommands::Trigger(trigger_cmd) => dispatch_system_trigger_command(trigger_cmd).await,
cli::SystemCommands::Redirect(redirect_cmd) => {
    dispatch_system_redirect_command(redirect_cmd).await
}
cli::SystemCommands::UpdateChannel { action } => {
    dispatch_system_update_channel_action(action).await
}
```

- [ ] **Step 4: Preserve completion generation imports**

`system.rs` must own:

```rust
use clap::CommandFactory;
use clap_complete::generate;
use std::io;
use crate::cli::Cli;
```

because `SystemCommands::Completions` calls `Cli::command()` and writes to `io::stdout()`.

- [ ] **Step 5: Update root imports**

Replace the temporary parent import with:

```rust
use super::system::dispatch_system_command;
```

- [ ] **Step 6: Verify Task 4**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --test live_host_mutation_safety
cargo run -p conary -- system completions bash >/dev/null
```

- [ ] **Step 7: Commit Task 4**

Run:

```bash
git add apps/conary/src/dispatch.rs apps/conary/src/dispatch/root.rs apps/conary/src/dispatch/system*.rs
git commit -m "refactor(conary): extract system dispatch routers"
```

## Task 5: Extract Live-Mutation Namespace Routers

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`
- Create:
  - `apps/conary/src/dispatch/ccs.rs`
  - `apps/conary/src/dispatch/model.rs`
  - `apps/conary/src/dispatch/automation.rs`

- [ ] **Step 1: Add module declarations**

Add to `dispatch.rs`:

```rust
mod automation;
mod ccs;
mod model;
```

- [ ] **Step 2: Move the existing helper functions**

Move these existing helper bodies into their target files and change visibility to `pub(super)`:

| Current helper | New file |
| --- | --- |
| `dispatch_ccs_command` | `dispatch/ccs.rs` |
| `dispatch_model_command` | `dispatch/model.rs` |
| `dispatch_automation_command` | `dispatch/automation.rs` |

- [ ] **Step 3: Preserve live-mutation gates**

Do not change these gate calls:

```text
conary ccs install
conary model apply
conary automation apply
```

`ccs.rs` should import:

```rust
use std::borrow::Cow;

use super::context::{legacy_replay_options, require_live_mutation};
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
```

`model.rs` and `automation.rs` only need `require_live_mutation` from `super::context`.

Add or preserve a route-level `live_host_mutation_safety` regression for `conary ccs install` refusal, dry-run, and `--yes` behavior so the moved CCS install gate stays covered.

- [ ] **Step 4: Update root imports**

Replace the final temporary parent imports with:

```rust
use super::automation::dispatch_automation_command;
use super::ccs::dispatch_ccs_command;
use super::model::dispatch_model_command;
```

- [ ] **Step 5: Verify Task 5**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test live_host_mutation_safety
cargo test -p conary --test query_scripts
```

- [ ] **Step 6: Commit Task 5**

Run:

```bash
git add apps/conary/src/dispatch.rs apps/conary/src/dispatch/root.rs apps/conary/src/dispatch/automation.rs apps/conary/src/dispatch/ccs.rs apps/conary/src/dispatch/model.rs apps/conary/tests/live_host_mutation_safety.rs
git commit -m "refactor(conary): extract live dispatch routers"
```

## Task 6: Clean Up Dispatch Hub And Validate Boundaries

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: child router modules as needed for unused imports only

- [ ] **Step 1: Remove obsolete parent imports**

After all helper bodies move out of `dispatch.rs`, the parent should no longer import:

```rust
use clap::CommandFactory;
use clap_complete::generate;
use std::borrow::Cow;
use std::io;
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
```

The parent should keep only:

```rust
use anyhow::Result;

use crate::cli::Cli;
use crate::command_risk;
```

- [ ] **Step 2: Verify no helper functions remain in the parent**

Run:

```bash
rg -n "fn (dispatch_|require_live_mutation|legacy_replay_options)" apps/conary/src/dispatch.rs
```

Expected: no output.

- [ ] **Step 3: Verify parent has only the public dispatch function**

Run:

```bash
rg -n "^(pub )?(async )?fn " apps/conary/src/dispatch.rs
```

Expected:

```text
<line>:pub async fn dispatch(cli: Cli) -> Result<()> {
```

- [ ] **Step 4: Verify no production wildcard imports in child routers**

Run:

```bash
rg -n "use super::\\*|use crate::.*::\\*" apps/conary/src/dispatch.rs apps/conary/src/dispatch
```

Expected: no output.

- [ ] **Step 5: Verify Task 6**

Run:

```bash
cargo fmt
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --lib live_host_safety::tests
cargo clippy -p conary --all-targets -- -D warnings
```

- [ ] **Step 6: Commit Task 6**

Run:

```bash
git add apps/conary/src/dispatch.rs apps/conary/src/dispatch
git commit -m "refactor(conary): collapse dispatch hub"
```

## Task 7: Update Dispatch Ownership Docs

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/query.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

- [ ] **Step 0: Determine the current `query.md` ledger disposition**

Run:

```bash
grep -P '^docs/modules/query\.md\t' docs/superpowers/documentation-accuracy-audit-ledger.tsv | cut -f8
```

If the output is `verified-no-change`, the final corrected count will be `66` after this task updates `docs/modules/query.md`; if it is already `corrected`, the corrected count remains `65`.

- [ ] **Step 1: Update architecture module map**

In `docs/ARCHITECTURE.md`, change the `dispatch.rs` map entry so it names the hub plus child routers:

```text
+-- dispatch.rs      Public command dispatch wrapper and live-host safety policy entrypoint
+-- dispatch/        Focused CLI command routers by namespace
```

- [ ] **Step 2: Update assistant subsystem map**

In `docs/llms/subsystem-map.md`, update the `apps/conary/` orientation bullet or the relevant "look here first" sections to mention:

```text
apps/conary/src/dispatch.rs and apps/conary/src/dispatch/
```

for command dispatch ownership.

- [ ] **Step 3: Update feature ownership routing**

In `docs/modules/feature-ownership.md`, add a `CLI Dispatch And Command Routing` ownership card if no focused CLI dispatch card exists yet. The entry should route future CLI argument-to-command wiring changes through:

```text
apps/conary/src/cli/
apps/conary/src/dispatch.rs
apps/conary/src/dispatch/
apps/conary/src/command_risk.rs
apps/conary/src/live_host_safety.rs
```

- [ ] **Step 4: Update query doc path references**

In `docs/modules/query.md`, replace dispatch-specific path references:

```text
dispatch.rs
```

with:

```text
dispatch/query.rs
dispatch/root.rs
```

where the text refers to query subcommands or root-level `sbom` routing respectively.

- [ ] **Step 5: Update docs-audit ledger rows for changed docs**

Update the existing ledger rows for:

```text
docs/ARCHITECTURE.md
docs/llms/subsystem-map.md
docs/modules/feature-ownership.md
docs/modules/query.md
docs/superpowers/documentation-accuracy-audit-summary.md
docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase21-dispatch-decomposition-plan.md
```

Keep the Phase 21 plan row as `corrected`. If `docs/modules/query.md` changes from `verified-no-change`, change its disposition to `corrected` and update the final ledger counts accordingly.

- [ ] **Step 6: Refresh docs-audit inventory and summary counts**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected inventory count remains `165` after Task 0. Corrected row count is at least `65`; it will be `66` if `docs/modules/query.md` moves from `verified-no-change` to `corrected`.

- [ ] **Step 7: Commit Task 7**

Run:

```bash
git add docs/ARCHITECTURE.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/query.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: update dispatch ownership routing"
```

## Task 8: Final Verification

**Files:** workspace-wide verification only.

- [ ] **Step 1: Formatting and compilation**

Run:

```bash
cargo fmt --check
cargo check -p conary
cargo check --workspace --all-targets
```

- [ ] **Step 2: Focused route and safety tests**

Run:

```bash
cargo test -p conary --lib dispatch -- --list
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --lib live_host_safety::tests
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test live_host_mutation_safety
cargo test -p conary --test query
cargo test -p conary --test query_scripts
cargo run -p conary -- system completions bash >/dev/null
```

Expected first command remains:

```text
0 tests, 0 benchmarks
```

- [ ] **Step 3: Owning package tests**

Run:

```bash
cargo test -p conary
cargo test --workspace --lib
```

- [ ] **Step 4: Clippy**

Run:

```bash
cargo clippy -p conary --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 5: Docs-audit and drift gates**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/maintainability-drift-report.sh
scripts/line-count-report.sh 30
git diff --check
```

- [ ] **Step 6: Boundary checks**

Run:

```bash
rg -n "fn (dispatch_|require_live_mutation|legacy_replay_options)" apps/conary/src/dispatch.rs
rg -n "use super::\\*|use crate::.*::\\*" apps/conary/src/dispatch.rs apps/conary/src/dispatch
wc -l apps/conary/src/dispatch.rs apps/conary/src/dispatch/*.rs
```

Expected:

- first `rg`: no output,
- second `rg`: no output,
- `dispatch.rs` substantially below 100 lines,
- no child router near the original 2,177-line hotspot.

- [ ] **Step 7: Final commit if any verification-only cleanup was needed**

If final verification required cleanup, commit it with a scoped subject. If not, skip this step.

- [ ] **Step 8: Push and prove sync**

Run:

```bash
git status --short --branch
git push
git status --short --branch
git rev-parse HEAD origin/main
git worktree list --porcelain
```

Expected after push:

- `git status --short --branch` shows `## main...origin/main` with no changed files,
- `git rev-parse HEAD origin/main` prints the same SHA twice,
- `git worktree list --porcelain` shows only `/home/peter/Conary` unless the user intentionally has another worktree.

## Non-Goals

- Do not change `apps/conary/src/cli/` argument definitions.
- Do not change command implementation signatures under `apps/conary/src/commands/`.
- Do not change `command_risk` classification.
- Do not change live-host safety wording, labels, classes, or dry-run handling.
- Do not add daemon, Remi, or conary-core behavior changes.
- Do not archive existing active plans in this phase.

## Agentic Review Checklist

Before lock-in, reviewers should verify:

1. Rust file-module layout is valid: `dispatch.rs` plus `dispatch/*.rs`, with no `dispatch/mod.rs`.
2. `crate::dispatch::dispatch(cli)` remains the only public entrypoint and `app.rs` needs no change.
3. `command_risk::enforce_cli_policy` still runs before routing.
4. All live-mutation labels and classes remain exactly mapped to their current commands.
5. `SystemCommands::Completions` keeps `Cli::command()` and `clap_complete::generate` imports in `system.rs`.
6. `repo.rs` maps `CliSecurityAdvisorySupport` to `SecurityAdvisorySupport` without import or namespace mistakes.
7. `bootstrap.rs` preserves the `--from is required when not using --from-adopted` error.
8. No child module uses `use super::*`.
9. Docs-audit math is correct for plan lock-in and for any doc row disposition changes in Task 7.
