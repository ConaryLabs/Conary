# Validator Soundness Follow-Up Design

## Status

Ready for implementation via
`docs/superpowers/plans/archive/2026-06-10-validator-soundness-follow-up-plan.md`.

## Goal

Close remaining fail-open edges in local validators after the evidence and
release gates are addressed, without turning the shell validators into a broad
static-analysis framework.

## Background

Recent work already hardened several validator paths, including coherency ledger
date validation, owner validation, completed-scope validation, and docs-truth
tests in `merge-validation.yml`. External review still identified smaller
coverage gaps:

- Some route extraction only handles a limited Axum method shape.
- There is no full documented `conary` CLI command existence check.
- Some validators rely on expected path lists or keyword presence rather than
  stronger structure checks.

## Policy Decision

Validators should fail closed for the narrow invariants they claim to protect:

- If a required scan path disappears, the validator reports that path.
- If a route registration uses a supported method, docs-truth extracts it.
- If route extraction sees an unsupported but obvious route shape, it should
  fail with an explanation rather than silently dropping it.
- CLI doc checking should start with constrained command references, not a
  natural-language parser for all docs.

## Scope

This track owns `scripts/check-doc-truth.sh`,
`scripts/check-coherency-ledger.sh`, `scripts/check-coherency-wave-scopes.sh`,
`scripts/check-release-matrix.sh`, and their tests where paired tests already
exist or are added by this track. It should not absorb Track 1 result-gate work,
Track 2 action-pin work, or Track 3 deploy-mode consistency checks.

## Implementation Shape

1. Add fixtures for each validator failure shape before changing scripts.
2. Derive conaryd route files from `apps/conaryd/src/daemon/routes/*.rs` or keep
   a documented allowlist that fails when new route files appear.
3. Support `PUT`, `PATCH`, and chained Axum route handlers in docs-truth route
   extraction.
4. Add a constrained `conary` command reference check for docs that contain
   backtick-style `conary <command>` snippets.
5. Strengthen release matrix checks where they only prove keyword presence.
6. Keep every new validator rule paired with a test script fixture.

## Verification Strategy

Required gates:

- `bash scripts/test-doc-truth.sh`
- `bash scripts/check-doc-truth.sh`
- `bash scripts/test-coherency-ledger.sh`
- `bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv`
- `bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv`
- `bash scripts/test-release-matrix.sh`
- `bash scripts/check-release-matrix.sh`
- docs-audit inventory and ledger checks
- `git diff --check`

## Non-Goals

- Do not parse arbitrary Markdown prose into command or route semantics.
- Do not require generated artifacts to be present in a fresh checkout unless
  the validator also generates them.
- Do not duplicate Rust compiler or Clippy responsibilities in shell.
