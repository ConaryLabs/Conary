---
last_updated: 2026-04-09
revision: 2
summary: Design for replacing ad hoc release versioning and tag logic with an explicit multi-product release matrix that preserves legacy tag continuity while standardizing future releases
---

# Release Matrix Realignment

> **Historical note:** This archived design is preserved for traceability. It
> describes the repository state and design intent at the time it was written,
> not the current canonical behavior. Use active docs under `docs/` and
> non-archived `docs/superpowers/` for current guidance.

## Context

Conary's release path has drifted away from the current codebase shape.

Observed state on April 9, 2026:

- local manifests define four binary products with independent versions:
  `conary`, `remi`, `conaryd`, and `conary-test`
- the checked-in release automation only supports three products:
  `conary`, `remi`, and `conaryd`
- historical tags do not fully match current product naming
  - Conary CLI uses `v*`
  - Remi history exists under `server-v*`
  - `conary-test` history exists under `test-v*`
  - current workflows and scripts expect `remi-v*` but do not understand the
    old `server-v*` lineage
- release logic is duplicated across `scripts/release.sh`,
  `.github/workflows/release-build.yml`, and
  `.github/workflows/deploy-and-verify.yml`
- shared crates such as `conary-core`, `conary-bootstrap`, and `conary-mcp`
  look like first-class versioned release units in Cargo manifests, but the
  actual externally released products are the app binaries and their packaging
  outputs

That mismatch creates three concrete risks:

- the next release can pick the wrong previous tag and compute the wrong next
  version
- different parts of the release path can disagree about which product a tag
  refers to or which artifacts should exist
- the release system can continue to encode old product structure even after
  the repository has been reorganized

## Goal

Replace the current ad hoc release mapping with one explicit release matrix
that:

- models every current releaseable product intentionally
- preserves historical version continuity across renamed legacy tag prefixes
- emits one canonical tag format per product going forward
- centralizes release identity, version ownership, artifact naming, and deploy
  mode in one checked-in source of truth
- keeps shared crates private to the workspace release model while still
  allowing their changes to influence the products that ship them

## Non-Goals

This design does not attempt to:

- redesign product packaging formats or deployment targets
- merge all products onto a single shared version number
- publish shared crates as independently released public artifacts
- pull the static `site/` and `web/` frontends into this binary-product release
  matrix; they remain outside the scope of this phase
- rewrite historical Git tags
- change product runtime behavior outside the release path
- define the full implementation task breakdown; that comes in the follow-up
  plan

## Decision

Adopt a first-class release matrix with four product tracks:

- `conary`
- `remi`
- `conaryd`
- `conary-test`

Each track has:

- one canonical future tag prefix
- zero or more accepted legacy tag prefixes used only for continuity lookup
- one or more version-owned manifests that represent release state
- a defined path scope for bump detection
- a defined artifact bundle shape
- a defined deploy mode

The release matrix becomes the sole authority for:

- tag-to-product resolution
- previous-release lookup
- version source ownership
- bump scope evaluation
- artifact bundle naming
- deploy eligibility

All other release entry points should consume the matrix instead of re-encoding
the same facts independently.

## Options Considered

### 1. Minimal compatibility patch

Patch `scripts/release.sh` to understand `server-v*`, leave the rest of the
release path mostly as-is, and defer broader cleanup.

Pros:

- smallest immediate change
- unblocks the next Remi release quickly

Cons:

- leaves duplicated release logic in place
- does not address `conary-test`
- preserves the same drift that caused the current ambiguity

### 2. Compatibility cutover for renamed tracks only

Preserve old history for renamed products, emit only new prefixes going
forward, but leave overall release topology otherwise unchanged.

Pros:

- solves the Remi continuity problem
- reduces naming drift

Cons:

- still leaves release truth fragmented across multiple files
- still treats `conary-test` as an accidental sidecar instead of a supported
  release track
- does not clarify how shared crates participate in product bumping

### 3. Full release-matrix rewrite

Model every current releaseable product explicitly, centralize release truth,
preserve legacy continuity, and standardize canonical future tag prefixes.

Pros:

- matches the repository's current product shape
- removes the main source of release ambiguity
- makes future release refactors cheaper because the matrix is explicit
- gives `conary-test` a supported release story

Cons:

- larger up-front change than a one-off patch
- requires coordinated updates to scripts, workflows, and docs

Recommended option: `3`.

## Proposed Release Topology

### Product tracks

The release matrix should define these four first-class tracks.

#### `conary`

- canonical tag prefix: `v`
- accepted legacy tag prefixes: none
- version-owned manifests:
  - `apps/conary/Cargo.toml`
  - `crates/conary-core/Cargo.toml`
  - `crates/conary-bootstrap/Cargo.toml`
  - `packaging/rpm/conary.spec`
  - `packaging/arch/PKGBUILD`
  - `packaging/deb/debian/changelog`
  - `packaging/ccs/ccs.toml`
- artifact mode:
  - CCS self-update package
  - distro packages (RPM, DEB, Arch)
  - GitHub release bundle
- deploy mode:
  - publish artifacts
  - deploy release payloads to Remi-hosted self-update/release endpoints

`conary-core` and `conary-bootstrap` remain private workspace crates, but this
track owns their shipped version alignment because the CLI is the externally
released product that carries them.

#### `remi`

- canonical tag prefix: `remi-v`
- accepted legacy tag prefixes:
  - `server-v`
- version-owned manifests:
  - `apps/remi/Cargo.toml`
- artifact mode:
  - Linux release bundle
  - GitHub release assets
- deploy mode:
  - remote bundle deployment
  - health verification

Legacy `server-v*` tags remain read-only continuity inputs. New releases must
emit only `remi-v*`.

#### `conaryd`

- canonical tag prefix: `conaryd-v`
- accepted legacy tag prefixes: none
- version-owned manifests:
  - `apps/conaryd/Cargo.toml`
- artifact mode:
  - Linux release bundle
  - GitHub release assets
- deploy mode:
  - remote bundle deployment
  - endpoint verification

#### `conary-test`

- canonical tag prefix: `conary-test-v`
- accepted legacy tag prefixes:
  - `test-v`
- version-owned manifests:
  - `apps/conary-test/Cargo.toml`
  - `crates/conary-mcp/Cargo.toml`
- artifact mode:
  - Linux release bundle
  - GitHub release assets
- deploy mode:
  - none for this phase

`conary-test` becomes an explicit artifact-only release track. It should gain a
supported build-and-release path, but not an automatic deployment job until the
repository has a stable dedicated rollout target for that product.

### Shared crate policy

Shared crates are not independent release tracks.

- `conary-core`
- `conary-bootstrap`
- `conary-mcp`

They remain private workspace implementation units. Their code changes may
trigger version bumps for dependent products, but they do not get their own
standalone tags or workflow lanes.

The matrix must therefore distinguish between:

- version-owned manifests that are bumped with a product release
- scope-only paths that can influence whether a product should release

This removes the current ambiguity where internal crate versions look public
while the actual released artifacts are app bundles and packaging outputs.

## Release Source Of Truth

Introduce one checked-in release-definition layer under `scripts/` that models
the complete release matrix in one place.

The matrix should define, per product:

- product key
- canonical tag prefix
- accepted legacy tag prefixes
- version-owned manifest paths
- additional scope paths that influence bump decisions
- artifact bundle name
- artifact file expectations
- deploy mode
- human-facing release name
- metadata fields that downstream workflows can trust without reloading the
  matrix from a repository checkout

The format can be shell-native data, a checked-in JSON/TOML file, or a small
helper script that emits structured results, but the critical property is that
every release entry point consumes the same definition.

Recommended shape:

- one helper under `scripts/` resolves:
  - `tag -> product`
  - `product -> canonical next tag`
  - `product -> previous tags to consider`
  - `product -> version-owned manifests`
  - `product -> artifact/deploy expectations`
- local release tooling and GitHub workflow shell steps both call that helper

The workflows should stop open-coding product identity with separate `case`
statements and artifact assumptions that can drift from the release script.

Because `deploy-and-verify` currently resolves releases from downloaded
artifacts rather than a checked-out repo, `release-build` must serialize the
matrix-derived downstream properties into `metadata.json`. At minimum that
metadata should include:

- canonical product key
- canonical tag prefix
- bundle artifact name
- deploy mode
- expected primary artifact pattern(s)

## Version Ownership Rules

### App binaries are the release products

The externally released products are:

- `conary`
- `remi`
- `conaryd`
- `conary-test`

Their app manifests anchor public release identity.

### Shared crates are product-coupled, not public release tracks

Shared crates should only be bumped when the owning product release requires
them.

For this phase, the recommended ownership is:

- `conary` release owns:
  - `apps/conary/Cargo.toml`
  - `crates/conary-core/Cargo.toml`
  - `crates/conary-bootstrap/Cargo.toml`
- `remi` release owns:
  - `apps/remi/Cargo.toml`
- `conaryd` release owns:
  - `apps/conaryd/Cargo.toml`
- `conary-test` release owns:
  - `apps/conary-test/Cargo.toml`
  - `crates/conary-mcp/Cargo.toml`

Rationale:

- `conary-core` and `conary-bootstrap` currently ship most directly as part of
  the CLI release surface
- `conary-mcp` currently ships most directly through `conary-test` and Remi,
  but giving it one owning track avoids multi-product manifest churn during
  this realignment
- other products still observe shared crate changes through bump scope
  detection, even when they do not own the shared crate version number

This intentionally favors a stable, understandable release model over trying to
perfectly encode every possible internal dependency relationship in public
version numbers.

### Migration constraint: `conary-test` and `conary-mcp`

The matrix cannot blindly synchronize `apps/conary-test/Cargo.toml` and
`crates/conary-mcp/Cargo.toml` to the selected app version until their current
lineages are aligned.

Current observed state:

- `apps/conary-test/Cargo.toml`: `0.3.0`
- `crates/conary-mcp/Cargo.toml`: `0.7.0`

That means a naive "set every owned manifest to the track app's next version"
implementation would downgrade `conary-mcp` on the first `conary-test`
release.

Required rule:

- release tooling must never downgrade an owned manifest version

Recommended cutover:

- perform a one-time alignment so `conary-test` starts its canonical release
  line at `0.7.0` before the matrix takes ownership of `conary-mcp`

If the implementation chooses not to do the one-time alignment, then the
release helper must treat owned manifest bumping as a monotonic per-manifest
operation and refuse any computed release that would move an owned manifest
backward.

## Tag Continuity And Canonical Emission

The cutover rule is:

- read old tags
- write new tags

That means:

- previous-release lookup must consider both canonical and accepted legacy
  prefixes for a product
- new releases must always emit the canonical prefix only

Examples:

- Remi should consider both `remi-v*` and `server-v*` when determining the last
  release, but the next release it creates must be `remi-vX.Y.Z`
- `conary-test` should consider both `conary-test-v*` and `test-v*`, but the
  next release it creates must be `conary-test-vX.Y.Z`

Historical tags are compatibility inputs only. The design does not rename or
recreate existing Git history.

Important implementation constraint:

- mixed-prefix history must be compared by stripping the accepted prefix and
  comparing the numeric version payload

Do not rely on native `git tag --sort=-version:refname` across mixed prefixes
such as `server-v*` and `remi-v*`. Prefix ordering can dominate the sort and
make an older legacy tag appear "newer" than a higher canonical version.

## Bump Scope Rules

Each product must define an explicit path scope used to determine whether a new
release is needed and what commits participate in changelog generation.

The scope should include:

- product-owned app code
- product-owned version manifests
- packaging/build/deploy files that materially affect shipped behavior
- shared-crate paths that can change the shipped product

Initial recommended scopes:

### `conary`

- `apps/conary/`
- `crates/conary-core/`
- `crates/conary-bootstrap/`
- `packaging/`
- `.github/workflows/release-build.yml`
- `.github/workflows/deploy-and-verify.yml`
- release-related scripts used by the CLI packaging path

### `remi`

- `apps/remi/`
- `crates/conary-core/`
- `crates/conary-bootstrap/`
- `crates/conary-mcp/`
- Remi deploy scripts and service/unit files
- release workflow files that affect Remi artifact handling

### `conaryd`

- `apps/conaryd/`
- `crates/conary-core/`
- daemon deploy scripts or service files
- release workflow files that affect `conaryd`

### `conary-test`

- `apps/conary-test/`
- `crates/conary-core/`
- `crates/conary-mcp/`
- test-release scripts or workflow files

The matrix helper should be the only place these scopes are defined.

## Artifact And Deploy Matrix

Artifact naming and deploy behavior should also be driven from the matrix
instead of hardcoded product-by-product in workflows.

Recommended release outputs:

### `conary`

- release bundle artifact name: `release-bundle`
- bundle contents:
  - `*.ccs`
  - optional `*.sig`
  - `*.rpm`
  - `*.deb`
  - `*.pkg.tar.zst`
  - `metadata.json`
  - `SHA256SUMS`
- deploy behavior:
  - publish to GitHub release
  - upload self-update payloads to Remi host
  - upload release package bundle to Remi host
  - verify public self-update endpoint

### `remi`

- release bundle artifact name: `remi-bundle`
- bundle contents:
  - `remi-<version>-linux-x64`
  - `remi-<version>-linux-x64.tar.gz`
  - `metadata.json`
- deploy behavior:
  - publish to GitHub release
  - remote host bundle deployment
  - health verification

### `conaryd`

- release bundle artifact name: `conaryd-bundle`
- bundle contents:
  - `conaryd-<version>-linux-x64`
  - `conaryd-<version>-linux-x64.tar.gz`
  - `metadata.json`
- deploy behavior:
  - publish to GitHub release
  - remote host bundle deployment
  - endpoint verification

### `conary-test`

- release bundle artifact name: `conary-test-bundle`
- bundle contents:
  - `conary-test-<version>-linux-x64`
  - `conary-test-<version>-linux-x64.tar.gz`
  - `metadata.json`
- deploy behavior:
  - publish to GitHub release
  - no automatic deploy in this phase

The workflows should consume these expectations through the matrix helper so
they fail clearly when a product's bundle definition changes without the
workflow being updated.

## Release Script Behavior

`scripts/release.sh` should stop hardcoding product arrays and instead operate
in terms of the matrix.

Required behavior:

- support the four canonical products:
  - `conary`
  - `remi`
  - `conaryd`
  - `conary-test`
- support `all` as the matrix-defined set of releasable products
- resolve the most recent prior release using canonical and legacy prefixes
- compute the next version from the real previous lineage
- bump all version-owned manifests for the selected product
- regenerate `Cargo.lock`
- update release-owned packaging/manifests when required
- create a release commit and canonical tag
- provide truthful `--dry-run` output for:
  - previous tag considered
  - next version
  - next canonical tag
  - files that would be updated
  - artifact/deploy mode for that product

The dry-run output should make cross-product consequences explicit when shared
crate scope changes imply that multiple products deserve a release.

## Workflow Behavior

Both GitHub workflows should be aligned to the same matrix.

### `release-build`

The prepare logic should:

- resolve product identity from either canonical or accepted legacy tag
  prefixes
- normalize that product to the matrix-defined canonical product key
- derive version, artifact expectations, and dry-run behavior through the
  shared matrix helper
- serialize the downstream matrix properties that `deploy-and-verify` needs
  into `metadata.json`

The build logic should:

- support all four products explicitly
- keep legacy tag compatibility only in tag resolution, not in new artifact or
  release naming
- add a `conary-test` build-and-release lane

### `deploy-and-verify`

The resolve logic should:

- consume release metadata produced by `release-build`
- use matrix-defined deploy eligibility instead of product-specific implicit
  assumptions
- enforce deploy decisions from serialized metadata, not from a reimplemented
  product lookup in the workflow file

The deploy logic should:

- retain current deploy behavior for `conary`, `remi`, and `conaryd`
- omit deploy jobs for `conary-test`
- fail clearly if a product is marked deployable in the matrix but lacks a
  defined deploy lane

The workflow must not silently succeed when release metadata identifies an
unknown product or a deployable product with no matching execution lane. Add a
catch-all validation step so "no job matched" is treated as an error, not a
successful no-op.

## Documentation Updates

The hardening pass should also update release-facing documentation so the new
model is discoverable and stable.

Minimum doc updates:

- `docs/operations/infrastructure.md`
  - reflect the four-track release matrix
  - document canonical tag formats
  - mention legacy tag continuity where relevant
- inline comments in app manifests that currently reference outdated tag-group
  names
- any release helper usage text that still reflects the earlier three-track
  model

The goal is that the docs describe the same product matrix that the automation
executes.

## Failure Handling

The release matrix should favor explicit failure over silent best-guess
behavior.

The helper or calling scripts should fail with specific errors for cases like:

- product defined but no version-owned manifest exists
- workflow requests an undefined product
- legacy tag resolves to more than one product
- workflow expects a bundle name not defined in the matrix
- deploy is attempted for a product with deploy mode `none`

This is part of the hardening work. Ambiguous release state should stop the
release path, not get papered over.

## Verification

The realignment should be proven in three layers.

### 1. Release helper verification

Add focused tests or verification cases for:

- canonical tag resolution
- legacy tag resolution
- mixed history continuity
- no-prior-release bootstrap behavior
- canonical next-tag emission
- per-product artifact/deploy metadata

### 2. Script verification

Add dry-run coverage that proves:

- `scripts/release.sh conary --dry-run`
- `scripts/release.sh remi --dry-run`
- `scripts/release.sh conaryd --dry-run`
- `scripts/release.sh conary-test --dry-run`

all report the correct previous lineage, next canonical tag, owned version
files, and artifact mode.

### 3. Workflow verification

Add or run checks that prove:

- `release-build` can resolve both legacy and canonical tags during the
  transition period
- `release-build` creates the expected bundle for each product
- `deploy-and-verify` only offers deployment for products whose matrix mode is
  deployable
- `conary-test` receives build-and-release handling without an accidental
  deploy path

## Files Expected To Change

The implementation phase should expect to touch at least:

- `scripts/release.sh`
- one new shared release-matrix helper under `scripts/`
- `.github/workflows/release-build.yml`
- `.github/workflows/deploy-and-verify.yml`
- `apps/conary-test/Cargo.toml`
- `crates/conary-mcp/Cargo.toml`
- release-facing docs under `docs/operations/`

Depending on the implementation shape, it may also touch:

- packaging/release helper scripts
- workflow validation scripts that check release assumptions

## Open Questions Resolved By This Design

- Should Remi continue historical version lineage even though the product name
  changed?
  - Yes. Read `server-v*`, emit `remi-v*`.
- Should `conary-test` become a supported release track?
  - Yes. It becomes a first-class build-and-release track with no deploy lane
    in this phase.
- Should shared crates be released independently?
  - No. They remain private workspace crates that influence product releases.

## Summary

Conary should stop inferring its release structure from a few historical
conventions and instead define it directly.

The release matrix realignment makes the repository's current product model
explicit, preserves continuity with legacy tags, standardizes future naming,
and gives both local scripts and GitHub workflows one shared source of release
truth.
