---
last_updated: 2026-03-31
revision: 1
summary: Add a fail-closed CLI acknowledgment for commands that can mutate the active host
---

# Live Host Mutation Safety Gate

## Summary

This design adds a single CLI acknowledgment flag,
`--allow-live-system-mutation`, for commands that can mutate the active host.
The enforcement lives at the top-level command layer, not inside low-level
helpers, so the warning is attached to the command the operator actually typed.

The default behavior becomes fail-closed: if a command can mutate the real
machine, Conary refuses to run it unless the operator explicitly opts in. Dry
runs remain allowed without the flag.

This design is intentionally honest about current implementation limits. Several
commands accept `--root` today, but still route into live composefs and
generation code paths rooted at `/conary`, `/etc`, or `/usr`. Those commands
must still be treated as live-host mutations until the underlying plumbing is
made truly alternate-root-safe.

## Problem Statement

Conary is approaching a public-facing stage, but it is still a package manager
with commands that can rebuild generations, remount `/usr`, rewrite the live
`/etc` overlay, execute package scriptlets, and change package ownership state.

The current CLI does not force an operator to explicitly acknowledge those
risks. In particular:

- package mutation commands can flow into generation rebuild and remount paths
- generation commands can rewire the live system view
- takeover can change package ownership across the whole machine
- some commands appear to support alternate roots, but still touch live paths

For the current project stage, the safer default is to refuse live-host
mutation unless the operator explicitly says they intend to modify the real
machine.

## Goals

- Fail closed on commands that can mutate the active host.
- Use one consistent acknowledgment flag across the CLI.
- Keep enforcement at the CLI or top-level command dispatch layer.
- Make the warning appear on the command the user actually invoked.
- Allow `--dry-run` flows without requiring the mutation flag.
- Be truthful about current alternate-root limitations.

## Non-Goals

- Enforcing the acknowledgment inside low-level composefs, transaction, or
  generation helpers.
- Making existing mutation commands truly alternate-root-safe.
- Gating metadata-only commands that do not mutate the live filesystem view.
- Introducing interactive confirmation prompts as the primary safety mechanism.
- Redesigning generation storage or composefs root plumbing in this slice.

## Current State

The repository already has several top-level commands that can mutate the live
system:

- `src/commands/install/mod.rs` stores package contents in CAS, commits DB
  state, then rebuilds and mounts a new generation through
  `src/commands/composefs_ops.rs`.
- `src/commands/remove.rs` removes package state from the DB and then rebuilds
  and remounts the active generation.
- `src/commands/system.rs` rollback flows also end in
  `rebuild_and_mount(..., Path::new("/conary"))`.
- `src/commands/restore.rs` rebuilds and remounts from live generation state.
- `src/commands/generation/switch.rs` can remount `/usr`, rebuild the live
  `/etc` overlay, and update `/conary/current`.
- `src/commands/generation/commands.rs` exposes live switch, rollback, and
  recovery entry points.
- `src/commands/generation/takeover.rs` can CAS-back packages, remove native PM
  ownership, build a generation, and prepare the system for activation.

The most important implementation truth is that several commands that accept a
`root` argument still end in helpers that hardcode live paths such as:

- `/conary`
- `/conary/mnt`
- `/conary/current`
- `/etc`
- `/usr`

That means `--root != "/"` is not currently a reliable guarantee that the
active machine will remain untouched for those command families.

## Approach Options

### Option 1: Central CLI Safety Gate With Shared Warning

Add one shared helper at the CLI dispatch layer. Dangerous commands call it
before doing any mutation. The helper requires
`--allow-live-system-mutation` when the command targets the active host.

Pros:

- consistent UX and wording
- low-risk change
- matches the desired ownership model: the command the user typed owns the
  warning

Cons:

- future commands still need to opt into the helper explicitly

### Option 2: Command-Local Checks Only

Implement bespoke guards separately in each mutating command.

Pros:

- explicit at each callsite

Cons:

- wording and behavior drift
- easier to miss wrapper commands
- higher maintenance burden

### Option 3: Global Safe-Mode Policy Layer

Introduce a broader runtime policy system that controls all mutation behavior
from config, environment, or a dedicated policy object.

Pros:

- strong long-term control surface

Cons:

- more invasive than needed right now
- delays the immediate fail-closed safety win

## Chosen Direction

Choose Option 1.

Add a single shared CLI acknowledgment flag and enforce it from top-level
dispatch for commands that can mutate the active host.

## Design

### Safety Boundary

A command counts as a live-host mutation if it can modify the active machine's
installed package state or live filesystem view outside a dry-run path.

For this slice, commands fall into two classes.

#### Class 1: Always Live

These commands are inherently tied to the active system and do not pretend to
support alternate roots:

- `system generation switch`
- `system generation rollback`
- `system generation recover`
- `system takeover`

#### Class 2: Currently Live Even With Root Arguments

These commands expose root-like or package-level mutation interfaces, but their
current implementations still flow into live composefs or generation paths and
must be treated as live-host mutations for now:

- `install`
- `remove`
- `update`
- `autoremove`
- `ccs install`
- `collection install`
- `system restore`
- `system state rollback`
- `rollback`

This second class is important because it keeps the guard honest. The CLI must
reflect what the code actually does today, not what the argument names imply.

### Commands Covered In This Slice

The acknowledgment gate should be enforced for the following user-facing
commands:

- `conary install`
- `conary remove`
- `conary update`
- `conary autoremove`
- `conary ccs install`
- `conary system restore`
- `conary system state rollback`
- `conary system generation switch`
- `conary system generation rollback`
- `conary system generation recover`
- `conary system takeover`

Wrapper commands that eventually trigger the same mutation paths must also
enforce the guard at the wrapper entrypoint so the warning belongs to the
command the operator typed:

- `conary install @collection` via `cmd_collection_install`
- group or collection update flows routed through `cmd_update_group`

### Commands Not Covered In This Slice

The following command families are out of scope for this first gate because
they do not directly remount the live system, rebuild generations, or execute
package mutation flows:

- `pin` / `unpin`
- repository management
- update-channel management
- state listing and diffing
- search, list, query, and verification commands

This is a pragmatic boundary, not a claim that metadata-only commands are
unimportant.

### Flag Shape

Add a global clap flag on `Cli`:

- `--allow-live-system-mutation`

Using a single global flag keeps the UX simple for both root-level and nested
commands, including `system generation ...`.

The flag is global in parsing terms, but only enforced by covered mutating
commands.

### Enforcement Model

Add one shared helper in the CLI layer or top-level dispatch, called before any
dangerous action.

Representative shape:

```rust
fn require_live_system_mutation_ack(
    allow_live_system_mutation: bool,
    command_name: &str,
    dry_run: bool,
    class: LiveMutationClass,
) -> Result<()>
```

Where `LiveMutationClass` begins with:

- `AlwaysLive`
- `CurrentlyLiveEvenWithRoot`

For this slice, both classes require the flag whenever `dry_run == false`.

This enum leaves a clean path for a future `RootAware` class once some commands
become truly alternate-root-safe.

### Warning Text

The refusal message should be detailed and explicit. It should explain that the
command can mutate the active host and may:

- rebuild or activate a generation
- remount `/usr`
- rewire the live `/etc` overlay
- execute package install or remove scriptlets
- change native package-manager ownership state

For commands in the "currently live even with root arguments" class, the
message should explicitly say that `--root` is not sufficient isolation for
this command yet because the implementation still routes into live generation
paths.

The message should end with a direct action:

- rerun with `--allow-live-system-mutation` only if you intend to modify the
  real machine

### Dispatch Points

The enforcement should happen in `src/main.rs`, before dispatch enters the
mutating command body.

Representative dispatch points:

- `Commands::Install`
- `Commands::Remove`
- `Commands::Update`
- `Commands::Autoremove`
- `SystemCommands::Restore`
- `StateCommands::Rollback`
- `GenerationCommands::Switch`
- `GenerationCommands::Rollback`
- `GenerationCommands::Recover`
- `SystemCommands::Takeover`
- `CcsCommands::Install`

This preserves the principle that the top-level command owns the warning.

### Dry-Run Behavior

Dry runs must bypass the acknowledgment requirement.

This keeps the safety gate compatible with exploration, demos, and operator
education. A user should be able to inspect a plan without acknowledging live
mutation.

### Relationship To Lower-Level Helpers

Low-level helpers such as:

- transaction engine helpers
- composefs rebuild helpers
- generation switch helpers
- CCS install internals

should remain focused on execution mechanics in this slice. They should not own
the operator acknowledgment.

This keeps the warning attached to the user-facing command surface and avoids
mixing policy into low-level filesystem code.

## Testing Plan

Add focused safety coverage at two levels.

### Unit Tests

Add helper-level tests for:

- dry-run bypass
- refusal when the flag is absent
- success when the flag is present
- "currently live even with root arguments" classification

### Integration Tests

Add CLI-facing tests for representative commands:

- `install` refuses without `--allow-live-system-mutation`
- `remove` refuses without `--allow-live-system-mutation`
- `update` refuses without `--allow-live-system-mutation`
- `system generation switch` refuses without the flag
- `system takeover --dry-run` succeeds without the flag
- a read-only command remains unaffected

### Message Assertions

Assert that refusal output:

- names the required flag
- explains that the command can mutate the active host
- mentions concrete risks such as generation rebuild, `/usr` remount, `/etc`
  overlay changes, scriptlet execution, or takeover ownership changes

## Follow-Up Work

This design intentionally documents a future cleanup target.

Once composefs and generation paths are parameterized away from hardcoded live
locations, some commands may be reclassified from
`CurrentlyLiveEvenWithRoot` to a truly alternate-root-safe class. At that
point, the helper can allow those commands without the live-mutation flag when
they target a non-live root.

That reclassification should not happen until the underlying code paths stop
touching live `/conary`, `/etc`, and `/usr`.
