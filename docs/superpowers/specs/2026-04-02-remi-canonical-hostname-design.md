---
last_updated: 2026-04-02
revision: 1
summary: Make remi.conary.io the canonical public package-service hostname while keeping packages.conary.io as a compatibility alias
---

# Remi Canonical Hostname Design

## Summary

Conary's active code, docs, CI, and frontend copy still treat
`packages.conary.io` as the default public package-service hostname. That no
longer matches the operator preference or the current Cloudflare setup.

The target steady state is:

- `https://remi.conary.io` is the canonical public package-service hostname
- `https://packages.conary.io` remains operational as a compatibility alias
- the repository stops teaching or defaulting to the old hostname
- this slice does not change the current two-frontend deployment shape

This is intentionally a naming and default-alignment change, not a deployment
topology rewrite.

## Problem Statement

The repository currently has drift between operational reality and tracked
defaults:

- README examples, site links, and deploy notes still point users at
  `packages.conary.io`
- runtime defaults such as self-update, default repository initialization, and
  test-harness configuration still embed `packages.conary.io`
- CI deployment verification checks `packages.conary.io`, reinforcing it as the
  "real" public host
- infrastructure and Cloudflare docs still describe `packages.conary.io` as the
  primary external name

That drift is small in any one place, but together it creates a muddled public
story. Users, maintainers, and future automation all receive the wrong
canonical host.

## Goals

- Make `https://remi.conary.io` the canonical public package-service hostname
  across active code, docs, CI, and frontend links.
- Preserve `https://packages.conary.io` as a compatibility alias during the
  rollout.
- Update default URLs atomically enough that new installs, self-update, and CI
  verification all agree on the same hostname.
- Keep the change easy to explain: "canonical host changed; alias still works."

## Non-Goals

- Removing or disabling the `packages.conary.io` alias in this slice.
- Changing DNS, Cloudflare rules, or reverse-proxy behavior directly from this
  repository.
- Collapsing `site/` and `web/` into one frontend.
- Redesigning Remi federation hostnames or port-based peer identity flows.
- Reworking public/private admin-surface policy beyond renaming the documented
  public host.

## Current State

### Runtime Defaults

Active runtime and test defaults still embed `packages.conary.io` in:

- `conary` default repo initialization
- self-update default channel URLs
- bootstrap and repository-sync examples
- `conary-test` global configuration and derived environment variables
- test artifact fetch URLs

### Docs And Frontends

Active user-facing copy still presents `packages.conary.io` as primary in:

- `README.md`
- infrastructure and integration docs
- Cloudflare deployment notes
- `site/` marketing links and install examples
- `web/README.md` and deploy notes

### CI And Verification

GitHub deployment verification still curls `packages.conary.io`, which means the
automation story disagrees with the desired public hostname.

## Approach Options

### Option 1: Canonicalize The Repo, Keep Alias Live

Change all active tracked defaults to `remi.conary.io`, but keep
`packages.conary.io` working operationally outside the repo.

Pros:

- lowest-risk rollout
- immediately fixes docs and defaults
- does not require a redirect cutover to land first
- easy to explain to contributors

Cons:

- both names continue to exist for a while
- alias cleanup remains future work

### Option 2: Canonicalize And Redirect Browser Traffic Only

Make `remi.conary.io` canonical in the repo and add browser redirects from
`packages.conary.io`, while preserving API compatibility for non-browser use.

Pros:

- stronger user-facing convergence
- still preserves backward compatibility for automation

Cons:

- requires live infra behavior changes outside this repo
- increases rollout coordination

### Option 3: Immediate Hard Cut

Replace the old host everywhere and expect all clients to move immediately.

Pros:

- cleanest end state on paper

Cons:

- unnecessary rollout risk
- likely to break older clients, scripts, or bookmarks

## Decision

Choose **Option 1**.

`remi.conary.io` becomes the canonical public hostname in the repository, while
`packages.conary.io` remains a compatibility alias outside the repository. That
captures the preferred operator-facing story without coupling this change to DNS
or redirect enforcement.

## Proposed Design

### Canonical Public Host

Use `https://remi.conary.io` for active public package-service references,
including:

- repository add/sync examples
- self-update channels and release download examples
- public collection URLs
- test-artifact and health-check URLs
- public MCP entrypoint examples such as `/mcp`
- package-site links from `conary.io`

### Compatibility Alias Policy

Do not remove `packages.conary.io` support in runtime logic during this slice.
The compatibility promise is operational rather than documentary:

- tracked defaults stop using `packages.conary.io`
- compatibility remains external behavior, not the repository's taught default
- if a test explicitly exists to confirm the default host, update the expected
  output to `remi.conary.io`

### Documentation Policy

Active docs should present one clear story:

- `remi.conary.io` is the preferred public package host
- `packages.conary.io` may be mentioned only when explaining compatibility or
  historical context
- local-only operator notes in ignored files do not block this change

### CI And Verification Policy

Deployment and smoke verification should test the canonical hostname so that
automation matches the user-facing documentation. The old host should not remain
the main health or release-verification target in tracked workflows.

## File Groups

This change spans four main file groups:

1. runtime defaults and tests
   - `apps/conary/...`
   - `crates/conary-core/...`
   - `apps/conary-test/...`
2. CI and tracked deploy verification
   - `.github/workflows/...`
3. active docs and frontend copy
   - `README.md`
   - `docs/...`
   - `site/...`
   - `web/README.md`
4. deploy and infrastructure notes
   - `deploy/...`

## Risks

### Risk: Partial Rename Drift

If docs, runtime defaults, and CI are not updated together, the repo becomes
even harder to reason about.

Mitigation:

- sweep active references in one change
- verify with a repo-wide search excluding build and archive outputs

### Risk: Hidden Compatibility Assumptions

Older tests or helper code may assert the exact old hostname.

Mitigation:

- update tests alongside defaults
- keep the alias working operationally outside the repo

### Risk: Over-Updating Historical Material

Some old docs are intentionally historical.

Mitigation:

- update active docs
- leave archive materials untouched unless they are still presented as current

## Verification Strategy

- run focused Rust tests that cover updated default-host behavior
- run `site` and `web` builds after link and copy updates
- run a repo-wide `rg` sweep excluding archive and build outputs to confirm
  active `packages.conary.io` references are gone or intentionally preserved
- keep `git diff --check` clean

## Acceptance Criteria

- Active code defaults use `remi.conary.io`.
- Active docs and site links present `remi.conary.io` as the primary public
  package hostname.
- CI deployment verification checks `remi.conary.io`.
- Remaining active `packages.conary.io` references are deliberate compatibility
  notes only, or there are none.
