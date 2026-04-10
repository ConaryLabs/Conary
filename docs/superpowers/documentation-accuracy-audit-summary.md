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

## WIP Clarifications

Pending.

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
