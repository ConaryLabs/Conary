# All-Tracks Release Hardening Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute an evidence-backed, ship-blocker-only release-hardening pass for the `conary`, `remi`, `conaryd`, and `conary-test` release tracks, fix release-surface blockers, narrow the release if needed, and then run the approved `version bump -> tag -> push` flow.

**Architecture:** Follow the approved spec in strict order: preflight and release-matrix capture, local Rust and frontend validation, release-facing truthfulness audit, GitHub dry-run rehearsal, secrets/readiness confirmation, go/no-go decision, then the real release cut. Keep one tracked checklist document as the execution ledger, and only modify release-surface files called out by the spec unless a command failure forces a focused follow-up fix plan.

**Tech Stack:** Bash, Git, Cargo, npm, SvelteKit, GitHub CLI (`gh`), GitHub Actions, Markdown, ripgrep (`rg`), repo-local release scripts, and the checked-in release/deploy workflows.

**Commit Convention:** Every non-release-fix commit in this plan should reference `docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md` in the commit body.

---

## Scope Guard

- This plan is for release hardening and release execution, not general cleanup.
- If a validation failure requires broad product code changes outside the release-surface files listed below, stop and write a focused fix plan for that blocker instead of improvising a large repair inside this release plan.
- Do not delete or overwrite unrelated untracked files without explicit human approval.
- Keep release-surface fixes separated by concern:
  - docs/UI truthfulness fixes
  - release/workflow plumbing fixes
  - final release commit produced by `./scripts/release.sh`
- If any track fails the ship-blocker gates and cannot be repaired quickly, drop it from the coordinated release and continue only with the approved subset.

## File Map

| File | Responsibility |
|------|----------------|
| `docs/superpowers/specs/2026-04-10-all-tracks-release-hardening-design.md` | Approved design/spec that defines the hardening pass and go/no-go rules |
| `docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md` | This implementation plan |
| `docs/superpowers/release-hardening-checklist-2026-04-10.md` | Execution ledger for pass/fail/waived status, release matrix capture, blockers, fixes, and final release commands |
| `scripts/release.sh` | Local version bump, changelog, commit, and tag entrypoint |
| `scripts/release-matrix.sh` | Product metadata, canonical tags, bundle names, deploy modes, and owned manifest mapping |
| `scripts/test-release-matrix.sh` | Regression test for release-matrix behavior |
| `scripts/check-release-matrix.sh` | Workflow/release-matrix consistency gate |
| `.github/workflows/release-build.yml` | GitHub release-build dry-run and live artifact workflow |
| `.github/workflows/deploy-and-verify.yml` | GitHub deploy/verify dry-run and live deployment workflow |
| `README.md` | Release badge, release summary, and first-touch project claims |
| `site/src/routes/install/+page.svelte` | Public install page copy and sample version output |
| `site/src/routes/compare/+page.svelte` | Public comparison page release/version claims |
| `web/src/routes/+layout.svelte` | Package frontend top-level install/deep-link entrypoints |
| `web/src/routes/+page.svelte` | Package frontend landing-page release-facing copy |
| `apps/conary/man/conary.1` | Checked-in release-facing manpage content |
| `site/package.json` | Public site build/check commands |
| `web/package.json` | Package frontend build/check commands |
| `apps/conary/src/commands/self_update.rs` | Current CLI self-update verification path reference |
| `crates/conary-core/src/self_update.rs` | Current update-signature verification implementation |

## Chunk 1: Preflight And Release Matrix

### Task 1: Capture the untouched worktree baseline, then create the checklist and artifact workspace

**Files:**
- Create: `docs/superpowers/release-hardening-checklist-2026-04-10.md`

- [ ] **Step 1: Capture the untouched worktree baseline before creating or committing anything**

Run:

```bash
mkdir -p /tmp/conary-release-hardening-2026-04-10
git status --short | tee /tmp/conary-release-hardening-2026-04-10/initial-git-status.txt
```

Expected:
- the saved file captures the real pre-plan worktree state
- unrelated untracked files, if any, are visible before any new checklist file or commit changes the repo state

- [ ] **Step 2: Create the checklist scaffold**

Create `docs/superpowers/release-hardening-checklist-2026-04-10.md` with these sections:

- Scope
- Phase 1 Release Matrix
- Local Gates
- Public-Surface Audit
- GitHub Dry-Run Rehearsal
- Secrets And Environment Readiness
- Blockers
- Fixes Made
- Release Decision
- Final Commands

Under `Release Decision`, include these exact fields:

- `Approved Tracks:`
- `Dropped Tracks:`
- `Blocked Tracks:`
- `Final Release Command:`

Include a table under `Phase 1 Release Matrix` with columns:

```text
track | current_tag | next_version | next_tag | bundle_name | deploy_mode | decision | notes
```

- [ ] **Step 3: Create the local artifact workspace**

Run:

```bash
mkdir -p /tmp/conary-release-hardening-2026-04-10/runs
mkdir -p /tmp/conary-release-hardening-2026-04-10/artifacts
```

Expected: all three directories exist and are empty or reusable.

- [ ] **Step 4: Copy the untouched baseline into the checklist**

Copy the contents of `/tmp/conary-release-hardening-2026-04-10/initial-git-status.txt`
into the checklist under `Scope` or `Blockers`, and for every untracked file
record one of:

- `approved to ignore during release hardening`
- `must be removed or moved before release`

- [ ] **Step 5: Commit the checklist scaffold**

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs: add release hardening checklist" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
```

### Task 2: Capture the release-matrix baseline

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read: `scripts/release.sh`
- Read: `scripts/release-matrix.sh`

- [ ] **Step 1: Run release-matrix self-tests**

Run:

```bash
bash scripts/test-release-matrix.sh
bash scripts/check-release-matrix.sh
```

Expected:
- `scripts/test-release-matrix.sh` exits `0`
- `scripts/check-release-matrix.sh` prints `Release matrix workflow checks passed.`

- [ ] **Step 2: Capture the coordinated release dry-run baseline**

Run:

```bash
./scripts/release.sh all --dry-run | tee /tmp/conary-release-hardening-2026-04-10/release-all-dry-run.txt
```

Expected:
- one `=== Releasing: <product> ===` block per track considered
- explicit `Current`, `Next version`, `Tag`, `Bundle`, and `Deploy mode` lines for releasable tracks
- explicit skip output for unreleasable tracks

- [ ] **Step 3: Transcribe the release matrix into the checklist**

From `/tmp/conary-release-hardening-2026-04-10/release-all-dry-run.txt`, record for each track:

- `track`
- `current_tag`
- `next_version`
- `next_tag`
- `bundle_name`
- `deploy_mode`
- `decision` = `candidate` or `drop-from-release`
- `notes`

Rule:
- if a track does not bump under `./scripts/release.sh all --dry-run`, mark it `drop-from-release` unless the human explicitly approves an override policy

- [ ] **Step 4: Commit the recorded release baseline**

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs: record release hardening baseline" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
```

## Chunk 2: Local Validation Gates

### Task 3: Run Rust format, lint, build, and owning-package tests

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read: `Cargo.toml`
- Read: `AGENTS.md`

- [ ] **Step 1: Run formatting and lint gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected:
- both commands exit `0`
- `clippy` ends without warnings or errors

- [ ] **Step 2: Run release builds for all app tracks**

Run:

```bash
cargo build -p conary --release
cargo build -p remi --release
cargo build -p conaryd --release
cargo build -p conary-test --release
```

Expected:
- each command exits `0`
- Cargo prints `Finished 'release' profile` for each target

- [ ] **Step 3: Run owning-package tests and harness inventory**

Run:

```bash
cargo test -p conary
cargo test -p conary-core
cargo test -p remi
cargo test -p conaryd
cargo test -p conary-test
cargo run -p conary-test -- list
```

Expected:
- each `cargo test` ends with `test result: ok`
- `cargo run -p conary-test -- list` exits `0` and prints the suite inventory

- [ ] **Step 4: Record results and stop on non-release-surface failures**

Action:
- mark each Rust gate `pass` or `fail` in the checklist
- if a failure is limited to release-plumbing files, continue to Task 6 below after capturing evidence
- if a failure requires broad product code changes outside the release-surface files in this plan, stop and write a focused fix plan before continuing

### Task 4: Run the public frontend validation gates

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read: `site/package.json`
- Read: `web/package.json`

- [ ] **Step 1: Validate the main site workspace**

Run:

```bash
(cd site && set -euo pipefail && npm ci && npm run check && npm run build)
```

Expected:
- `npm ci` completes successfully using `site/package-lock.json`
- `npm run check` exits `0`
- `npm run build` exits `0`

- [ ] **Step 2: Validate the package frontend workspace**

Run:

```bash
(cd web && set -euo pipefail && npm ci && npm run check && npm run build)
```

Expected:
- `npm ci` completes successfully using `web/package-lock.json`
- `npm run check` exits `0`
- `npm run build` exits `0`

- [ ] **Step 3: Record results**

Action:
- mark both frontend gates `pass` or `fail` in the checklist
- if either workspace fails only because release-facing copy needs edits in the files listed in Task 5, continue into Task 5
- otherwise stop and write a focused fix plan before continuing

## Chunk 3: Public-Surface Truthfulness Audit And Release-Surface Fixes

### Task 5: Sweep release-facing copy for stale versions and misleading claims

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read or Modify: `README.md`
- Read or Modify: `site/src/routes/install/+page.svelte`
- Read or Modify: `site/src/routes/compare/+page.svelte`
- Read or Modify: `web/src/routes/+layout.svelte`
- Read or Modify: `web/src/routes/+page.svelte`
- Read or Modify: `apps/conary/man/conary.1`

- [ ] **Step 1: Run the release-surface grep sweep**

Substitute the actual current and next versions recorded in the checklist; do
not run the literal placeholder strings. If `rg` is unavailable, use
`grep -rnE` with the same pattern and paths.

```bash
CONARY_CURRENT="<value from checklist>"
CONARY_NEXT="<value from checklist>"
REMI_CURRENT="<value from checklist>"
REMI_NEXT="<value from checklist>"
rg -n "${CONARY_CURRENT}|${CONARY_NEXT}|${REMI_CURRENT}|${REMI_NEXT}|version-|Release |Conary is a " \
  README.md site web apps/conary/man -g '!**/archive/**' \
  | tee /tmp/conary-release-hardening-2026-04-10/release-surface-grep.txt
```

Expected:
- every exact-version hit is captured in one file for manual review
- the grep results point to a short, auditable list of release-facing files

- [ ] **Step 2: Review the known release-facing files manually**

Review these exact files line by line against the approved release decision:

- `README.md`
- `site/src/routes/install/+page.svelte`
- `site/src/routes/compare/+page.svelte`
- `web/src/routes/+layout.svelte`
- `web/src/routes/+page.svelte`
- `apps/conary/man/conary.1`

Decision rule:
- if a file contains a stale hardcoded version or a misleading release/install claim, fix it now
- if a file is honest but outdated only in historical or test-fixture context, record it as non-blocking and leave it alone

- [ ] **Step 3: Rebuild after any release-surface copy fixes**

If any of the files above changed, rerun only the affected validation commands:

```bash
set -euo pipefail
cargo build -p conary --release
(cd site && npm run check && npm run build)
(cd web && npm run check && npm run build)
```

Expected:
- all rerun commands exit `0`

- [ ] **Step 4: Commit the release-surface fixes**

If any release-facing file changed:

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git add -u -- README.md site web apps/conary/man
git diff --cached --name-only
git commit -m "docs: refresh release-facing copy for release hardening" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
```

If no file changed, stage only the checklist update and commit:

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs: record release-surface audit" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
```

### Task 6: Repair release-plumbing blockers if the local gates exposed them

**Files:**
- Modify only if local validation or dry-runs require it:
  - `scripts/release.sh`
  - `scripts/release-matrix.sh`
  - `scripts/test-release-matrix.sh`
  - `scripts/check-release-matrix.sh`
  - `.github/workflows/release-build.yml`
  - `.github/workflows/deploy-and-verify.yml`
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`

- [ ] **Step 1: Decide whether a release-plumbing fix is in scope**

Only continue if a failure from Chunk 1 or Chunk 2 is confined to:

- release matrix resolution
- workflow metadata serialization
- workflow artifact routing
- release-bundle naming
- deploy-mode routing

If the failure is not confined to those concerns, stop and write a focused fix plan.

- [ ] **Step 2: Apply the minimal release-plumbing fix and rerun the owning gate**

After each fix, rerun the exact command that failed:

```bash
bash scripts/test-release-matrix.sh
bash scripts/check-release-matrix.sh
./scripts/release.sh all --dry-run
```

If the failure was workflow-related, rerun the local gate first and then continue into Chunk 4 for the GitHub dry-run rehearsal.

- [ ] **Step 3: Commit the release-plumbing fix**

```bash
git add scripts/release.sh scripts/release-matrix.sh scripts/test-release-matrix.sh scripts/check-release-matrix.sh .github/workflows/release-build.yml .github/workflows/deploy-and-verify.yml docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "fix(release): repair release hardening blocker" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
```

## Chunk 4: GitHub Dry-Run Rehearsal

### Task 7: Rehearse `release-build` for each candidate track

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read: `.github/workflows/release-build.yml`
- Read: `scripts/release-matrix.sh`

- [ ] **Step 1: Dispatch one dry-run `release-build` per candidate track**

For each track still marked `candidate`, run:

```bash
gh workflow run release-build.yml --ref main -f product=<TRACK> -f tag_name=<NEXT_TAG> -f dry_run=true
```

Then wait for GitHub to register the workflow run and capture the newest
workflow-dispatch run ID:

```bash
sleep 10
gh run list --workflow release-build.yml --event workflow_dispatch --limit 5 --json databaseId,displayTitle,status,conclusion,createdAt > /tmp/conary-release-hardening-2026-04-10/runs/release-build-<TRACK>.json
jq '.[0]' /tmp/conary-release-hardening-2026-04-10/runs/release-build-<TRACK>.json
jq -r '.[0].databaseId' /tmp/conary-release-hardening-2026-04-10/runs/release-build-<TRACK>.json
```

Expected:
- `gh workflow run` exits `0`
- the saved JSON contains a newest run with a non-empty `databaseId`
- the newest run is visibly the just-dispatched `workflow_dispatch` run for the
  expected workflow, not an older completed run

- [ ] **Step 2: Wait for each dry-run to finish**

For each captured run ID:

```bash
gh run watch <RUN_ID> --exit-status
```

Expected:
- each candidate track run exits `0`

- [ ] **Step 3: Record run IDs in the checklist**

For each candidate track, record:

- `release-build` run ID
- final conclusion
- artifact directory target under `/tmp/conary-release-hardening-2026-04-10/artifacts/`

### Task 8: Download and validate `release-build` artifacts

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read: `.github/workflows/release-build.yml`
- Read: `apps/conary/src/commands/self_update.rs`
- Read: `crates/conary-core/src/self_update.rs`

- [ ] **Step 1: Download one artifact bundle per candidate track**

For each candidate track:

```bash
gh run download <RUN_ID> --dir /tmp/conary-release-hardening-2026-04-10/artifacts/<TRACK>
find /tmp/conary-release-hardening-2026-04-10/artifacts/<TRACK> -maxdepth 3 -type f | sort
```

Expected:
- each run downloads successfully
- the artifact tree contains `metadata.json`
- the artifact tree contains the product's expected bundle artifact(s)

- [ ] **Step 2: Validate serialized metadata for each candidate track**

For each candidate track:

```bash
metadata_file=$(find /tmp/conary-release-hardening-2026-04-10/artifacts/<TRACK> -name metadata.json -print -quit)
jq '{product,version,tag_name,bundle_name,deploy_mode,dry_run}' "$metadata_file"
```

Expected:
- `product`, `version`, `tag_name`, `bundle_name`, and `deploy_mode` match the checklist
- `dry_run` is `"true"` or `true` as modeled by the workflow

- [ ] **Step 3: Validate the primary artifact names**

Check for these exact patterns:

- `conary`: `*.ccs`, `*.rpm`, `*.deb`, `*.pkg.tar.zst`, `SHA256SUMS`, and optional `*.sig`
- `remi`: `remi-<VERSION>-linux-x64.tar.gz`
- `conaryd`: `conaryd-<VERSION>-linux-x64.tar.gz`
- `conary-test`: `conary-test-<VERSION>-linux-x64.tar.gz`

Use:

```bash
find /tmp/conary-release-hardening-2026-04-10/artifacts/<TRACK> -type f | sort
```

Expected:
- the track's expected primary artifact filenames exist

- [ ] **Step 4: Validate `conary` checksums**

Run:

```bash
conary_bundle_dir=$(dirname "$(find /tmp/conary-release-hardening-2026-04-10/artifacts/conary -name SHA256SUMS -print -quit)")
(cd "$conary_bundle_dir" && sha256sum -c SHA256SUMS)
```

Expected:
- every line ends with `OK`

- [ ] **Step 5: Attempt the self-update signature rehearsal**

First inspect the current supported verification path:

```bash
sed -n '1,220p' apps/conary/src/commands/self_update.rs
sed -n '1,140p' crates/conary-core/src/self_update.rs
```

Then decide:

- `crates/conary-core/src/self_update.rs` currently declares
  `TRUSTED_UPDATE_KEYS` as an empty slice, so unless that changes before
  execution, the expected outcome is `signature rehearsal incomplete`

- if there is a repo-supported operator path to feed the downloaded `sha256` and `*.sig` into the existing self-update verification flow, run it and record the exact command and output in the checklist
- if there is no operator-usable path without writing new code, record
  `signature rehearsal incomplete` in the checklist, mark the release `no-go`
  per the spec, and stop for explicit human override before any live cut work

Do **not** hand-wave this step. Either exercise the current path or stop.

- [ ] **Step 6: Commit the dry-run artifact evidence**

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs: record release dry-run evidence" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
```

### Task 9: Rehearse `deploy-and-verify` for the deployable tracks

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read: `.github/workflows/deploy-and-verify.yml`

- [ ] **Step 1: Dispatch one dry-run deploy rehearsal per deployable candidate track**

For each candidate track in `{conary, remi, conaryd}`:

```bash
gh workflow run deploy-and-verify.yml --ref main -f product=<TRACK> -f source_run=<RELEASE_BUILD_RUN_ID> -f environment=production -f dry_run=true
```

Then wait for GitHub to register the workflow run and capture the newest run
ID. In `dry_run=true`, only the verification path should execute; deploy jobs
with environment bindings should remain skipped.
`--ref main` is intentional here: this rehearsal is using the
`workflow_dispatch` inputs path, while the live chain uses the `workflow_run`
trigger after `release-build` completes.

```bash
sleep 10
gh run list --workflow deploy-and-verify.yml --event workflow_dispatch --limit 5 --json databaseId,displayTitle,status,conclusion,createdAt > /tmp/conary-release-hardening-2026-04-10/runs/deploy-and-verify-<TRACK>.json
jq '.[0]' /tmp/conary-release-hardening-2026-04-10/runs/deploy-and-verify-<TRACK>.json
jq -r '.[0].databaseId' /tmp/conary-release-hardening-2026-04-10/runs/deploy-and-verify-<TRACK>.json
```

- [ ] **Step 2: Wait for each deploy rehearsal to finish**

For each captured run ID:

```bash
gh run watch <RUN_ID> --exit-status
```

Expected:
- each deployable dry-run exits `0`

- [ ] **Step 3: Record the routing results**

Action:
- record each `deploy-and-verify` run ID and conclusion in the checklist
- record the exact `source_run=<RELEASE_BUILD_RUN_ID>` value used for each
  deploy rehearsal and confirm it matches the candidate track's recorded
  `release-build` run ID from Task 7
- note that `conary-test` is intentionally excluded because `deploy_mode=none`

## Chunk 5: Secrets Readiness, Release Decision, And Live Cut

### Task 10: Verify secret presence and usability confirmation

**Files:**
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`
- Read: `.github/workflows/release-build.yml`
- Read: `.github/workflows/deploy-and-verify.yml`

- [ ] **Step 1: Check repo-level and production-environment secret presence if access exists**

Run:

```bash
gh secret list
gh secret list --env production
```

Expected:
- `RELEASE_SIGNING_KEY` is present in the repo-level secret list if the operator has access
- deploy secrets are present in the `production` environment secret list if the operator has access

- [ ] **Step 2: If the operator cannot inspect secrets directly, get explicit usability confirmation**

Required confirmations:

- `RELEASE_SIGNING_KEY`
- `REMI_SSH_KEY`
- `REMI_SSH_TARGET`
- `CONARYD_SSH_KEY`
- `CONARYD_SSH_TARGET`
- `CONARYD_VERIFY_URL`

Record in the checklist one of:

- `verified directly`
- `confirmed by environment owner`
- `not confirmed`

Rule:
- any `not confirmed` entry makes the release `no-go`

- [ ] **Step 3: Decide the approved release set**

Use the checklist evidence and mark each track:

- `approved`
- `dropped`
- `blocked`

Then write the exact final release command set into the checklist, choosing one of:

```bash
./scripts/release.sh all
```

```bash
./scripts/release.sh conary remi
```

```bash
./scripts/release.sh remi
```

No other release command syntax is allowed.

- [ ] **Step 4: Commit the release decision**

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs: record release decision" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
```

### Task 11: Cut the approved release and push it

**Files:**
- Modify via `./scripts/release.sh`: owned manifests, `Cargo.lock`, optional `CHANGELOG.md`, and release tags for the approved track set
- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`

- [ ] **Step 1: Verify the worktree is ready for the release script**

Run:

```bash
git status --short
```

Expected:
- only intentional tracked changes remain
- no unresolved blockers remain in the checklist

- [ ] **Step 2: Execute the approved release command**

Run exactly the command recorded in the checklist, for example:

```bash
./scripts/release.sh all
```

Expected:
- the script updates owned manifests
- the script updates `Cargo.lock`
- the script creates the release commit(s)
- the script creates canonical tag(s)

- [ ] **Step 3: Inspect the release result before pushing**

Run:

```bash
git status --short
git show --stat --decorate --summary HEAD
git tag --points-at HEAD
git tag --points-at HEAD | grep -E '^(v|remi-v|conaryd-v|conary-test-v)'
```

Expected:
- `git status --short` is clean after the release script
- `git show` describes the release commit generated by the script
- `git tag --points-at HEAD` shows the canonical tag(s) just created
- the grep command confirms the tag names are in canonical format

- [ ] **Step 4: Push the release commit and tags**

Run:

```bash
git fetch origin
git status -sb
git push
git push --tags
```

Expected:
- `git fetch origin` exits `0`
- `git status -sb` shows the expected local branch relationship before pushing
- both pushes exit `0`

Rule:
- if `git status -sb` shows the local branch is behind or diverged from the
  remote branch, stop and re-confirm the correct push target before proceeding

- [ ] **Step 5: Capture and watch the live GitHub workflows**

After the pushes complete, wait for GitHub to register the new runs and capture
their IDs before watching them.

Run:

```bash
sleep 10
gh run list --workflow release-build.yml --limit 20 --json databaseId,displayTitle,event,createdAt,status,conclusion > /tmp/conary-release-hardening-2026-04-10/runs/live-release-build.json
gh run list --workflow deploy-and-verify.yml --limit 20 --json databaseId,displayTitle,event,createdAt,status,conclusion > /tmp/conary-release-hardening-2026-04-10/runs/live-deploy-and-verify.json
jq '.[0:5]' /tmp/conary-release-hardening-2026-04-10/runs/live-release-build.json
jq '.[0:5]' /tmp/conary-release-hardening-2026-04-10/runs/live-deploy-and-verify.json
```

Then record the relevant live run IDs in the checklist and watch them:

```bash
gh run watch <LIVE_RELEASE_BUILD_RUN_ID> --exit-status
gh run watch <LIVE_DEPLOY_RUN_ID> --exit-status
```

Expected:
- live release-build succeeds for each approved track
- live deploy-and-verify succeeds for each approved deployable track

- [ ] **Step 6: Record the final outcome**

Update the checklist with:

- approved tracks actually released
- pushed commit SHA(s)
- pushed tag(s)
- live workflow run IDs
- final status: `released` or `stopped`

- [ ] **Step 7: Commit the final checklist state**

If the release completed:

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs: finalize release hardening evidence" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
git push
```

## Halt And Resume

If execution stops at a blocker before the live cut:

- update the checklist immediately with the failing step, command output
  location, and blocker classification
- commit the checklist before walking away
- do not leave staged-but-uncommitted release-surface edits in the worktree
- if exploratory edits were made but not committed, either commit them as a
  focused blocker fix or stash them with a note in the checklist before pausing

When resuming:

- start with `git status --short`
- confirm the checklist reflects the current repo state
- restore any intentionally stashed work
- rerun the failed step and continue from there rather than skipping ahead

If the release did not complete, still commit the final checklist state with:

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs: record halted release hardening pass" -m "Refs docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md"
git push
```

---

## Completion Criteria

This plan is complete when:

- the checklist exists and records every hardening phase
- every local gate is marked `pass`, `fail`, or `waived`
- every candidate track has a recorded release decision
- every required GitHub dry-run has a recorded run ID and conclusion
- secret presence/usability is confirmed or the release is explicitly halted
- the approved release command has either been executed successfully or the pass
  has been stopped with blockers recorded
- the final checklist state is committed
