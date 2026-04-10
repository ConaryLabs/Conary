# Documentation Accuracy Audit Summary

## Scope

Pending.

## Verification Commands

Pending.

## Major Corrections

- Root README badge now points at the real `pr-gate` workflow instead of a
  removed `ci.yml` workflow.
- Root source-build quick start now uses `./target/debug/conary` instead of
  assuming the freshly built CLI is already on `PATH`.
- README and CONTRIBUTING now describe the workspace correctly as seven members,
  including `crates/conary-bootstrap`.
- CONTRIBUTING and the PR template now use current verification commands and PR
  expectations instead of stale `cargo clippy -- -D warnings` /
  `cargo fmt -- --check` guidance.
- CHANGELOG now explains that legacy `server-v*` and `test-v*` headings are
  historical continuity markers, not current canonical release tags.
- SECURITY now describes the disclosure/triage process without unverifiable SLA
  promises.
- ARCHITECTURE now reflects schema v66 and includes `crates/conary-bootstrap`
  in both the system overview and workspace package map.
- The query module guide now reflects the real user-facing surface:
  `label` remains nested under `conary query`, while SBOM is a top-level
  `conary sbom` command backed by the query module internals.
- The CCS format spec now uses the current `conary ccs keygen/sign/verify`
  command names instead of the old standalone `ccs-*` tooling names.
- `docs/INTEGRATION-TESTING.md` and `apps/conary-test/README.md` now point at
  the workspace-correct `apps/conary/tests/integration/remi/...` paths, current
  Phase 2/4 suite coverage, and `<distro>-phase<N>.json` result filenames.
- `docs/SCRIPTLET_SECURITY.md` now reflects the current scriptlet executor:
  `crates/` paths, `RLIMIT_NPROC=1024`, target-root execution, and the modern
  `conary query scripts` inspection command.
- `docs/conaryopedia-v2.md` now matches the current Remi admin surface:
  loopback external admin bind on `127.0.0.1:8082`, unauthenticated `/health`
  and `/v1/admin/openapi.json`, and the real repo/federation/test-data/MCP
  endpoints.
- `deploy/CLOUDFLARE.md` and `deploy/FORGE.md` now use the current health-check
  behavior and workspace-correct container troubleshooting paths.
- The scriptlet harness and adversarial fixture READMEs now use
  workspace-correct paths and describe the current contained/live-root and
  tracked-large-fixture behavior.

## WIP Clarifications

- `bootstrap/stage0/README.md` now explicitly marks the checked-in
  `crosstool-ng` config as a historical reference. The supported bootstrap
  workflow is the `conary bootstrap ...` CLI surface, not `ct-ng build` in the
  `bootstrap/stage0/` directory.

## Archive/Delete Decisions

- Archived recent completed planning/design artifacts into tracked archive
  subtrees:
  - `docs/superpowers/plans/archive/2026-04-07-docs-source-selection-refresh-plan.md`
  - `docs/superpowers/plans/archive/2026-04-07-source-selection-program-plan.md`
  - `docs/superpowers/plans/archive/2026-04-09-forge-integration-hardening-plan.md`
  - `docs/superpowers/plans/archive/2026-04-09-release-matrix-realignment-plan.md`
  - `docs/superpowers/specs/archive/2026-04-07-source-selection-policy-design.md`
  - `docs/superpowers/specs/archive/2026-04-09-forge-integration-hardening-design.md`
  - `docs/superpowers/specs/archive/2026-04-09-release-matrix-realignment-design.md`
- No tracked planning/spec files were deleted in Chunk 1.

## Residual Risks

Pending.

## Final Counts

Pending.
