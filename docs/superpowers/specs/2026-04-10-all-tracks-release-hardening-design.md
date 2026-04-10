---
last_updated: 2026-04-10
revision: 1
summary: Ship-blocker-only hardening design for a coordinated Conary, Remi, conaryd, and conary-test release before public announcement
---

# All-Tracks Release Hardening: Design Spec

**Date:** 2026-04-10  
**Status:** Draft for user review (design approved in conversation)  
**Goal:** Define a release-hardening pass for a coordinated `all` release that
stops on real ship blockers, rehearses the GitHub release and deploy control
plane in dry-run mode, and leaves the repo ready for `version bump -> tag ->
push`.

---

## Scope

This task covers the four supported release tracks modeled by
`scripts/release-matrix.sh`:

- `conary`
- `remi`
- `conaryd`
- `conary-test`

The hardening pass is intentionally narrow:

- include build, test, packaging, release-routing, deploy-routing, and
  release-facing documentation truthfulness
- include GitHub workflow rehearsal in `dry_run=true` mode
- include release-secret and environment readiness checks
- exclude general cleanup, wishlist refactors, and non-release polish unless
  they directly affect release credibility

This task is allowed to conclude that a coordinated `all` release should not
ship. If one track fails a ship-blocker gate, the correct outcome may be to cut
only a subset of tracks or delay the release entirely.

## Non-Goals

- adding new features for the announcement
- broad refactoring not tied to release confidence
- live production deployment rehearsal as part of the hardening task itself
- forcing a version bump for a track that the current release policy does not
  consider releasable without an explicit override decision

---

## Repository Context

Conary already has a concrete multi-track release model:

- local bump and tag entrypoint: `./scripts/release.sh [product|all]`
- release metadata and routing helper: `scripts/release-matrix.sh`
- release-matrix consistency checks:
  - `scripts/test-release-matrix.sh`
  - `scripts/check-release-matrix.sh`
- GitHub release workflow: `.github/workflows/release-build.yml`
- GitHub deploy workflow: `.github/workflows/deploy-and-verify.yml`

The supported canonical tags are:

- `v*` for `conary`
- `remi-v*` for `remi`
- `conaryd-v*` for `conaryd`
- `conary-test-v*` for `conary-test`

Dry-run rehearsal is already built into both GitHub workflows, so the hardening
task should use those existing paths instead of inventing a parallel release
simulation.

---

## Design

### Phase 1: Release Baseline Sanity

Start with the minimum checks that tell us whether a coordinated release is
even plausible.

Run:

- `git status --short`
- `bash scripts/test-release-matrix.sh`
- `bash scripts/check-release-matrix.sh`
- `./scripts/release.sh all --dry-run`

This phase answers four questions:

1. Is the working tree clean enough to release intentionally?
2. Does the release-matrix logic still resolve products, tags, bundle names,
   and deploy modes correctly?
3. Do the current commits imply a releasable bump for each track under the
   repo's conventional-commit policy?
4. What exact tags and versions would the coordinated release cut?

If `./scripts/release.sh all --dry-run` shows that a track does not bump, that
is a release decision point, not a cosmetic detail. A track with no qualifying
commits is not automatically part of the coordinated release.

### Phase 2: Local Build And Test Gates

Only after the release metadata looks sane do we pay for workspace validation.

Run:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build -p conary --release`
- `cargo build -p remi --release`
- `cargo build -p conaryd --release`
- `cargo build -p conary-test --release`
- `cargo test -p conary`
- `cargo test -p conary-core`
- `cargo test -p remi`
- `cargo test -p conaryd`
- `cargo run -p conary-test -- list`

This phase proves that the source tree is releasable before GitHub Actions is
asked to package or route anything. It is not intended to rerun every deep
integration suite locally unless one of the owning-package gates exposes a
problem that needs narrower follow-up.

### Phase 3: Public-Surface Truthfulness Audit

The release script updates owned manifests and packaging state, but it does not
update every release-facing copy surface. This phase is a targeted audit for
stale or misleading public claims.

Audit:

- top-level README release badge and release summary text
- install and comparison pages on the public site
- checked-in release-facing man page content
- any release/build/download wording that a subreddit reader could encounter
  immediately after clicking through

Known likely review targets discovered during brainstorming:

- `README.md`
- `site/src/routes/install/+page.svelte`
- `site/src/routes/compare/+page.svelte`
- `apps/conary/man/conary.1`

This phase should also sweep for hardcoded exact version strings that are not
updated by `scripts/release.sh`, especially on public pages and release-facing
docs. Test fixtures and archival/spec content are not blockers unless they are
shown to end users during the release flow.

### Phase 4: GitHub Dry-Run Rehearsal

Rehearse the actual GitHub release control plane without creating live releases
or deploying live binaries.

For each intended release track, manually dispatch `release-build` with:

- `tag_name` set to the exact next canonical tag predicted by
  `./scripts/release.sh all --dry-run`
- `product` set to the matching release track
- `dry_run=true`

Expected coverage:

- `conary`
- `remi`
- `conaryd`
- `conary-test`

Then manually dispatch `deploy-and-verify` with `dry_run=true` using the
`release-build` run IDs for the deployable tracks only:

- `conary`
- `remi`
- `conaryd`

`conary-test` intentionally has no deploy lane and should not be forced
through one.

Success criteria for this phase:

- release metadata resolves correctly from the modeled tag
- the expected bundle artifact is created for each product
- artifact names match the workflow's own lookup rules
- deploy routing matches product deploy mode
- dry-run verification jobs succeed for deployable products

### Phase 5: Secrets And Environment Readiness

Dry-run rehearsals do not prove every live-release prerequisite. Before the
real bump/tag/push sequence, verify that the required GitHub environment
configuration exists for the live path.

At minimum, confirm the presence and intended scope of:

- `RELEASE_SIGNING_KEY`
- `REMI_SSH_KEY`
- `REMI_SSH_TARGET`
- `CONARYD_SSH_KEY`
- `CONARYD_SSH_TARGET`
- `CONARYD_VERIFY_URL`

This phase is about existence, environment placement, and release-path
coverage, not secret rotation or value changes.

---

## Execution Order

Run the hardening pass in this order:

1. release baseline sanity
2. local build and test gates
3. public-surface truthfulness audit
4. GitHub dry-run rehearsal
5. secrets and environment readiness
6. final go/no-go decision

This order is intentional. It fails early on release-policy and routing
problems before spending time on deeper validation.

---

## Go/No-Go Rules

The hardening pass should stop and report `no-go` if any of the following are
true:

- any release track fails its release build, owning test suite, `fmt`, or
  `clippy` gate
- `./scripts/release.sh all --dry-run` shows that an expected track does not
  bump and there is no explicit decision to narrow or override the coordinated
  release
- public release-facing copy is stale or misleading in a way that would be
  visible from the subreddit thread, README, install page, or GitHub release
- a GitHub dry-run rehearsal fails to build, bundle, route, or resolve
  artifacts the way the live workflow expects
- required live-release secrets or environment configuration are missing

Known non-blockers may be waived only if they are clearly outside the release
path and do not affect first-touch credibility.

---

## Outputs

The hardening task should leave behind:

- a short checklist with every hardening item marked `pass`, `fail`, or
  `waived`
- a list of fixes made during the pass
- a list of remaining blockers, if any
- the exact release commands to run once green
- a recommendation on whether the release should remain `all tracks` or narrow
  to a subset

If the pass is green and the coordinated release still makes sense, the final
release step should be:

```bash
./scripts/release.sh all
git push
git push --tags
```

If the pass shows only a subset is justified, the final release commands should
be narrowed accordingly rather than forcing the full coordinated cut.

---

## Known Risks Already Identified

The brainstorming pass already surfaced a few likely hardening items:

- the repo currently has untracked scratch files that would need an explicit
  release decision before tagging
- release-facing version strings exist outside the manifests updated by
  `scripts/release.sh`
- the coordinated `all` release depends on every track still being releasable
  under the repo's current conventional-commit bump rules

These are not speculative risks; they should be checked directly in the
hardening task rather than treated as background assumptions.
