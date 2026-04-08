---
last_updated: 2026-04-07
revision: 1
summary: Design for configurable source-selection policy with eligibility, latest-version ranking, Repology-backed freshness data, and strict handling for explicit version constraints
---

# Source Selection Policy

## Context

Conary already has the beginnings of a source-policy system, but the current
runtime behavior is narrower than the product direction we want.

Today:

- runtime source eligibility is primarily controlled by distro pinning and
  mixing policy in
  `crates/conary-core/src/db/models/distro_pin.rs`
- resolver-side policy filtering is handled in
  `crates/conary-core/src/repository/resolution_policy.rs`
- canonical expansion and ranking live in
  `crates/conary-core/src/resolver/canonical.rs`
- the canonical registry is bootstrapped from curated rules, AppStream, and
  Repology in `crates/conary-core/src/canonical/`
- the model layer already exposes richer source-policy concepts such as
  `system.profile`, `allowed_distros`, and a structured source pin in
  `crates/conary-core/src/model/parser.rs`

However, current root and update selection is still dominated by:

- request scope
- package overrides
- distro pin
- system affinity
- alphabetical tie-breaking

Repology data is currently used as a cross-distro identity and cache source,
not as an active selector for "highest available version."

That is misaligned with the intended product direction:

- cross-distro mixing is a core feature
- users should be able to choose a policy like "latest version among allowed
  candidates"
- explicit version constraints must still remain strict and authoritative

## Goal

Define a first-class source-selection policy model that:

- separates candidate eligibility from candidate ranking
- supports a configurable `latest` mode for unversioned requests
- interprets `latest` as "highest available version," not "most recently
  updated"
- uses Repology as the cross-distro latest-version signal in the first
  milestone
- falls back to current policy ranking when Repology lacks usable data
- keeps explicit version constraints strict and scheme-native
- applies consistently across install, update, and replatform/planning flows

## Non-Goals

This design does not attempt to:

- make Repology the authoritative installation metadata source
- weaken or ignore explicit version constraints
- expose every internal ranking knob directly in the first user-facing surface
- solve transitive cross-distro constraint semantics in the same pass as the
  first `latest` policy rollout
- decide the global default policy yet

## Product Principles

### 1. Eligibility and ranking are different concerns

The system first decides which candidates are allowed, then decides how to rank
the allowed set.

### 2. `latest` means latest version

`latest` is about highest available version among equivalent canonical package
implementations, not repository recency or update timestamp.

### 3. Explicit constraints override `latest`

If the user gives an explicit version constraint, that request must be handled
strictly. `latest` does not relax or replace explicit version semantics.

### 4. Missing latest-version data should not break normal operation

When Repology has no usable signal, the resolver should fall back to the
current policy ranking.

## Proposed Model

### Internal policy matrix

Internally, the source-selection policy should be modeled along separate axes:

- `eligibility_mode`
  - `strict`
  - `guarded`
  - `permissive`
- `selection_mode`
  - `policy`
  - `latest`
  - `prefer-latest` (deferred)
- `latest_source`
  - `repology`
- `latest_fallback`
  - `policy`
- `constraint_handling`
  - strict native comparison
- `scope`
  - install
  - update
  - replatform/planning

Not all of these need to be user-configurable in the first milestone.

### User-facing presets

Expose a small preset surface first:

- `policy`
  - current behavior based on request scope, overrides, pin, affinity, and
    existing tie-breakers
- `latest`
  - choose the highest available version among allowed canonical
    implementations using Repology as the latest-version signal
  - if Repology cannot decide, fall back to `policy`

Keep `prefer-latest` as an internal-ready but not yet exposed future mode.

## Eligibility Layer

Eligibility continues to be determined by the existing source policy system:

- request scope such as `--repo` or `--from-distro`
- distro pin
- mixing policy
- package override rules
- any future allowed-distro filters from the model layer

This means `latest` does not override pinning or mixing policy. It only ranks
within the set of candidates that are already allowed.

If the system is pinned to Fedora with strict mixing, and Arch is newer, Arch
is not eligible. If guarded or permissive policy makes Arch eligible, `latest`
may choose it.

## Ranking Layer

### `policy` mode

Preserve current ranking behavior:

- explicit request scope first
- package override
- pinned distro
- affinity
- stable tie-breaker

### `latest` mode

For unversioned requests:

1. expand to canonical candidates
2. apply eligibility filtering as today
3. look up Repology latest-version information for the canonical package and
   each eligible distro implementation
4. rank candidates by highest available version
5. when candidates tie on latest available version, fall back to current policy
   ranking
6. when Repology has missing, stale, or incomplete data, fall back to current
   policy ranking instead of failing

This makes `latest` a ranking policy, not an authorization policy.

## Latest-Version Source

For the first milestone, Repology is the source of cross-distro latest-version
signal.

Why:

- Conary already ingests and caches Repology data in `repology_cache`
- Repology is specifically designed to compare equivalent projects across
  distributions
- AppStream and canonical rules are strong identity inputs, but they are not a
  complete source of cross-distro version ordering

Repology should be treated as a ranking signal for unversioned selection, not
as authoritative install metadata.

## Constraint Handling

Explicit version constraints remain strict and scheme-native.

That means:

- `latest` only affects unversioned requests
- once a request includes an explicit version expression, the resolver must use
  candidate-aware native version semantics
- incompatible candidates must be rejected, not silently treated as unversioned

This incorporates the earlier root-request version-matching problem as a
sub-problem of the broader source-selection design.

## Persistence And Configuration

### Runtime persistence

The current runtime persistence model stores only:

- pinned distro
- mixing policy

The design should extend runtime source-policy persistence to also store
`selection_mode`.

### Declarative model

The model layer already has a good home for this:

- `system.profile`
- `allowed_distros`
- structured source pin

Long-term, the declarative model should be the canonical source of truth for
source-selection policy, with runtime compatibility storage mirroring the
effective state.

### CLI shape

The current `conary distro ...` commands are still useful but are too narrow as
the primary long-term surface. Over time they should evolve toward
"source policy" terminology rather than only "distro pin" terminology.

For the first milestone, compatibility with the existing CLI is more important
than a full command-surface rewrite.

## Scope Of Application

This policy should apply consistently across:

- install
- update
- replatform and planning flows

That is the desired product behavior. Users should not need to reason about one
selection policy for install and a different one for update.

Implementation may still be staged, but the design target is consistent
cross-surface behavior.

## Milestones

### Milestone 1: Source policy and latest-version ranking

Deliver:

- persistent `selection_mode`
- user-facing `policy` and `latest`
- `latest` ranking for unversioned requests
- Repology-backed latest-version signal
- fallback to current policy ranking
- integration into install, update, and replatform/planning flows

This milestone does not yet change explicit version-constraint handling beyond
preserving current strictness.

### Milestone 2: Explicit versioned request correctness

Deliver:

- candidate-scheme-aware comparison for explicit versioned requests
- fail-closed handling for scheme incompatibility
- clear diagnostics when no candidate can interpret the request

This milestone folds the previously isolated root-version-matching work into
the broader policy system.

## Error Handling

### In `latest` mode with missing ranking data

Do not fail. Fall back to `policy` ranking.

### In versioned requests

Do fail closed when no candidate can interpret the explicit constraint under
its native scheme.

### Diagnostics

User-facing output should clearly distinguish:

- "latest policy chose X because it is the highest available allowed version"
- "fell back to policy because latest-version data was unavailable"
- "explicit version request rejected candidate set because no candidate could
  interpret or satisfy the constraint"

## Risks

### Repology staleness and incompleteness

Repology is useful but not perfect. The design mitigates this by:

- treating it as a ranking signal, not authoritative install metadata
- falling back to policy ranking when it cannot help

### Product ambiguity around defaults

The team does not yet know whether `policy` or `latest` should be the default.
The first milestone should avoid hard-coding that decision into the design.

### Overexposing internal knobs

The internal matrix is valuable for architecture, but exposing every axis too
early would make the user-facing policy surface harder to understand.

### Inconsistent rollout across flows

If install gets `latest` but update or replatform do not, the system will feel
internally contradictory. The design should keep those surfaces aligned as the
target even if implementation is staged.

## Testing

Add coverage for:

1. `policy` mode preserving current ranking behavior.
2. `latest` mode choosing the highest available allowed canonical
   implementation using Repology data.
3. tie cases where equal latest-version candidates fall back to current policy
   ranking.
4. missing Repology data falling back to policy ranking.
5. pinned or strict-mixing systems preventing `latest` from selecting otherwise
   newer disallowed distros.
6. update and replatform/planning flows observing the same selection mode as
   install.
7. explicit version constraints overriding `latest` ranking semantics.

## Acceptance Criteria

This design is complete when:

- source eligibility and source ranking are modeled separately
- `latest` is defined as highest available version, not most recently updated
- `latest` uses Repology as its first ranking signal
- missing Repology data falls back to current policy ranking
- explicit version constraints remain strict and authoritative
- the system can evolve defaults later without redesigning the policy model
