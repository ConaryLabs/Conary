---
last_updated: 2026-03-28
summary: Design for bringing active feature claims, command surfaces, implementation, tests, and docs back into alignment
---

# Feature Claims Completion Design

## Goal

Bring every actively claimed feature in Conary's public and maintainer docs into one of two states:

1. Fully implemented, reachable through the current command surface, and backed by positive automated coverage.
2. Explicitly documented as limited or experimental with wording that matches the real product.

## Problem

The core product is in much better shape than the docs and coverage story now suggest. The audit exposed three recurring failure modes:

- active docs still describe commands that no longer exist or no longer match the supported UX
- some advanced features exist only as partial implementations
- several documented flows are only covered by smoke tests or graceful-failure tests instead of positive happy-path verification

This is now more of a truth-and-finish pass than a first-pass feature build.

## Decisions

### Standardize on the current command surface

Conary will keep the current command names and nesting. We will not add compatibility aliases for legacy spellings.

This means:

- bootstrap docs and tests move to `cross-tools`, `temp-tools`, `system`, `config`, and `tier2`
- collection docs stop referring to `update-group`
- capability docs stop referring to the old `enforce` wording and instead document the supported `capability` subcommands

### No overstated active docs

After this pass, active docs must not imply that a feature is fully usable if the product only supports a narrower subset.

### Every repaired feature gets a positive test

If we claim a user-visible workflow, we need at least one positive automated path that proves it works.

## Work Streams

### 1. Command-Surface Reconciliation

Align docs, examples, help text, and integration manifests with the command surface the binary actually exposes today.

Primary targets:

- bootstrap command names
- collection workflow examples
- capability command examples
- integration-testing docs that currently overstate which commands are positively validated

This stream changes wording and tests, not product semantics.

### 2. Feature Completion

Finish the partial implementations that are preventing us from truthfully claiming the features we already want to advertise.

### Derived packages

`derive build` must produce a real installable result instead of only updating DB state. The build path should create a concrete artifact or trove representation that can be queried, installed, and tracked consistently.

Parent-update stale marking must also be wired into the normal install and update flows so `derive stale` reflects real dependency drift instead of relying on dead code.

### Capability runtime enforcement

The existing runtime restriction machinery should become a supported public flow under `conary capability run <pkg> -- <command>`.

That includes:

- promoting the command from hidden/unimplemented to supported
- ensuring the enforcement path uses the package's declared capabilities
- documenting the supported runtime UX instead of the removed `enforce` terminology

### Other partial surfaces

For the remaining `partial` rows, the rule is:

- complete the behavior if the implementation gap is tractable in this pass
- otherwise narrow the docs to the subset we can actually prove

Candidates here include config-management happy paths, trigger mutation flows, label mutation flows, and proof-oriented command families like trust, automation, provenance diff, and federation management.

### 3. Coverage Expansion

Add positive automated coverage for the current weak spots.

Priority targets:

- config diff / backup / restore on tracked files
- label add / delegate / link
- trigger enable / disable / add / remove
- `ccs shell` and `ccs run`
- selective component installs
- direct local RPM / DEB / Arch install flows
- cleaner successful takeover path
- improved proof tests for trust, automation, federation, and provenance where we keep those claims active

The phase-4 suite should stop being a mostly-smoke “command exists” pass for these surfaces and become a real feature-verification layer.

### 4. Final Doc Alignment

Once code and tests reflect reality, refresh the active docs so they describe the real product and the real maturity level.

Primary targets:

- `README.md`
- `docs/conaryopedia-v2.md`
- `docs/INTEGRATION-TESTING.md`
- module docs covering bootstrap, federation, and CCS/runtime flows
- any maintainer or LLM-facing docs that still reference the old commands or overstated behavior

## Implementation Shape

This work cuts across several subsystems, so the implementation should stay organized by intent rather than by file type:

- command definitions and help text
- runtime command implementations
- derived-package storage/build/install integration
- transaction/update hooks for stale-marking
- integration manifests and fixture helpers
- public docs and module docs

The changes should be landed in small, test-backed increments even if they ship in one push.

## Testing Strategy

Minimum final verification:

- `cargo test --features server`
- `cargo clippy --features server -- -D warnings`

During implementation, each repaired feature should get the narrowest useful regression first, then fold into the full verification pass.

The success bar for the audit rows is:

- no active `doc drift`
- no active `untested` rows for still-claimed features
- `partial` rows either completed or explicitly narrowed in docs

## Risks

### Scope creep

This work touches many feature families. The mitigation is to keep the pass anchored to the audit rows and resist unrelated cleanup.

### Over-correcting docs before code lands

Docs should be the last stream to finish, not the first, so we do not rewrite the truth twice.

### Hidden implementation debt in advanced flows

Some surfaces may reveal larger gaps than the audit showed. If that happens, the fallback is to narrow the active claim instead of shipping another overstated feature.

## Expected Outcome

At the end of this work, Conary should present a tighter and more credible product shape:

- the commands in the docs are the commands users can actually run
- the advanced features we claim are real, not placeholders
- the integration story proves successful workflows instead of mostly proving “no panic”
