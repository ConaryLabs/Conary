# Codebase Simplification Pass

**Date:** 2026-04-03
**Scope:** Full codebase (~234k lines Rust, 7 crates)
**Goal:** Dead code removal, large-file decomposition, code clarity, deduplication

## Approach

Break the codebase into 12 logical chunks. Each chunk gets a dedicated
`simplify/<chunk-name>` branch created from `main`. Chunks are independent and
can be reviewed/merged individually.

## Chunks

| # | Branch | Crate(s) | Modules | ~Lines |
|---|--------|----------|---------|--------|
| 1 | `simplify/core-db` | conary-core | db | 19k |
| 2 | `simplify/core-model-ccs` | conary-core | model, ccs | 24k |
| 3 | `simplify/core-repository` | conary-core | repository, canonical | 19k |
| 4 | `simplify/core-build` | conary-core | derivation, recipe, derived, scriptlet | 16k |
| 5 | `simplify/core-resolver` | conary-core | resolver, dependencies, flavor, version | 9k |
| 6 | `simplify/core-system` | conary-core | capability, bootstrap, generation, container, filesystem | 26k |
| 7 | `simplify/core-supporting` | conary-core | packages, components, trust, provenance, automation, transaction, delta, trigger, compression, + loose files (hash, json, label, util, self_update, error, lib, federation_discovery) | 17k |
| 8 | `simplify/cli-dispatch` | conary | cli/, dispatch.rs, app.rs, main.rs, live_host_safety.rs | 6k |
| 9 | `simplify/cli-commands` | conary | commands/ | 33k |
| 10 | `simplify/remi` | remi | server/, federation/, trust.rs, lib.rs, bin/ | 34k |
| 11 | `simplify/conaryd` | conaryd | daemon/, lib.rs | 8k |
| 12 | `simplify/conary-test` | conary-test | all | 14k |

## What Each Pass Does

### Removes
- Unused functions, types, imports, constants
- `#[allow(dead_code)]` markers on code that can just be deleted
- Unreachable match arms and dead conditional branches
- Commented-out code

### Decomposes
- Files over ~500 lines: examine for split opportunities
- Files over 1,000 lines: strong candidates for extraction into submodules
- Functions over ~80 lines: look for extractable helpers

### Clarifies
- Reduce nesting depth (early returns, guard clauses)
- Simplify complex conditionals
- Replace verbose patterns with idiomatic Rust
- Deduplicate repeated logic within the chunk
- Improve unclear variable/function names

### Preserves
- All public APIs and module boundaries between crates
- All behavior and semantics
- File header path comments (per AGENTS.md convention)
- Test coverage (tests updated only if internal signatures change)
- No new dependencies added

## Execution

Chunks run in parallel (2-3 at a time) using isolated git worktrees. Each
agent works only within its designated modules. The branch receives one or more
commits describing what was simplified.

## Ordering

Start with smaller chunks (core-resolver, cli-dispatch, conaryd) to validate
the approach, then tackle the larger ones (core-db, core-model-ccs,
cli-commands, remi).

## Review

Each branch is independently reviewable and mergeable. Branches that cause
test failures should be fixed before marking complete.
