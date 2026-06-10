# Release And CI Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put test and policy gates in front of tag-triggered release artifacts and direct-main validation.

**Architecture:** Strengthen CI as configuration plus tested shell policy. Action pinning is enforced by enumeration, release-build jobs depend on validation, and merge-validation mirrors PR safety classes that do not require privileged infrastructure.

**Tech Stack:** GitHub Actions YAML, Bash validators, Cargo fmt/Clippy/tests.

---

## Design Source

- `docs/superpowers/specs/archive/2026-06-10-release-ci-safety-design.md`

## File Map

| Path | Purpose |
| --- | --- |
| `.github/workflows/release-build.yml` | Add validation dependency before build and publish jobs. |
| `.github/workflows/merge-validation.yml` | Mirror PR fmt, dependency, Clippy, and test classes where practical. |
| `.github/workflows/pr-gate.yml` | Wire release/deploy helper script tests if not already covered. |
| `scripts/check-github-action-runtimes.sh` | Fail on unpinned external action references. |
| `scripts/check-release-matrix.sh` | Keep release workflow policy checks aligned. |
| `scripts/test-release-matrix.sh` | Behavior tests for release matrix logic. |
| `scripts/test-remi-deploy-helper.sh` | Behavior tests for deploy helper logic. |

## Task 0: Baseline

- [ ] Run:

```bash
bash scripts/check-github-action-runtimes.sh
bash scripts/check-release-matrix.sh
bash scripts/test-release-matrix.sh
bash scripts/test-remi-deploy-helper.sh
```

Expected: all pass before edits.

## Task 1: Make Action Pin Checking Fail Closed

- [ ] Add test fixtures to `scripts/check-github-action-runtimes.sh` or a new script-local test helper that includes:

```yaml
steps:
  - uses: actions/checkout@v6
  - uses: ./.github/actions/setup-rust-workspace
  - uses: actions/cache@668228422ae6a00e4ad889ee87cd7109ec5666a7
```

- [ ] The fixture must fail for `actions/checkout@v6`, pass for the local action, and pass for the full-SHA cache reference.
- [ ] Rewrite the checker to enumerate every `uses:` line in:
  - `.github/workflows/*.yml`
  - `.github/actions/*/action.yml`
- [ ] Accept local actions that start with `./`.
- [ ] Accept external actions only when the ref matches `@[0-9a-f]{40}` or an explicit reviewed allowlist.
- [ ] Run `bash scripts/check-github-action-runtimes.sh`.

## Task 2: Add Release-Build Validation Dependency

- [ ] Add a `workspace-validation` job to `.github/workflows/release-build.yml`.
- [ ] The job should check out code, use `.github/actions/setup-rust-workspace`, and run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test --doc --workspace --verbose
```

- [ ] Add `needs: [prepare, workspace-validation]` or equivalent dependencies to release build jobs that produce publishable artifacts.
- [ ] Add `workspace-validation` to publish job dependencies where a publish job can otherwise run after a build-only dependency.
- [ ] Run `bash scripts/check-release-matrix.sh`.

## Task 3: Mirror Main-Push Validation

- [ ] In `.github/workflows/merge-validation.yml`, add jobs or steps for:
  - `cargo fmt --check`
  - dependency consistency check used by PR gate
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --exclude conary-test --verbose`
  - `cargo test -p conary-test --verbose`
  - `cargo test --doc --workspace --verbose`
- [ ] Keep the existing docs-truth and local-smoke jobs.
- [ ] Do not add QEMU/KVM TOML integration execution to hosted `ubuntu-latest`.

## Task 4: Wire Script Tests

- [ ] Add CI steps for:

```bash
bash scripts/test-release-matrix.sh
bash scripts/test-remi-deploy-helper.sh
```

- [ ] Place them in the cheapest job that already has shell tools and checked-out sources.
- [ ] If a test needs a package unavailable on GitHub-hosted runners, document that in the step and keep the local command in the final verification list.

## Task 5: Normalize Script Invocation

- [ ] Audit every helper script under `scripts/` that has a shebang but lacks the executable bit:

```bash
find scripts -maxdepth 1 -type f -name '*.sh' -print0 \
  | xargs -0 -I{} sh -c 'head -n 1 "$1" | grep -q "^#!" && [ ! -x "$1" ] && printf "%s\n" "$1"' sh {} \
  | LC_ALL=C sort
```

Expected current findings before this track:

```text
scripts/check-doc-audit-ledger.sh
scripts/check-github-action-runtimes.sh
scripts/docs-audit-inventory.sh
scripts/forge-container-cleanup.sh
scripts/forge-preflight.sh
scripts/forge-smoke.sh
```

- [ ] Decide whether those scripts should support direct execution. If yes, run:

```bash
chmod +x \
  scripts/check-doc-audit-ledger.sh \
  scripts/check-github-action-runtimes.sh \
  scripts/docs-audit-inventory.sh \
  scripts/forge-container-cleanup.sh \
  scripts/forge-preflight.sh \
  scripts/forge-smoke.sh
```

- [ ] If any script should stay `bash`-only, leave its mode unchanged and update docs/plans that mention it to consistently invoke it with `bash`.
- [ ] Run `git diff --summary` and confirm the executable-bit change is intentional if `chmod` was used.

## Task 6: Final Verification And Commit

- [ ] Run:

```bash
bash scripts/check-github-action-runtimes.sh
bash scripts/check-release-matrix.sh
bash scripts/test-release-matrix.sh
bash scripts/test-remi-deploy-helper.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test --doc --workspace --verbose
git diff --check
```

- [ ] Review `git diff --name-only`, then stage only the exact files changed by
  this track. Do not stage directories.
- [ ] Commit:

```bash
git commit -m "ci: gate release builds with workspace validation"
```

Do not use a broad directory add in a dirty worktree. The expected file set is
under `.github/workflows/` and `scripts/`, but the final stage list must come
from the actual diff.
