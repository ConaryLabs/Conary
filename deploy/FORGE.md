# Forge Server Setup

Forge (`forge.conarylabs.com`) is the trusted GitHub Actions runner host for
Conary validation and test-harness operations.

## Server Details

- **SSH:** `ssh peter@forge.conarylabs.com`
- **OS:** Fedora 43
- **RAM:** 8GB
- **Disk:** 151GB
- **Role:** self-hosted GitHub Actions runner plus local `conary-test` service
  and control-plane validation

## Quick Setup

```bash
# From an admin workstation, mint a short-lived registration token:
gh api -X POST repos/ConaryLabs/Conary/actions/runners/registration-token --jq .token

# Copy the token to Forge only for the setup run:
export GITHUB_RUNNER_REGISTRATION_TOKEN="<token>"

# Then install or refresh the runner host on Forge:
sudo -E bash /home/peter/Conary/deploy/setup-forge.sh

# Confirm the runner service:
systemctl status github-actions-runner --no-pager
```

`deploy/setup-forge.sh` installs Podman, ensures the Rust toolchain is present
for the runner user, downloads the GitHub Actions runner binaries, registers a
single trusted runner, and installs the checked-in systemd unit from
`deploy/systemd/github-actions-runner.service`.

If you prefer a persistent GitHub CLI login on Forge, the script still supports
that path when `GITHUB_RUNNER_REGISTRATION_TOKEN` is not provided.

## Runner Role

- The first rollout uses one runner with the custom label `forge-trusted`.
- Trusted lanes such as `merge-validation` and `scheduled-ops` should target
  this host explicitly.
- `pr-gate` stays on GitHub-hosted runners.
- No separate source-control or CI service is part of the target setup.

## Supported Deployment Commands

Trusted/default deploys should now go through the managed rollout wrapper:

```bash
# Trusted default: fetch and deploy an exact GitHub ref on Forge
./scripts/deploy-forge.sh --group control_plane --ref main

# Roll out a broader named group from a specific commit
./scripts/deploy-forge.sh --group all_forge_tooling --ref 78e7194e

# Debug-only local snapshot deploy over the active Forge checkout
./scripts/deploy-forge.sh --unit conary_test --path "$(pwd)"
```

Under the hood:

- `--ref` runs the managed `conary-test deploy rollout ... --ref ...` flow on
  Forge and is the normal supported mode
- `--path` keeps the `rsync` boundary, syncing directly over `~/Conary` before
  invoking the same managed rollout against that active checkout
- rollout groups and units come from `deploy/forge-rollouts.toml`
- verification stays tied to `scripts/forge-smoke.sh`

`scripts/deploy-forge.sh` is now a convenience wrapper, not the deployment
brain. Build/restart/verify ordering lives in `conary-test deploy rollout`.

## Supported Validation Commands

```bash
# Supported Forge control-plane smoke:
bash scripts/forge-smoke.sh

# Or point at an alternate local service port:
bash scripts/forge-smoke.sh --port 9099

# Run Remi health checks:
./scripts/remi-health.sh --smoke
./scripts/remi-health.sh --full
```

`forge-smoke.sh` resolves the local port with `--port` > `CONARY_TEST_PORT` >
`9090`, prefers `target/debug/conary-test` when present, and falls back to
`conary-test` on `$PATH`.

Raw `cargo run -p conary-test -- run ...` remains useful for deeper manual
debugging, but it is no longer the main supported Forge smoke path.

## Troubleshooting

**Runner service is unhealthy:**
```bash
journalctl -u github-actions-runner -f
systemctl status github-actions-runner --no-pager
```

**GitHub authentication is missing or expired:**
```bash
sudo -u peter -H gh auth status
sudo -u peter -H gh auth login --hostname github.com
```

**Runner registration without persistent `gh` auth:**
```bash
export GITHUB_RUNNER_REGISTRATION_TOKEN="$(gh api -X POST repos/ConaryLabs/Conary/actions/runners/registration-token --jq .token)"
ssh peter@forge.conarylabs.com 'sudo -E bash /home/peter/Conary/deploy/setup-forge.sh'
```

**Local validation tools are missing:**
```bash
sudo -u peter -H bash -lc 'cargo --version && podman --version'
```

**Container builds fail locally:**
```bash
podman system prune -a
podman build -f apps/conary/tests/integration/remi/containers/Containerfile.fedora43 apps/conary/tests/integration/remi/
```
