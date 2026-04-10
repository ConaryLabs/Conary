# Release Matrix Realignment Implementation Plan

> **Historical note:** This archived implementation plan is preserved for
> traceability. It reflects the intended work and repository state at the time
> it was written, not the current execution contract. Use active docs under
> `docs/` and non-archived `docs/superpowers/` for current guidance.

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current ad hoc release/tag/version flow with a shared release matrix that supports `conary`, `remi`, `conaryd`, and `conary-test`, preserves legacy `server-v*` and `test-v*` lineage for history lookup, and leaves the repository ready for the next intentional release/tag bump.

**Architecture:** Add one checked-in shell helper under `scripts/` that defines every release track in one place and exposes machine-consumable subcommands for tag resolution, version comparison, owned-manifest lists, and workflow metadata. Refactor `scripts/release.sh` and the GitHub release workflows to consume that helper instead of hardcoded arrays and `case` trees. Handle the `conary-test`/`conary-mcp` mismatch as an explicit migration step, then protect the release path with shell verification scripts and a PR-gated drift check.

**Tech Stack:** Bash, git tag/query plumbing, GitHub Actions YAML, Cargo manifests, existing packaging scripts, `cargo metadata`, `rg`, and repo-local shell verification scripts.

---

## Scope Guard

- This plan only covers binary-product release plumbing for `conary`, `remi`, `conaryd`, and `conary-test`.
- Static frontends under `site/` and `web/` stay out of the release matrix in this phase.
- Do not add a new language/runtime for release tooling. Keep the shared matrix shell-native so workflows and local scripts can consume it directly.
- Do not rewrite or recreate historical tags.
- Do not add a `conary-test` deploy target in this phase.
- Do not publish shared crates as standalone release tracks.

## File Map

| File | Responsibility |
|------|----------------|
| `scripts/release-matrix.sh` | New single source of truth for products, tag prefixes, version-owned manifests, scopes, bundle names, deploy modes, and metadata serialization |
| `scripts/test-release-matrix.sh` | New shell test harness for helper semantics and `scripts/release.sh` dry-run behavior |
| `scripts/check-release-matrix.sh` | New drift checker that verifies workflows and helper metadata stay aligned |
| `scripts/release.sh` | Matrix-driven release execution, dry-run output, owned-manifest updates, and no-downgrade guard |
| `.github/workflows/release-build.yml` | Metadata-driven release-build workflow with explicit `conary-test` support and GitHub release publication for non-`conary` bundles |
| `.github/workflows/deploy-and-verify.yml` | Metadata-driven deploy routing, explicit `deploy_mode=none` handling, and catch-all validation |
| `.github/workflows/pr-gate.yml` | Run the release-matrix drift checker on PRs so workflow/script drift is caught before merges |
| `apps/conary-test/Cargo.toml` | One-time canonical baseline alignment and updated release-track comment |
| `crates/conary-mcp/Cargo.toml` | Owned-manifest partner for `conary-test`; must not be downgraded |
| `docs/operations/infrastructure.md` | Release-flow documentation for the four-track matrix and legacy-prefix continuity |

## Chunk 1: Shared Matrix Helper

### Task 1: Create the shared release-matrix helper and prove mixed-prefix version lookup

**Files:**
- Create: `scripts/release-matrix.sh`
- Create: `scripts/test-release-matrix.sh`

- [ ] **Step 1: Write failing helper tests before adding the matrix**

Create `scripts/test-release-matrix.sh` as a pure shell test harness that exercises the helper through CLI-style subcommands. Cover at least:

```bash
assert_eq "$(bash scripts/release-matrix.sh resolve-tag remi-v0.5.0 --format shell | rg '^product=' -o -r '$0')" "product=remi"
assert_eq "$(bash scripts/release-matrix.sh resolve-tag server-v0.5.0 --format shell | rg '^product=' -o -r '$0')" "product=remi"
assert_eq "$(bash scripts/release-matrix.sh resolve-tag test-v0.3.0 --format shell | rg '^product=' -o -r '$0')" "product=conary-test"

assert_eq \
  "$(bash scripts/release-matrix.sh latest-version-from-list remi server-v0.5.0 remi-v0.4.0 remi-v0.6.0)" \
  "0.6.0"

assert_eq "$(bash scripts/release-matrix.sh field conary-test deploy_mode)" "none"
assert_eq "$(bash scripts/release-matrix.sh field conary bundle_name)" "release-bundle"
```

Also add one negative test that proves an unknown tag prefix fails with a clear error.

- [ ] **Step 2: Define the product matrix in one place**

Implement `scripts/release-matrix.sh` with explicit data for:

- `conary`
- `remi`
- `conaryd`
- `conary-test`

The helper must expose at least these subcommands:

```bash
scripts/release-matrix.sh products
scripts/release-matrix.sh field <product> <field>
scripts/release-matrix.sh resolve-tag <tag> [--format shell|json]
scripts/release-matrix.sh canonical-tag <product> <version>
scripts/release-matrix.sh latest-version-from-list <product> <tag...>
scripts/release-matrix.sh latest-version-from-git <product>
scripts/release-matrix.sh max-owned-version <product>
scripts/release-matrix.sh owned-paths <product>
scripts/release-matrix.sh metadata-json <product> <version> <tag> <dry_run>
```

Fields should include:

- canonical tag prefix
- accepted legacy prefixes
- bundle name
- deploy mode
- version-owned manifests
- bump-scope paths
- primary artifact patterns

- [ ] **Step 3: Make mixed-prefix version comparison prefix-agnostic**

Do **not** use `git tag --sort=-version:refname` to compare mixed canonical and legacy tag sets.

Implement version comparison by:

1. selecting only tags that match the product's canonical or legacy prefixes
2. stripping the matched prefix to get the numeric payload
3. sorting numeric versions only (for example with `sort -V`)
4. returning the winning numeric version plus the source tag when needed

Add a helper path for both:

- test-time tag lists (`latest-version-from-list`)
- repo-time git lookup (`latest-version-from-git` or equivalent internal helper)

- [ ] **Step 4: Add workflow-ready metadata serialization**

Make `metadata-json` emit the downstream data that `deploy-and-verify` needs without re-checking out the repo, for example:

```json
{
  "product": "remi",
  "canonical_tag_prefix": "remi-v",
  "tag_name": "remi-v0.5.1",
  "version": "0.5.1",
  "bundle_name": "remi-bundle",
  "deploy_mode": "remote_bundle",
  "artifact_patterns": ["remi-0.5.1-linux-x64.tar.gz"],
  "dry_run": "false"
}
```

Keep the helper the only authority for these values.

- [ ] **Step 5: Verify**

Run: `bash scripts/test-release-matrix.sh`

Expected: canonical and legacy tag resolution works, mixed-prefix version comparison returns the highest numeric version, and product metadata is exposed consistently.

- [ ] **Step 6: Commit**

```bash
git add scripts/release-matrix.sh scripts/test-release-matrix.sh
git commit -m "feat(release): add shared release matrix helper"
```

## Chunk 2: Version Alignment And Release Script Realignment

### Task 2: Align `conary-test` and `conary-mcp` to a shared canonical baseline

**Files:**
- Modify: `apps/conary-test/Cargo.toml`
- Modify: `crates/conary-mcp/Cargo.toml`
- Modify: `scripts/test-release-matrix.sh`

- [ ] **Step 1: Add a failing no-downgrade test case**

Extend `scripts/test-release-matrix.sh` with a case that models:

- historical `test-v0.3.0`
- owned manifests at `0.3.0` and `0.7.0`

and proves the release logic must not compute a target version that would move `crates/conary-mcp/Cargo.toml` backward.

At this step, the test should fail because the repo still has:

- `apps/conary-test/Cargo.toml`: `0.3.0`
- `crates/conary-mcp/Cargo.toml`: `0.7.0`

- [ ] **Step 2: Perform the one-time canonical baseline alignment**

Update:

- `apps/conary-test/Cargo.toml` from `0.3.0` to `0.7.0`
- `crates/conary-mcp/Cargo.toml` stays at `0.7.0`

Also replace the outdated inline comment in `apps/conary-test/Cargo.toml` with something explicit, for example:

```toml
# Canonical release track: conary-test-v* (legacy history: test-v*)
```

This is an intentional migration step, not a product-feature bump.

- [ ] **Step 3: Verify the manifests are aligned and still parse**

Run: `cargo generate-lockfile --quiet`

Expected: `Cargo.lock` updates to reflect the new workspace-member version baseline.

Run: `rg -n '^version = ' apps/conary-test/Cargo.toml crates/conary-mcp/Cargo.toml`

Expected: both files report `0.7.0`.

Run: `cargo metadata --no-deps --format-version 1 >/dev/null`

Expected: success with no manifest parse errors.

- [ ] **Step 4: Commit**

```bash
git add apps/conary-test/Cargo.toml crates/conary-mcp/Cargo.toml Cargo.lock
git commit -m "chore(release): align conary-test with conary-mcp baseline"
```

### Task 3: Refactor `scripts/release.sh` to consume the matrix and enforce monotonic owned versions

**Files:**
- Modify: `scripts/release.sh`
- Modify: `scripts/release-matrix.sh`
- Modify: `scripts/test-release-matrix.sh`

- [ ] **Step 1: Add failing dry-run release-flow tests**

Extend `scripts/test-release-matrix.sh` to create temporary git repos and prove these dry-run behaviors:

```bash
# Remi should read old server tags but emit new remi tags.
assert_contains \
  "$(run_release_dry_run remi with_tags server-v0.5.0 fix(remi): tighten deploy flow)" \
  "Tag: remi-v0.5.1"

# Mixed canonical + legacy history must choose the highest numeric version.
assert_contains \
  "$(run_release_dry_run remi with_tags server-v1.0.0 remi-v2.0.0 fix(remi): tighten deploy flow)" \
  "Current: remi-v2.0.0"

# conary-test must not compute a release lower than its owned manifest max.
assert_contains \
  "$(run_release_dry_run conary-test with_tags test-v0.3.0 fix(test): update bundle layout)" \
  "Current: conary-test-v0.7.0"
```

- [ ] **Step 2: Replace hardcoded product arrays with helper-driven queries**

Refactor `scripts/release.sh` so it:

- accepts `conary-test` in `usage()`
- asks `scripts/release-matrix.sh` for canonical prefix, legacy prefixes, scopes, owned paths, bundle name, and deploy mode
- stops carrying its own duplicated `TAG_PREFIX` and `PATH_SCOPES` maps

Keep the packaging update path only for the `conary` track, but drive its file list from matrix-owned paths rather than implicit script knowledge.

Important constraint:

- keep the existing format-specific `update_packaging_versions()` logic for
  `.spec`, `PKGBUILD`, Debian changelog, and CCS manifest edits
- only invoke each format-specific update when that exact file path appears in
  the matrix-owned path set for the selected product

- [ ] **Step 3: Make current-version selection monotonic**

When computing the release baseline, use the maximum of:

- the highest numeric version found in canonical/legacy tag history for that product
- the highest numeric version already present in the product's owned manifests

In shell terms, the logic should look like:

```bash
history_version="$(bash scripts/release-matrix.sh latest-version-from-git "$product")"
manifest_version="$(bash scripts/release-matrix.sh max-owned-version "$product")"
current_version="$(printf '%s\n%s\n' "$history_version" "$manifest_version" | sort -V | tail -1)"
```

Also add a hard failure if the computed release target would be lower than any owned manifest version.

- [ ] **Step 4: Keep `--dry-run` truthful and operator-friendly**

Update the dry-run output to print:

- previous tag(s) considered
- current numeric baseline
- next version
- next canonical tag
- owned manifests to be updated
- bundle name
- deploy mode

That output should make it obvious why `remi` is continuing from `server-v*` history and why `conary-test` starts from `0.7.0`.

- [ ] **Step 5: Verify**

Run: `bash scripts/test-release-matrix.sh`

Expected: temp-repo dry-run cases pass for `remi`, `conaryd`, and `conary-test`.

Run:

```bash
./scripts/release.sh conary --dry-run
./scripts/release.sh remi --dry-run
./scripts/release.sh conaryd --dry-run
./scripts/release.sh conary-test --dry-run
```

Expected: each product resolves to the correct canonical release line, and `conary-test` does not attempt to downgrade `conary-mcp`.

- [ ] **Step 6: Commit**

```bash
git add scripts/release.sh scripts/release-matrix.sh scripts/test-release-matrix.sh
git commit -m "feat(release): drive release script from shared matrix"
```

## Chunk 3: Workflow Realignment

### Task 4: Make `release-build` metadata-driven and add `conary-test` release publication

**Files:**
- Modify: `.github/workflows/release-build.yml`
- Create: `scripts/check-release-matrix.sh`

- [ ] **Step 1: Write a failing release-workflow drift check**

Create `scripts/check-release-matrix.sh` with focused assertions that fail against the current workflow state. Cover at least:

- `release-build.yml` knows about `conary-test`
- workflow-prepared metadata contains `bundle_name` and `deploy_mode`
- `remi`, `conaryd`, and `conary-test` all have a GitHub-release publication path
- bundle names in the workflow match the helper output

Use simple `rg`-based checks plus helper queries, for example:

```bash
expected_bundle="$(bash scripts/release-matrix.sh field conary-test bundle_name)"
rg -q "$expected_bundle" .github/workflows/release-build.yml
```

- [ ] **Step 2: Move product resolution in `prepare` to the shared helper**

In `.github/workflows/release-build.yml`:

- keep canonical tag push triggers as the normal release path
- add `conary-test-v*` to the `on.push.tags` trigger list
- continue supporting legacy prefixes only in helper-based resolution for manual modeling / continuity logic
- call `scripts/release-matrix.sh resolve-tag ...` in the `prepare` job
- write matrix-derived fields into `release-metadata/metadata.json`
- export the same matrix-derived fields through `$GITHUB_OUTPUT` and the
  `prepare` job `outputs:` block so downstream jobs can use them without
  reparsing JSON locally

Include at least:

- product
- tag_name
- version
- dry_run
- bundle_name
- deploy_mode
- artifact_patterns

- [ ] **Step 3: Add the `conary-test` build-and-bundle lane**

Add a `build-conary-test` job parallel to `build-remi` / `build-conaryd`:

- build `cargo build -p conary-test --release --verbose`
- package:
  - `conary-test-<version>-linux-x64`
  - `conary-test-<version>-linux-x64.tar.gz`
  - `metadata.json`
- upload bundle artifact named from the matrix (`conary-test-bundle`)

- [ ] **Step 4: Publish GitHub releases for all binary bundles**

The current workflow only publishes a GitHub release for the `conary` bundle. Add a publication path for:

- `remi`
- `conaryd`
- `conary-test`

Recommended shape:

- keep `bundle-conary` as-is for `conary`
- prefer a tiny per-product publish step or adjacent publish job for `remi`,
  `conaryd`, and `conary-test`
- do **not** create a single fan-in publish job that `needs` mutually exclusive
  build jobs unless you explicitly guard against GitHub Actions skipped-needs
  behavior

Each binary publication path should:

- download the matching bundle artifact
- create or update the GitHub release for the current tag
- upload the bundle files as assets

If an implementation chooses a shared publish job anyway, it must use
non-default skip handling such as `if: ${{ !cancelled() }}` plus explicit
inspection of `needs.*.result` so skipped unrelated build jobs do not suppress
publication.

- [ ] **Step 5: Verify**

Run: `bash scripts/check-release-matrix.sh`

Expected: the check now sees all four products, matrix-owned bundle names, and a release-publication path for every releasable product.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release-build.yml scripts/check-release-matrix.sh
git commit -m "feat(release): align release-build with product matrix"
```

### Task 5: Make `deploy-and-verify` honor serialized deploy metadata and fail loudly on mismatches

**Files:**
- Modify: `.github/workflows/deploy-and-verify.yml`
- Modify: `scripts/check-release-matrix.sh`

- [ ] **Step 1: Extend the drift check with deploy-mode assertions**

Add failing checks that prove:

- deployable products (`conary`, `remi`, `conaryd`) still have explicit deployment paths
- `conary-test` is explicitly recognized as `deploy_mode=none`
- the workflow does not silently fall through to success for unknown deployable products

- [ ] **Step 2: Drive resolve outputs from serialized metadata**

In `.github/workflows/deploy-and-verify.yml`, update the `resolve` job to read from `metadata.json`:

- `product`
- `version`
- `tag_name`
- `dry_run`
- `bundle_name`
- `deploy_mode`
- `artifact_patterns`

Do **not** re-derive deploy eligibility with another local `case` tree that can drift from the helper.

For manual `workflow_dispatch`, keep `source_run` / `environment` / `dry_run`
overrides, but either remove the manual `product` override entirely or validate
that it exactly matches the serialized metadata product before continuing.

- [ ] **Step 3: Add explicit validation and no-deploy handling**

Add a validation step or job that:

- fails immediately if `deploy_mode != none` and the product is not one of the explicitly supported deploy lanes
- fails immediately if bundle metadata is missing or mismatched

Also add one explicit non-deploy path for `deploy_mode=none`, for example:

```yaml
no-deploy-required:
  if: ${{ needs.resolve.outputs.deploy_mode == 'none' }}
  runs-on: ubuntu-latest
  steps:
    - run: echo "No deployment configured for ${{ needs.resolve.outputs.product }}"
```

That job should make the `conary-test` no-deploy outcome visible and intentional.

- [ ] **Step 4: Keep current deploy behavior for deployable products**

Retain the existing operational behavior for:

- `conary`
- `remi`
- `conaryd`

but gate them off the resolved metadata rather than implicit product-only assumptions. Keep the current health / endpoint verification steps intact.

- [ ] **Step 5: Verify**

Run: `bash scripts/check-release-matrix.sh`

Expected: deploy-mode checks pass, `conary-test` has a visible no-deploy lane, and no deployable matrix product is left without an explicit execution path.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/deploy-and-verify.yml scripts/check-release-matrix.sh
git commit -m "feat(release): make deploy workflow metadata-driven"
```

## Chunk 4: Docs, Guardrails, And Final Verification

### Task 6: Add a PR guard for release-matrix drift

**Files:**
- Modify: `.github/workflows/pr-gate.yml`
- Modify: `scripts/check-release-matrix.sh`

- [ ] **Step 1: Add the new checker to PR validation**

Add a small job to `.github/workflows/pr-gate.yml` that runs:

```bash
bash scripts/check-release-matrix.sh
```

Keep it separate from `workflow-runtime-policy` so release-matrix drift is visible on its own.

- [ ] **Step 2: Keep the checker narrow and deterministic**

Limit `scripts/check-release-matrix.sh` to static repo checks:

- helper fields vs workflow bundle names
- helper deploy modes vs workflow lane presence
- `conary-test` inclusion in release-build
- `conary-test` exclusion from deployable lanes

Do not turn it into a live GitHub API or remote-host test.

- [ ] **Step 3: Verify**

Run: `bash scripts/check-release-matrix.sh`

Expected: success locally with no network calls.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/pr-gate.yml scripts/check-release-matrix.sh
git commit -m "test(release): gate workflow drift with release-matrix checks"
```

### Task 7: Update docs and release-facing comments

**Files:**
- Modify: `docs/operations/infrastructure.md`
- Modify: `apps/conary-test/Cargo.toml`
- Modify if needed: `scripts/release.sh`

- [ ] **Step 1: Update the release docs to match the matrix**

In `docs/operations/infrastructure.md`, update the release-flow section to cover:

- four supported product tracks
- canonical tag forms:
  - `v*`
  - `remi-v*`
  - `conaryd-v*`
  - `conary-test-v*`
- legacy lookup continuity:
  - `server-v*` -> `remi`
  - `test-v*` -> `conary-test`
- `conary-test` as build-and-release only, with no deploy lane in this phase

- [ ] **Step 2: Remove stale inline release comments**

Make sure comments that still imply the old three-track setup are rewritten to the new model, especially in `apps/conary-test/Cargo.toml` and any release-script usage text.

- [ ] **Step 3: Verify**

Run:

```bash
rg -n "server-v|test-v|conary-test-v|remi-v|conaryd-v" docs/operations/infrastructure.md apps/conary-test/Cargo.toml scripts/release.sh
```

Expected: any remaining legacy prefixes appear only in intentional continuity language, not as canonical future-release instructions.

- [ ] **Step 4: Commit**

```bash
git add docs/operations/infrastructure.md apps/conary-test/Cargo.toml scripts/release.sh
git commit -m "docs(release): document four-track release matrix"
```

### Task 8: Final verification before execution handoff

**Files:**
- Verify only: existing modified files from previous tasks

- [ ] **Step 1: Run the helper and workflow guard scripts**

Run:

```bash
bash scripts/test-release-matrix.sh
bash scripts/check-release-matrix.sh
bash scripts/check-github-action-runtimes.sh
```

Expected: all pass locally.

- [ ] **Step 2: Run dry-run releases for every track**

Run:

```bash
./scripts/release.sh conary --dry-run
./scripts/release.sh remi --dry-run
./scripts/release.sh conaryd --dry-run
./scripts/release.sh conary-test --dry-run
```

Expected:

- `conary` reports packaging-owned files and `release-bundle`
- `remi` continues from `server-v*` history but emits `remi-v*`
- `conaryd` remains unchanged in naming
- `conary-test` reports `conary-test-v*`, uses the aligned `0.7.0` baseline, and never proposes downgrading `conary-mcp`

- [ ] **Step 3: Re-check Cargo manifests after all version/comment changes**

Run: `cargo metadata --no-deps --format-version 1 >/dev/null`

Expected: success.

- [ ] **Step 4: Summarize cutover behavior in the handoff**

Capture in the execution handoff:

- legacy tags are still read for history lookup only
- new releases emit canonical tags only
- `conary-test` is now a supported release track with no deploy lane
- release/build/deploy workflows consume matrix-owned metadata instead of duplicating product assumptions
