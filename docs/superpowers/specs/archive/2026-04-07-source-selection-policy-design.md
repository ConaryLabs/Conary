---
last_updated: 2026-04-09
revision: 5
summary: Design for a coherent source-selection program with profile-aware policy decomposition, persistent runtime mirrors, Repology-backed latest signal, and staged integration across install, update, and replatform flows
---

# Source Selection Policy

> **Historical note:** This archived design is preserved for traceability. It
> describes the repository state and design intent at the time it was written,
> not the current canonical behavior. Use active docs under `docs/` and
> non-archived `docs/superpowers/` for current guidance.

## Context

Conary already separates source eligibility from source ranking in practice,
but not yet as an explicit policy axis.

Existing eligibility inputs already live in the codebase:

- `ResolutionPolicy.request_scope` (`RequestScope`) in
  `crates/conary-core/src/repository/resolution_policy.rs`
- `ResolutionPolicy.mixing` (`DependencyMixingPolicy`) in
  `crates/conary-core/src/repository/resolution_policy.rs`
- `ResolutionPolicy.profiles` (`SourceSelectionProfile`) in
  `crates/conary-core/src/repository/resolution_policy.rs`
- runtime distro pinning in `crates/conary-core/src/db/models/distro_pin.rs`
- per-package overrides via `PackageOverride` in
  `crates/conary-core/src/db/models/distro_pin.rs`
- model-layer source-policy intent in `SystemConfig.profile`,
  `SystemConfig.allowed_distros`, and `SystemConfig.pin` in
  `crates/conary-core/src/model/parser.rs`

This design should add a new ranking axis, `selection_mode`, alongside those
existing axes rather than collapsing them into a single eligibility mode.

### Current architecture

There are three distinct ranking sites today:

1. `CanonicalResolver::rank_candidates_with_policy()` in
   `crates/conary-core/src/resolver/canonical.rs`
   - used for root canonical expansion and cross-distro implementation ranking
   - current order: request scope -> package override -> pinned distro ->
     affinity -> alphabetical distro
2. `PackageSelector::select_best()` in
   `crates/conary-core/src/repository/selector.rs`
   - used when choosing a concrete repository package from already filtered
     matches
   - current order: repository priority -> scheme-aware version ->
     repository name
3. `ConaryProvider::sort_candidates()` in
   `crates/conary-core/src/resolver/provider/traits.rs`
   - used by the resolvo SAT solver for transitive dependency ordering
   - current order: exact-name -> repository priority -> scheme-aware version
     -> installed -> name -> repository name

Candidate routing happens later, after selection, through `PackageResolution`
and `resolve_package()` in `crates/conary-core/src/repository/resolution.rs`.
That layer decides Binary / Remi / Recipe / Delegate / Legacy strategy and is
downstream of source selection.

Repology is already foundational in this system, not just an optional cache.
Phase 2 of canonical rebuild in `apps/remi/src/server/canonical_job.rs`
creates canonical identity mappings directly from `repology_cache`, so Repology
grouping quality already affects the identity layer before this design adds any
ranking behavior on top.

## Goal

Define `selection_mode` as a first-class source-ranking policy for unversioned
requests while preserving the existing eligibility model and the current strict
handling of explicit version constraints.

## Program Target

This initiative now targets a coherent end state across the whole
source-selection program, not just a narrow install-time slice.

The intended exit criteria for the overall program are:

- root install requests honor the same effective source-selection policy
- transitive dependency resolution is consistent with that policy
- update participates in source-selection rather than bypassing it
- replatform / model-apply surfaces can plan and execute against the same
  effective policy

Implementation can still be staged, but the planning unit should be the full
program. We should not treat known in-path gaps as background debt and then
build a partial feature around them.

## Non-Goals

This design does not attempt to:

- replace `RequestScope`, `DependencyMixingPolicy`, `SourceSelectionProfile`,
  pinning, or overrides with a new umbrella policy type
- perform raw cross-scheme version arithmetic in the first slice unless we
  explicitly choose to take that on
- deliver every surface in a single undifferentiated implementation step
- weaken explicit version constraints
- change routing strategy selection after a candidate has been chosen
- add per-invocation `--selection-mode` overrides to install/update in this
  initiative
- decide a single unified global default between `policy` and `latest` yet

## Product Principles

### 1. Eligibility and ranking remain separate

The system first decides which candidates are allowed, then decides how to rank
the allowed set.

### 2. `selection_mode` is additive

`selection_mode` is a new ranking axis layered onto the current policy model.
It does not replace request scope, mixing policy, profiles, pinning, or
overrides.

### 3. `system.profile` remains the model-layer preset surface

`system.profile` already exists and should remain the declarative preset layer
for source-selection intent. Decomposed fields such as `selection_mode` and
`allowed_distros` should be treated as explicit overrides of profile-derived
defaults, not as a separate competing model.

### 4. `latest` is version intent, not timestamp intent

The user-facing intent behind `latest` is "pick the newest allowed version,"
not "pick the thing updated most recently." However, the first milestone must
be honest about how it approximates that intent.

### 5. Explicit constraints stay strict

If the user supplies an explicit version constraint, the resolver must continue
to use native scheme-aware semantics and reject mismatched schemes.

### 6. Missing or weak latest signal must not break normal operation

When Repology cannot provide a usable signal, the resolver should fall back to
today's policy ranking rather than failing.

## Proposed Model

### Existing policy axes remain

Existing policy structure continues to carry:

- `RequestScope`
- `DependencyMixingPolicy`
- `SourceSelectionProfile`
- runtime pinning / `PackageOverride`
- `allowed_distros` as an explicit allowlist once it is wired into the
  effective policy path

### New axis: `selection_mode`

Add a new ranking axis:

- `selection_mode`
  - `policy`
  - `latest`
  - `prefer_latest` (deferred)

`selection_mode` affects ranking only. It never makes an ineligible candidate
eligible.

### Presets and decomposition

Treat `system.profile` as a higher-level preset name that maps to decomposed
policy defaults.

The current required mapping is:

- `balanced/latest-anywhere`
  - `selection_mode = latest`
  - no additional allowlist restriction by itself
  - existing pin/mixing rules still apply if set elsewhere

Future profiles may map to multiple axes at once, including:

- `selection_mode`
- `allowed_distros`
- mixing defaults
- future ranking/eligibility presets

When both a profile and decomposed fields are present, explicit decomposed
fields win.

Unknown profile names should produce a clear validation or model-apply error
rather than silently degrading to defaults.

### Runtime mirrors and source of truth

When the user manages policy declaratively through the model:

- the model is the source of truth
- runtime DB state is an effective mirror used by install/update/replatform
  commands

When no model is in use:

- runtime DB state is the source of truth for imperative CLI flows

This matches current source-pin behavior, where model apply writes effective
runtime state instead of having install/update read the model file directly.

### Transitional default behavior

This design intentionally preserves the current split default behavior during
the rollout rather than forcing an unreviewed product-default decision:

- imperative/runtime flows default to `SelectionMode::Policy` when no runtime
  setting is present
- model-backed flows preserve the current default profile,
  `balanced/latest-anywhere`, which maps to `selection_mode = latest`

So a fresh imperative CLI workflow and a fresh model-backed workflow may still
start from different defaults during this initiative. That is a compatibility
choice, not a claim that the long-term global default has been settled.

Choosing one unified default across all surfaces is intentionally deferred
beyond this program. The current work should not silently change either
existing surface while wiring the subsystem together.

### Persistence

Persist decomposed runtime policy state in the existing `settings` table
introduced by schema v46, using dotted namespace + kebab-case keys:

- migration: `crates/conary-core/src/db/migrations/v41_current.rs`
- accessors: `crates/conary-core/src/db/models/settings.rs`
- keys:
  - `source.selection-mode`
  - `source.allowed-distros`

This keeps ranking and allowlist state separate from `distro_pin`, which should
continue to represent pinning and mixing state only.

## Selection Semantics

### `policy`

`policy` preserves current behavior at each existing ranking site.

### `latest` in v1

The first milestone introduces `latest` only at the root canonical ranking
site: `CanonicalResolver::rank_candidates_with_policy()`.

For an unversioned root request:

1. expand to canonical-equivalent candidates
2. apply existing eligibility rules
3. annotate each eligible candidate with Repology signal
4. if one or more candidates have a positive latest signal, rank that subset
   ahead of the rest
5. break ties inside that subset using the existing `policy` ranking
6. if no candidate has a usable positive signal, fall back entirely to
   existing `policy` ranking

V1 does not change `PackageSelector::select_best()` or
`ConaryProvider::sort_candidates()`. Those remain explicit future integration
points and should be called out as such so later work can extend
`selection_mode` beyond root canonical ranking.

Eligibility for that request must already reflect:

- request scope
- pin/mixing policy
- package/profile overrides
- allowed distro filtering when configured

## Repology Signal

V1 does not attempt to compare RPM, Debian, and Arch version strings directly.
The current codebase does not support that:

- `compare_mixed_repo_versions()` in
  `crates/conary-core/src/repository/versioning.rs` returns `None` when
  schemes differ
- `compare_package_versions_desc()` in
  `crates/conary-core/src/resolver/provider/matching.rs` also declines
  cross-scheme ordering

So the v1 latest signal is Repology status, not raw cross-scheme version
arithmetic.

The data path for that signal is:

- `ResolverCandidate.canonical_id`
- `canonical_packages.name`
- `repology_cache.project_name`

So latest-signal lookups should batch by canonical package name plus eligible
distros rather than scanning `repology_cache`.

### Positive signal

A candidate has a usable positive latest signal only when:

- the candidate maps to a Repology project through the existing canonical map
- the candidate's distro has a corresponding `repology_cache` row
- `status == "newest"`
- `version` is present and non-empty
- `fetched_at` is recent enough (7 days old or newer)

### Non-positive signal

The following do not count as positive latest signals in v1:

- `status` of `outdated`, `devel`, `rolling`, `unique`, or `NULL`
- missing or empty `version`
- stale `fetched_at`
- unmapped, duplicate, or otherwise ambiguous Repology rows

When none of the eligible candidates have a usable positive signal, ranking
falls back to `policy`.

If all Repology rows are stale, missing, or ambiguous, the system silently
degrading to `policy` is safe behavior, but it should emit a diagnostic so the
user understands why `latest` did not influence the result.

This keeps milestone 1 honest: Repology can tell us which allowed candidates
are on the newest known upstream version, but v1 does not pretend to perform
full cross-scheme version ordering among all candidates.

## Constraint Handling

Explicit version constraints remain strict and scheme-native.

Relevant existing behavior:

- `constraint_matches_package()` in
  `crates/conary-core/src/resolver/provider/matching.rs` rejects cross-scheme
  mismatches
- `compare_mixed_repo_versions()` in
  `crates/conary-core/src/repository/versioning.rs` returns `None` across
  schemes

That means `latest` applies only to unversioned requests in v1. Versioned
cross-distro selection remains a separate follow-on milestone.

## Scope Of Application

### Install / root requests

Root install requests are the first ranking site that should become
`selection_mode` aware.

Install should continue to consume runtime effective policy, not read the model
file directly at resolution time.

### Transitive dependency resolution

`ConaryProvider::sort_candidates()` is the SAT-solver integration point for
this program and is part of milestone 2, not a vague follow-on.

The chosen milestone-2 direction is:

- root canonical ranking lands first in milestone 1
- SAT provider ordering becomes `selection_mode` aware in milestone 2 so
  transitive resolution follows the same effective policy
- exact-name repository selection is evaluated in the same milestone and wired
  if it remains a real bypass after SAT/provider integration

### Update

Current update resolution passes `policy: None` and `is_root: false` in
`apps/conary/src/commands/update.rs`, so it is not currently source-policy
aware.

Update needs an explicit semantic choice in the plan:

- same-source update: stay within the currently installed source / repository
- source re-evaluation update: reconsider eligible sources and possibly switch
  distros

The second option is effectively controlled replatforming, not a small
extension of the current update path.

Current update CLI surfaces do not expose per-invocation source-scope flags
such as install's `--from` / `--repo`, so update should consume the persisted
effective policy in this initiative rather than inventing a second temporary
override model.

If update is allowed to switch sources under `selection_mode=latest`, that
switch must not be silent. The update flow should:

- preview the proposed source changes
- explain why they were selected
- require confirmation unless `--yes` is supplied
- support dry-run inspection of source-switching updates

### Replatform / model apply

`cmd_model_apply()` in `apps/conary/src/commands/model.rs` currently renders
`ReplatformReplace` actions as planning-only and does not invoke the resolver
for those replacements.

The coherent end state for this initiative includes selection-aware replatform
planning and execution, which means the plan must include either resolver
integration in model diffing, a real executor path, or both.

## Planning Rule: Fix In-Path Gaps, Do Not Design Around Them

When this initiative touches a subsystem that is already supposed to
participate in source selection, but currently bypasses policy or is only
partially wired up, that gap should be pulled into the spec/plan as explicit
enabling work instead of being treated as background debt.

The intent is to come out of this work with a coherent system, not a new layer
that quietly works around old inconsistencies.

### Include a pre-existing gap in this initiative when:

- the gap sits directly on the source-selection path for an in-scope flow
- leaving it unfixed would force special-case behavior or contradictory
  semantics
- the initiative would otherwise need adapter logic around known incomplete
  plumbing

### Keep a pre-existing gap out of this initiative when:

- it is adjacent but not on the execution path for the current flow
- fixing it is valuable but not required for coherent source-selection
  semantics
- it would materially expand scope without changing the user-facing behavior of
  this initiative

### Current known in-path gaps

These should be treated as explicit prerequisites or named workstreams in the
plan, not as invisible assumptions:

- `system.profile` already exists as model-layer source-policy intent, so any
  new decomposed runtime field must define mapping and precedence instead of
  creating a second competing representation
- `allowed_distros` already exists in the model layer but is not enforced by
  install/update resolution yet, so eligibility wiring for that field is part
  of the foundational work
- update currently resolves with `policy: None` and `is_root: false`, so any
  future "latest-aware update" work first needs real source-policy plumbing
- root canonical ranking and SAT solver ordering are separate systems, so any
  requirement for coherent latest semantics across transitive resolution must
  include `ConaryProvider::sort_candidates()` work rather than assuming root
  ranking is enough
- replatform is currently planning-only, so any requirement for
  selection-aware replatforming must include resolver integration or executor
  work rather than assuming the existing model-apply path already supports it
- persistent `selection_mode` does not exist yet, so any cross-surface behavior
  depends on first establishing a real stored effective policy
- true cross-scheme version ordering does not exist, so the design must either
  add it explicitly or choose a narrower signal-based milestone instead of
  implying the comparison already works

## Milestones

### Milestone 1: foundation and root ranking

Deliver:

- profile-to-runtime decomposition and precedence rules
- persistent `selection_mode` and `allowed_distros` runtime mirrors in
  `settings`
- user-facing `policy` and `latest`
- `latest` support in `CanonicalResolver::rank_candidates_with_policy()` for
  unversioned root requests
- effective-policy eligibility honoring `allowed_distros`
- Repology status-based signal using `status == "newest"`
- policy fallback when no usable latest signal exists
- diagnostics that explain whether `latest` drove the choice or whether
  fallback occurred, including stale-cache fallback

This milestone establishes the policy substrate and the first working ranking
site, but it is not the completion point for the overall program.

### Milestone 2: resolver coherence

Deliver:

- extend `selection_mode` into `ConaryProvider::sort_candidates()` for
  transitive resolution
- evaluate whether `PackageSelector::select_best()` needs a latest-aware mode
  for noncanonical exact-name selection
- define how latest should interact with provider candidates and canonical
  fallbacks

This milestone closes the gap between root selection and transitive solver
ordering so installs behave coherently end-to-end.

### Milestone 3: update coherence

Deliver:

- decide whether update `latest` means same-source only or eligible-source
  re-evaluation
- make update resolution source-policy aware
- align update behavior with the persisted effective `selection_mode`
- add preview / confirmation safeguards for source-switching updates

### Milestone 4: replatform coherence

Deliver:

- integrate selection policy into model diff / replatform execution surfaces
- make replatform planning and execution use the same effective policy model

The overall initiative is not complete until milestones 1 through 4 land as a
coherent program.

### Separate milestone: versioned cross-distro correctness

This remains the follow-on to the earlier root-version-matching work:

- candidate-aware scheme selection before constraint construction
- fail-closed behavior when no candidate can interpret a requested constraint
- clear diagnostics for scheme mismatch

## Error Handling

### In `latest` mode with weak signal

Do not fail. Fall back to `policy` ranking.

### In versioned requests

Continue to fail closed when no candidate can interpret the explicit
constraint under its native scheme.

### Diagnostics

User-facing output should clearly distinguish:

- "`latest` chose X because Repology marked it `newest` among allowed
  candidates"
- "fell back to `policy` because no usable Repology latest signal was
  available"
- "fell back to `policy` because Repology data was stale"
- "update will switch source for X from Fedora to Arch because Arch is the
  newest allowed candidate"
- "explicit version request rejected candidate set because no candidate could
  interpret or satisfy the constraint"

## Risks

### Repology grouping errors

Repology project grouping quality already affects canonical identity. This
design adds ranking dependence on the same substrate, so incorrect grouping can
now affect both identity and `latest` choice.

### Repology staleness

Repology is useful but not authoritative install metadata. The design mitigates
this by requiring recent data for a positive signal and falling back to
`policy` when the signal is weak. That means offline or behind-sync clients can
degrade to `policy`, and should be told when that happens.

### Product ambiguity around defaults

The team has not yet chosen a single unified default across imperative and
model-backed flows. This design preserves the current surface-specific defaults
for compatibility while making the effective policy explicit and shared.

### Partial rollout confusion

If users assume `latest` changes update or replatform behavior before those
surfaces are integrated, the system will feel inconsistent. The design must be
explicit about milestone boundaries.

## Testing

Add coverage for:

1. `policy` preserving current root canonical ranking behavior.
2. `latest` preferring eligible candidates with Repology `status == "newest"`.
3. ties among `newest` candidates falling back to current policy ranking.
4. missing, stale, or ambiguous Repology signal falling back to `policy`.
5. pinned or strict-mixing systems preventing `latest` from choosing otherwise
   newer disallowed distros.
6. model preset mapping such as `balanced/latest-anywhere ->
   selection_mode=latest`, with explicit decomposed overrides taking
   precedence.
7. `allowed_distros` constraining candidate eligibility once configured.
8. explicit version constraints continuing to reject cross-scheme mismatches.
9. milestone 1 preserving current update and replatform behavior while later
   milestones add explicit coverage for those surfaces.
10. resolver-coherence tests showing root and transitive ordering follow the
   same effective policy after milestone 2.
11. update-coherence tests covering the chosen update semantic model and
    source-switch preview/confirmation behavior.
12. replatform-coherence tests covering planning and execution against the same
    effective policy.

## Acceptance Criteria

This design is complete when:

- `selection_mode` is modeled as a new ranking axis alongside existing policy
  axes
- `system.profile` has a defined relationship to decomposed policy fields
- the three current ranking sites are explicitly documented
- the program scope is honest about which in-path gaps must be addressed before
  the initiative is truly complete
- `latest` v1 is defined in terms of concrete Repology signal, not vague
  "highest version" language
- `allowed_distros` is part of the effective eligibility model rather than a
  documented-but-unenforced field
- fallback behavior is concrete and non-breaking
- explicit version constraints remain strict and authoritative
- install, update, and replatform are all represented as part of one coherent
  implementation program even if the code lands in staged milestones
