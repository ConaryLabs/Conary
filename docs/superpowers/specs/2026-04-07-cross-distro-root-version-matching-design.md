---
last_updated: 2026-04-07
revision: 1
summary: Design for root-request cross-distro version matching that binds version semantics to the candidate repository scheme at evaluation time
---

# Cross-Distro Root Version Matching

## Context

The current root install flow can preserve repository and distro scope through
`ResolutionPolicy`, but version constraints may still be bound to a
`VersionScheme` too early in the resolver path.

Today:

- request scope is built in
  `apps/conary/src/commands/install/mod.rs`
- canonical and policy ranking may choose candidates from multiple distro
  flavors
- repository and dependency rows are converted to `ConaryConstraint` values in
  `crates/conary-core/src/resolver/provider/loading.rs`
- candidate version matching happens in
  `crates/conary-core/src/resolver/provider/matching.rs`

This creates a gap for root package selection: a user-supplied version request
can be interpreted in one scheme before the resolver has actually committed to
the candidate repository whose version semantics should control comparison.

The current code documents this with a live TODO in
`crates/conary-core/src/resolver/provider/matching.rs` about detecting the
target repository version scheme before constructing the constraint.

## Goal

Make versioned root requests work correctly when canonical expansion and policy
ranking consider candidates from different distro schemes.

For this first milestone:

- root-package candidate evaluation must interpret the requested version using
  the candidate repository's `VersionScheme`
- candidates whose scheme cannot parse or compare the request must be rejected
- failure must be fail-closed rather than silently dropping version semantics
- transitive dependency behavior must remain unchanged

## Non-Goals

This design does not attempt to:

- change transitive dependency matching in strict, guarded, or permissive
  mixing flows
- redesign repository dependency row loading or installed-package constraint
  handling
- add delta fetch, recipe strategy wiring, or verify-input persistence
- weaken version constraints for incompatible candidates

## Decision

Adopt delayed scheme binding for root requests.

The resolver should preserve the raw root version request until candidate
evaluation, then parse and compare it using the candidate repository's scheme.
Installed-package constraints and already-schemed repository dependency rows
stay on their existing paths for this milestone.

This avoids guessing the target scheme too early while keeping the change
surface focused on the root-package selection workflow that is currently wrong.

## Options Considered

### 1. Delay scheme binding until candidate matching

Keep the raw root request intact and evaluate it per candidate using that
candidate's `VersionScheme`.

Pros:

- matches the actual bug boundary
- aligns version semantics with the candidate actually being evaluated
- supports canonical expansion and policy ranking cleanly
- naturally supports fail-closed behavior

Cons:

- introduces a new root-request constraint path or equivalent matching seam

### 2. Infer scheme from request scope before constraint creation

Bind the request to a scheme up front from `--from-distro` or `--repo`.

Pros:

- smaller immediate change

Cons:

- brittle when repo scope is indirect
- still guesses too early
- does not compose cleanly with ranked candidates from multiple repos

### 3. Re-check versions in a second pass after ranking

Leave current constraint creation in place and layer a root-only validation
pass after ranking.

Pros:

- targeted to the symptom

Cons:

- duplicates matching logic
- splits responsibility across ranking and matching layers
- makes later extension harder

Recommended option: `1`.

## Proposed Design

### Root request representation

Introduce a root-request representation that preserves:

- package name
- optional raw version text
- whether the request is versioned

This representation must not permanently bind to a repository version scheme
when first parsed from CLI input.

The design does not require changing every `ConaryConstraint` user. It is
acceptable to add a root-only variant or parallel evaluation path as long as
repository dependency rows and installed-package constraints continue to behave
as they do today.

### Matching behavior

For root-package candidate evaluation:

1. take the raw request version text
2. inspect the candidate repository's `VersionScheme`
3. parse the request under that scheme
4. compare the candidate version under that same scheme
5. accept or reject the candidate based on the result

Candidate outcomes must be distinguished as:

- `satisfies`: candidate scheme parsed the request and comparison succeeded
- `version_mismatch`: candidate scheme parsed the request but the candidate did
  not satisfy it
- `scheme_incompatible`: candidate scheme could not parse or compare the
  request at all

### Fail-closed behavior

If a candidate is scheme-incompatible, the resolver must reject it.

The resolver must not:

- reinterpret the request as unversioned
- treat incompatibility as a soft preference
- allow policy ranking to override version semantics

If every ranked candidate is rejected because the request is incompatible with
their schemes, the final error should explicitly say that no eligible candidate
could interpret the requested version constraint.

## Data Flow

The root-package resolution flow becomes:

1. parse the user request into `name + optional raw version text`
2. build request scope and resolution policy as today
3. run canonical expansion and policy/ranking as today
4. evaluate ranked root candidates one by one
5. bind the raw version request to each candidate's scheme at evaluation time
6. reject candidates that are version mismatches or scheme-incompatible
7. select the first ranked candidate that satisfies policy and version matching
8. if none survive, return a diagnostic that distinguishes incompatibility from
   ordinary mismatch

This preserves the existing ownership split:

- policy decides which candidates are allowed and how they are ranked
- candidate version scheme decides how version text is interpreted

## Error Handling

The final user-facing error should distinguish:

### Comparable but not satisfied

At least one candidate understood the request, but none of the comparable
candidates satisfied the version.

This remains a normal "no candidate satisfies constraint" style failure.

### No comparable candidates

Every candidate that survived policy/ranking was incompatible with the request
syntax under its own scheme.

This must fail closed with a diagnostic that includes:

- requested package and raw version text
- repositories or distros considered
- candidate schemes that were incompatible

The current warning-only behavior in `matching.rs` should not remain the sole
signal for successful root resolution paths. A successful root resolution
should not rely on or emit the old scheme-mismatch warning for the winning
candidate.

## Implementation Shape

The smallest coherent change is:

- keep `build_resolution_policy()` and root candidate ranking behavior intact
- add a root-request-specific version evaluation seam
- keep installed and repository dependency row loading unchanged
- centralize comparable / mismatch / incompatible decisions in the matching
  layer instead of scattering them across ranking code

Likely touch points:

- `apps/conary/src/commands/install/mod.rs`
- `crates/conary-core/src/resolver/provider/loading.rs`
- `crates/conary-core/src/resolver/provider/matching.rs`
- any root candidate selection helpers that currently assume a pre-bound scheme

## Testing

Add regression coverage for:

1. Debian root request succeeds when canonical expansion presents mixed-scheme
   candidates and the Debian candidate satisfies the request.
2. RPM root request succeeds under the mirror case.
3. A versioned root request fails closed when every ranked candidate is scheme
   incompatible.
4. Successful root resolution no longer depends on the current
   `Version scheme mismatch` warning path for the winning candidate.
5. Transitive dependency behavior remains unchanged in this milestone.

## Risks

### Hidden coupling with `ConaryConstraint`

Some root resolution code may assume every versioned request is already encoded
as a `ConaryConstraint`. If so, the implementation should add a narrow root
request abstraction rather than force repository dependency rows onto the same
new path.

### Diagnostics becoming noisy

If every incompatible candidate records a separate warning during normal search,
logs may become noisy. Prefer collecting incompatibility reasons for the final
error path over warning on every rejected candidate during successful
resolution.

### Scope creep into transitive dependencies

The implementation should resist extending this behavior into transitive
dependency matching during the same pass. That belongs in a later design.

## Acceptance Criteria

This design is complete when:

- root requests with explicit version constraints evaluate versions using the
  candidate repository's scheme
- incompatible candidates are rejected, not silently relaxed
- the final error distinguishes incompatibility from normal mismatch
- transitive dependency behavior is unchanged
- regression tests cover mixed-scheme root candidate selection
