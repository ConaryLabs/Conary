# Limited Public Release Readiness Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:executing-plans to implement this plan. If the user explicitly authorizes subagents, use superpowers:subagent-driven-development for independent chunks. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring Conary to a truthful, limited public preview state with green trusted validation, current local QEMU evidence, resolved release-facing security alerts, aligned supported-distro coverage, and accurate public/assistant/operator documentation.

**Architecture:** Treat release readiness as a sequence of gates, not one large cleanup. The original remote Forge validation lane is paused because the old VPS runner does not expose `/dev/kvm`; keep hosted health/audit/build/list checks active and use local KVM-backed QEMU evidence until a replacement runner exists. Then clear security/dependency alerts, align distro test coverage with the public support matrix, refresh docs, and run the final release dry run. conaryd package execution, ISO export, OCI convergence, production-wide unwrap cleanup, and broad redesign work are out of scope for this release gate and should be documented honestly as future work.

**Tech Stack:** Rust/Cargo workspace, Tokio services, rusqlite, conary-test integration harness, GitHub Actions, Forge self-hosted runner, Podman, QEMU, npm/Svelte/Vite, existing release scripts.

---

## Release Definition

This plan targets a limited public preview, not a broad stable release.

Release-ready means:

- `main` is green for the hosted `merge-validation` build/list/Remi smoke lane.
- `scheduled-ops` hosted health, audit, and manifest-inventory jobs are green.
- Local KVM-backed QEMU validation passes with `scripts/local-qemu-validation.sh`, and logs are recorded in the readiness note.
- Remote Forge-backed deep validation, QEMU, and conaryd staging are explicitly paused until a replacement KVM-capable runner exists.
- Public supported distros are exactly Fedora 44, Ubuntu 26.04 LTS, and Arch.
- Ubuntu 26.04 validation proves both the container image and Remi upstream package source are 26.04/`resolute`, not merely the family-level `ubuntu` route backed by Noble metadata.
- Noble/Ubuntu 24.04 is legacy for this release target. It must not remain in active build, test, release, Remi, site, or user-facing documentation paths except as an explicitly quarantined historical/internal note.
- Docs do not imply Debian/Linux Mint or conaryd install/remove/update support.
- Rust and frontend release-facing security alerts are fixed or explicitly documented with a narrow waiver.
- Release dry runs complete without changing tags unless the user explicitly approves the actual release.

Stop the plan if any critical validation cannot be made green. Record the blocker in `ROADMAP.md` and the final readiness notes instead of shipping around it.

## Remote Validation Discipline

Do local implementation on a release-readiness branch. Remote commands that use GitHub Actions only validate pushed Git refs, not unpushed local commits.

- Before remote validation, push the branch and use `--ref <branch>` for rehearsal runs when possible.
- Before claiming release readiness, merge the branch to `main`, `git fetch origin main`, rerun the hosted remote gates on `main`, and rerun local QEMU validation against the exact `main` commit.
- Capture the exact GitHub Actions run ID after every `gh workflow run`, then watch that run ID with `gh run watch <run-id> --exit-status`.
- Every workflow-dispatch validation must pass a unique `run_label` input and find the run by that label, not by "latest run on branch".
- Push the branch after every commit and before every remote workflow/deploy rehearsal.
- Record run IDs and URLs in the final readiness note.

## Temporary Forge Carve-Out

As of 2026-05-08, the old Forge VPS is retired and must not be used as release
evidence. The active temporary contract is:

- GitHub-hosted `merge-validation`: build release-facing binaries, list
  integration manifests, and run Remi smoke health.
- GitHub-hosted `scheduled-ops`: keep Remi full health, cargo audit, and
  manifest inventory alive; log an explicit warning that remote deep/QEMU is
  paused.
- Local QEMU release gate: run `scripts/local-qemu-validation.sh` on a
  development machine with `/dev/kvm`; preserve the log directory in the final
  readiness note.
- conaryd deploy mode is `none` until a replacement staging host exists.

## File Map

- Create: `scripts/forge-preflight.sh`
  - Host/runtime preflight for trusted Forge lanes. It must support `--mode container` for merge smoke and `--mode qemu` for virtualization jobs.
- Create: `deploy/repair-forge-runtime.sh`
  - Root-owned Forge runtime repair script for packages, linger, and rootless Podman socket without re-registering the GitHub runner.
- Modify: `deploy/setup-forge.sh`
  - Reuse the runtime repair logic during full runner setup, but do not require full runner re-registration for routine Podman/QEMU repair.
- Modify: `scripts/forge-smoke.sh`
  - Reuse the preflight and optionally assert the running service commit.
- Modify: `.github/workflows/merge-validation.yml`
  - Run Forge preflight before the smoke subset so infrastructure failures are clear.
- Modify: `.github/workflows/scheduled-ops.yml`
  - Run Forge preflight before deep/QEMU jobs, restore generation-export QEMU rotation, and align the distro matrix.
- Modify: `.github/workflows/release-build.yml`
  - Move the release DEB build container from Ubuntu 24.04 to Ubuntu 26.04 so release artifacts do not contradict the public support matrix.
- Modify: `deploy/FORGE.md`, `docs/operations/infrastructure.md`, `docs/INTEGRATION-TESTING.md`, `apps/conary-test/README.md`
  - Document the current Forge runner/service contract and integrated-test rotation.
- Modify: `apps/conary/tests/integration/remi/config.toml`
  - Align the integration distro key with `ubuntu-26.04`, or explicitly quarantine `ubuntu-noble` as legacy-only if Ubuntu 26.04 is blocked.
- Modify: `apps/conary-test/src/config/mod.rs`
  - Align embedded conary-test config fixtures/defaults with the Ubuntu 26.04 target.
- Modify: `apps/conary/tests/integration/remi/manifests/*.toml`
  - Replace or quarantine hard-coded `ubuntu-noble` distro overrides and setup commands.
- Create or modify: `apps/conary/tests/integration/remi/containers/Containerfile.ubuntu-26.04`
  - Ubuntu 26.04 integration image for the public distro matrix.
- Modify: `deploy/remi.toml.example`, `scripts/remi-health.sh`
  - Make the Ubuntu upstream release/codename check explicit enough to distinguish 26.04/`resolute` from Noble.
- Modify: `Cargo.toml`, `Cargo.lock`, crate-local `Cargo.toml` files as needed
  - Resolve Rust advisories.
- Create: `scripts/release-cargo-audit.sh`
  - Single release-audit command used by local verification and scheduled CI so advisory waivers cannot drift between environments.
- Modify: `site/package.json`, `site/package-lock.json`, `web/package.json`, `web/package-lock.json`
  - Resolve frontend advisories and rebuild lockfiles.
- Modify: `README.md`, `ROADMAP.md`, `CHANGELOG.md`, `SECURITY.md`, `CONTRIBUTING.md`, `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `.github/*`, `site/src/**`, `web/src/**`, `docs/llms/**`, `docs/ARCHITECTURE.md`, `docs/modules/source-selection.md`, `docs/conaryopedia-v2.md`
  - Public state, roadmap, supported distro, and release limitation cleanup.
- Modify: `apps/conaryd/src/bin/conaryd.rs`, `apps/conaryd/src/daemon/mod.rs`, `apps/conaryd/src/daemon/routes.rs`, `apps/conaryd/src/daemon/client.rs`
  - Make conaryd CLI help, rustdoc, and API-facing docs truthful about current package-operation support.
- Move or modify: all completed top-level dated plans under `docs/superpowers/plans/`
  - Keep only genuinely active plans at top level. Move completed implementation records to `docs/superpowers/plans/archive/`, or add an explicit retained-active rationale.
- Modify: `docs/superpowers/documentation-accuracy-audit-*.tsv`, `docs/superpowers/documentation-accuracy-audit-summary.md`
  - Refresh the doc audit ledger after the doc changes.

---

## Chunk 1: Restore Forge Integrated Testing

### Task 1.1: Add Forge Preflight Coverage

**Files:**
- Create: `scripts/forge-preflight.sh`
- Create: `deploy/repair-forge-runtime.sh`
- Modify: `deploy/setup-forge.sh`
- Modify: `scripts/forge-smoke.sh`
- Modify: `.github/workflows/merge-validation.yml`
- Modify: `.github/workflows/scheduled-ops.yml`
- Docs: `deploy/FORGE.md`, `docs/operations/infrastructure.md`

- [ ] **Step 1: Write the Forge preflight script**

Create `scripts/forge-preflight.sh` with checks for the expected rootless Podman socket and toolchain. Keep it read-only.

```bash
#!/usr/bin/env bash
set -euo pipefail

MODE="container"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="${2:-}"
      shift 2
      ;;
    -h|--help)
      echo "Usage: forge-preflight.sh [--mode container|qemu]"
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

case "$MODE" in
  container|qemu) ;;
  *) echo "invalid mode: ${MODE}" >&2; exit 1 ;;
esac

RUNNER_UID="${FORGE_RUNNER_UID:-$(id -u)}"
PODMAN_SOCKET="${PODMAN_SOCKET:-/run/user/${RUNNER_UID}/podman/podman.sock}"

fail() {
  echo "[forge-preflight] ERROR: $*" >&2
  exit 1
}

echo "[forge-preflight] checking Podman socket: ${PODMAN_SOCKET}"
test -S "${PODMAN_SOCKET}" || fail "missing Podman socket; enable linger and podman.socket for the runner user"

echo "[forge-preflight] checking Podman CLI"
command -v podman >/dev/null 2>&1 || fail "podman is not installed"
DOCKER_HOST="unix://${PODMAN_SOCKET}" podman info >/dev/null
curl --unix-socket "${PODMAN_SOCKET}" -fsS http://d/v1.41/_ping >/dev/null \
  || fail "Podman socket did not answer Docker-compatible API ping"

if [[ "${MODE}" == "qemu" ]]; then
  echo "[forge-preflight] checking QEMU tools"
  command -v qemu-system-x86_64 >/dev/null 2>&1 || fail "qemu-system-x86_64 is not installed"
  command -v qemu-img >/dev/null 2>&1 || fail "qemu-img is not installed"
  command -v scp >/dev/null 2>&1 || fail "scp is not installed"
  command -v rg >/dev/null 2>&1 || fail "ripgrep is not installed"
  test -e /dev/kvm || fail "/dev/kvm is missing; scheduled QEMU gates require a KVM-capable Forge runner"
fi

echo "[forge-preflight] ok"
```

- [ ] **Step 2: Run shell syntax check**

Run:

```bash
bash -n scripts/forge-preflight.sh
```

Expected: no output.

- [ ] **Step 3: Add a non-disruptive Forge runtime repair script**

Create `deploy/repair-forge-runtime.sh`. It must not stop, replace, or re-register the GitHub Actions runner. It only installs runtime dependencies and enables linger/rootless Podman for the runner user.

```bash
#!/usr/bin/env bash
set -euo pipefail

[[ $EUID -ne 0 ]] && { echo "This script must be run as root" >&2; exit 1; }

RUNNER_USER="${FORGE_RUNNER_USER:-peter}"

dnf install -y podman git curl tar jq gh ca-certificates qemu-system-x86 qemu-img openssh-clients edk2-ovmf ripgrep

runner_uid="$(id -u "$RUNNER_USER")"
loginctl enable-linger "$RUNNER_USER"
sudo -H -u "$RUNNER_USER" env XDG_RUNTIME_DIR="/run/user/${runner_uid}" systemctl --user enable --now podman.socket
sudo -H -u "$RUNNER_USER" test -S "/run/user/${runner_uid}/podman/podman.sock"
sudo -H -u "$RUNNER_USER" env DOCKER_HOST="unix:///run/user/${runner_uid}/podman/podman.sock" podman info >/dev/null
curl --unix-socket "/run/user/${runner_uid}/podman/podman.sock" -fsS http://d/v1.41/_ping >/dev/null
```

- [ ] **Step 4: Update Forge setup to reuse runtime repair**

In `deploy/setup-forge.sh`, make `install_packages` install the QEMU/OpenSSH dependencies listed above, and add a function after `ensure_rust`:

```bash
ensure_podman_socket() {
    local runner_uid
    runner_uid="$(id -u "$RUNNER_USER")"

    log "Enabling rootless Podman socket for ${RUNNER_USER}..."
    loginctl enable-linger "$RUNNER_USER"
    runner_shell "XDG_RUNTIME_DIR=/run/user/${runner_uid} systemctl --user enable --now podman.socket"
    runner_shell "test -S /run/user/${runner_uid}/podman/podman.sock"
    runner_shell "DOCKER_HOST=unix:///run/user/${runner_uid}/podman/podman.sock podman info >/dev/null"
    curl --unix-socket "/run/user/${runner_uid}/podman/podman.sock" -fsS http://d/v1.41/_ping >/dev/null
}
```

Call `ensure_podman_socket` before `configure_runner`.

- [ ] **Step 5: Extend smoke checks without leaking secrets**

Update `scripts/forge-smoke.sh`:

- Add `--expected-commit COMMIT`.
- Call `bash scripts/forge-preflight.sh --mode container` unless `CONARY_FORGE_SKIP_PREFLIGHT=1`.
- After reading deploy status JSON, compare `payload["binary"]["git_commit"]` to the expected commit when the flag is supplied.
- Do not print bearer tokens, process arguments, or environment variables.

- [ ] **Step 6: Wire preflight into trusted workflows**

Add an optional `run_label` workflow_dispatch input and top-level `run-name` to both `.github/workflows/merge-validation.yml` and `.github/workflows/scheduled-ops.yml`:

```yaml
run-name: ${{ github.workflow }} ${{ inputs.run_label || github.run_id }}
```

```yaml
      run_label:
        description: Unique label for deterministic release-readiness run lookup.
        required: false
        default: ""
```

In `.github/workflows/merge-validation.yml`, add before `Build trusted-lane binaries`:

```yaml
      - name: Forge preflight
        run: bash scripts/forge-preflight.sh --mode container
```

In `.github/workflows/scheduled-ops.yml`, add `--mode container` to `deep-validation` before builds and `--mode qemu` to the `qemu` job before builds. The health job can remain Remi-only unless it starts depending on Podman.

- [ ] **Step 7: Verify locally**

Run:

```bash
bash -n scripts/forge-preflight.sh scripts/forge-smoke.sh deploy/setup-forge.sh deploy/repair-forge-runtime.sh
git diff --check
```

Expected: syntax checks pass and `git diff --check` emits no whitespace errors.

- [ ] **Step 8: Commit Forge preflight work**

```bash
git add scripts/forge-preflight.sh scripts/forge-smoke.sh deploy/setup-forge.sh deploy/repair-forge-runtime.sh .github/workflows/merge-validation.yml .github/workflows/scheduled-ops.yml deploy/FORGE.md docs/operations/infrastructure.md
git commit -m "ops(forge): restore trusted-runner preflight"
```

### Task 1.2: Repair the Forge Host and Redeploy conary-test

**Files:**
- No code changes expected after Task 1.1.
- Operator verification: Forge host, GitHub Actions runner, conary-test service.

- [ ] **Step 1: Push the implementation ref before remote work**

Push the current branch before running commands that consume a remote Git ref:

```bash
branch="$(git branch --show-current)"
git push -u origin "${branch}"
```

Expected: the branch exists on GitHub and can be used with `--ref "${branch}"` for rehearsal runs. Do not use `--ref main` until the branch has been merged.

- [ ] **Step 2: Check that Forge is idle before host repair**

Run:

```bash
gh run list --workflow merge-validation.yml --status in_progress --limit 10
gh run list --workflow scheduled-ops.yml --status in_progress --limit 10
```

Expected: no trusted Forge jobs are in progress, or the user explicitly approves a maintenance window before continuing.

- [ ] **Step 3: Copy and apply the non-disruptive runtime repair**

Run on the local machine:

```bash
rsync -az deploy/repair-forge-runtime.sh peter@replacement.example:/tmp/conary-repair-forge-runtime.sh
ssh peter@replacement.example 'sudo bash /tmp/conary-repair-forge-runtime.sh'
```

Expected:

- `podman.socket` is enabled and active for user `peter`.
- `/run/user/1000/podman/podman.sock` exists.
- `qemu-system-x86_64`, `qemu-img`, and `scp` are installed.
- `github-actions-runner` remains active and was not re-registered.

- [ ] **Step 4: Verify rootless Podman directly**

```bash
ssh peter@replacement.example 'systemctl --user is-enabled podman.socket && systemctl --user is-active podman.socket && test -S /run/user/1000/podman/podman.sock && DOCKER_HOST=unix:///run/user/1000/podman/podman.sock podman info >/dev/null && curl --unix-socket /run/user/1000/podman/podman.sock -fsS http://d/v1.41/_ping >/dev/null && echo ok'
```

Expected: `enabled`, `active`, then `ok`.

- [ ] **Step 5: Redeploy conary-test from the pushed branch for rehearsal**

```bash
branch="$(git branch --show-current)"
FORGE_HOST=peter@replacement.example ./scripts/deploy-forge.sh --group control_plane --ref "${branch}"
```

Expected: rollout completes and restarts the `conary-test` service.

- [ ] **Step 6: Verify Forge service freshness for the rehearsed ref**

```bash
branch="$(git branch --show-current)"
git fetch origin "${branch}"
expected_commit="$(git rev-parse "origin/${branch}")"
ssh peter@replacement.example "cd /home/peter/Conary && bash scripts/forge-smoke.sh --expected-commit ${expected_commit}"
```

Expected: smoke passes and the running binary commit matches `origin/${branch}`.

- [ ] **Step 7: Record Forge status**

Capture the non-secret fields from:

```bash
ssh peter@replacement.example 'cd /home/peter/Conary && target/debug/conary-test --json deploy status --port 9090'
```

Expected:

- `degraded` is false.
- `binary.git_commit` matches the rehearsed branch commit.
- `checkout_matches_rollout` and `binary_matches_rollout` are true.

### Task 1.3: Restore QEMU Generation-Export Rotation

**Files:**
- Modify: `.github/workflows/scheduled-ops.yml`
- Docs: `docs/INTEGRATION-TESTING.md`, `apps/conary-test/README.md`

- [ ] **Step 1: Fix scheduled QEMU suite references**

In `.github/workflows/scheduled-ops.yml`, change the Group N QEMU command to the bare suite name so `conary-test` resolves it through the manifest directory:

```yaml
      - name: Run Group N QEMU tests
        run: cargo run -p conary-test -- run --distro fedora44 --suite phase3-group-n-qemu
```

- [ ] **Step 2: Add Group O to scheduled QEMU validation**

In `.github/workflows/scheduled-ops.yml`, keep Group N and add a separate step:

```yaml
      - name: Run Group O generation-export QEMU tests
        run: cargo run -p conary-test -- run --distro fedora44 --suite phase3-group-o-generation-export
```

- [ ] **Step 3: Add explicit not-skipped QEMU assertions**

Wrap the QEMU commands with `tee` and check the logs for expected boot markers and absence of skip text. For Group O, the workflow step should include checks equivalent to:

```bash
set -euo pipefail
cargo run -p conary-test -- run --distro fedora44 --suite phase3-group-o-generation-export | tee /tmp/conary-group-o-qemu.log
! rg -i 'qemu.*skipped|boot skipped|skipping qemu' /tmp/conary-group-o-qemu.log
rg 'installed-runtime-generation-export-booted|bootstrap-run-generation-export-booted' /tmp/conary-group-o-qemu.log
```

Add a matching not-skipped check for Group N using expected markers from `apps/conary/tests/integration/remi/manifests/phase3-group-n-qemu.toml`, such as `boot-verified`, `generation-a`, `generation-b`, `kernel-update-active`, and `fallback-generation`.

- [ ] **Step 4: Verify manifest inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: output includes `Generation Artifact Export QEMU` with 4 tests.

- [ ] **Step 5: Commit QEMU rotation fix**

```bash
git add .github/workflows/scheduled-ops.yml docs/INTEGRATION-TESTING.md apps/conary-test/README.md
git commit -m "ci: restore generation export qemu rotation"
```

- [ ] **Step 6: Run manual scheduled validation on the pushed branch**

```bash
branch="$(git branch --show-current)"
git push origin "${branch}"
label="release-readiness-scheduled-$(date +%Y%m%d%H%M%S)-${RANDOM}"
gh workflow run scheduled-ops.yml --ref "${branch}" -f run_label="${label}" -f run_deep_validation=true -f run_qemu=true
sleep 10
run_id="$(gh run list --workflow scheduled-ops.yml --branch "${branch}" --event workflow_dispatch --limit 20 --json databaseId,displayTitle --jq ".[] | select(.displayTitle | contains(\"${label}\")) | .databaseId" | head -n1)"
test -n "${run_id}"
gh run watch "${run_id}" --exit-status
gh run view "${run_id}" --json conclusion,url
```

Expected: health, audit, deep-validation, and qemu jobs all pass after the later security and distro chunks are complete. The QEMU logs must show the boot markers and must not contain QEMU skip messages. If audit or Ubuntu fails here before those chunks, continue to the relevant chunk and rerun this step afterward.

---

## Chunk 2: Clear Security and Dependency Release Gates

### Task 2.1: Fix Rust Security Advisories

**Files:**
- Create: `scripts/release-cargo-audit.sh`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify crate-local `Cargo.toml` files only if direct dependency versions must change.
- Create or modify: `docs/superpowers/release-security-waivers-2026-05-06.md`
- Modify: `.github/workflows/scheduled-ops.yml` if audit ignore comments need to point to the waiver file.

- [ ] **Step 1: Capture current Rust advisory graph**

Run:

```bash
cargo audit
cargo tree -i ml-dsa
cargo tree -i tough
rg -n 'RUSTSEC-|--ignore' .github/workflows/scheduled-ops.yml docs/superpowers docs/superpowers/reviews/archive
```

Expected: confirms the current advisory paths before edits. At the time this plan was written, `ml-dsa` arrived through `pgp -> rpm -> conary-core`, and `tough` alerts were visible in Dependabot. Also capture which existing `--ignore` ID maps to which crate/dependency path, even if the only prior explanation is in archived review material.

- [ ] **Step 2: Try semver-compatible updates first**

Run:

```bash
cargo update -p ml-dsa
cargo update -p tough
cargo audit
```

Expected: advisories disappear if patched versions are compatible with existing parent constraints.

- [ ] **Step 3: If semver update is insufficient, update parent crates**

Inspect available versions:

```bash
cargo info rpm
cargo info sigstore
cargo info tough
```

Then update the direct dependency that owns the vulnerable transitive dependency. Prefer the smallest version bump that removes the advisory.

- [ ] **Step 4: Document or remove every audit ignore**

Treat the existing scheduled audit ignores as release blockers until each one is either removed or documented in `docs/superpowers/release-security-waivers-2026-05-06.md`.

For every remaining ignored advisory, record:

- advisory ID,
- crate and dependency path,
- severity,
- whether the affected code is reachable in Conary,
- reason the ignore is acceptable for a limited preview,
- expiry condition or date.

Only keep an ignore for high/medium advisories if:

- the vulnerable code path is unreachable in Conary,
- the reason is written in the waiver file,
- the user explicitly approves the waiver before release.

- [ ] **Step 5: Add a single release cargo-audit wrapper**

Create `scripts/release-cargo-audit.sh` and move the scheduled ignore list there with comments that point to `docs/superpowers/release-security-waivers-2026-05-06.md`.

The script must run `cargo audit` with no ignores when no waivers remain. If approved waivers remain, it must include only those exact `--ignore` IDs.

Update `.github/workflows/scheduled-ops.yml` to run:

```bash
bash scripts/release-cargo-audit.sh
```

- [ ] **Step 6: Verify Rust dependency changes**

Run:

```bash
bash scripts/release-cargo-audit.sh
cargo test -p conary-core
cargo test -p conary
cargo test -p remi
cargo test -p conaryd
cargo test -p conary-test
cargo run -p conary-test -- list
```

Expected: all pass.

- [ ] **Step 7: Commit Rust advisory fix**

```bash
git add Cargo.toml Cargo.lock scripts/release-cargo-audit.sh .github/workflows/scheduled-ops.yml docs/superpowers/release-security-waivers-2026-05-06.md
git commit -m "security: refresh vulnerable rust dependencies"
```

If crate-local manifests changed, add those exact manifest paths. Do not use broad `git add crates apps`; inspect `git status --short` and stage only files changed for this task.

### Task 2.2: Fix Frontend Advisories

**Files:**
- Modify: `site/package.json`
- Modify: `site/package-lock.json`
- Modify: `web/package.json`
- Modify: `web/package-lock.json`

- [ ] **Step 1: Capture current npm advisories**

Run:

```bash
set -euo pipefail
(cd site && npm audit)
(cd web && npm audit)
```

Expected: current `vite`, `postcss`, and related alerts are visible before edits.

- [ ] **Step 2: Update vulnerable frontend dependencies**

In both `site/` and `web/`, update Vite/PostCSS-related lockfile entries using the least broad command that clears the alerts:

```bash
set -euo pipefail
(cd site && npm update vite postcss --save-dev)
(cd web && npm update vite postcss --save-dev)
```

If `npm audit` still reports advisories, use `npm audit fix` only after inspecting the proposed major changes.

- [ ] **Step 3: Verify frontend builds**

Run:

```bash
set -euo pipefail
(cd site && npm ci && npm run check && npm run build)
(cd web && npm ci && npm run check && npm run build)
```

Expected: both projects install from lockfile, type-check, and build.

- [ ] **Step 4: Commit frontend advisory fix**

```bash
git add site/package.json site/package-lock.json web/package.json web/package-lock.json
git commit -m "security: refresh frontend toolchain dependencies"
```

### Task 2.3: Verify GitHub Security Surface

**Files:**
- No code changes expected unless alerts remain and need documentation.

- [ ] **Step 1: Check Dependabot open alerts**

Run:

```bash
gh api 'repos/ConaryLabs/Conary/dependabot/alerts?state=open' --jq 'map({package:.dependency.package.name,severity:.security_advisory.severity,patched:.security_vulnerability.first_patched_version.identifier})'
```

Expected: no high or medium alerts remain for release-shipped code. Low alerts can remain only if documented and accepted.

- [ ] **Step 2: Re-run scheduled audit job deterministically**

Run:

```bash
branch="$(git branch --show-current)"
git push origin "${branch}"
label="release-readiness-audit-$(date +%Y%m%d%H%M%S)-${RANDOM}"
gh workflow run scheduled-ops.yml --ref "${branch}" -f run_label="${label}"
sleep 10
run_id="$(gh run list --workflow scheduled-ops.yml --branch "${branch}" --event workflow_dispatch --limit 20 --json databaseId,displayTitle --jq ".[] | select(.displayTitle | contains(\"${label}\")) | .databaseId" | head -n1)"
test -n "${run_id}"
gh run watch "${run_id}" --exit-status
gh run view "${run_id}" --json conclusion,url
```

Expected: `audit` job passes.

---

## Chunk 3: Align Supported Distros and Integration Coverage

### Task 3.1: Move Ubuntu Integration Target to 26.04

**Files:**
- Modify: `apps/conary/tests/integration/remi/config.toml`
- Modify: `apps/conary-test/src/config/mod.rs`
- Create: `apps/conary/tests/integration/remi/containers/Containerfile.ubuntu-26.04`
- Modify or retire: `apps/conary/tests/integration/remi/containers/Containerfile.ubuntu-noble`
- Modify: `apps/conary/tests/integration/remi/manifests/*.toml`
- Modify: `deploy/remi.toml.example`
- Modify: `scripts/remi-health.sh`
- Modify: `.github/workflows/scheduled-ops.yml`
- Modify: `.github/workflows/release-build.yml`
- Docs: `docs/INTEGRATION-TESTING.md`, `README.md`, `docs/modules/source-selection.md`, `docs/conaryopedia-v2.md`, `deploy/FORGE.md`

- [ ] **Step 1: Prove or implement the Remi Ubuntu 26.04 upstream switch**

Search every active reference to Noble:

```bash
rg -n 'ubuntu-noble|noble|Ubuntu 24\\.04|24\\.04|ubuntu:24\\.04|Debian 12|Ubuntu / Debian' deploy/remi.toml.example apps/conary/tests/integration/remi apps/conary-test scripts docs README.md site web .github
```

Expected: every active reference is either replaced with Ubuntu 26.04/`resolute` or explicitly quarantined as legacy coverage. Do not claim Ubuntu 26.04 support while `deploy/remi.toml.example` or production Remi configuration still points to `releases = ["noble"]` for the supported Ubuntu source.

Update the Remi example config to use the Ubuntu 26.04 codename:

```toml
[upstream.ubuntu]
releases = ["resolute"]
```

If production Remi cannot be moved to 26.04 during this plan, stop Chunk 3 and downgrade the public release claim to "Ubuntu support pending 26.04 Remi source migration."

- [ ] **Step 2: Verify Remi example config is no longer Noble**

After editing `deploy/remi.toml.example`, run:

```bash
rg -n 'releases = \\["resolute"\\]' deploy/remi.toml.example
test "$(rg -c 'noble|24\\.04' deploy/remi.toml.example || true)" = "0"
```

Expected: the Ubuntu upstream example uses `resolute`, and no Noble/24.04 references remain in `deploy/remi.toml.example`.

- [ ] **Step 3: Add release-aware Remi health checks**

Add new `scripts/remi-health.sh --full` validation so it checks not just family endpoints (`ubuntu`) but the configured Ubuntu release/codename backing the public support claim. Today the script only checks family paths such as `fedora`, `ubuntu`, and `arch`; this release-aware check is new functionality. The check can use Remi admin/repository metadata or a lightweight package metadata query, but it must prove that Ubuntu content is 26.04/`resolute`, not Noble.

Expected: `./scripts/remi-health.sh --full` fails if Remi is still serving Noble for the public Ubuntu lane.

- [ ] **Step 4: Update release build image**

In `.github/workflows/release-build.yml`, update the DEB release build container:

```yaml
container:
  image: docker.io/library/ubuntu:26.04
```

Then verify:

```bash
rg -n 'ubuntu:24\\.04|Ubuntu 24\\.04|noble' .github/workflows/release-build.yml
```

Expected: no active Noble/24.04 release-build references remain.

- [ ] **Step 5: Create Ubuntu 26.04 container image**

Create `Containerfile.ubuntu-26.04` from the current Noble image, with:

```dockerfile
FROM ubuntu:26.04
```

Keep package installation and conary-test setup equivalent unless Ubuntu 26.04 package names changed.

- [ ] **Step 6: Add `ubuntu-26.04` config key**

In `apps/conary/tests/integration/remi/config.toml`, add:

```toml
[distros.ubuntu-26.04]
remi_distro = "ubuntu"
repo_name = "ubuntu-remi"
test_package = "patch"
test_binary = "/usr/bin/patch"
test_package_2 = "nano"
test_binary_2 = "/usr/bin/nano"
test_package_3 = "jq"
test_binary_3 = "/usr/bin/jq"
```

Update `remove_default_repos` from `ubuntu-noble` to `ubuntu-26.04` only after the repository registry and Remi source have moved. If any internal repository name stays family-level (`ubuntu`), document that distinction; if any active source remains Noble, do not mark Ubuntu 26.04 release-ready.

- [ ] **Step 7: Update conary-test Rust config fixtures/defaults**

In `apps/conary-test/src/config/mod.rs`, update embedded test config that currently uses:

```toml
[distros.ubuntu-noble]
remi_distro = "ubuntu-noble"
repo_name = "conary-ubuntu-noble"
```

to the 26.04 target. Verify:

```bash
rg -n 'ubuntu-noble|conary-ubuntu-noble|noble|24\\.04' apps/conary-test/src/config/mod.rs
```

Expected: no active Noble references remain in conary-test source defaults/fixtures.

- [ ] **Step 8: Update manifest overrides and hard-coded suite setup**

Update every active manifest reference to `ubuntu-noble`, including:

- `apps/conary/tests/integration/remi/manifests/phase1-core.toml`
- `apps/conary/tests/integration/remi/manifests/phase3-group-g.toml`
- `apps/conary/tests/integration/remi/manifests/phase3-group-m.toml`
- `apps/conary/tests/integration/remi/manifests/phase4-group-b.toml`
- `apps/conary/tests/integration/remi/manifests/phase4-group-c.toml`
- `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
- `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`

Run this after edits:

```bash
rg -n 'ubuntu-noble|noble|Ubuntu 24\\.04|24\\.04' apps/conary/tests/integration/remi .github/workflows/scheduled-ops.yml docs/INTEGRATION-TESTING.md apps/conary-test/README.md
```

Expected: remaining hits are explicitly legacy/quarantined and not part of release validation.

- [ ] **Step 9: Update scheduled distro matrix**

In `.github/workflows/scheduled-ops.yml`, change:

```yaml
        distro: [fedora44, ubuntu-noble, arch]
```

to:

```yaml
        distro: [fedora44, ubuntu-26.04, arch]
```

- [ ] **Step 10: Keep or quarantine Noble explicitly**

If `ubuntu-noble` is retained, rename its docs as legacy/internal coverage and remove it from public release claims. Do not leave it looking like one of the supported release targets.

- [ ] **Step 11: Verify manifest parsing and affected Ubuntu suite expansion**

Run:

```bash
cargo run -p conary-test -- list
rg -n 'ubuntu-noble|noble|Ubuntu 24\\.04|24\\.04' apps/conary/tests/integration/remi .github/workflows/scheduled-ops.yml docs/INTEGRATION-TESTING.md apps/conary-test/README.md
```

Expected: manifest inventory loads and remaining Noble references are explicitly legacy/quarantined.

- [ ] **Step 12: Run affected Ubuntu 26.04 suites on Forge**

After Forge Podman is fixed:

```bash
branch="$(git branch --show-current)"
git push origin "${branch}"
label="release-readiness-ubuntu-$(date +%Y%m%d%H%M%S)-${RANDOM}"
gh workflow run merge-validation.yml --ref "${branch}" -f run_label="${label}" -f smoke_distro=ubuntu-26.04
sleep 10
run_id="$(gh run list --workflow merge-validation.yml --branch "${branch}" --event workflow_dispatch --limit 20 --json databaseId,displayTitle --jq ".[] | select(.displayTitle | contains(\"${label}\")) | .databaseId" | head -n1)"
test -n "${run_id}"
gh run watch "${run_id}" --exit-status
gh run view "${run_id}" --json conclusion,url
```

Also run targeted suites that had Ubuntu-specific overrides:

```bash
FORGE_HOST=peter@replacement.example ./scripts/deploy-forge.sh --group control_plane --ref "${branch}"
ssh peter@replacement.example 'cd /home/peter/Conary && cargo run -p conary-test -- run --suite phase3-group-g --distro ubuntu-26.04 --phase 3'
ssh peter@replacement.example 'cd /home/peter/Conary && cargo run -p conary-test -- run --suite phase3-group-m --distro ubuntu-26.04 --phase 3'
ssh peter@replacement.example 'cd /home/peter/Conary && cargo run -p conary-test -- run --suite phase4-group-b --distro ubuntu-26.04 --phase 4'
ssh peter@replacement.example 'cd /home/peter/Conary && cargo run -p conary-test -- run --suite phase4-group-c --distro ubuntu-26.04 --phase 4'
ssh peter@replacement.example 'cd /home/peter/Conary && cargo run -p conary-test -- run --suite phase4-group-d --distro ubuntu-26.04 --phase 4'
ssh peter@replacement.example 'cd /home/peter/Conary && cargo run -p conary-test -- run --suite phase4-group-e --distro ubuntu-26.04 --phase 4'
```

Expected: smoke and targeted suites pass against Ubuntu 26.04. If they fail because Ubuntu 26.04 image or upstream packages are unavailable, stop and mark Ubuntu 26.04 as a release blocker. Do not ship claiming Ubuntu 26.04 support until this passes.

- [ ] **Step 13: Commit distro test alignment**

```bash
git add apps/conary/tests/integration/remi/config.toml apps/conary-test/src/config/mod.rs apps/conary/tests/integration/remi/containers apps/conary/tests/integration/remi/manifests deploy/remi.toml.example scripts/remi-health.sh .github/workflows/scheduled-ops.yml .github/workflows/release-build.yml docs/INTEGRATION-TESTING.md README.md docs/modules/source-selection.md docs/conaryopedia-v2.md deploy/FORGE.md
git commit -m "test: align ubuntu integration target with supported lts"
```

### Task 3.2: Verify Public Distro Catalog and Remi Behavior

**Files:**
- Modify only if verification exposes drift: `crates/conary-core/src/repository/distro.rs`, `apps/conary/src/commands/distro.rs`, `README.md`, `docs/modules/source-selection.md`.

- [ ] **Step 1: Verify CLI distro catalog**

Run:

```bash
cargo run -p conary -- distro list
```

Expected output includes only:

- Fedora 44
- Ubuntu 26.04 LTS
- Arch Linux

- [ ] **Step 2: Verify supported-distro tests**

Run:

```bash
cargo test -p conary-core distro
cargo test -p conary distro
```

Expected: all distro catalog/source-selection tests pass.

- [ ] **Step 3: Verify Remi health and repo state**

Run:

```bash
./scripts/remi-health.sh --full
```

Expected: Remi health passes and proves the Ubuntu lane is backed by 26.04/`resolute`. If repository names are family-level (`ubuntu`) while test names are release-level (`ubuntu-26.04`), document that distinction in integration docs.

---

## Chunk 4: Refresh Public, Operator, Dev, and Assistant Docs

### Task 4.1: Correct Public Release Claims

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `CHANGELOG.md`
- Modify: `SECURITY.md`
- Modify: `CONTRIBUTING.md`
- Modify: `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`
- Modify: `.github/ISSUE_TEMPLATE/*.md`, `.github/PULL_REQUEST_TEMPLATE.md`, `.github/copilot-instructions.md`
- Modify: `site/src/**`, `web/src/**`, `site/README.md`, `web/README.md`
- Modify: `docs/llms/**`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/conaryopedia-v2.md`

- [ ] **Step 1: Update supported distro wording**

Search:

```bash
rg -n 'Debian|Linux Mint|Mint|ubuntu-noble|Ubuntu 24\\.04|Fedora, Arch, Ubuntu, Debian' README.md ROADMAP.md CONTRIBUTING.md SECURITY.md CHANGELOG.md AGENTS.md CLAUDE.md GEMINI.md .github docs docs/llms apps/conary-test deploy site/src web/src site/README.md web/README.md
```

Expected: every user-facing support claim is either Fedora 44, Ubuntu 26.04 LTS, Arch, or clearly marked as internal parser/legacy test coverage.

- [ ] **Step 2: Update site/install and GitHub template distro copy**

Explicitly update:

- `.github/ISSUE_TEMPLATE/bug_report.md` to use Ubuntu 26.04 instead of Ubuntu 24.04.
- `site/src/routes/install/+page.svelte` to remove `Ubuntu / Debian` and `Ubuntu 24.04+ / Debian 12+`, replacing them with Ubuntu 26.04 LTS wording.
- Any site meta description that says only generic Ubuntu if the surrounding page is a release support matrix.

- [ ] **Step 3: Update release status**

In `README.md` and `ROADMAP.md`, describe the project as a limited public preview candidate once this plan passes, with these caveats:

- conaryd package execution is not implemented and package routes return 501.
- ISO export and OCI convergence remain future work.
- aarch64/riscv64 generation export boot assets remain future work.
- Forge validation and security gates are part of the release criteria.

- [ ] **Step 4: Update host OS wording separately from client support**

Verify `docs/operations/infrastructure.md` host OS claims against the actual Remi host. If the Remi origin still runs Ubuntu 24.04, keep the factual host note but add that host OS is independent of supported client distro scope. If the host has moved, update the note.

- [ ] **Step 5: Update conaryopedia distro mapping examples**

In `docs/conaryopedia-v2.md`, remove Debian from user-facing distro support examples. In the package type detection section, replace wording like `ubuntu/debian -> DEB` with `ubuntu -> DEB`; if Debian parser support remains internal, label it as internal parser compatibility rather than public distro support.

- [ ] **Step 6: Add architecture caveat to frontend feature copy**

In `site/src/routes/features/+page.svelte`, keep broad bootstrap/build target wording if accurate, but add a concise caveat that generation artifact export is currently x86_64-only, with aarch64/riscv64 boot assets reserved for future work.

- [ ] **Step 7: Update changelog/security status**

In `CHANGELOG.md`, add an unreleased/public-preview readiness section summarizing:

- Remi async/blocking cleanup.
- Dynamic distro catalog.
- Forge test recovery.
- Security dependency refresh.
- Distro support matrix.

In `SECURITY.md`, ensure supported versions and reporting expectations match the limited preview.

- [ ] **Step 8: Verify doc examples**

Run:

```bash
rg -n 'conary distro list|Available distros|fedora-44|ubuntu-26.04|ubuntu-noble|Debian' README.md docs/conaryopedia-v2.md docs/modules/source-selection.md docs/INTEGRATION-TESTING.md
rg -n 'ISO export|--format iso|OCI export|--format oci|aarch64 generation export|riscv64 generation export|x86_64/aarch64/riscv64|package operations|install/remove/update|REST API for package operations' README.md ROADMAP.md docs apps/conaryd/src site/src web/src
rg -n 'Ubuntu 24\\.04|Debian 12|Ubuntu / Debian' site/src/routes/install .github docs README.md
```

Expected: examples match current command output and public support scope. ISO, OCI, non-x86_64 generation-export, and conaryd package-operation claims are either removed, marked reserved/future, or explicitly described as returning 501/not implemented.

### Task 4.2: Make conaryd Package Limits Honest Everywhere

**Files:**
- Modify: `apps/conaryd/src/bin/conaryd.rs`
- Modify: `apps/conaryd/src/daemon/mod.rs`
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Modify: `apps/conaryd/src/daemon/client.rs`
- Verify or modify: `apps/conaryd/src/daemon/auth.rs`
- Verify or modify: `apps/conaryd/src/daemon/jobs.rs`
- Modify: `README.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/conaryopedia-v2.md`
- Optional tests: `apps/conaryd/src/daemon/routes/transactions.rs` if help/route tests need adjustment.

- [ ] **Step 1: Update CLI help description**

Change the `conaryd` clap description from package-operation API wording to current truth:

```rust
/// Local Conary daemon with job queue, SSE progress streaming, and enhance-job
/// support. Install/remove/update package jobs are accepted only as explicit
/// Not Implemented responses until the daemon package executor is built.
```

- [ ] **Step 2: Verify route honesty remains intact**

Run:

```bash
cargo test -p conaryd test_package_routes_return_not_implemented
cargo test -p conaryd package_routes
```

Expected: package routes return 501 and do not look operational.

- [ ] **Step 3: Verify help text**

Run:

```bash
cargo run -p conaryd -- --help
```

Expected: help text does not claim install/remove/update execution support.

- [ ] **Step 4: Update README conaryd section explicitly**

In `README.md` around the conaryd section, replace wording that says the daemon provides a REST API for package operations with current truth:

- conaryd currently provides daemon scaffolding, queue/SSE plumbing, read/query routes, and enhance-job support.
- install/remove/update package routes intentionally return `501 Not Implemented`.
- package execution remains future work behind a shared operation executor.

- [ ] **Step 5: Verify rustdoc/API-facing conaryd text**

Run:

```bash
rg -n 'package operations|install/remove/update|Install|Remove|Update|Not Implemented|501|PolicyKit|job' apps/conaryd/src/daemon apps/conaryd/src/bin README.md docs/ARCHITECTURE.md docs/conaryopedia-v2.md
```

Expected: route/client/module docs describe install/remove/update package jobs as currently unavailable through conaryd, with direct 501 responses, while enhance jobs remain supported. PolicyKit action IDs and generic job enum names may remain as internal future-facing infrastructure only if nearby comments do not imply package execution is available today.

### Task 4.3: Archive or Reframe Completed Plans

**Files:**
- Move or modify: `docs/superpowers/plans/2026-04-10-all-tracks-release-hardening-plan.md`
- Move or modify: `docs/superpowers/plans/2026-04-16-conaryd-forge-staging-deployment-plan.md`
- Move or modify: `docs/superpowers/plans/2026-04-22-generation-artifact-export-unification-plan.md`
- Move or modify: `docs/superpowers/plans/2026-04-30-installed-runtime-generations-self-contained-plan.md`
- Move or modify: `docs/superpowers/plans/2026-05-02-audit-hardening-plan.md`
- Move or modify: `docs/superpowers/plans/2026-05-02-redesign-followups-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Triage every top-level dated plan**

For every top-level plan except this release-readiness plan:

- If an archive file with the same basename already exists, compare it before moving:

```bash
cmp -s docs/superpowers/plans/<name>.md docs/superpowers/plans/archive/<name>.md
```

If the files are identical, remove only the top-level duplicate with `git rm docs/superpowers/plans/<name>.md`. If they differ, merge the status/current-context changes into one archive copy, then remove the top-level copy. Do not let `git mv` fail halfway because the archive target already exists.
- If it is completed historical work, move it to `docs/superpowers/plans/archive/` with `git mv`.
- If it must remain active, add a status banner explaining why it is still active and what exact remaining task is live.
- For the May 2 plans, leave remaining future work called out as:
  - production unwrap cleanup,
  - conaryd package executor dedicated plan,
  - broader ISO/OCI/generation export follow-ups.

- [ ] **Step 2: Verify active plan tree**

Run:

```bash
find docs/superpowers/plans -maxdepth 1 -type f -name '*.md' -print | sort
```

Expected: only genuinely active plans remain at top level. At minimum, this release-readiness plan should remain; any other top-level plan must have an explicit active-status banner.

### Task 4.4: Refresh Documentation Audit Ledger

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Regenerate inventory**

Stage new or moved documentation paths before regenerating the inventory because `scripts/docs-audit-inventory.sh` uses `git ls-files`. For newly created docs that are not ready to commit yet, use intent-to-add:

```bash
git add -N docs/superpowers/plans/2026-05-06-limited-public-release-readiness-plan.md
git ls-files --error-unmatch docs/superpowers/plans/2026-05-06-limited-public-release-readiness-plan.md
```

For moved plan files, prefer `git mv` before this step so the inventory sees the archive path. Record previous paths in the ledger notes for moved records.

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 2: Update ledger rows**

Update `docs/superpowers/documentation-accuracy-audit-ledger.tsv` so every tracked doc path is covered, including this plan if it is tracked.

For newly tracked files, use the current path as `origin_path`. For moved historical plans, use the current archive path as `origin_path` after the baseline regeneration and mention the former top-level path in the `notes` column.

- [ ] **Step 3: Verify ledger**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
```

Expected: ledger check passes.

- [ ] **Step 4: Commit doc refresh**

```bash
git add README.md ROADMAP.md CHANGELOG.md SECURITY.md CONTRIBUTING.md AGENTS.md CLAUDE.md GEMINI.md .github docs site web apps/conaryd/src/bin/conaryd.rs apps/conaryd/src/daemon/mod.rs apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/client.rs apps/conary-test/README.md deploy/FORGE.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: refresh limited public release readiness"
```

---

## Chunk 5: Final Verification and Release Dry Run

### Task 5.1: Run Local Verification Gate

**Files:**
- No edits expected unless failures require a focused fix.

- [ ] **Step 1: Rust formatting and linting**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both pass.

- [ ] **Step 2: Rust tests**

Run:

```bash
cargo test -p conary-core
cargo test -p conary
cargo test -p remi
cargo test -p conaryd
cargo test -p conary-test
cargo run -p conary-test -- list
```

Expected: all pass and conary-test lists the generation-export QEMU suite.

- [ ] **Step 3: Frontend checks**

Run:

```bash
set -euo pipefail
(cd site && npm ci && npm run check && npm run build)
(cd web && npm ci && npm run check && npm run build)
```

Expected: both pass.

- [ ] **Step 4: Security checks**

Run:

```bash
set -euo pipefail
bash scripts/release-cargo-audit.sh
(cd site && npm audit)
(cd web && npm audit)
```

Expected: no high or medium release-facing advisories remain unless the user has approved a documented waiver.

- [ ] **Step 5: Commit final fixes if needed**

If any verification fix was required:

Inspect `git status --short`, stage only the exact files changed for the fix, then commit:

```bash
git add path/to/changed-file
git commit -m "fix: clear limited release verification"
```

### Task 5.2: Run Trusted Remote Verification Gate

**Files:**
- No edits expected unless failures require a focused fix.

- [ ] **Step 1: Run merge validation**

```bash
branch="$(git branch --show-current)"
git push origin "${branch}"
label="release-readiness-fedora-$(date +%Y%m%d%H%M%S)-${RANDOM}"
gh workflow run merge-validation.yml --ref "${branch}" -f run_label="${label}" -f smoke_distro=fedora44
sleep 10
run_id="$(gh run list --workflow merge-validation.yml --branch "${branch}" --event workflow_dispatch --limit 20 --json databaseId,displayTitle --jq ".[] | select(.displayTitle | contains(\"${label}\")) | .databaseId" | head -n1)"
test -n "${run_id}"
gh run watch "${run_id}" --exit-status
gh run view "${run_id}" --json conclusion,url
```

Expected: all jobs pass.

- [ ] **Step 2: Run merge validation for Ubuntu 26.04**

```bash
branch="$(git branch --show-current)"
git push origin "${branch}"
label="release-readiness-ubuntu-$(date +%Y%m%d%H%M%S)-${RANDOM}"
gh workflow run merge-validation.yml --ref "${branch}" -f run_label="${label}" -f smoke_distro=ubuntu-26.04
sleep 10
run_id="$(gh run list --workflow merge-validation.yml --branch "${branch}" --event workflow_dispatch --limit 20 --json databaseId,displayTitle --jq ".[] | select(.displayTitle | contains(\"${label}\")) | .databaseId" | head -n1)"
test -n "${run_id}"
gh run watch "${run_id}" --exit-status
gh run view "${run_id}" --json conclusion,url
```

Expected: smoke passes.

- [ ] **Step 3: Run hosted scheduled checks and local QEMU validation**

```bash
branch="$(git branch --show-current)"
git push origin "${branch}"
label="release-readiness-scheduled-$(date +%Y%m%d%H%M%S)-${RANDOM}"
gh workflow run scheduled-ops.yml --ref "${branch}" -f run_label="${label}" -f run_deep_validation=true -f run_qemu=false
sleep 10
run_id="$(gh run list --workflow scheduled-ops.yml --branch "${branch}" --event workflow_dispatch --limit 20 --json databaseId,displayTitle --jq ".[] | select(.displayTitle | contains(\"${label}\")) | .databaseId" | head -n1)"
test -n "${run_id}"
gh run watch "${run_id}" --exit-status
gh run view "${run_id}" --json conclusion,url
CONARY_LOCAL_VALIDATION_RUN_ID="release-readiness-${branch}-$(date +%Y%m%d%H%M%S)" scripts/local-qemu-validation.sh
```

Expected: hosted health, audit, and manifest-inventory checks pass. The local QEMU script runs Group N and Group O on a KVM-capable development machine, emits expected boot markers, and fails on any skip message. Record the local log directory in the readiness note.

- [ ] **Step 4: Verify recent run list**

```bash
gh run list --workflow merge-validation.yml --limit 5
gh run list --workflow scheduled-ops.yml --limit 5
```

Expected: newest manual runs are successful.

### Task 5.3: Release Dry Run

**Files:**
- No release commit/tag unless the user explicitly approves actual release execution.

- [ ] **Step 1: Check release matrix scripts**

Run:

```bash
bash scripts/check-release-matrix.sh
bash scripts/test-release-matrix.sh
```

Expected: both pass.

- [ ] **Step 2: Run product dry runs**

Run:

```bash
./scripts/release.sh conary --dry-run
./scripts/release.sh remi --dry-run
./scripts/release.sh conaryd --dry-run
./scripts/release.sh conary-test --dry-run
```

Expected: each product either reports no version-bumping commits or shows the expected next tag/changelog without mutating files.

- [ ] **Step 3: Verify no accidental mutations**

Run:

```bash
git status --short
git tag --points-at HEAD
```

Expected: clean working tree except intentional plan/docs changes already committed; no new release tag unless approved.

- [ ] **Step 4: Merge to main and repeat hosted/local gates**

Only after branch validation passes, merge to `main` through the repo's normal process, fetch the final state, and rerun Task 5.2 on `main`:

```bash
git fetch origin main
git switch main
git pull --ff-only origin main
```

Expected: final hosted `merge-validation`/`scheduled-ops` manual runs are green on `main`, and local QEMU evidence was generated from the exact `main` commit.

- [ ] **Step 5: Repeat release dry runs on main**

After the merge and final remote gates on `main`, rerun the dry runs from Step 2:

```bash
./scripts/release.sh conary --dry-run
./scripts/release.sh remi --dry-run
./scripts/release.sh conaryd --dry-run
./scripts/release.sh conary-test --dry-run
```

Expected: dry-run evidence reflects the exact `main` commit that would be released.

### Task 5.4: Final Readiness Decision

**Files:**
- Modify: `ROADMAP.md`
- Modify: `CHANGELOG.md`
- Create: `docs/superpowers/limited-public-release-readiness-2026-05-06.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Write final readiness note**

Create a short readiness note only after all gates pass. Include:

- exact commit,
- local verification commands,
- GitHub run IDs,
- local QEMU log directory and boot-marker evidence,
- explicit note that Forge-backed remote validation/conaryd staging is paused,
- remaining known limitations,
- release recommendation.

- [ ] **Step 2: Include the readiness note in the doc audit ledger**

Run:

```bash
git add -N docs/superpowers/limited-public-release-readiness-2026-05-06.md
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Update the ledger row for the readiness note, then verify:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

- [ ] **Step 3: Commit readiness note**

```bash
git add docs/superpowers/limited-public-release-readiness-2026-05-06.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md ROADMAP.md CHANGELOG.md
git commit -m "docs: record limited public release readiness"
```

- [ ] **Step 4: Verify clean tree after readiness note**

```bash
git status --short
```

Expected: clean working tree.

- [ ] **Step 5: Ask user for release decision**

Stop before real release commands. Present the evidence and ask whether to:

- cut the limited public preview release,
- open a draft release/PR,
- defer and keep hardening.

Do not run `./scripts/release.sh <product>` without `--dry-run`, create tags, or push release tags until the user explicitly approves.

---

## Future Work Explicitly Deferred

- conaryd package executor for install/remove/update jobs.
- Production-wide unwrap cleanup and eventual targeted `clippy::unwrap_used`.
- ISO export implementation.
- OCI generation export convergence.
- aarch64/riscv64 generation export boot assets.
- Broader distro expansion beyond Fedora 44, Ubuntu 26.04 LTS, and Arch.
- Long-term Forge freshness UX inside `conary-test deploy status` beyond the script-level expected-commit check.
