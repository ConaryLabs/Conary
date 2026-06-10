# Deploy And Artifact Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Remi deploy helpers verify CI-produced artifacts and remove stale deploy paths that contradict supported operator workflows.

**Architecture:** Test the deploy helper through its fake-root harness, then change helper behavior. Keep release matrix policy checks close to workflow routing, and update operator docs only after behavior is settled.

**Tech Stack:** Bash, GitHub Actions YAML, release matrix scripts, docs-audit.

---

## Design Source

- `docs/superpowers/specs/archive/2026-06-10-deploy-artifact-integrity-design.md`

## File Map

| Path | Purpose |
| --- | --- |
| `deploy/remi-deploy-helper.sh` | Verify checksum and signature inputs before install. |
| `scripts/test-remi-deploy-helper.sh` | Add fake-root deploy integrity tests. |
| `.github/workflows/deploy-and-verify.yml` | Remove or label unreachable conaryd deploy lanes and retire old exceptions. |
| `scripts/check-release-matrix.sh` | Detect deploy jobs that conflict with `deploy_mode=none`. |
| `scripts/test-release-matrix.sh` | Test release matrix deploy-mode policy. |
| `scripts/rebuild-remi.sh` | Delete, archive, or convert to a supported-path pointer. |
| `deploy/.credentials.toml` | Ignored host-local credentials file. Inspect only if needed; do not stage or require tracked edits here. |
| `docs/operations/infrastructure.md` | Name the supported Remi deployment path. |
| `docs/operations/release-artifact-matrix.md` | Keep artifact integrity status honest. |

## Task 0: Baseline

- [ ] Run:

```bash
bash scripts/test-remi-deploy-helper.sh
bash scripts/check-release-matrix.sh
bash scripts/test-release-matrix.sh
```

Expected: all pass before edits.

## Task 1: Add Deploy-Helper Integrity Tests

- [ ] In `scripts/test-remi-deploy-helper.sh`, add a fake staging bundle with a `SHA256SUMS` file that matches the bundle contents.
- [ ] Add a positive assertion that `deploy/remi-deploy-helper.sh deploy-conary 0.8.0 "$staging"` succeeds when checksums and `.ccs.sig` are present.
- [ ] Add a negative assertion that corrupting one staged file after writing `SHA256SUMS` fails.
- [ ] Add a negative assertion that a staged `.ccs` without `.ccs.sig` fails on the live release path.
- [ ] Run `bash scripts/test-remi-deploy-helper.sh`.
  Expected before implementation: at least the new negative tests expose current behavior.

## Task 2: Verify Checksums Instead Of Regenerating Trust

- [ ] In `deploy/remi-deploy-helper.sh`, require `SHA256SUMS` in the staging directory for release installs.
- [ ] Run `sha256sum -c SHA256SUMS` from the staging directory before copying files into the release directory.
- [ ] Stop regenerating `SHA256SUMS` from installed files as the source of trust. If the installed release directory needs a copy, install the verified input file.
- [ ] Reject symlinked checksum and signature inputs.
- [ ] Run `bash scripts/test-remi-deploy-helper.sh`.

## Task 3: Require CCS Signatures Where Policy Requires Them

- [ ] In `deploy/remi-deploy-helper.sh`, when a staged `.ccs` exists, require a sibling `.ccs.sig`.
- [ ] Install both into the self-update directory.
- [ ] Keep dry-run or rehearsal behavior explicit if it uses deterministic signing keys.
- [ ] Run `bash scripts/test-remi-deploy-helper.sh`.

## Task 4: Clean Up Deploy Routing Drift

- [ ] In `scripts/check-release-matrix.sh`, add a check: products with `deploy_mode=none` must not have live `deploy-<product>:` or `verify-<product>:` jobs unless the job is explicitly labeled as paused future wiring.
- [ ] Add matching tests to `scripts/test-release-matrix.sh` for `conaryd` and `conary-test`.
- [ ] Decide whether to remove `verify-conaryd` and `deploy-conaryd` from `.github/workflows/deploy-and-verify.yml` or annotate them as paused and covered by the new policy.
- [ ] Retire the `24273700060` one-off exception if no current workflow needs it.
- [ ] Run `bash scripts/check-release-matrix.sh` and `bash scripts/test-release-matrix.sh`.

## Task 5: Remove Or Reframe Legacy Remi Deploy Paths

- [ ] Inspect `scripts/rebuild-remi.sh`.
- [ ] If it contradicts `conary-remi-deploy`, replace its body with a short failure message pointing to the supported helper.
- [ ] If it remains supported for local-only development, update the header and docs to say so.
- [ ] Inspect `deploy/.credentials.toml` if present for stale comments that mention retired `--features server` or old binary names. Because the file is ignored and may contain host-local credentials, do not stage it; capture any durable correction in tracked operator docs or templates instead.
- [ ] Update `docs/operations/infrastructure.md` and `docs/operations/release-artifact-matrix.md`.

## Task 6: Avoid Double Restart When Deploying Remi

- [ ] Trace the current `deploy-and-verify.yml` Remi remote block.
- [ ] If it still runs `deploy-remi` followed by `configure-concurrency`, either:
  - make `configure-concurrency` skip restart when called in the same remote transaction; or
  - move config update before the deploy helper's restart.
- [ ] Add a helper test that counts restart calls in fake-root mode if the helper supports a fake `systemctl`.

## Task 7: Final Verification And Commit

- [ ] Run:

```bash
bash scripts/test-remi-deploy-helper.sh
bash scripts/check-release-matrix.sh
bash scripts/test-release-matrix.sh
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

- [ ] Commit:

```bash
git add deploy/remi-deploy-helper.sh scripts/test-remi-deploy-helper.sh scripts/check-release-matrix.sh scripts/test-release-matrix.sh .github/workflows/deploy-and-verify.yml scripts/rebuild-remi.sh docs/operations/infrastructure.md docs/operations/release-artifact-matrix.md
git commit -m "fix(deploy): verify remi release artifacts"
```

Only stage paths that changed.
