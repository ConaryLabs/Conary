---
last_updated: 2026-04-01
revision: 2
summary: Refresh the live-host mutation safety gate around the current apps/conary dispatch seams and verified command readiness
---

# Live Host Mutation Safety Gate Refresh

## Summary

This design refreshes the live-host mutation safety gate for the current
workspace layout and current CLI behavior on `main`.

The gate remains a fail-closed CLI acknowledgment:
`--allow-live-system-mutation`.
It applies to the Conary command families that can mutate the active host's
package, generation, activation, or Conary-managed live filesystem state
outside a dry-run path.

The important change from the March draft is not the product decision. The
important change is the grounding:

- the CLI now lives in `apps/conary/`
- top-level routing now happens in `apps/conary/src/dispatch.rs`
- the covered command set should be recomputed from current behavior, not
  copied forward from pre-refactor paths
- command readiness proof is part of the feature definition, not a later
  polish step

This design intentionally keeps the gate focused. It does not try to classify
every command that can write a file on the host, and it does not try to solve
alternate-root correctness in the same slice.

## Problem Statement

Conary still has command families that can rebuild generations, remount
`/usr`, rewrite the live `/etc` overlay, execute install/remove scriptlets, or
change package ownership state on the real machine.

The project is also still early enough that those live-host mutation paths have
not yet earned a "safe by default on the current host" posture.

The March 31 design captured the right intent, but it no longer matches the
current tree closely enough to execute directly:

- its file map targets the pre-workspace-refactor `src/` layout
- the real top-level seam is now `apps/conary/src/dispatch.rs`, not `src/main.rs`
- some command families need to be reclassified against current behavior
- the old branch added helper and readiness work, but never actually landed the
  end-to-end refusal integration layer the plan called for

So the current need is twofold:

1. restore an honest, current design for the safety gate on today's tree
2. keep the original readiness bar so the gate cannot be used to hide uncertain
   or broken command paths

## Goals

- Fail closed on Conary-managed live-system mutation commands.
- Keep one explicit acknowledgment flag across the CLI.
- Enforce the policy at the CLI boundary on the command the operator actually
  typed.
- Keep the enforcement logic centralized behind one helper-owned seam.
- Allow dry-run inspection flows without requiring the acknowledgment flag.
- Treat command-readiness proof as a required deliverable of the feature.
- Surface broken or insufficiently wired commands as blockers instead of
  papering over them with the gate.

## Non-Goals

- Making current `--root` or root-like arguments truly isolated.
- Refactoring composefs, generation, activation, or state-root plumbing in this
  slice.
- Expanding this design into a general safety policy for every host-writing CLI
  command.
- Gating adjacent mixed-mode command families such as `system adopt` or
  `config restore` in this same pass.
- Using interactive confirmation prompts as the primary protection mechanism.
- Treating the safety gate as a substitute for fixing broken command families.

## Current State

### CLI Structure

The current CLI shape is:

- `apps/conary/src/main.rs` -> thin entrypoint
- `apps/conary/src/app.rs` -> parse/bootstrap/error presentation
- `apps/conary/src/dispatch.rs` -> real top-level routing
- `apps/conary/src/cli/*` -> clap definitions

That means the correct enforcement seam is the app-level dispatch layer.

### Current Mutation Reality

Several command families still route into live composefs, generation, rollback,
or takeover flows even when they expose `root` or package-scoped arguments.

Today, current behavior still supports the key rationale for an explicit gate:

- install/update/remove flows can rebuild and remount generations
- system restore routes into live rebuild-and-mount paths outside dry-run mode
- generation switch/rollback/recover are inherently active-host operations
- takeover changes ownership and generation state on the real system
- some commands with root-like arguments still do not provide real isolation

### Important Reclassification

One command family from the March draft should be treated differently now:

- `system state revert` is currently planning-only and bails instead of applying
  real mutation, so it should not be in the gated set for this slice

By contrast, `system state rollback` still routes into a real rollback path and
remains in scope.

### Adjacent But Out-of-Scope Paths

Current `main` also has adjacent commands that deserve later review but should
not be folded into this gate refresh:

- `system adopt` and related adopt/refresh/convert flows
- `config restore`, which writes tracked config content back to disk

Those commands matter, but including them now would turn this work from a
focused live-system mutation gate into a broader host-write classification
project.

## Approach Options

### Option 1: Behavior-First Refresh On Current `main`

Recompute the gated boundary from today's dispatch tree and actual behavior,
then design the gate around the current `apps/conary` seams.

Pros:

- honest to the current tree
- keeps the original product intent
- supports the readiness-proof bar
- avoids stale file-map and command-map assumptions

Cons:

- requires a fresh command-boundary pass instead of a mechanical port

### Option 2: Literal Historical Port

Keep the March command set and structure nearly unchanged, only renaming files
for the workspace move.

Pros:

- fastest design update

Cons:

- carries forward stale assumptions
- makes command-boundary exceptions harder to explain cleanly
- risks spec drift from today's actual behavior

### Option 3: Broad Host-Write Safety Spec

Expand the project from "live Conary mutation" to "any command that can write
to the host," including adjacent paths such as config restore.

Pros:

- most comprehensive long-term safety story

Cons:

- much larger scope
- likely to block on unrelated command classification work
- dilutes the original focused goal

## Chosen Direction

Choose Option 1.

Refresh the gate from the current dispatch tree and current behavior, while
preserving the original requirement that covered command families must be shown
to be real, wired, and meaningfully tested before this feature is considered
done.

## Design

### Safety Boundary

This gate should cover `Conary-managed system mutation`.

For this design, that means commands that can mutate the active machine's
installed package state, generation state, activation state, or Conary-managed
live filesystem view outside a dry-run path.

This is intentionally narrower than "every command that can write a host file"
and intentionally broader than "only commands that call the generation switch
subcommand directly."

In particular, "generation state" includes creating and deleting generation
artifacts, not only switching the active generation.

### Covered Command Families

The covered set on current `main` is:

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
- `conary system generation build`
- `conary system generation gc`
- `conary system takeover`

Wrapper entrypoints must be covered at the command the operator actually typed,
not only at the lower helper:

- `conary install @collection`
- `conary update @collection`

The refusal should therefore name the user-facing entrypoint rather than an
internal helper such as `cmd_collection_install` or `cmd_update_group`.

### Explicit Exclusions

The design should explicitly exclude:

- `system state revert`, because the non-dry-run path is not implemented as a
  real mutating operation
- read-only/admin commands such as search, query, verify, repo, state list,
  state show, state diff, generation list, and generation info
- broader host-write paths such as `system adopt` and `config restore`
- adjacent config-management surfaces such as `config backup` and `config check`

These exclusions are design choices, not oversights. They keep this slice
focused on dangerous Conary-managed live mutation while leaving room for a
separate broader safety review later.

### Flag Shape

Add one global clap flag on `Cli`:

- `--allow-live-system-mutation`

The flag should be global in parsing terms so it works cleanly with nested
command shapes, including `system generation ...` and `system state ...`.

The flag is not universally enforced. It is only consulted by the covered
mutating command paths.

### Enforcement Model

Add a dedicated helper module in the app crate:

- `apps/conary/src/live_host_safety.rs`

Representative shape:

```rust
pub enum LiveMutationClass {
    AlwaysLive,
    CurrentlyLiveEvenWithRootArguments,
}

pub struct LiveMutationRequest {
    pub command_name: &'static str,
    pub class: LiveMutationClass,
    pub dry_run: bool,
}

pub fn require_live_system_mutation_ack(
    allow_live_system_mutation: bool,
    request: &LiveMutationRequest,
) -> anyhow::Result<()>;
```

`dispatch::dispatch(cli)` owns the callsites. Before each covered dispatch arm
enters the actual command implementation, it constructs a
`LiveMutationRequest` and asks the helper whether to proceed.

The helper should also own a tiny retirement seam such as
`live_mutation_ack_enforced() -> bool` so removing the feature later is a
one-file change rather than a repo-wide unwind.

### Mutation Classes

The March design's two classes still make sense and should be kept:

- `AlwaysLive`
- `CurrentlyLiveEvenWithRootArguments`

`AlwaysLive` applies to commands that are inherently tied to the active host,
such as generation switch/rollback/recover and takeover.

`CurrentlyLiveEvenWithRootArguments` applies to commands whose CLI surface can
look package-scoped or alternate-root-aware but whose current implementations
still flow into live generation, composefs, rollback, or similar active-host
paths.

For this slice, both classes require the acknowledgment whenever `dry_run` is
false.

### Dispatch Ownership

The gate should live in `apps/conary/src/dispatch.rs`, not in low-level command
helpers and not in `apps/conary/src/main.rs`.

That keeps:

- the warning attached to the user-facing command
- policy separate from execution mechanics
- low-level helpers focused on install, rollback, composefs, or takeover logic

This also matches the current app structure: `main.rs` is just an entrypoint,
while dispatch is the true top-level command router.

### Dry-Run Behavior

Dry-run paths must bypass the acknowledgment.

This is part of the product decision, not an implementation convenience. Users
should be able to inspect plans, demo behavior, and understand what Conary
would do without first opting into live host mutation.

Dry-run bypass must apply only where the command already has a real dry-run
mode. It must not be used to blur the distinction between planning-only
commands and genuinely mutating commands.

### Warning Text

The refusal message should be:

- command-specific
- direct about active-host mutation
- truthful about current implementation limits
- explicit about why the safeguard exists now

The message should explain that Conary is still early software and that the
command can mutate the active host through paths such as:

- generation rebuild or activation work
- `/usr` remounts
- live `/etc` overlay changes
- scriptlet execution
- takeover or rollback ownership changes

For `CurrentlyLiveEvenWithRootArguments`, the message should also explain that
`--root` or similar surface arguments are not sufficient isolation for this
command yet.

The message should end with a direct rerun instruction using
`--allow-live-system-mutation`.

### Relationship To Lower-Level Helpers

This design does not move the acknowledgment policy into:

- install internals
- CCS install internals
- composefs rebuild helpers
- rollback helpers
- generation helpers
- takeover helpers

Those helpers should remain execution-focused in this slice.

The top-level dispatch layer owns the operator-facing safety decision.

## Verification and Readiness

Readiness proof is part of the feature, not a trailing verification note.

This work is only complete when the gate wiring exists *and* each covered
command family has passing evidence that it is real, currently wired, and
meaningfully exercised today.

### Verification Layers

The design should require three layers of coverage:

1. parser and helper coverage
2. refusal-path integration coverage
3. command-readiness coverage

### Parser and Helper Coverage

Add focused tests for:

- parsing `--allow-live-system-mutation` through nested command shapes
- dry-run bypass
- refusal when the flag is absent
- success when the flag is present
- message shape for both mutation classes

### Refusal-Path Integration Coverage

Add a dedicated CLI-facing integration test file under `apps/conary/tests/`
that runs the built `conary` binary and verifies:

- representative covered commands refuse without the flag
- representative dry-run commands succeed without the flag
- at least one wrapper entrypoint is tested so the message matches the command
  the operator typed
- unaffected read-only commands remain unaffected

The March branch never actually landed this layer; the refreshed design should
make it explicit rather than assuming it exists.

### Command-Readiness Coverage

The implementation plan should reuse existing coverage first, then add narrow
smoke or disposable-host coverage where the evidence is thin.

Expected current readiness anchors include:

- `install` / `remove` / `update` / `autoremove`
  - `apps/conary/tests/workflow.rs`
  - `apps/conary/tests/batch_install.rs`
  - `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`
  - `apps/conary/tests/integration/remi/manifests/phase2-group-a.toml`
  - `apps/conary/tests/integration/remi/manifests/phase3-group-j.toml`
  - `apps/conary/tests/integration/remi/manifests/phase3-group-m.toml`
  - `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`
- `ccs install`
  - `apps/conary/tests/component.rs`
  - `apps/conary/tests/integration/remi/manifests/phase2-group-a.toml`
  - `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
  - `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`
- `system restore`
  - a local readiness smoke test for dry-run behavior
  - current `main` only clearly shows dry-run evidence in
    `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
  - the implementation must add meaningful non-dry-run disposable-host
    coverage before `system restore` is considered ready
  - if that evidence cannot be added honestly, the command stays blocked as a
    readiness issue instead of being waved through by the gate
- `system state rollback`
  - `apps/conary/tests/workflow.rs`
  - `apps/conary/tests/integration/remi/manifests/phase3-group-h.toml`
- `system generation build` / `gc` / `switch` / `rollback`
  - `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`
  - `apps/conary/tests/integration/remi/manifests/phase2-group-b.toml`
  - `apps/conary/tests/integration/remi/manifests/phase3-group-h.toml`
  - `apps/conary/tests/integration/remi/manifests/phase3-group-l.toml`
  - `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`
- `system generation recover`
  - new meaningful coverage if none exists today
  - if that cannot be provided honestly, it becomes a blocker
- `system takeover`
  - `apps/conary/src/commands/generation/takeover.rs`
  - `apps/conary/src/commands/generation/takeover_state.rs`
  - `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`
  - `apps/conary/tests/integration/remi/manifests/phase2-group-b.toml`
  - `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`

If a covered command family is obviously broken, insufficiently wired, or only
"covered" by a planning path, the correct result is to stop and surface it as a
blocker rather than letting the gate hide that uncertainty.

## Follow-On Work

This design should explicitly tee up a second, separate effort after the gate
work is complete and audited:

- deeper state-root and activation-root hardening
- broader classification of adjacent host-writing command families
- eventual reclassification of commands once alternate-root behavior becomes
  real rather than aspirational

That follow-on effort is the right place to revisit commands such as
`system adopt` and `config restore`, and to decide whether some commands can
eventually move out of the "currently live even with root arguments" category.

Until then, this safety-gate work should stay focused on a single question:
which current Conary command families can mutate the active host, and how do we
make that boundary explicit and honest at the CLI today?
