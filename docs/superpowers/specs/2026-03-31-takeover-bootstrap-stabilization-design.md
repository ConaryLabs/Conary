# Takeover And Bootstrap Stabilization Design

## Summary

This design turns two "alpha" surfaces into a concrete supported implementation
track:

1. `conary system takeover` becomes a stable, resumable pipeline that reaches
   full ownership, builds a generation, writes boot entries, and stops in a
   ready-to-activate state without automatically live-switching.
2. `conary bootstrap` closes the manifest-and-seed gaps by making
   `seed`, `run`, `verify-convergence`, and `diff-seeds` real end-to-end
   workflows backed by persisted facts instead of placeholder commands.

The work is intentionally one initiative with two milestones. `system takeover`
is the first executable milestone. Bootstrap follows on the same orchestration
principles so the implementation does not diverge into two different kinds of
long-running state.

## Approved Scope

### In Scope

- Stable takeover across RPM, dpkg, and pacman.
- Takeover success contract: plan, adopt/CAS-back, PM ownership transfer,
  generation build, boot entry creation, then stop before automatic live
  switch.
- Persisted operation state for takeover and bootstrap workflows.
- Bootstrap manifest-and-seed path:
  `bootstrap seed`, `bootstrap run`, `bootstrap verify-convergence`, and
  `bootstrap diff-seeds`.
- Deterministic convergence reporting using recorded derivation outputs.

### Out Of Scope

- Automatic live system switching as part of the stable takeover default path.
- Tier 2 / self-hosting completion.
- Boot-image polish beyond what is needed for the manifest-and-seed workflow.
- New daemon or service infrastructure for orchestration.

## Current State

### Takeover

The repository already has a three-phase takeover ladder in
`src/commands/generation/takeover.rs`:

- `cas`: adopt and CAS-back packages
- `owned`: remove packages from the native package manager database
- `generation`: build a generation, write a boot entry, and live-switch

The main gaps are operational rather than conceptual:

- success is inferred from inline execution rather than persisted state
- partial failures are surfaced as warnings, but not recorded durably
- the default stable milestone still needs a "stop before live switch" contract
- cross-PM support exists in helpers, but not yet as a proved stable surface

### Bootstrap

The repository already has the major primitives needed for the bootstrap path:

- local seed loading and hash verification in
  `conary-core/src/derivation/seed.rs`
- seed-based derivation execution in `cmd_bootstrap_run`
- per-package `output_hash` and per-run `build_env_hash` persisted in the
  derivation index
- cross-seed comparison logic in
  `conary-core/src/derivation/convergence.rs`

The main missing pieces are:

- `bootstrap verify-convergence` is still a stub
- `bootstrap diff-seeds` is still a stub
- `bootstrap run` produces useful outputs, but does not yet persist
  operation-level metadata for later inspection and comparison

## Approach Options

### Option 1: Keep The Current CLI, Add Durable Operation State

Keep the existing public commands, but add explicit operation records under the
hood for takeover and bootstrap.

Pros:

- smallest user-facing CLI churn
- lets current docs evolve instead of being rewritten again
- upgrades "best-effort" paths into inspectable, resumable workflows

Cons:

- requires new internal plumbing around existing helpers

### Option 2: Tighten The Current Inline Flows Only

Implement the missing bootstrap commands and harden takeover in place without
durable operation records.

Pros:

- smallest code diff

Cons:

- weaker resume and audit story
- harder to support real-machine failure recovery

### Option 3: Introduce A New Explicit Workflow Surface

Add new nouns such as `system takeover plan/apply/resume`.

Pros:

- cleanest long-term UX

Cons:

- highest CLI churn
- highest docs churn
- delays implementation value

### Chosen Direction

Choose Option 1. Keep the current command surface, but add explicit operation
state underneath it.

## Architecture

This initiative uses one small orchestration layer shared across two milestones.

### Milestone 1: Stable System Takeover

`conary system takeover` becomes a durable pipeline instead of a single long
inline action. Internally it records:

- discovered package manager and bootloader details
- requested target level
- takeover package buckets
- blocked packages
- phase outcomes and failure reasons
- generation number
- boot entry result

The stable path executes:

1. plan
2. CAS-back / adopt
3. native package-manager ownership transfer
4. generation build
5. boot entry write

It then stops in a ready-to-activate state without automatically calling the
live-switch path.

### Milestone 2: Bootstrap Manifest-And-Seed Completion

`conary bootstrap seed`, `run`, `verify-convergence`, and `diff-seeds` become
the supported bootstrap path. Each seed is verified input, each run records
enough metadata for later inspection, and each comparison command consumes
persisted results instead of acting like a placeholder.

## Components

### Shared Orchestration Records

Add a small persisted operation record for long-running takeover and bootstrap
workflows. This record should live near existing bootstrap or takeover outputs
rather than in a new service.

Each record needs:

- operation kind
- start and end timestamps
- requested parameters
- discovered environment facts
- per-phase status
- produced artifacts
- failure or warning details

### Takeover Components

Keep the existing takeover helpers responsible for real mutations, but stop
using stdout as the only source of truth.

New takeover state should capture:

- PM type and bootloader type
- total package inventory and package buckets
- blocked packages
- per-package adoption or removal failures
- final ownership summary
- generated generation number
- boot entry outcome
- whether activation is pending

### Bootstrap Components

Add parallel operation records for seed-based runs.

Bootstrap operation state should capture:

- manifest path
- seed ID
- recipe directory
- selected stage cutoff
- package filters
- work and output directories
- derivation DB path
- profile hash
- generation output path

This makes later comparison commands deterministic and inspectable.

## Data Flow

### Takeover Flow

1. Detect PM and bootloader state.
2. Compute takeover plan and persist it.
3. Execute each phase with explicit success or failure recording.
4. Build generation and write boot entry.
5. Mark the operation as ready to activate instead of live-switching.

If a phase fails, the command stops at a defined boundary and preserves enough
state to resume or inspect without recomputing the entire takeover plan.

### Bootstrap Flow

1. Verify and load each seed.
2. Run the manifest pipeline in isolated work and output directories.
3. Persist seed ID, profile hash, derivation DB path, and generation outputs.
4. Compare runs through the derivation index by `build_env_hash`.
5. Report convergence and seed differences from persisted facts.

### Reporting Rule

User-visible status must come from persisted facts, not only from transient
stdout side effects.

## Command-Level Design

### `conary system takeover`

Keep the existing command as the public entrypoint, but change the stable
generation path so it does not automatically live-switch.

Planned behavior:

- `--dry-run` produces the persisted plan and reports it without mutation.
- stable success means:
  - discovery succeeded
  - required PM operations succeeded or were recorded precisely as incomplete
  - generation build succeeded
  - boot entry outcome is known and recorded
- the command ends in a ready-to-activate state
- activation can remain a later explicit action, not an automatic side effect

### `conary bootstrap run`

Keep `bootstrap run` as the manifest-driven executor, but record enough
operation metadata for subsequent inspection and comparison.

### `conary bootstrap verify-convergence`

Implement this by loading recorded build outputs for two seeds and comparing
package `output_hash` values through the derivation index using
`build_env_hash`.

Behavior:

- fail clearly if the runs are not comparable
- succeed only when the comparison set is explicit
- report matched, mismatched, and skipped counts
- optionally print per-package diffs when requested

### `conary bootstrap diff-seeds`

Implement this as an input-side inspection tool rather than a semantic proof.

Behavior:

- compare seed metadata such as `seed_id`, source, target triple, and origin
  distro/version
- report content and metadata differences between the two seed directories
- help explain why convergence may or may not be expected

## Safety Model

### Takeover

The stable takeover contract must be stricter than the current best-effort
path.

Requirements:

- no automatic live switch in the stable default path
- no stable success when package discovery is incomplete
- no silent downgrade from failed PM removal to "everything worked"
- final status must distinguish:
  - complete and ready to activate
  - complete with warnings
  - incomplete / resumable
  - failed

### Bootstrap

Bootstrap comparison commands should favor deterministic evidence over vague
success.

Requirements:

- seed input must be hash-verified
- convergence must compare persisted outputs, not inferred package names alone
- diff output must be descriptive and explicit about what is and is not proved

## Testing Strategy

### Unit Tests

Add focused tests for:

- operation record state transitions
- takeover status reduction rules
- convergence report rendering and edge cases
- seed-diff comparison logic

### Integration Tests

Add CLI-level coverage for:

- takeover dry-run and persisted plan artifacts
- takeover phase completion and ready-to-activate status
- bootstrap run metadata persistence
- verify-convergence success and mismatch cases
- diff-seeds descriptive output

### Multi-PM Coverage

The stable takeover claim covers RPM, dpkg, and pacman. Testing does not need
hardware parity across distros, but it does need distro-backed proof that each
PM exercises the supported path rather than silently dropping into best-effort
behavior.

### Real-Flow Coverage

Add at least one end-to-end harness that proves the supported takeover contract:

- full ownership
- generation build
- boot entry creation attempt or precise degraded result
- no automatic live switch

## Non-Goals And Deferred Work

The following remain intentionally deferred after this design:

- Tier 2 / self-hosting completion
- full boot-image artifact polish
- automatic activation of the newly built generation as part of stable takeover

## Implementation Sequencing

1. Add shared persisted operation-state primitives and status rendering.
2. Refactor `system takeover` around those primitives and stop the stable path
   before live switch.
3. Add takeover tests and multi-PM proof coverage.
4. Persist bootstrap run metadata needed for later comparison.
5. Implement `bootstrap verify-convergence`.
6. Implement `bootstrap diff-seeds`.
7. Update active docs and feature-audit language to reflect the new supported
   contract.
