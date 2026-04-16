# Conaryd Forge Staging Deployment Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the direct GitHub-driven Forge staging deployment for `conaryd`, rerun `deploy-and-verify` against the already-published `conaryd-v0.6.0` release, and move `conaryd` from release-published/deployment-blocked to Forge staging deployment verified.

**Architecture:** Implement the first truthful `conaryd` deploy path entirely in checked-in assets: a root-owned systemd unit, a Unix-socket health verifier, a Forge-side install helper, pinned SSH trust material, and a GitHub workflow that stages the exact `source_run` bundle plus the matching deploy assets. Keep `conary-test` rollout-framework changes out of the critical path; this plan only covers the direct GitHub helper path and records rollout coherence as deferred follow-up work.

**Tech Stack:** GitHub Actions YAML, Bash, systemd unit files, `curl` plus `python3` JSON checks, release metadata from `scripts/release-matrix.sh`, and tracked docs under `deploy/` and `docs/`.

---

## Scope Decision

- This plan intentionally defers `deploy/forge-rollouts.toml` and `apps/conary-test/src/deploy/*.rs` changes.
- This plan uses the explicit bootstrap exception from the spec for the already-published `conaryd-v0.6.0` bundle (`source_run=24273700060`).
- This plan encodes the bootstrap exception mechanically: the workflow should accept an explicit `deploy_asset_ref` only for `source_run=24273700060`, and should fail closed if that override is supplied on any other run.
- Future `conaryd` releases must stage helper/verifier/unit assets from the same tagged repo revision as the bundle being deployed; the bootstrap exception is one-time only.
- The initial "no pre-existing `conaryd.service`" preflight applies only to the one-time operator bootstrap check. The steady-state helper preflight must tolerate an already-installed unit for later releases.

## File Map

**Create:**

- `deploy/systemd/conaryd.service`
- `scripts/conaryd-health.sh`
- `scripts/install-conaryd-on-forge.sh`
- `deploy/ssh/forge-known-hosts`
- `deploy/sudoers/conaryd-forge`

**Modify:**

- `.github/workflows/deploy-and-verify.yml`
- `scripts/check-release-matrix.sh`
- `deploy/FORGE.md`
- `docs/operations/infrastructure.md`
- `docs/superpowers/release-hardening-checklist-2026-04-10.md`

**Explicitly defer in this plan:**

- `deploy/forge-rollouts.toml`
- `apps/conary-test/src/deploy/manifest.rs`
- `apps/conary-test/src/deploy/plan.rs`
- `apps/conary-test/src/deploy/orchestrator.rs`
- `apps/conary-test/src/deploy/status.rs`
- `apps/conary-test/src/handlers.rs`

---

## Chunk 1: Forge Bootstrap Assets

### Task 1: Capture And Commit Trusted Forge Bootstrap Material

**Files:**

- Create: `deploy/ssh/forge-known-hosts`
- Create: `deploy/sudoers/conaryd-forge`

- [ ] **Step 1: Verify the trusted Forge host key from an already-trusted operator machine**

Run:

```bash
ssh-keygen -F forge.conarylabs.com -f ~/.ssh/known_hosts
```

Expected: one or more existing host-key lines for `forge.conarylabs.com` from a previously trusted interactive login. Do not use opportunistic CI-time `ssh-keyscan` as the source of truth.

- [ ] **Step 2: Confirm the bootstrap preflight assumptions on Forge before writing artifacts**

Run:

```bash
ssh peter@forge.conarylabs.com '
  set -euo pipefail
  sudo -n true
  test -f /var/lib/conary/conary.db
  ! systemctl cat conaryd.service >/dev/null 2>&1
  getenforce
'
```

Expected: passwordless `sudo -n` works, `/var/lib/conary/conary.db` exists, `conaryd.service` is absent for the bootstrap rerun, and SELinux mode is printed for operator awareness.

- [ ] **Step 3: Write `deploy/ssh/forge-known-hosts` with the verified key material**

```text
forge.conarylabs.com ssh-ed25519 <verified-host-key-from-trusted-known_hosts>
```

If `ssh-keygen -F` returns a hashed known-hosts entry, extract the key type and key material from that output and rewrite the tracked file with an explicit `forge.conarylabs.com ...` hostname prefix. Keep the file limited to the exact trusted host entry or entries needed by the workflow.

- [ ] **Step 4: Write the initial narrowed sudoers artifact**

```sudoers
# /etc/sudoers.d/conaryd-forge
Cmnd_Alias CONARYD_INSTALL = \
    /usr/bin/install -m 0755 * /usr/local/bin/conaryd, \
    /usr/bin/install -m 0644 * /etc/systemd/system/conaryd.service, \
    /usr/bin/rm -f /usr/local/bin/conaryd, \
    /usr/bin/rm -f /etc/systemd/system/conaryd.service, \
    /usr/bin/systemctl daemon-reload, \
    /usr/bin/systemctl restart conaryd
Cmnd_Alias CONARYD_VERIFY = \
    /usr/bin/curl --fail --silent --show-error --unix-socket /run/conary/conaryd.sock http\://localhost/health

peter ALL=(root) NOPASSWD: CONARYD_INSTALL, CONARYD_VERIFY
Defaults!CONARYD_INSTALL !requiretty
Defaults!CONARYD_VERIFY !requiretty
```

Keep all commands as absolute paths. Do not "simplify" this to PATH-based commands or broad wildcard copies; the point is to allow only fixed-destination installs and one exact socket-health probe.

- [ ] **Step 5: Verify the tracked bootstrap artifacts exist and contain the expected anchors**

Run:

```bash
test -f deploy/ssh/forge-known-hosts
test -f deploy/sudoers/conaryd-forge
rg -n "forge.conarylabs.com|CONARYD_INSTALL|CONARYD_VERIFY|/run/conary/conaryd.sock" deploy/ssh/forge-known-hosts deploy/sudoers/conaryd-forge
```

Expected: all checks pass and the pinned host plus narrow command surface are visible in the tracked files.

- [ ] **Step 6: Commit the bootstrap artifacts**

```bash
git add deploy/ssh/forge-known-hosts deploy/sudoers/conaryd-forge
git commit -m "feat(deploy): add conaryd Forge bootstrap trust material"
```

### Task 2: Add The Forge `conaryd` Systemd Unit

**Files:**

- Create: `deploy/systemd/conaryd.service`

- [ ] **Step 1: Prove the unit file does not exist yet**

Run:

```bash
test ! -f deploy/systemd/conaryd.service
```

Expected: exit code `0`.

- [ ] **Step 2: Add the checked-in systemd unit with the exact first-pass runtime shape**

```ini
[Unit]
Description=Conary daemon (Forge staging)
After=network.target

[Service]
Type=notify
NotifyAccess=main
TimeoutStartSec=180
User=root
Group=root
RuntimeDirectory=conary
StateDirectory=conary
ExecStart=/usr/local/bin/conaryd --db /var/lib/conary/conary.db --socket /run/conary/conaryd.sock
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Do not pass `--tcp`.

- [ ] **Step 3: Run an optional local syntax sanity check**

Run:

```bash
if command -v systemd-analyze >/dev/null 2>&1; then
  systemd-analyze verify "$(pwd)/deploy/systemd/conaryd.service" || true
fi
```

Expected: if local `systemd-analyze` is available, there should be no syntax or unknown-lvalue errors. Loader-path warnings from verifying a checked-in file outside `/etc/systemd/system` are advisory only; the authoritative validation is the Forge-side helper restart plus successful health verification.

- [ ] **Step 4: Commit the new unit**

```bash
git add deploy/systemd/conaryd.service
git commit -m "feat(systemd): add conaryd Forge staging unit"
```

### Task 3: Add The Unix-Socket Health Verifier

**Files:**

- Create: `scripts/conaryd-health.sh`

- [ ] **Step 1: Prove the verifier script does not exist yet**

Run:

```bash
test ! -f scripts/conaryd-health.sh
```

Expected: exit code `0`.

- [ ] **Step 2: Write the checked-in verifier with explicit expected-version input**

```bash
#!/usr/bin/env bash
set -euo pipefail

EXPECTED_VERSION=""
SOCKET_PATH="/run/conary/conaryd.sock"
SERVICE_NAME="conaryd"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --expected-version) EXPECTED_VERSION="${2:-}"; shift 2 ;;
    --expected-version=*) EXPECTED_VERSION="${1#*=}"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$EXPECTED_VERSION" ]] || { echo "--expected-version is required" >&2; exit 1; }

if [[ "$(systemctl is-active "$SERVICE_NAME" 2>/dev/null || true)" != "active" ]]; then
  echo "service-not-running: ${SERVICE_NAME}" >&2
  exit 1
fi

PAYLOAD="$(sudo -n /usr/bin/curl --fail --silent --show-error \
  --unix-socket "$SOCKET_PATH" http://localhost/health)"

python3 - "$EXPECTED_VERSION" "$PAYLOAD" <<'PY'
import json
import sys

expected = sys.argv[1]
payload = json.loads(sys.argv[2])

if payload.get("status") != "healthy":
    raise SystemExit(f"unexpected status: {payload!r}")
if payload.get("version") != expected:
    raise SystemExit(
        f"version mismatch: expected {expected}, got {payload.get('version')}"
    )
PY

echo "[conaryd-health] ok"
```

Keep the verifier's `sudo -n /usr/bin/curl ...` argv exactly in sync with the `CONARYD_VERIFY` sudoers alias above. Do not wrap the whole verifier script in `sudo`; the intent is to keep elevation limited to the fixed socket probe.

- [ ] **Step 3: Make the verifier executable in git and on the runner**

Run:

```bash
chmod +x scripts/conaryd-health.sh
git add --chmod=+x scripts/conaryd-health.sh
```

Expected: the script is tracked with mode `100755`, matching the repo's existing executable-script convention under `scripts/`.

- [ ] **Step 4: Syntax-check the verifier**

Run:

```bash
bash -n scripts/conaryd-health.sh
```

Expected: exit code `0`.

- [ ] **Step 5: Commit the verifier**

```bash
git commit -m "feat(deploy): add conaryd Unix-socket health verifier"
```

### Task 4: Add The Forge Install Helper With Rollback

**Files:**

- Create: `scripts/install-conaryd-on-forge.sh`

- [ ] **Step 1: Prove the helper script does not exist yet**

Run:

```bash
test ! -f scripts/install-conaryd-on-forge.sh
```

Expected: exit code `0`.

- [ ] **Step 2: Write the helper around a fixed remote staging directory and explicit arguments**

```bash
#!/usr/bin/env bash
set -euo pipefail

STAGING_DIR=""
VERSION=""
EXPECTED_SHA256=""
BUNDLE_PATH=""
UNIT_PATH=""
VERIFIER_PATH=""
PREVIOUS_VERSION=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --staging-dir) STAGING_DIR="${2:-}"; shift 2 ;;
    --bundle) BUNDLE_PATH="${2:-}"; shift 2 ;;
    --expected-version) VERSION="${2:-}"; shift 2 ;;
    --expected-sha256) EXPECTED_SHA256="${2:-}"; shift 2 ;;
    --unit-file) UNIT_PATH="${2:-}"; shift 2 ;;
    --verifier) VERIFIER_PATH="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$STAGING_DIR" && -n "$BUNDLE_PATH" && -n "$VERSION" && -n "$EXPECTED_SHA256" && -n "$UNIT_PATH" && -n "$VERIFIER_PATH" ]] \
  || { echo "missing required arguments" >&2; exit 1; }

sudo -n true
test -d "$STAGING_DIR" || { echo "missing staging dir: $STAGING_DIR" >&2; exit 1; }
test -f /var/lib/conary/conary.db || { echo "missing /var/lib/conary/conary.db" >&2; exit 1; }

actual_sha="$(sha256sum "$BUNDLE_PATH" | awk '{print $1}')"
[[ "$actual_sha" == "$EXPECTED_SHA256" ]] || {
  echo "bundle hash mismatch: expected $EXPECTED_SHA256 got $actual_sha" >&2
  exit 1
}

tmpdir="$(mktemp -d "${STAGING_DIR}/install.XXXXXX")"
backup_bin="${tmpdir}/conaryd.previous"
backup_unit="${tmpdir}/conaryd.service.previous"
had_previous_bin=false
had_previous_unit=false

cleanup() { rm -rf "$tmpdir"; }
trap cleanup EXIT

if [[ -f /usr/local/bin/conaryd ]]; then
  cp /usr/local/bin/conaryd "$backup_bin"
  PREVIOUS_VERSION="$("$backup_bin" --version 2>/dev/null | awk '{print $2}' || true)"
  had_previous_bin=true
fi

if [[ -f /etc/systemd/system/conaryd.service ]]; then
  cp /etc/systemd/system/conaryd.service "$backup_unit"
  had_previous_unit=true
fi

tar xzf "$BUNDLE_PATH" -C "$tmpdir"
sudo -n install -m 0755 "${tmpdir}/conaryd-${VERSION}-linux-x64" /usr/local/bin/conaryd
sudo -n install -m 0644 "$UNIT_PATH" /etc/systemd/system/conaryd.service
sudo -n systemctl daemon-reload
sudo -n systemctl restart conaryd

if ! bash "$VERIFIER_PATH" --expected-version "$VERSION"; then
  if [[ "$had_previous_bin" == true ]]; then
    sudo -n install -m 0755 "$backup_bin" /usr/local/bin/conaryd
  else
    sudo -n rm -f /usr/local/bin/conaryd
  fi
  if [[ "$had_previous_unit" == true ]]; then
    sudo -n install -m 0644 "$backup_unit" /etc/systemd/system/conaryd.service
  else
    sudo -n rm -f /etc/systemd/system/conaryd.service
  fi
  sudo -n systemctl daemon-reload
  if [[ "$had_previous_bin" == true ]]; then
    sudo -n systemctl restart conaryd || true
    if [[ -n "$PREVIOUS_VERSION" ]]; then
      bash "$VERIFIER_PATH" --expected-version "$PREVIOUS_VERSION" || true
    fi
  else
    echo "no rollback target existed" >&2
  fi
  systemctl status --no-pager conaryd || true
  exit 1
fi
```

Implementation notes:

- helper preflight should assert `sudo -n true`, the staged files, and `/var/lib/conary/conary.db`, but should **not** assert that `conaryd.service` is absent; that absence check is bootstrap-operator-only
- keep `/var/lib/conary/conary.db` in place
- use plain `test -f` and plain `cp` for reading `/usr/local/bin/conaryd` and `/etc/systemd/system/conaryd.service`; those paths are readable without sudo on Forge and should not be part of the privileged allowlist
- print `systemctl status --no-pager conaryd` on failed restart or failed verification
- rerun the verifier after rollback against the restored service's version, not the new version that just failed

- [ ] **Step 3: Tighten the helper so rollback verifies the restored service explicitly**

Adjust the rollback branch so it records the previous binary version before overwrite and reruns:

```bash
bash "$VERIFIER_PATH" --expected-version "$PREVIOUS_VERSION" || true
```

If no previous binary existed, print a clear "no rollback target existed" message and remove the newly staged managed files before exit.

- [ ] **Step 4: Make the helper executable in git**

Run:

```bash
chmod +x scripts/install-conaryd-on-forge.sh
git add --chmod=+x scripts/install-conaryd-on-forge.sh
```

Expected: the helper is tracked with mode `100755`, and the Step 2 helper snippet already uses `bash "$VERIFIER_PATH" ...` for both normal and rollback verification so the flow stays robust even if an SCP path or checkout mode ever drops the executable bit on Forge.

- [ ] **Step 5: Syntax-check both scripts together**

Run:

```bash
bash -n scripts/conaryd-health.sh scripts/install-conaryd-on-forge.sh
```

Expected: exit code `0`.

- [ ] **Step 6: Commit the helper work**

```bash
git commit -m "feat(deploy): add conaryd Forge install helper"
```

---

## Chunk 2: GitHub Deploy Integration

### Task 5: Replace The Inline `deploy-conaryd` Lane With The Checked-In Helper Path

**Files:**

- Modify: `.github/workflows/deploy-and-verify.yml`

- [ ] **Step 1: Run the existing workflow guard script and capture the current baseline**

Run:

```bash
bash scripts/check-release-matrix.sh
```

Expected: pass before the workflow edits.

- [ ] **Step 2: Add checkout to `deploy-conaryd` so the job can read tracked deploy assets**

Add an optional `workflow_dispatch` input such as `deploy_asset_ref`, then extend `resolve` so it emits both `deploy_asset_ref` and `bootstrap_exception`.

The resolve logic should be:

- for the one-time bootstrap rerun only:
  - require `source_run=24273700060`
  - require `tag_name=conaryd-v0.6.0`
  - require a non-empty `deploy_asset_ref`
  - set `bootstrap_exception=true`
- for all other runs:
  - require `deploy_asset_ref` to be empty
  - set `deploy_asset_ref="$tag_name"`
  - set `bootstrap_exception=false`

This keeps the bootstrap exception mechanically limited instead of relying on operator memory.

Place this logic inside the existing `resolve` job's `Read release metadata` / `meta` shell step, after `tag_name` has been read from `metadata.json` and before the step writes outputs to `"$GITHUB_OUTPUT"`. Do not move it into a separate step, because the snippet depends on the local `tag_name` shell variable already computed inside `meta`.

Expected YAML shape:

```yaml
on:
  workflow_dispatch:
    inputs:
      deploy_asset_ref:
        description: Optional repo ref for checked-in deploy assets.
        required: false
```

```yaml
jobs:
  resolve:
    outputs:
      deploy_asset_ref: ${{ steps.meta.outputs.deploy_asset_ref }}
      bootstrap_exception: ${{ steps.meta.outputs.bootstrap_exception }}
```

```bash
manual_deploy_asset_ref="${{ github.event.inputs.deploy_asset_ref || '' }}"

if [[ "$SOURCE_RUN" == "24273700060" && "$tag_name" == "conaryd-v0.6.0" ]]; then
  [[ -n "$manual_deploy_asset_ref" ]] || {
    echo "bootstrap rerun requires deploy_asset_ref" >&2
    exit 1
  }
  deploy_asset_ref="$manual_deploy_asset_ref"
  bootstrap_exception="true"
else
  [[ -z "$manual_deploy_asset_ref" ]] || {
    echo "deploy_asset_ref is bootstrap-only and may only be used with source_run=24273700060" >&2
    exit 1
  }
  deploy_asset_ref="$tag_name"
  bootstrap_exception="false"
fi

echo "deploy_asset_ref=${deploy_asset_ref}" >> "$GITHUB_OUTPUT"
echo "bootstrap_exception=${bootstrap_exception}" >> "$GITHUB_OUTPUT"
```

For `workflow_run` events, `github.event.inputs.deploy_asset_ref` is empty, so this logic should always fall through to the steady-state branch and set `deploy_asset_ref="$tag_name"`.

- [ ] **Step 3: Check out the deploy assets at the resolved asset ref**

Use the same pinned checkout action already used in `release-build`, but point it at the resolved deploy-asset revision:

```yaml
- uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
  with:
    fetch-depth: 0
    ref: ${{ needs.resolve.outputs.deploy_asset_ref }}
```

- [ ] **Step 4: Replace `CONARYD_VERIFY_URL` and opportunistic host trust with pinned assets**

The job should:

- drop `CONARYD_VERIFY_URL`
- keep `CONARYD_SSH_KEY` and `CONARYD_SSH_TARGET`
- write the private key to `~/.ssh/conaryd_key`
- use `UserKnownHostsFile=deploy/ssh/forge-known-hosts`
- use `StrictHostKeyChecking=yes`
- fail before SCP/SSH if the host key mismatches

Expected YAML shape:

```yaml
ssh_opts=(
  -i ~/.ssh/conaryd_key
  -o UserKnownHostsFile=deploy/ssh/forge-known-hosts
  -o StrictHostKeyChecking=yes
)
```

- [ ] **Step 5: Stage the bundle plus the matching checked-in deploy assets into a fixed remote directory**

Resolve the bundle from the downloaded artifact directory and the helper assets from the checked-out repo, then compute the expected hash on the runner:

```bash
bundle_dir="$(find source-artifacts -type d -name "$BUNDLE_NAME" -print -quit)"
bundle="$(find "$bundle_dir" -maxdepth 1 -name "conaryd-${VERSION}-linux-x64.tar.gz" -print -quit)"
helper="${GITHUB_WORKSPACE}/scripts/install-conaryd-on-forge.sh"
verifier="${GITHUB_WORKSPACE}/scripts/conaryd-health.sh"
unit="${GITHUB_WORKSPACE}/deploy/systemd/conaryd.service"
EXPECTED_SHA256="$(sha256sum "$bundle" | awk '{print $1}')"
remote_stage="/var/tmp/conaryd-deploy-${VERSION}"
```

Do **not** copy the checked-in assets into the downloaded bundle directory locally. Stage them from their real source paths:

```bash
ssh "${ssh_opts[@]}" "$CONARYD_SSH_TARGET" "rm -rf '${remote_stage}' && mkdir -p '${remote_stage}'"
scp "${ssh_opts[@]}" \
  "$bundle" \
  "$helper" \
  "$verifier" \
  "$unit" \
  "${CONARYD_SSH_TARGET}:${remote_stage}/"
```

- [ ] **Step 6: Invoke the helper with explicit version and integrity arguments**

Expected remote call shape:

```bash
ssh "${ssh_opts[@]}" "$CONARYD_SSH_TARGET" \
  "bash ${remote_stage}/install-conaryd-on-forge.sh \
    --staging-dir ${remote_stage} \
    --bundle ${remote_stage}/conaryd-${VERSION}-linux-x64.tar.gz \
    --expected-version ${VERSION} \
    --expected-sha256 ${EXPECTED_SHA256} \
    --unit-file ${remote_stage}/conaryd.service \
    --verifier ${remote_stage}/conaryd-health.sh"
```

- [ ] **Step 7: Keep `verify-conaryd` dry-run simple**

Leave the dry-run lane artifact-oriented: it should confirm the tarball exists, print the resolved `deploy_asset_ref`, and show the exact checked-in asset paths it would stage, but it should not try to SSH to Forge in dry-run mode.

- [ ] **Step 8: Re-run the workflow guard script**

Run:

```bash
bash scripts/check-release-matrix.sh
```

Expected: pass after the workflow edits.

- [ ] **Step 9: Commit the workflow rewrite**

```bash
git add .github/workflows/deploy-and-verify.yml
git commit -m "feat(ci): deploy conaryd via Forge helper path"
```

### Task 6: Teach The Workflow Guard Script About The New Conaryd Path

**Files:**

- Modify: `scripts/check-release-matrix.sh`

- [ ] **Step 1: Add guard checks for the new `conaryd` deploy behavior**

Require patterns that prove:

- `deploy-conaryd` contains `actions/checkout`
- the workflow exposes and resolves `deploy_asset_ref`
- the workflow hard-limits the bootstrap exception to `source_run=24273700060`
- `CONARYD_VERIFY_URL` is absent
- the workflow references `deploy/ssh/forge-known-hosts`
- the workflow references `StrictHostKeyChecking=yes`
- the workflow stages `scripts/install-conaryd-on-forge.sh`
- the workflow stages `scripts/conaryd-health.sh`
- the workflow stages `deploy/systemd/conaryd.service`
- the workflow computes `EXPECTED_SHA256` on the runner
- the workflow creates the remote staging directory before SCP

Expected additions:

```bash
require_match "$deploy_workflow" 'deploy_asset_ref' 'bootstrap-only deploy asset ref input'
require_match "$deploy_workflow" 'bootstrap_exception' 'bootstrap exception resolve output'
require_match "$deploy_workflow" '24273700060' 'one-time conaryd bootstrap exception gate'
require_match "$deploy_workflow" 'ref: \$\{\{ needs\.resolve\.outputs\.deploy_asset_ref \}\}' 'deploy assets checked out from resolved asset ref'
require_match "$deploy_workflow" 'deploy/ssh/forge-known-hosts' 'pinned Forge host trust'
require_match "$deploy_workflow" 'StrictHostKeyChecking=yes' 'strict host-key checking for conaryd'
require_match "$deploy_workflow" 'scripts/install-conaryd-on-forge\.sh' 'checked-in conaryd helper staging'
require_match "$deploy_workflow" 'EXPECTED_SHA256="\$\(sha256sum "\$bundle" \| awk' 'runner-side conaryd bundle hash computation'
require_match "$deploy_workflow" "mkdir -p '\\\$\\{remote_stage\\}'" 'remote staging directory creation'
forbid_match "$deploy_workflow" 'CONARYD_VERIFY_URL' 'legacy public verify URL'
```

- [ ] **Step 2: Run the guard script again**

Run:

```bash
bash scripts/check-release-matrix.sh
```

Expected: pass with the new conaryd-specific assertions.

- [ ] **Step 3: Commit the guard updates**

```bash
git add scripts/check-release-matrix.sh
git commit -m "test(ci): guard conaryd Forge deploy workflow"
```

---

## Chunk 3: Docs, Verification, And Bootstrap Rerun

### Task 7: Update The Tracked Operator Docs

**Files:**

- Modify: `deploy/FORGE.md`
- Modify: `docs/operations/infrastructure.md`

- [ ] **Step 1: Update `deploy/FORGE.md` to describe `conaryd` as a local-only staging system service on Forge**

Cover:

- Forge now hosts `conaryd` as a local-only staging daemon
- verification uses `scripts/conaryd-health.sh`
- deploys come from `deploy-and-verify`, not ad hoc public endpoint curls
- host bootstrap prerequisites are `deploy/ssh/forge-known-hosts` and `deploy/sudoers/conaryd-forge`

- [ ] **Step 2: Update `docs/operations/infrastructure.md` to reflect the real release/deploy path**

Cover:

- `conaryd` remains a deployable GitHub release track
- its deployment verification is Forge-local over the Unix socket
- public production hosting is still not solved
- `conary-test` managed rollout remains the Forge control-plane path, but not for `conaryd` yet

- [ ] **Step 3: Verify the updated docs contain the required phrases**

Run:

```bash
rg -n "local-only staging daemon|scripts/conaryd-health.sh|deploy-and-verify|deploy/ssh/forge-known-hosts" deploy/FORGE.md docs/operations/infrastructure.md
```

Expected: the updated operator story is visible in both docs.

- [ ] **Step 4: Commit the doc updates**

```bash
git add deploy/FORGE.md docs/operations/infrastructure.md
git commit -m "docs(deploy): describe conaryd Forge staging path"
```

### Task 8: Run Local Verification Before The Live Bootstrap Rerun

**Files:**

- Modify: none

- [ ] **Step 1: Syntax-check all new shell scripts**

Run:

```bash
bash -n scripts/conaryd-health.sh scripts/install-conaryd-on-forge.sh scripts/check-release-matrix.sh
```

Expected: exit code `0`.

- [ ] **Step 2: Verify the systemd unit again**

Run:

```bash
cargo test -p conaryd
```

Expected: pass. This is the relevant owning-package regression check for the daemon whose socket behavior the deploy path depends on, but it does **not** by itself prove the `/health` JSON contract; the live Forge verifier remains the authoritative contract check.

- [ ] **Step 3: Re-run the workflow guard script**

Run:

```bash
bash scripts/check-release-matrix.sh
```

Expected: pass.

### Task 9: Perform The Bootstrap `conaryd-v0.6.0` Rerun And Record The Result

**Files:**

- Modify: `docs/superpowers/release-hardening-checklist-2026-04-10.md`

- [ ] **Step 1: Install the bootstrap sudoers file on Forge once as a root operator**

Run:

```bash
scp deploy/sudoers/conaryd-forge peter@forge.conarylabs.com:/tmp/conaryd-forge
ssh peter@forge.conarylabs.com 'sudo install -m 0440 /tmp/conaryd-forge /etc/sudoers.d/conaryd-forge && sudo visudo -cf /etc/sudoers.d/conaryd-forge'
```

Expected: `visudo` validation succeeds.

- [ ] **Step 2: Dispatch the live `deploy-and-verify` rerun for the published bundle**

Do this only after the workflow changes from Chunk 2 have landed on `main`, because `gh workflow run --ref main ...` reads the workflow definition from `main` and needs the new `deploy_asset_ref` input to exist there.

Run:

```bash
WORKFLOW_REF=main
BOOTSTRAP_ASSET_REF=<immutable-commit-sha-that-first-introduced-helper-assets>
BEFORE_RUN_ID_FILE=/tmp/conaryd-bootstrap-before-run-id
before_id="$(gh run list --workflow deploy-and-verify.yml --event workflow_dispatch --limit 1 --json databaseId -q '.[0].databaseId')"
printf '%s\n' "$before_id" > "$BEFORE_RUN_ID_FILE"

gh workflow run deploy-and-verify.yml \
  --ref "$WORKFLOW_REF" \
  -f product=conaryd \
  -f source_run=24273700060 \
  -f environment=production \
  -f dry_run=false \
  -f deploy_asset_ref="$BOOTSTRAP_ASSET_REF"
```

Expected: a new workflow run is created for the bootstrap exception path, and the immutable bootstrap asset ref is recorded in the dispatch inputs.

- [ ] **Step 3: Watch the run to completion and inspect the `deploy-conaryd` job**

Run:

```bash
BEFORE_RUN_ID_FILE=/tmp/conaryd-bootstrap-before-run-id
before_id="$(cat "$BEFORE_RUN_ID_FILE" 2>/dev/null || true)"
run_id=""
# If before_id is empty, this is the first manual dispatch and the first
# non-empty candidate is the run we want to watch.
for _ in {1..15}; do
  candidate="$(gh run list --workflow deploy-and-verify.yml --event workflow_dispatch --limit 1 --json databaseId -q '.[0].databaseId')"
  if [[ -n "$candidate" && "$candidate" != "$before_id" ]]; then
    run_id="$candidate"
    break
  fi
  sleep 2
done
[[ -n "$run_id" ]] || { echo "failed to resolve deploy-and-verify run id" >&2; exit 1; }
gh run watch "$run_id" --exit-status
gh run view "$run_id" --log | rg -n "bundle hash|conaryd-health|restart conaryd|deploy_asset_ref"
```

Expected: `deploy-conaryd` succeeds and the helper logs show artifact-hash verification, systemd restart, and successful Unix-socket health verification.

- [ ] **Step 4: Record the rerun evidence in the release-hardening checklist**

Update the existing `- Live \`conaryd\` cut:` block in [release-hardening-checklist-2026-04-10.md](/home/peter/Conary/docs/superpowers/release-hardening-checklist-2026-04-10.md) by grepping for that exact heading and the existing "remaining follow-up before conaryd can be considered fully released" bullet, rather than relying on a drifting line number.

Record:

- workflow run ID
- `source_run=24273700060`
- use of the bootstrap exception plus the immutable `deploy_asset_ref`
- success or failure of the helper path
- whether `conaryd` now moves from deployment-blocked to Forge staging deployment verified

If the rerun succeeds, replace the existing "remaining follow-up before conaryd can be considered fully released" bullet with concrete success wording and update the `Blocked Tracks` / `Final Release Command` summary to reflect the new state.

- [ ] **Step 5: Commit the evidence update**

```bash
git add docs/superpowers/release-hardening-checklist-2026-04-10.md
git commit -m "docs(release): record conaryd Forge staging bootstrap rerun"
```

---

## Deferred Follow-Up: Rollout Framework Coherence

Do **not** fold this into the direct deploy lane work above.

If we later choose to make `conaryd` a managed Forge rollout unit, write a separate plan that:

- adds a bundle-install or external-helper deploy mode to `apps/conary-test/src/deploy/manifest.rs`
- teaches `apps/conary-test/src/deploy/orchestrator.rs` an explicit privileged execution handoff for system units
- adds a `conaryd_local_health` verify mode that reuses the checked-in Unix-socket verifier contract
- updates `deploy/forge-rollouts.toml` with a dedicated `conaryd_staging` group only after the executor semantics are safe

That follow-up must not reintroduce `cargo build -p conaryd` on Forge as the deployment mechanism for release artifacts.

---

## Final Verification Checklist

- [ ] `bash -n scripts/conaryd-health.sh scripts/install-conaryd-on-forge.sh scripts/check-release-matrix.sh`
- [ ] `bash scripts/check-release-matrix.sh`
- [ ] `cargo test -p conaryd`
- [ ] live `deploy-and-verify` rerun succeeds for `product=conaryd`, `source_run=24273700060`, `dry_run=false`
- [ ] `docs/superpowers/release-hardening-checklist-2026-04-10.md` records the final outcome

Plan complete and saved to `docs/superpowers/plans/2026-04-16-conaryd-forge-staging-deployment-plan.md`. Ready to execute?
