---
last_updated: 2026-04-02
revision: 1
summary: Reframe Conary CI/CD around product-aware release tracks, trusted validation lanes, and internal harness infrastructure
---

# Conary CI/CD Topology Design

## Summary

This design reframes Conary CI/CD around the project as it exists today:

- three release-track apps: `conary`, `remi`, `conaryd`
- one internal validation service: `conary-test`
- shared workspace components: `conary-core`, `conary-mcp`, packaging assets,
  and deploy scripts

The core decision is to stop treating the repository like a single product and
to stop treating `conary-test` like a peer release-track app.

The recommended topology uses five explicit automation lanes:

- `pr-gate`
- `merge-validation`
- `release-build`
- `deploy-and-verify`
- `scheduled-ops`

Each lane has one primary purpose, one trust boundary, and one operator story.
That split is the main simplification. It keeps ordinary pull-request checks
fast and safe, gives `conary-test` a first-class role in trusted validation,
and makes production promotion legible instead of hiding it inside a generic
"CI" story.

## Problem Statement

Conary's current CI/CD story has useful pieces, but the overall shape has
started to drift from the current workspace and product structure.

Today:

- [`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml) is a small
  GitHub Actions gate that manually enumerates crate builds and tests
- [`.github/workflows/release.yml`](../../../.github/workflows/release.yml) is
  a more complex `conary`-centric release pipeline triggered only by `v*` tags
- [`scripts/release.sh`](../../../scripts/release.sh) still models four version
  tracks, including `conary-test`
- [`docs/operations/infrastructure.md`](../../operations/infrastructure.md) and
  [`docs/INTEGRATION-TESTING.md`](../../INTEGRATION-TESTING.md) describe a
  deeper trusted-validation and operational story involving Forge,
  `conary-test`, and Remi

That produces three kinds of confusion:

1. product confusion: the workspace is multi-product, but the release pipeline
   looks like only `conary` is truly first-class
2. trust-boundary confusion: ordinary CI, trusted harness validation, and
   production deployment are close enough together to blur in maintainers'
   heads
3. lifecycle confusion: `conary-test` is versioned like a product even though
   its actual role is internal validation infrastructure

The result is not catastrophic complexity. The individual pieces make sense.
The problem is that the pieces no longer tell one clean story together.

## Goals

- Make the CI/CD topology match the current workspace and product boundaries.
- Treat `conary`, `remi`, and `conaryd` as the only release-track apps.
- Treat `conary-test` as trusted internal validation infrastructure rather than
  a public release-track product.
- Separate untrusted pull-request validation from trusted infrastructure access.
- Keep required merge-blocking checks fast, stable, and easy to explain.
- Give deeper `conary-test` validation a clear home without forcing it into
  every pull request.
- Make deployment and post-deploy verification explicit rather than implicit.
- Align the design with current GitHub Actions security and deployment
  practices as of 2026-04-02.

## Non-Goals

- Replacing Forge or `conary-test` with a GitHub-only validation model.
- Making every deep distro or adversarial suite a required pull-request check.
- Turning shared crates such as `conary-core` or `conary-mcp` into separately
  released products.
- Redesigning package formats, signing formats, or distribution channels in
  this slice.
- Defining the exact final YAML for every workflow in this design document.
- Eliminating all operator judgment from release promotion or production
  deployment.

## Current State

### Product And Workspace Reality

The current workspace in [`Cargo.toml`](../../../Cargo.toml) has six members:

- `apps/conary`
- `apps/remi`
- `apps/conaryd`
- `apps/conary-test`
- `crates/conary-core`
- `crates/conary-mcp`

That is not a single-binary repo anymore. It is a multi-app workspace with
shared crates and infrastructure code.

### Workflow Reality

Current automation has a split personality:

- `ci.yml` is a straightforward GitHub-hosted build/lint/test gate
- `release.yml` builds native packages for `conary`, creates a GitHub release,
  and deploys artifacts to Remi
- Forge and `conary-test` carry the deeper real-system validation story outside
  ordinary PR CI

This is already close to a multi-lane architecture, but it is not named or
documented that way yet.

### Release-Track Drift

`scripts/release.sh` still exposes four release groups:

- `conary`
- `remi`
- `conaryd`
- `conary-test`

But only the `conary` tag family currently maps to the GitHub artifact release
workflow. That mismatch creates avoidable cognitive drift.

## Approach Options

### Option 1: GitHub-Only Unified Pipeline

Keep everything inside GitHub Actions and try to make one automation surface
cover PR checks, heavy validation, releases, and deployment.

Pros:

- least moving parts on paper
- easiest to discover from the repository UI alone

Cons:

- poor fit for Conary's trusted harness and host-backed validation story
- encourages mixing untrusted PR execution with infrastructure access
- makes expensive validation pressure every merge gate unless aggressively
  split later anyway

### Option 2: Split-Lane GitHub-First Topology

Use GitHub Actions for the primary control plane, keep PR checks GitHub-hosted,
and reserve Forge plus `conary-test` for trusted merge-time and scheduled
validation.

Pros:

- best fit for the current Conary structure
- preserves the value of Forge and `conary-test`
- keeps trust boundaries legible
- matches modern GitHub deployment and release practices well

Cons:

- requires clearer workflow naming and docs
- requires maintainers to accept that not all validation belongs in one lane

### Option 3: Forge-Centric CI/CD

Make Forge the primary CI/CD system and treat GitHub mostly as a mirror or
source host.

Pros:

- maximum control over host environment and private validation
- strong fit for heavy distro/container-backed testing

Cons:

- higher operational burden
- less legible to new contributors
- weaker default separation between untrusted PR code and privileged runners
  unless extra isolation work is done

## Chosen Direction

Choose Option 2.

Conary should adopt a split-lane GitHub-first topology:

- GitHub Actions is the primary orchestration and visibility surface
- ordinary pull-request checks stay GitHub-hosted and unprivileged
- Forge and `conary-test` remain important, but only in trusted validation and
  operational lanes
- deployment is explicit and protected rather than bundled into generic CI

This is the best fit for both the codebase structure and the project's current
operational reality.

## Design

### Product Taxonomy

Conary should use the following product model consistently across code,
workflows, and docs.

Release-track apps:

- `conary`
- `remi`
- `conaryd`

Internal validation infrastructure:

- `conary-test`

Shared non-product components:

- `conary-core`
- `conary-mcp`
- packaging assets under `packaging/`
- deployment and operations scripts under `scripts/` and `deploy/`

The important consequence is that `conary-test` is not a peer release-track app
just because it has a Cargo manifest and a binary. Its primary responsibility is
to validate the real products.

### Automation Lanes

Conary should have five named automation lanes.

#### 1. `pr-gate`

Purpose:

- answer "is this pull request safe to merge?"

Characteristics:

- triggered by `pull_request`
- runs on GitHub-hosted runners only
- has no infrastructure or deploy secrets
- exposes a small stable set of required checks

Typical responsibilities:

- formatting
- linting
- workspace tests
- doctests
- dependency review and lightweight security checks

#### 2. `merge-validation`

Purpose:

- answer "did trusted post-merge validation find a regression on real systems?"

Characteristics:

- triggered on trusted refs such as `main`, manual dispatch, or post-merge
  chaining
- can use Forge and `conary-test`
- can access internal validation credentials and test endpoints

Typical responsibilities:

- curated `conary-test` smoke suites
- distro-backed validation with real containers
- targeted service smoke for `remi` and `conaryd`
- fixture publishing or refresh work when appropriate

#### 3. `release-build`

Purpose:

- answer "can we produce a valid release artifact set for a specific product?"

Characteristics:

- triggered only from product tag families
- builds artifacts and provenance material
- does not perform production deployment itself

Typical responsibilities:

- build package artifacts
- generate checksums
- generate attestations and SBOMs where supported
- publish release assets

#### 4. `deploy-and-verify`

Purpose:

- answer "did promotion to the target environment succeed and land cleanly?"

Characteristics:

- separate from artifact building
- runs behind protected GitHub environments
- uses deployment concurrency to serialize production changes

Typical responsibilities:

- promote approved release artifacts
- deploy to Remi or other target services
- run post-deploy landing verification

#### 5. `scheduled-ops`

Purpose:

- answer "is the deployed system still healthy and aligned with expectations?"

Characteristics:

- triggered by cron and manual dispatch
- not a merge blocker
- can run trusted operational checks

Typical responsibilities:

- Remi health checks
- self-update sanity checks
- release landing verification
- fixture freshness or drift checks
- certificate, secret, or environment drift monitoring as needed

### Trust Boundaries

The design should make trust boundaries obvious instead of implicit.

#### Pull Request Trust Boundary

`pr-gate` must assume the code under test is untrusted.

Therefore it should:

- run only on GitHub-hosted runners
- avoid Forge and Remi access
- avoid long-lived deployment secrets
- keep `GITHUB_TOKEN` permissions minimal

This matches current GitHub security guidance better than allowing ordinary PR
workflows to touch privileged self-hosted infrastructure.

#### Trusted Validation Boundary

`merge-validation` and `scheduled-ops` may use trusted infrastructure because
they run only on trusted refs or explicit operator-triggered contexts.

That is where Forge and `conary-test` belong.

#### Deployment Boundary

`deploy-and-verify` should be the only lane that can perform production
promotion. It should require a protected environment and should serialize
production changes with a concurrency key such as the environment name.

### Validation Depth And Cadence

Validation should be split by urgency, realism, and cost.

#### Pull-Request Depth

Every PR should get fast, merge-blocking validation:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- workspace tests
- doctests
- dependency review

For Rust test execution, `cargo-nextest` is a good fit for CI-oriented unit and
integration test runs, with doctests remaining separate.

#### Merge-Time Trusted Depth

Every trusted merge should get a curated, high-signal validation layer that is
more realistic than the PR gate but still fast enough to run routinely.

This is the correct home for:

- a small `conary-test` smoke set
- a preferred primary distro path
- service-specific smoke when `remi` or `conaryd` paths change materially

#### Nightly Or Manual Deep Depth

Broader coverage belongs in scheduled or manual validation lanes:

- larger distro matrices
- deeper `conary-test` groups
- packaging and release smoke across supported distro families
- adversarial or long-running suites

This avoids turning every pull request into a full-system certification event.

### Release Tracks And Tags

Tag families should map directly to release-track apps.

- `v*` for `conary`
- `remi-v*` for `remi`
- `conaryd-v*` for `conaryd`

`conary-test` should not have a release tag family.

Shared crates such as `conary-core` and `conary-mcp` should ride along with the
released app that consumes them. They are implementation components, not
standalone release-track products.

### `conary-test` Lifecycle

`conary-test` should be treated as internal infrastructure with deployment
provenance, not semantic-versioned release identity.

Its operational identity should be:

- commit SHA
- source ref or branch
- build timestamp
- clean or dirty workspace state if relevant

If maintainers want occasional milestone tags for the harness, those may exist
as historical markers, but they should not define the everyday CI/CD story.

### Operator Ergonomics

The workflow layout should make failures self-explanatory.

- `pr-gate` failure means code-quality or immediate regression issue
- `merge-validation` failure means trusted system validation caught a problem
- `release-build` failure means the artifacts could not be produced or proven
- `deploy-and-verify` failure means promotion or landing verification failed
- `scheduled-ops` failure means post-deploy drift or degradation was detected

Required checks should stay few and stable. Experimental, scheduled, and
investigative workflows should not become merge blockers by accident.

### Best-Practice Alignment

This design intentionally aligns with current platform guidance as of
2026-04-02:

- use reusable workflows for repeated build and release logic
- use GitHub environments and deployment protection rules for deployment
- use workflow or job concurrency for branch CI cancellation and serialized
  production deploys
- keep `GITHUB_TOKEN` permissions minimal by default and elevate only per job
- pin third-party actions to full commit SHAs where practical
- generate attestations and SBOMs for release artifacts
- use dependency review for dependency-changing pull requests
- avoid top-level required-workflow path filters that can leave required checks
  pending when skipped

## Migration Shape

This design does not require an all-at-once rewrite. A staged migration is the
right fit.

### Stage 1: Product And Doc Cleanup

- stop describing `conary-test` as a release-track product
- update operations docs to use the release-track-app vs internal-infra model
- define the five-lane topology in one canonical doc

### Stage 2: Release Alignment

- update `scripts/release.sh` so the normal release groups match the chosen
  product taxonomy
- align GitHub release triggers with the three release-track apps
- decide whether `remi` and `conaryd` need GitHub release assets, service-only
  tagged builds, or another explicit release representation

### Stage 3: Workflow Split

- evolve `ci.yml` into a clearly named `pr-gate` workflow or equivalent
- split build from deploy in the current `release.yml` path
- give trusted `conary-test` validation its own lane instead of hiding it in
  generic CI language

### Stage 4: Operational Hardening

- add protected deployment environments
- add deployment concurrency
- add post-deploy verification and scheduled operational checks
- add artifact provenance improvements where supported and appropriate

## Success Criteria

This design is successful when:

- a new maintainer can explain the automation model in a few sentences
- the repository has exactly three normal release-track app lines
- `conary-test` is clearly documented and operated as internal validation
  infrastructure
- ordinary PR workflows cannot reach Forge or production systems
- production deployment is explicit, protected, and serialized
- required merge checks remain fast, stable, and understandable

## Open Questions

The topology is settled, but a few implementation questions should be answered
in the follow-on plan:

- Should `remi` and `conaryd` publish GitHub release assets, or should their
  release track be represented only through tagged service builds and deploy
  records?
- Should `merge-validation` run on every `main` push, or should some portions
  be operator-invoked when high-cost infrastructure is constrained?
- Should workspace test execution stay on `cargo test` first, or should the
  project adopt `cargo-nextest` for the main CI lane now?

## References

- [`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml)
- [`.github/workflows/release.yml`](../../../.github/workflows/release.yml)
- [`scripts/release.sh`](../../../scripts/release.sh)
- [`Cargo.toml`](../../../Cargo.toml)
- [`docs/operations/infrastructure.md`](../../operations/infrastructure.md)
- [`docs/INTEGRATION-TESTING.md`](../../INTEGRATION-TESTING.md)
