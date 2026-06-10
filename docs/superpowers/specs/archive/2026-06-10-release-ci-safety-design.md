# Release And CI Safety Design

## Status

Ready for implementation via
`docs/superpowers/plans/archive/2026-06-10-release-ci-safety-plan.md`.

## Goal

Ensure release tags and direct pushes cannot bypass the basic validation that
protects ordinary pull requests, and make workflow policy checks fail closed
when unpinned GitHub Actions are introduced.

## Background

The repository currently has strong PR validation, but release builds are
triggered directly by tags and publish artifacts without an upstream workspace
test dependency in `.github/workflows/release-build.yml`. `merge-validation.yml`
builds release-facing binaries and checks docs, but it does not mirror PR fmt,
dependency consistency, Clippy, or workspace test gates.

The existing action-runtime checker confirms known pinned references are
present. It does not enumerate every `uses:` line and reject unpinned external
actions. It also does not cover every workflow with production-adjacent
credentials and deploy behavior.

## Policy Decision

Release and deploy automation should be boring:

- A live release tag cannot publish artifacts until a workspace validation job
  for the same SHA succeeds.
- Direct pushes to `main` should run the same basic safety classes as PRs, or
  the repository must explicitly rely on branch protection and document that
  policy.
- Every external GitHub Action reference must be pinned to a full commit SHA,
  unless it is a local action or a reviewed allowlist entry.
- Release matrix and deploy helper behavior tests should run in automation.

## Scope

This track owns workflow validation and policy scripts. It may touch GitHub
Actions workflow files, action pinning scripts, release matrix tests, deploy
helper tests, and script executable bits. It does not change release artifact
contents or Remi deploy integrity; those belong to Track 3.

## Implementation Shape

1. Add tests for the action pin checker that prove an unpinned `uses:` line
   fails.
2. Rewrite `scripts/check-github-action-runtimes.sh` to enumerate all workflow
   `uses:` references and reject unpinned external actions.
3. Include all workflows and `.github/actions/*/action.yml` files in that scan.
4. Add a release validation job in `release-build.yml` and make build/publish
   jobs depend on it.
5. Mirror PR fmt, dependency consistency, Clippy, and workspace test gates into
   `merge-validation.yml`, or add an explicit checked branch-protection
   precondition if the project chooses PR-only enforcement.
6. Wire `scripts/test-release-matrix.sh` and
   `scripts/test-remi-deploy-helper.sh` into CI.
7. Normalize executable bits for scripts that are meant to support
   `./scripts/...` invocation.

## Verification Strategy

Required gates:

- `bash scripts/check-github-action-runtimes.sh`
- a negative local fixture or temporary-copy test showing unpinned actions fail
- `bash scripts/check-release-matrix.sh`
- `bash scripts/test-release-matrix.sh`
- `bash scripts/test-remi-deploy-helper.sh`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --exclude conary-test --verbose`
- `cargo test -p conary-test --verbose`
- docs-audit and docs-truth checks if workflow docs change

## Non-Goals

- Do not redesign artifact signing or deploy checksum behavior in this track.
- Do not require QEMU/KVM integration suites on hosted runners.
- Do not add broad new release products.
