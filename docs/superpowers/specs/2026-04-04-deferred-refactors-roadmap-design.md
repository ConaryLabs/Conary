---
last_updated: 2026-04-09
revision: 3
summary: Roadmap for deferred post-simplification refactors tracked in issues #30 through #34
---

# Deferred Refactors Roadmap

## Context

The 2026-04-03/04 codebase simplification pass intentionally stopped short of
introducing new cross-crate abstractions. During Phase 2, the remaining
duplication patterns were recorded in:

- `docs/superpowers/specs/cross-crate-duplication-findings.md`
- GitHub issues `#30` through `#34`

Those issues are related, but they do not belong in one implementation pass.
They differ in risk, subsystem breadth, and abstraction pressure.

## Goal

Turn the deferred cleanup issues into a staged refactor roadmap that:

- preserves the safety and clarity gains from the simplification pass
- avoids a broad "deduplicate everything" effort
- starts with the most concrete, lowest-risk ownership cleanup
- treats analysis-heavy work as design-first rather than code-first

## Workstreams

### Track 1: Remi config ownership

Issues:

- `#31` centralize server config-default ownership in `remi` and `conaryd`
- `#34` reduce overlap between `RemiConfig` and `ServerConfig`

Planned first slice:

- implement `remi` only
- make `RemiConfig` the source of truth for the overlapping external/default
  semantics currently split between `RemiConfig`, `ServerConfig::default()`,
  and `RemiConfig::to_server_config()`
- keep `ServerConfig` as the normalized runtime-facing type for the subset of
  startup/runtime fields that already flow through it
- defer `conaryd` until the `remi` pattern is proven

Why first:

- it is intra-crate rather than cross-crate
- the duplication target is concrete and already has a conversion seam
- it establishes a pattern for later config work without forcing a workspace
  abstraction

### Track 2: Shared binary bootstrap helpers

Issue:

- `#30` extract shared binary bootstrap helpers across app crates

Scope:

- reevaluate only after Track 1 clarifies what belongs to config ownership
  versus binary startup
- focus on minimal shared startup primitives such as tracing/env-filter setup
  and top-level bootstrap conventions

Not in scope:

- unifying Clap schemas
- forcing one Tokio runtime model across binaries
- centralizing user-facing error presentation unless the value is clear

### Track 3: CLI/daemon operation descriptors

Issue:

- `#32` formalize shared operation descriptors across CLI and daemon

Scope:

- start with analysis/spec work, not direct implementation
- identify which operation names, request semantics, and status surfaces are
  truly duplicated versus intentionally distinct at the CLI and daemon layers

Why later:

- it crosses crate boundaries
- it risks creating shared abstractions before the daemon boundary is formally
  shaped enough to support them cleanly

### Track 4: Shared Axum server composition helpers

Issue:

- `#33` evaluate shared Axum server composition helpers

Scope:

- only after Track 3 or after a separate design demonstrates a stable common
  server shell between `remi` and `conaryd`
- bias toward small helpers around middleware and route assembly rather than
  shared "service framework" abstractions

Why last:

- it carries the highest abstraction risk
- it is the most likely track to produce a neat but harmful common layer

## Sequencing

Recommended order:

1. `remi` config ownership (`#31` + `#34`)
2. bootstrap helper refinement (`#30`)
3. operation descriptor investigation (`#32`)
4. Axum/server composition investigation (`#33`)

This order is intentional:

- Track 1 is a concrete refactor with strong local ownership.
- Track 2 becomes easier once config ownership stops leaking into binary
  bootstrap decisions.
- Tracks 3 and 4 should remain design-led until the local ownership questions
  are settled.

## Non-Goals

This roadmap does not authorize:

- a single mega-refactor spanning all five issues
- new cross-workspace utility crates without a dedicated design
- broad API reshaping during the first config ownership slice
- simultaneous `remi` and `conaryd` config rewrites in the same pass

## Deliverables Per Track

Each track should produce its own:

- design spec
- implementation plan
- focused verification list
- small, reviewable commits

Tracks 3 and 4 may conclude with an approved design and a "do not implement"
decision if the abstraction cost outweighs the duplication.

## Verification Expectations

Every track should verify both behavior preservation and abstraction discipline.
At minimum:

- targeted crate tests for the subsystem being changed
- one explicit statement of preserved precedence/ownership rules
- tests covering any new conversion or normalization layer
- confirmation that the refactor reduced duplicated ownership rather than merely
  moving code around

## Issue Handling

Issue `#31` should remain open after the first `remi` slice because it still
tracks the later `conaryd` half of the config-ownership work.

Issue `#34` is expected to be fully addressed by the first `remi` slice if the
implemented design removes the split default ownership between `RemiConfig` and
`ServerConfig` without broadening the scope.

## First Execution Slice

The first implementation spec should target Track 1 for `remi` only:

- `RemiConfig` owns the overlapping defaults and external semantics currently
  duplicated across `RemiConfig`, `ServerConfig::default()`, and
  `to_server_config()`
- `ServerConfig` remains the normalized runtime view derived from
  `RemiConfig` for fields such as bind address, storage-derived paths, cache
  sizing, conversion concurrency, rate limiting, audit logging, and web root
- CLI overrides remain supported but mutate `RemiConfig` before validation and
  conversion
- startup-time settings that are still crate-local and not yet part of the
  normalized runtime surface may continue to be read directly from `RemiConfig`
  in this slice, including admin listener settings, R2/search/federation
  enablement, prewarm settings, canonical fetch settings, and environment-based
  bootstrap paths

That slice remains tracked here, alongside Issue `#31` and the server-config
ownership notes in
`docs/superpowers/specs/cross-crate-duplication-findings.md`. No separate
retained archived design doc currently exists for it.
