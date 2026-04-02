---
last_updated: 2026-04-02
revision: 3
summary: Reframe Conary CI/CD around a GitHub-only long-term control plane, product-aware release tracks, and internal harness infrastructure
---

# Conary CI/CD Topology Design

## Summary

This design reframes Conary CI/CD around the project as it exists today while
also defining the target steady state:

- three release-track apps: `conary`, `remi`, `conaryd`
- one internal validation service: `conary-test`
- shared workspace components: `conary-core`, `conary-mcp`, packaging assets,
  and deploy scripts

The core decisions are:

- stop treating the repository like a single product
- stop treating `conary-test` like a peer release-track app
- make GitHub the only long-term CI/CD control plane
- deprecate and remove Forgejo from the target topology

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
- the repository still contains active Forgejo workflows under
  [`.forgejo/workflows/`](../../../.forgejo/workflows/) and Remi-side Forgejo
  proxy code that reflect an older parallel control-plane story

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
- Make GitHub Actions the only long-term CI/CD control plane.
- Treat `conary`, `remi`, and `conaryd` as the only release-track apps.
- Treat `conary-test` as trusted internal validation infrastructure rather than
  a public release-track product.
- Separate untrusted pull-request validation from trusted infrastructure access.
- Keep required merge-blocking checks fast, stable, and easy to explain.
- Give deeper `conary-test` validation a clear home without forcing it into
  every pull request.
- Make deployment and post-deploy verification explicit rather than implicit.
- Remove Forgejo from the target-state architecture and active operator story.
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

### Legacy Control-Plane Drift

The repository currently has two control-plane stories:

- GitHub Actions under [`.github/workflows/`](../../../.github/workflows/)
- Forgejo workflows under [`.forgejo/workflows/`](../../../.forgejo/workflows/)
  plus Remi-side Forgejo proxy surfaces

Those paths overlap in purpose:

- Forgejo `ci.yaml` and `integration.yaml` run trusted validation on Forge
- Forgejo `release.yaml` verifies that a GitHub-built release landed on Remi
- Forgejo `remi-health.yaml` and `e2e.yaml` cover scheduled health and deep
  validation

That overlap is useful historical context, but it should not survive as the
long-term topology. The target state should have one orchestrator, not two.

### Release-Track Drift

`scripts/release.sh` still exposes four release groups:

- `conary`
- `remi`
- `conaryd`
- `conary-test`

But only the `conary` tag family currently maps to the GitHub artifact release
workflow. That mismatch creates avoidable cognitive drift.

### Branch Drift

Current GitHub CI still triggers on both `main` and `develop`, but the current
repository branch state only shows `main` as the active long-lived branch.

The design therefore assumes `main` is the only active long-lived branch unless
project governance explicitly restores a `develop` branch later.

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
- GitHub Actions is the only long-term control plane
- ordinary pull-request checks stay GitHub-hosted and unprivileged
- Forge and `conary-test` remain important, but only in trusted validation and
  operational lanes
- Forgejo is transitional legacy and should be removed from the target state
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

### Control Plane And Forge Execution

GitHub Actions should be the only long-term orchestrator.

Forge may continue to exist as trusted execution capacity, but not as an
independent workflow control plane.

The long-term execution model should be:

- GitHub-hosted runners for `pr-gate`
- a restricted GitHub self-hosted runner group on Forge for trusted lanes such
  as `merge-validation` and `scheduled-ops`
- protected GitHub environments for deployment lanes

This assumes the repository and runner configuration prevent untrusted fork PRs
from ever reaching trusted Forge-hosted runners, whether by repository privacy,
runner-group restrictions, or equivalent workflow restrictions.

That means:

- ordinary pull requests never run on Forge-hosted runners
- trusted `main`, schedule, and manual workflows may target Forge-hosted GitHub
  runners
- `.forgejo/workflows/*` are retired once their GitHub equivalents exist

The current rsync-plus-SSH flow in
[`scripts/deploy-forge.sh`](../../../scripts/deploy-forge.sh) remains a valid
manual or transitional path during migration, but it is not the desired
long-term control-plane mechanism.

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

The PR gate should prefer dependency review for newly introduced dependency
changes rather than making `cargo audit` a routine merge blocker.

For Rust test execution, `cargo-nextest` is a good fit for CI-oriented unit and
integration test runs, with doctests remaining separate.

#### Merge-Time Trusted Depth

Every trusted merge should get a curated, high-signal validation layer that is
more realistic than the PR gate but still fast enough to run routinely.

This is the correct home for:

- a small `conary-test` smoke set on every push to `main`
- a preferred primary distro path, with Fedora 43 as the default merge-time
  smoke target unless a stronger reason appears
- service-specific smoke when `remi` or `conaryd` paths change materially

The recommended default cadence is:

- every push to `main`: one trusted smoke subset plus lightweight service smoke
- nightly or manual: broader cross-distro and deeper harness coverage

This keeps lane 2 routine and useful without turning it into a full daily E2E
matrix on every merge.

#### Nightly Or Manual Deep Depth

Broader coverage belongs in scheduled or manual validation lanes:

- larger distro matrices
- deeper `conary-test` groups
- packaging and release smoke across supported distro families
- adversarial or long-running suites

This avoids turning every pull request into a full-system certification event.

`cargo audit` belongs here or in a dedicated manual security workflow, not in
the ordinary PR gate. Accepted advisory exceptions should remain explicit and
reviewable.

### Release Tracks And Tags

Tag families should map directly to release-track apps.

- `v*` for `conary`
- `remi-v*` for `remi`
- `conaryd-v*` for `conaryd`

`conary-test` should not have a release tag family.

Release representation should differ by product responsibility:

- `conary` should continue to publish GitHub release assets because it produces
  user-consumed package artifacts and self-update payloads
- `remi` and `conaryd` should be release-track services represented by tagged
  GitHub builds, deployment provenance, and deploy records rather than public
  GitHub asset bundles by default

If the project later wants packaged service artifacts for `remi` or `conaryd`,
that can be added as an explicit follow-on decision. It is not required for
this topology.

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

Its Cargo manifest version may remain for normal Rust packaging and build
metadata, but it should become informational rather than a promoted release
identity.

During migration, status surfaces such as `conary-test deploy status` should
continue to report the Cargo version but should give commit/ref provenance
higher operational weight.

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
- state explicitly that `main` is the active long-lived branch and remove stale
  `develop` assumptions unless the branch is intentionally restored

### Stage 2: Release Alignment

- update `scripts/release.sh` so the normal release groups match the chosen
  product taxonomy
- align GitHub release triggers with the three release-track apps
- keep `conary` as the only public artifact-release line by default
- represent `remi` and `conaryd` as tagged service-build and deploy lines in
  GitHub rather than introducing public asset bundles automatically

### Stage 3: Workflow Split

- evolve `ci.yml` into a clearly named `pr-gate` workflow or equivalent
- split build from deploy in the current `release.yml` path
- give trusted `conary-test` validation its own lane instead of hiding it in
  generic CI language
- retire the active role of `.forgejo/workflows/*` by replacing each live need
  with a GitHub Actions lane or scheduled workflow
- remove Remi-side Forgejo proxy and admin/MCP integration code once GitHub is
  the only remaining workflow control plane
- remove stale push-to-`main` behavior from the PR gate lane so merge-time
  validation clearly owns trusted push execution
- factor obvious repeated workflow setup into reusable GitHub automation,
  starting with Rust/toolchain/bootstrap setup for release and validation jobs

### Stage 4: Operational Hardening

- add protected deployment environments
- add deployment concurrency
- add post-deploy verification and scheduled operational checks
- pin third-party actions to commit SHAs
- add artifact provenance improvements such as attestations and SBOM generation
  where supported and appropriate
- move `cargo audit` into scheduled or manually invoked trusted security lanes

## Success Criteria

This design is successful when:

- a new maintainer can explain the automation model in a few sentences
- the repository has exactly three normal release-track app lines
- the repository uses GitHub as the only active workflow control plane
- no active CI/CD design docs depend on `.forgejo/workflows/*`
- `conary-test` is clearly documented and operated as internal validation
  infrastructure
- ordinary PR workflows cannot reach Forge or production systems
- production deployment is explicit, protected, and serialized
- required merge checks remain fast, stable, and understandable
- the primary GitHub workflow entrypoints map cleanly to the five named lanes,
  with reusable helper workflows allowed underneath
- stale `develop` workflow triggers are removed unless the branch is explicitly
  restored by project choice

## Open Questions

The topology is settled, but a few implementation questions should be answered
in the follow-on plan:

- Should workspace test execution stay on `cargo test` first, or should the
  project adopt `cargo-nextest` for the main CI lane now?
- Should the initial GitHub-on-Forge trusted runner setup be one runner or a
  small labeled pool?

## References

- [`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml)
- [`.github/workflows/release.yml`](../../../.github/workflows/release.yml)
- [`.forgejo/workflows/ci.yaml`](../../../.forgejo/workflows/ci.yaml)
- [`.forgejo/workflows/integration.yaml`](../../../.forgejo/workflows/integration.yaml)
- [`.forgejo/workflows/e2e.yaml`](../../../.forgejo/workflows/e2e.yaml)
- [`.forgejo/workflows/release.yaml`](../../../.forgejo/workflows/release.yaml)
- [`.forgejo/workflows/remi-health.yaml`](../../../.forgejo/workflows/remi-health.yaml)
- [`scripts/release.sh`](../../../scripts/release.sh)
- [`scripts/deploy-forge.sh`](../../../scripts/deploy-forge.sh)
- [`Cargo.toml`](../../../Cargo.toml)
- [`docs/operations/infrastructure.md`](../../operations/infrastructure.md)
- [`docs/INTEGRATION-TESTING.md`](../../INTEGRATION-TESTING.md)
