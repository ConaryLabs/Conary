# Deploy And Artifact Integrity Design

## Status

Ready for implementation via
`docs/superpowers/plans/archive/2026-06-10-deploy-artifact-integrity-plan.md`.

## Goal

Make deploy helpers verify the artifacts produced by CI instead of
re-checksumming whatever arrives on the host, and remove stale or unreachable
deploy paths that mislead operators and agents.

## Background

External review identified three deploy-integrity concerns:

- `deploy/remi-deploy-helper.sh` regenerates `SHA256SUMS` in the installed
  release directory.
- CCS signatures are copied only when present, even when the release path says
  signed CCS artifacts are mandatory.
- `deploy-and-verify.yml` contains conaryd deploy and verify jobs gated on
  `deploy_mode == 'remote_bundle'`, while `scripts/release-matrix.sh` marks
  `conaryd` as `deploy_mode=none`.

The Remi deploy workflow also runs `deploy-remi` and then
`configure-concurrency`, which can restart the service twice. Some older helper
scripts and credential comments still describe retired Remi deployment shapes.

## Policy Decision

Deploy paths should prove provenance instead of rebuilding trust on the target:

- CI-produced checksums are input evidence and must be verified before install.
- A missing required signature is a deploy failure, not a warning.
- A product with `deploy_mode=none` should not have live-looking deploy jobs
  unless they are explicitly documented as paused future wiring and checked by
  policy.
- Operator docs should name one supported Remi deploy path.

## Scope

This track owns deploy helpers, deploy workflow routing, release matrix
consistency checks, deploy helper tests, credential templates, and operator
deployment docs. It does not add a replacement conaryd staging host.

## Implementation Shape

1. Extend deploy-helper tests with checksum verification and missing-signature
   failure cases.
2. Change `deploy/remi-deploy-helper.sh` to verify shipped `SHA256SUMS` before
   installing release files.
3. Require `.ccs.sig` when a `.ccs` artifact is present on a live release path.
4. Decide whether Remi config updates should be applied before the service
   restart or through a `--skip-restart` helper mode.
5. Decide whether `scripts/rebuild-remi.sh` is deleted, archived, or rewritten
   as a pointer to `/usr/local/sbin/conary-remi-deploy`.
6. Retire the one-off conaryd bootstrap deploy exception if it is no longer
   needed.
7. Add release-matrix checks for unreachable deploy jobs.
8. Update deploy docs and credential comments to match the supported path.

## Verification Strategy

Required gates:

- `bash scripts/test-remi-deploy-helper.sh`
- `bash scripts/check-release-matrix.sh`
- `bash scripts/test-release-matrix.sh`
- targeted shell fixture proving checksum mismatch fails
- targeted shell fixture proving missing required CCS signature fails
- `bash scripts/check-doc-truth.sh`
- docs-audit inventory and ledger checks
- `git diff --check`

## Non-Goals

- Do not re-enable conaryd remote deployment.
- Do not change the release build artifact matrix beyond integrity metadata.
- Do not touch hosted secrets or host-local credential files.
