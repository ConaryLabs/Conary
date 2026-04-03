# CI/CD Topology Migration Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate Conary to a GitHub-only CI/CD control plane, align release tracks with the three real products (`conary`, `remi`, `conaryd`), keep `conary-test` as internal validation infrastructure, retire Forgejo workflows and Remi's Forgejo bridge code, and land the five-lane automation model from the approved topology spec.

**Architecture:** Keep GitHub Actions as the only orchestrator. Use GitHub-hosted runners for the untrusted `pr-gate`, one restricted self-hosted GitHub runner on Forge for the first trusted-lane rollout, and protected GitHub environments for deployment. Migrate in four chunks: taxonomy/docs and Forge host setup, GitHub workflow lane replacement, Remi/Forgejo control-plane removal, then hardening and rollout verification. Deployment becomes explicit, protected, and serialized; the legacy main-push Remi auto-deploy path is intentionally removed during this migration. Keep `cargo test` as the initial Rust test runner during the migration; revisit `cargo-nextest` only after the lane split is green.

**Tech Stack:** GitHub Actions YAML, GitHub self-hosted runners, Rust 2024 workspace, Axum admin/MCP surfaces in `apps/remi`, shell scripts under `scripts/` and `deploy/`, Markdown docs under `docs/`.

---

## Preconditions

- Execute this migration from a fresh feature worktree, not directly on `main`.
- Confirm repository admin access for:
  - GitHub Actions workflow edits
  - GitHub protected environments
  - GitHub self-hosted runner registration
  - GitHub branch protection required-check updates
- Use one restricted Forge-hosted GitHub runner for the first rollout with a
  label such as `forge-trusted`. Add a pool later only if queueing data proves
  it is needed.
- If the repository is ever public, or if fork PR policy changes, keep the
  `forge-trusted` runner workflow-restricted so untrusted fork PRs cannot target
  it.
- Keep branch-level validation easy to debug: every new workflow in this plan
  should support `workflow_dispatch` before it becomes the only automatic path.
- The approved topology spec is
  `docs/superpowers/specs/2026-04-02-ci-cd-topology-design.md`.

## File Map

- Create: `.github/actions/setup-rust-workspace/action.yml`
- Create: `.github/workflows/pr-gate.yml`
- Create: `.github/workflows/merge-validation.yml`
- Create: `.github/workflows/release-build.yml`
- Create: `.github/workflows/deploy-and-verify.yml`
- Create: `.github/workflows/scheduled-ops.yml`
- Create: `deploy/systemd/github-actions-runner.service`
- Modify: `scripts/release.sh`
- Modify: `deploy/FORGE.md`
- Modify: `deploy/setup-forge.sh`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Delete: `.github/workflows/ci.yml`
- Delete: `.github/workflows/release.yml`
- Delete: `.forgejo/workflows/ci.yaml`
- Delete: `.forgejo/workflows/integration.yaml`
- Delete: `.forgejo/workflows/e2e.yaml`
- Delete: `.forgejo/workflows/release.yaml`
- Delete: `.forgejo/workflows/remi-health.yaml`
- Modify: `apps/remi/src/server/auth.rs`
- Modify: `apps/remi/src/server/config.rs`
- Modify: `apps/remi/src/server/mod.rs`
- Modify: `apps/remi/src/server/routes/admin.rs`
- Modify: `apps/remi/src/server/routes.rs`
- Modify: `apps/remi/src/server/mcp.rs`
- Modify: `apps/remi/src/server/audit.rs`
- Modify: `apps/remi/src/server/handlers/admin/mod.rs`
- Modify: `apps/remi/src/server/handlers/admin/tokens.rs`
- Modify: `apps/remi/src/server/handlers/admin/repos.rs`
- Modify: `apps/remi/src/server/handlers/openapi.rs`
- Delete: `apps/remi/src/server/forgejo.rs`
- Delete: `apps/remi/src/server/handlers/admin/ci.rs`

## Implementation Defaults

- `main` is the only active long-lived branch. Remove stale `develop` triggers.
- `cargo test` remains the first migration-pass test runner in GitHub CI.
- `cargo audit` moves out of merge-blocking PR CI and into trusted scheduled or
  manually triggered security work.
- `conary` remains the only public artifact-release line by default.
- `remi` and `conaryd` become tagged service-build and deploy lines without
  public GitHub asset bundles in the first pass.
- deployment is explicit, protected, and serialized; the legacy Remi
  auto-deploy-on-merge path is intentionally removed
- Forge remains a host, not a control plane.

## Chunk 1: Taxonomy And Forge Host Setup

### Task 1: Align Release Groups And Core Docs With The Three-Product Model

**Files:**
- Modify: `scripts/release.sh`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Test: `scripts/release.sh`

- [ ] **Step 1: Capture the current obsolete release surface**

Run:

```bash
./scripts/release.sh conary-test --dry-run
rg -n "conary-test|develop|Forge-side release checks|Forgejo" \
  scripts/release.sh docs/operations/infrastructure.md docs/INTEGRATION-TESTING.md .github/workflows/ci.yml
```

Expected:

- `conary-test --dry-run` is currently accepted even though it is no longer a
  release-track product
- the grep output shows stale control-plane or branch references that this task
  will remove or rewrite

- [ ] **Step 2: Remove the obsolete `conary-test` release group from `scripts/release.sh`**

Make these exact structural edits:

```bash
# Keep only the real release-track groups.
Usage: $0 [conary|remi|conaryd|all] [--dry-run]

declare -A TAG_PREFIX=(
  [conary]="v"
  [remi]="remi-v"
  [conaryd]="conaryd-v"
)

declare -A PATH_SCOPES=(
  [conary]="apps/conary/ crates/conary-core/ packaging/ .github/workflows/release-build.yml .github/workflows/deploy-and-verify.yml scripts/sign-release.sh"
  [remi]="apps/remi/ deploy/systemd/remi.service scripts/rebuild-remi.sh scripts/bootstrap-remi.sh"
  [conaryd]="apps/conaryd/"
)
```

Also update the `all` expansion and the usage banner so `conary-test` is no
longer presented as a valid release target.

The workflow paths under the `conary` scope are created in Task 5; it is
acceptable for this task to reference them before they exist in the branch.

- [ ] **Step 3: Update the tracked docs to reflect the new taxonomy**

Edit `docs/operations/infrastructure.md` so it says:

- Forge is the trusted GitHub runner host for `conary-test` validation
- GitHub Actions is the only long-term CI/CD control plane
- `scripts/release.sh` supports `conary|remi|conaryd|all`
- release verification is a GitHub workflow concern, not a Forgejo concern

Edit `docs/INTEGRATION-TESTING.md` so the CI integration section describes:

- GitHub `merge-validation` as the trusted on-merge smoke lane
- GitHub `scheduled-ops` as the nightly and scheduled deep-validation lane
- `conary-test deploy status` as commit-aware internal infrastructure status,
  not product release identity

- [ ] **Step 4: Run the green checks**

Run:

```bash
bash -n scripts/release.sh
./scripts/release.sh conary --dry-run
./scripts/release.sh remi --dry-run
./scripts/release.sh conaryd --dry-run
./scripts/release.sh conary-test --dry-run || true
rg -n "conary-test|develop|Forgejo" scripts/release.sh docs/operations/infrastructure.md docs/INTEGRATION-TESTING.md
```

Expected:

- `bash -n` exits 0
- the first three dry runs are accepted
- `conary-test --dry-run` prints usage or exits nonzero
- remaining `conary-test`, `develop`, or `Forgejo` matches are intentional
  historical references, not active workflow instructions

- [ ] **Step 5: Commit**

```bash
git add scripts/release.sh docs/operations/infrastructure.md docs/INTEGRATION-TESTING.md
git commit -m "docs(ci): align release taxonomy with product model"
```

### Task 2: Replace Forgejo Host Setup With A GitHub Runner Host Setup

**Files:**
- Modify: `deploy/FORGE.md`
- Modify: `deploy/setup-forge.sh`
- Create: `deploy/systemd/github-actions-runner.service`
- Test: `deploy/setup-forge.sh`

- [ ] **Step 1: Capture the current Forgejo-specific host story**

Run:

```bash
rg -n "Forgejo|forgejo-runner|mirror|linux-native" deploy/FORGE.md deploy/setup-forge.sh
```

Expected: the docs and setup script still describe a Forgejo web app plus
Forgejo runner as the CI/CD control plane on Forge.

- [ ] **Step 2: Rewrite the Forge host script to install a GitHub self-hosted runner instead**

Keep `deploy/setup-forge.sh` as the single setup entrypoint, but change its
responsibility:

- install Podman and Rust prerequisites if missing
- install or update the GitHub Actions runner binary
- fetch an ephemeral registration token with `gh api
  repos/<owner>/<repo>/actions/runners/registration-token` or the equivalent
  org-scoped GitHub API call, then register one restricted runner against the
  Conary repository or org
- assign a label such as `forge-trusted`
- restrict the runner to the trusted workflows from this plan so `pr-gate` and
  fork PR traffic cannot target it
- write a checked-in systemd unit from
  `deploy/systemd/github-actions-runner.service`

Use a service template like:

```ini
[Unit]
Description=GitHub Actions Runner (Conary Forge)
After=network.target

[Service]
User=peter
WorkingDirectory=/home/peter/actions-runner
ExecStart=/home/peter/actions-runner/run.sh
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Do not keep Forgejo installation, mirroring, or Forgejo runner registration in
this script. Do not stop the existing `forgejo.service` or
`forgejo-runner.service` yet; decommission them in Task 9 only after the GitHub
runner lane has been verified.

- [ ] **Step 3: Rewrite `deploy/FORGE.md` around the new GitHub-runner role**

Update the doc so it explains:

- Forge is a GitHub self-hosted runner host for trusted validation
- the first rollout uses one runner with `forge-trusted`
- no Forgejo service is part of the target setup
- manual validation commands on Forge still use `cargo run -p conary-test ...`
  and `./scripts/remi-health.sh`

- [ ] **Step 4: Verify the rewritten host setup artifacts**

Run:

```bash
bash -n deploy/setup-forge.sh
rg -n "Forgejo|forgejo-runner|mirror" deploy/FORGE.md deploy/setup-forge.sh deploy/systemd/github-actions-runner.service
```

Expected:

- `bash -n` exits 0
- no active setup instructions mention Forgejo or Forgejo Runner

- [ ] **Step 5: Commit**

```bash
git add deploy/FORGE.md deploy/setup-forge.sh deploy/systemd/github-actions-runner.service
git commit -m "ops(ci): switch Forge setup to GitHub runner host"
```

## Chunk 2: GitHub Workflow Lane Replacement

### Task 3: Introduce The `pr-gate` Workflow And Retire The Old `ci.yml`

**Files:**
- Create: `.github/actions/setup-rust-workspace/action.yml`
- Create: `.github/workflows/pr-gate.yml`
- Delete: `.github/workflows/ci.yml`
- Test: `.github/workflows/pr-gate.yml`

- [ ] **Step 1: Record the current PR-gate problems**

Run:

```bash
sed -n '1,120p' .github/workflows/ci.yml
```

Expected:

- the current workflow mixes `push` and `pull_request`
- it still includes `develop`
- it runs `cargo audit` as a merge-blocking job
- it is not named after the lane it represents

- [ ] **Step 2: Create a reusable Rust workspace setup action**

Create `.github/actions/setup-rust-workspace/action.yml` with a composite
action that centralizes:

- `actions/checkout`
- `dtolnay/rust-toolchain`
- `actions/cache`

Use an interface like:

```yaml
name: setup-rust-workspace
description: Checkout, install Rust, and warm workspace cargo cache.
inputs:
  components:
    default: ""
runs:
  using: composite
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
      with:
        components: ${{ inputs.components }}
    - uses: actions/cache@v4
      with:
        path: |
          ~/.cargo/registry
          ~/.cargo/git
          target
        key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
```

Use this composite action only in GitHub-hosted jobs. Trusted jobs running on
`forge-trusted` should do a plain checkout and rely on their preinstalled Rust
toolchain instead of cache-heavy shared setup.

- [ ] **Step 3: Create `.github/workflows/pr-gate.yml`**

The first-pass workflow should be branch-testable and merge-safe:

```yaml
name: pr-gate
on:
  pull_request:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  pull-requests: read

concurrency:
  group: pr-gate-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: true
```

Add jobs for:

- `fmt`
- `clippy`
- `workspace-tests` using `cargo test --workspace --exclude conary-test`
- `conary-test-crate`
- `doctests`
- `dependency-review` using GitHub's dependency review action

Give each job a stable `name:` matching the intended required check name:
`fmt`, `clippy`, `workspace-tests`, `conary-test-crate`, `doctests`, and
`dependency-review`.

Do not include `push` triggers, `develop`, or `cargo audit`.

- [ ] **Step 4: Remove the old workflow file once parity exists**

Delete `.github/workflows/ci.yml` after `pr-gate.yml` exists and carries the
merge-blocking responsibilities.

- [ ] **Step 5: Update GitHub branch protection required checks during cutover**

Update the repository branch protection rules so they require the new
`pr-gate` checks and no longer wait on the retired `ci.yml` job names.

Specifically:

- remove old required checks such as `Test` and `Security Audit` only after the
  new workflow has emitted green checks on the branch
- add the new required checks `fmt`, `clippy`, `workspace-tests`,
  `conary-test-crate`, `doctests`, and `dependency-review`
- do not leave a merge window where neither the old nor new checks are
  required

Use the GitHub web UI or `gh api` if you already manage branch protection as
code.

- [ ] **Step 6: Verify the new PR gate locally and in GitHub**

Run locally:

```bash
git diff --check
rg -n "develop|cargo audit|push:" .github/workflows/pr-gate.yml || true
test ! -f .github/workflows/ci.yml
```

Then push the branch and run:

```bash
gh workflow run pr-gate.yml --ref <branch-name>
gh run watch --exit-status
```

Expected:

- local checks are clean
- `pr-gate.yml` contains only `pull_request` and `workflow_dispatch`
- the manual dispatch run completes successfully on GitHub

- [ ] **Step 7: Commit**

```bash
git add .github/actions/setup-rust-workspace/action.yml .github/workflows/pr-gate.yml
git add -A .github/workflows/ci.yml
git commit -m "ci: add pr-gate workflow"
```

### Task 4: Add `merge-validation` And `scheduled-ops`, Then Retire Forgejo Validation Workflows

**Files:**
- Create: `.github/workflows/merge-validation.yml`
- Create: `.github/workflows/scheduled-ops.yml`
- Delete: `.forgejo/workflows/ci.yaml`
- Delete: `.forgejo/workflows/integration.yaml`
- Delete: `.forgejo/workflows/e2e.yaml`
- Delete: `.forgejo/workflows/release.yaml`
- Delete: `.forgejo/workflows/remi-health.yaml`
- Modify: `docs/INTEGRATION-TESTING.md`
- Test: `.github/workflows/merge-validation.yml`
- Test: `.github/workflows/scheduled-ops.yml`

- [ ] **Step 1: Capture the current Forgejo validation behavior**

Run:

```bash
sed -n '1,220p' .forgejo/workflows/ci.yaml
sed -n '1,220p' .forgejo/workflows/integration.yaml
sed -n '1,220p' .forgejo/workflows/e2e.yaml
sed -n '1,200p' .forgejo/workflows/remi-health.yaml
```

Expected:

- trusted validation still lives in Forgejo workflow files instead of GitHub
  Actions
- `.forgejo/workflows/ci.yaml` still auto-deploys Remi on `main` pushes

- [ ] **Step 2: Create `.github/workflows/merge-validation.yml`**

Use this first-pass trigger and runner shape:

```yaml
name: merge-validation
on:
  push:
    branches: [main]
  workflow_dispatch:
    inputs:
      smoke_distro:
        required: false
        default: fedora43

jobs:
  smoke:
    runs-on: [self-hosted, forge-trusted]
```

The first trusted smoke job should:

- build `conary`, `remi`, `conaryd`, and `conary-test`
- run one Fedora 43 `conary-test` smoke subset
- run `./scripts/remi-health.sh --smoke`

Keep this lane intentionally small. Do not migrate the whole E2E matrix here.
This lane intentionally stops at validation and does not deploy Remi on `main`
pushes.

- [ ] **Step 3: Create `.github/workflows/scheduled-ops.yml`**

Use `schedule` plus `workflow_dispatch` and split jobs by responsibility:

- `health`: `./scripts/remi-health.sh --full`
- `audit`: `cargo audit` with the existing accepted ignore list
- `deep-validation`: nightly `conary-test` coverage migrated from Forgejo E2E
- optional `qemu` job gated by schedule or explicit manual input

Give `deep-validation` the Forge runner and allow `audit` to stay
GitHub-hosted if that is simpler.

- [ ] **Step 4: Remove the superseded Forgejo workflow files**

Delete:

```text
.forgejo/workflows/ci.yaml
.forgejo/workflows/integration.yaml
.forgejo/workflows/e2e.yaml
.forgejo/workflows/release.yaml
.forgejo/workflows/remi-health.yaml
```

Stage the directory removal explicitly with `git rm -r .forgejo/workflows`
instead of relying on empty-directory disappearance as an implicit side effect.

Then update `docs/INTEGRATION-TESTING.md` so the CI Integration section points
at the new GitHub workflow names and no longer mentions manual Forgejo API
dispatch.

Call out in the docs and rollout notes that removing `.forgejo/workflows/ci.yaml`
intentionally retires the old auto-deploy-Remi-on-merge behavior. Going
forward, Remi deploys happen only through `deploy-and-verify.yml` via manual
dispatch or successful `remi-v*` release builds. Do not merge this branch until
Task 5 adds that replacement path.

- [ ] **Step 5: Verify the trusted-lane replacements**

Run locally:

```bash
test ! -f .forgejo/workflows/ci.yaml
test ! -f .forgejo/workflows/integration.yaml
test ! -f .forgejo/workflows/e2e.yaml
test ! -f .forgejo/workflows/release.yaml
test ! -f .forgejo/workflows/remi-health.yaml
git diff --check
```

Then push the branch and run:

```bash
gh workflow run merge-validation.yml --ref <branch-name>
gh workflow run scheduled-ops.yml --ref <branch-name>
gh run list --workflow merge-validation.yml --limit 1
gh run list --workflow scheduled-ops.yml --limit 1
```

Expected:

- Forgejo workflow files are gone and `.forgejo/workflows/` has been explicitly
  removed from the tree
- both GitHub workflows are dispatchable
- the GitHub UI or `gh run list` shows them as the new trusted validation lanes

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/merge-validation.yml .github/workflows/scheduled-ops.yml docs/INTEGRATION-TESTING.md
git add -A .forgejo/workflows
git commit -m "ci: migrate trusted validation lanes to GitHub"
```

### Task 5: Split Release Build From Deployment And Verification

**Files:**
- Create: `.github/workflows/release-build.yml`
- Create: `.github/workflows/deploy-and-verify.yml`
- Delete: `.github/workflows/release.yml`
- Modify: `scripts/release.sh`
- Test: `.github/workflows/release-build.yml`
- Test: `.github/workflows/deploy-and-verify.yml`

- [ ] **Step 1: Capture the current release coupling**

Run:

```bash
sed -n '1,280p' .github/workflows/release.yml
if test -f .forgejo/workflows/release.yaml; then
  sed -n '1,200p' .forgejo/workflows/release.yaml
else
  git show HEAD~1:.forgejo/workflows/release.yaml | sed -n '1,200p'
fi
```

Expected:

- build, GitHub release creation, and Remi deployment are still fused together
- a separate Forgejo workflow is still doing release landing verification

If Task 4 is not the immediate parent commit, use any pre-Task-4 revision that
still contains `.forgejo/workflows/release.yaml`.

- [ ] **Step 2: Create `.github/workflows/release-build.yml`**

Keep this workflow tag-driven and branch-testable:

```yaml
name: release-build
on:
  push:
    tags: ['v*', 'remi-v*', 'conaryd-v*']
  workflow_dispatch:
    inputs:
      product:
        required: true
      tag_name:
        required: true
      dry_run:
        required: false
        default: "true"
```

Implement product-specific jobs:

- `conary`: existing RPM/DEB/Arch/CCS packaging matrix plus GitHub release
  asset publication
- `conary`: carry forward the current `sign_hash` helper build and CCS signing
  flow, with `RELEASE_SIGNING_KEY` treated as a required release secret for the
  signing step
- `remi`: `cargo build -p remi --release`, upload retained workflow artifact,
  write build provenance summary
- `conaryd`: `cargo build -p conaryd --release`, upload retained workflow
  artifact, write build provenance summary

Do not deploy from this workflow.

- [ ] **Step 3: Create `.github/workflows/deploy-and-verify.yml`**

Use protected environments and serialized deploys:

```yaml
name: deploy-and-verify
on:
  workflow_run:
    workflows: [release-build]
    types: [completed]
  workflow_dispatch:
    inputs:
      product:
        required: true
      source_run:
        required: true
      environment:
        required: true
      dry_run:
        required: false
        default: "true"

concurrency:
  group: deploy-and-verify
  cancel-in-progress: false
```

The single `deploy-and-verify` concurrency group intentionally serializes all
product deployments as the safer first rollout.

Implement first-pass deployment logic:

- `conary`: download release-build artifacts, deploy to Remi, verify
  `/v1/ccs/conary/latest`
- `remi`: deploy the release-built binary to the Remi host and verify health
- `conaryd`: deploy the release-built binary to the daemon host and verify its
  health or version endpoint

Add a branch-testable `dry_run` path so staging validation does not require a
real tagged production deploy.

This workflow intentionally replaces the legacy Remi auto-deploy-on-merge path.
Going forward, operators deploy Remi by pushing a `remi-v*` tag that feeds this
workflow from `release-build`, or by running `gh workflow run
deploy-and-verify.yml` manually.

- [ ] **Step 4: Remove the old coupled release workflows**

Delete:

```text
.github/workflows/release.yml
```

Update `scripts/release.sh` path scopes if the workflow filenames changed.

- [ ] **Step 5: Verify the split release path**

Run locally:

```bash
git diff --check
test ! -f .github/workflows/release.yml
```

Then push the branch and run branch-safe dispatches:

```bash
gh workflow run release-build.yml --ref <branch-name> -f product=conary -f tag_name=test-v0.0.0 -f dry_run=true
gh run watch --exit-status
gh workflow run deploy-and-verify.yml --ref <branch-name> -f product=conary -f source_run=<release-build-run-id> -f environment=staging -f dry_run=true
gh run watch --exit-status
```

Expected:

- release build is manually dispatchable without a real tag push
- deploy-and-verify is manually dispatchable in `dry_run` mode
- no active workflow files remain in the old fused layout

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release-build.yml .github/workflows/deploy-and-verify.yml scripts/release.sh
git add -A .github/workflows/release.yml
git commit -m "ci: split release build from deployment"
```

## Chunk 3: Remi Forgejo Bridge Removal

### Task 6: Remove Forgejo Admin Endpoints, Routes, Scopes, And OpenAPI Surface

**Files:**
- Modify: `apps/remi/src/server/auth.rs`
- Modify: `apps/remi/src/server/routes/admin.rs`
- Modify: `apps/remi/src/server/routes.rs`
- Modify: `apps/remi/src/server/handlers/admin/mod.rs`
- Modify: `apps/remi/src/server/handlers/admin/tokens.rs`
- Modify: `apps/remi/src/server/handlers/admin/repos.rs`
- Modify: `apps/remi/src/server/handlers/openapi.rs`
- Modify: `apps/remi/src/server/audit.rs`
- Delete: `apps/remi/src/server/handlers/admin/ci.rs`
- Test: `apps/remi/src/server/auth.rs`
- Test: `apps/remi/src/server/handlers/openapi.rs`
- Test: `apps/remi/src/server/audit.rs`

- [ ] **Step 1: Write the failing tests for the removed CI surface**

Update or add focused tests that assert:

- `validate_scopes("ci:read")` and `validate_scopes("ci:trigger")` now fail
- the OpenAPI spec no longer contains `/v1/admin/ci/*` paths
- token-scope help text no longer advertises `ci:read` or `ci:trigger`
- `derive_action("GET", "/v1/admin/ci/workflows")` returns `"unknown"`

Use exact assertions like:

```rust
assert!(validate_scopes("ci:read").is_err());
assert!(!json_text.contains("/v1/admin/ci/workflows"));
assert_eq!(derive_action("GET", "/v1/admin/ci/workflows"), "unknown");
```

- [ ] **Step 2: Run the focused red-phase tests**

Run:

```bash
cargo test -p remi validate_scopes -- --exact
cargo test -p remi test_openapi_spec_returns_valid_json -- --exact
cargo test -p remi test_derive_action_ci -- --exact
```

Expected: at least one assertion fails because the CI scopes and routes still
exist.

- [ ] **Step 3: Remove the external admin CI surface**

Make these structural edits:

- remove `CiRead` and `CiTrigger` from `apps/remi/src/server/auth.rs`
- remove the `/v1/admin/ci/*` routes from `apps/remi/src/server/routes/admin.rs`
- remove `mod ci;` and `pub use ci::*;` from
  `apps/remi/src/server/handlers/admin/mod.rs`
- delete `apps/remi/src/server/handlers/admin/ci.rs`
- remove CI path entries from `apps/remi/src/server/handlers/openapi.rs`
- remove CI-specific audit action mapping in `apps/remi/src/server/audit.rs`
- update token and repos tests that currently use `ci:read` as a non-admin
  scope; replace it with another still-valid scope such as `repos:read`

- [ ] **Step 4: Re-run the focused tests and then the package tests**

Run:

```bash
cargo test -p remi validate_scopes -- --exact
cargo test -p remi test_openapi_spec_returns_valid_json -- --exact
cargo test -p remi test_derive_action_ci -- --exact
cargo test -p remi
```

Expected:

- focused tests pass with the new no-CI-surface expectations
- `cargo test -p remi` passes cleanly

- [ ] **Step 5: Commit**

```bash
git add apps/remi/src/server/auth.rs apps/remi/src/server/routes/admin.rs apps/remi/src/server/routes.rs apps/remi/src/server/handlers/admin/mod.rs apps/remi/src/server/handlers/admin/tokens.rs apps/remi/src/server/handlers/admin/repos.rs apps/remi/src/server/handlers/openapi.rs apps/remi/src/server/audit.rs
git add -A apps/remi/src/server/handlers/admin/ci.rs
git commit -m "refactor(remi): remove Forgejo admin CI surface"
```

### Task 7: Remove The Remaining Forgejo Client, MCP Tools, And Server Wiring

**Files:**
- Modify: `apps/remi/src/server/config.rs`
- Modify: `apps/remi/src/server/mod.rs`
- Modify: `apps/remi/src/server/mcp.rs`
- Delete: `apps/remi/src/server/forgejo.rs`
- Test: `apps/remi/src/server/mcp.rs`
- Test: `apps/remi/src/server/mod.rs`

- [ ] **Step 1: Write the failing assertions for the remaining Forgejo bridge**

Add or update focused tests so they assert:

- Remi config no longer exposes `forgejo_url` or `forgejo_token`
- the MCP tool list no longer contains `ci_list_workflows`, `ci_list_runs`,
  `ci_get_run`, `ci_get_logs`, `ci_dispatch`, or `ci_mirror_sync`
- server state no longer stores Forgejo configuration

If there is no direct MCP tool-list test today, add a small parser or text test
around the router/tool registration that fails while those tool names still
exist.

- [ ] **Step 2: Run the red-phase remi tests**

Run:

```bash
cargo test -p remi mcp -- --nocapture
cargo test -p remi config -- --nocapture
```

Expected: the new assertions fail while Forgejo config and MCP tools still
exist.

- [ ] **Step 3: Remove the remaining Forgejo bridge code**

Make these structural edits:

- delete `apps/remi/src/server/forgejo.rs`
- remove `pub mod forgejo;` and state fields such as `forgejo_url` and
  `forgejo_token` from `apps/remi/src/server/mod.rs`
- remove the admin-section `forgejo_url` and `forgejo_token` config fields from
  `apps/remi/src/server/config.rs`
- delete the Forgejo MCP tools from `apps/remi/src/server/mcp.rs`
- remove any startup-time wiring that copies Forgejo config into server state

- [ ] **Step 4: Re-run the focused tests and the full remi test suite**

Run:

```bash
cargo test -p remi mcp -- --nocapture
cargo test -p remi config -- --nocapture
cargo test -p remi
rg -n "forgejo|ci_list_workflows|ci_dispatch|ci_mirror_sync" apps/remi/src || true
```

Expected:

- the remi tests pass
- the grep shows no active Forgejo bridge code under `apps/remi/src`

- [ ] **Step 5: Commit**

```bash
git add apps/remi/src/server/config.rs apps/remi/src/server/mod.rs apps/remi/src/server/mcp.rs
git add -A apps/remi/src/server/forgejo.rs
git commit -m "refactor(remi): remove Forgejo control-plane bridge"
```

## Chunk 4: Hardening And Rollout

### Task 8: Pin Third-Party Actions And Move Security Audit Into The Trusted Lane

**Files:**
- Modify: `.github/workflows/pr-gate.yml`
- Modify: `.github/workflows/scheduled-ops.yml`
- Modify: `.github/workflows/release-build.yml`
- Modify: `.github/workflows/deploy-and-verify.yml`
- Test: `.github/workflows/*.yml`

- [ ] **Step 1: Record the currently unpinned action refs**

Run:

```bash
rg -n "uses: .*@(v|stable|main|master)" .github/workflows .github/actions
```

Expected: tag-based refs such as `@v4`, `@stable`, or `@v2` are still present.

- [ ] **Step 2: Pin each third-party action to a full commit SHA**

Pin every active workflow usage, including:

- `actions/checkout`
- `actions/cache`
- `dtolnay/rust-toolchain`
- `softprops/action-gh-release`
- GitHub dependency review action

Keep a short trailing comment with the human-friendly tag or release name for
maintainability.

- [ ] **Step 3: Keep `cargo audit` only in the trusted lane**

Ensure:

- `pr-gate.yml` has no `cargo audit`
- `scheduled-ops.yml` owns the `cargo audit` job and carries the explicit
  accepted ignore list already present today

- [ ] **Step 4: Verify the hardening pass**

Run:

```bash
git diff --check
rg -n "uses: .*@(v|stable|main|master)" .github/workflows .github/actions || true
rg -n "cargo audit" .github/workflows
```

Expected:

- no active workflow uses tag or branch refs for third-party actions
- `cargo audit` appears only in the trusted scheduled/manual lane

- [ ] **Step 5: Commit**

```bash
git add .github/workflows .github/actions
git commit -m "security(ci): pin actions and isolate cargo audit"
```

### Task 9: Run End-To-End Rollout Verification And Update The Docs To Match Reality

**Files:**
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `deploy/FORGE.md`
- Test: whole migration

- [ ] **Step 1: Run the code and script verification commands**

Run:

```bash
bash -n scripts/release.sh
bash -n deploy/setup-forge.sh
cargo test -p remi
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 2: Run the GitHub branch-safe workflow verification path**

Push the branch, then run:

```bash
gh workflow run pr-gate.yml --ref <branch-name>
gh workflow run merge-validation.yml --ref <branch-name>
gh workflow run scheduled-ops.yml --ref <branch-name>
gh workflow run release-build.yml --ref <branch-name> -f product=conary -f tag_name=test-v0.0.0 -f dry_run=true
gh workflow run deploy-and-verify.yml --ref <branch-name> -f product=conary -f source_run=<release-build-run-id> -f environment=staging -f dry_run=true
gh run list --limit 10
```

Expected:

- all five lane entrypoints are visible in GitHub
- the branch-safe manual dispatches succeed
- there are no active Forgejo workflows left to run

- [ ] **Step 3: Decommission the legacy Forgejo services on Forge**

After the new GitHub workflows have been dispatch-tested successfully, stop and
disable the retired services on Forge:

```bash
ssh forge 'sudo systemctl disable --now forgejo forgejo-runner || true'
ssh forge 'systemctl is-enabled forgejo forgejo-runner || true'
ssh forge 'systemctl is-active github-actions-runner || true'
```

Expected:

- `forgejo` and `forgejo-runner` are disabled or missing
- `github-actions-runner` remains active

- [ ] **Step 4: Run the final structural grep checks**

Run:

```bash
test ! -d .forgejo/workflows || test -z "$(find .forgejo/workflows -type f -print)"
test ! -f .github/workflows/ci.yml
test ! -f .github/workflows/release.yml
rg -n "develop" .github/workflows docs/operations/infrastructure.md docs/INTEGRATION-TESTING.md || true
rg -n "Forgejo|forgejo-runner|ci:read|ci:trigger" apps/remi/src deploy/FORGE.md docs/operations/infrastructure.md docs/INTEGRATION-TESTING.md || true
```

Expected:

- no active Forgejo workflow files remain
- no old `ci.yml` or `release.yml` remain
- `develop`, `Forgejo`, `ci:read`, and `ci:trigger` appear only in deliberate
  historical/archive references, not active implementation paths

- [ ] **Step 5: Commit**

```bash
git add docs/operations/infrastructure.md docs/INTEGRATION-TESTING.md deploy/FORGE.md
git commit -m "docs(ci): finalize GitHub control-plane migration"
```

- [ ] **Step 6: Tag the migration as complete in the execution notes**

Write a short execution summary in the final PR description or execution log:

- lanes created
- old workflows removed
- Forgejo bridge removed from Remi
- release taxonomy aligned
- verification commands run

## Final Verification Checklist

- `bash -n scripts/release.sh`
- `bash -n deploy/setup-forge.sh`
- `cargo test -p remi`
- `git diff --check`
- `gh workflow run pr-gate.yml --ref <branch-name>`
- `gh workflow run merge-validation.yml --ref <branch-name>`
- `gh workflow run scheduled-ops.yml --ref <branch-name>`
- `gh workflow run release-build.yml --ref <branch-name> -f product=conary -f tag_name=test-v0.0.0 -f dry_run=true`
- `gh workflow run deploy-and-verify.yml --ref <branch-name> -f product=conary -f source_run=<release-build-run-id> -f environment=staging -f dry_run=true`
- `ssh forge 'sudo systemctl disable --now forgejo forgejo-runner || true'`

## Notes For The Implementer

- Keep the first rollout small and legible. Do not add `cargo-nextest`,
  multi-runner autoscaling, or public `remi`/`conaryd` asset releases in the
  same slice unless the migration cannot land without them.
- Prefer replacing old workflow files rather than leaving duplicate control
  planes around "temporarily." The explicit goal is one orchestrator.
- When in doubt, favor a branch-testable `workflow_dispatch` path before
  changing the live automatic trigger.
- Preserve `conary-test` operational metadata, but treat the Cargo version as
  informational only. Commit/ref provenance is the real identity.
