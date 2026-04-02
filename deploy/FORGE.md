# Forge Server Setup

Forge (`forge.conarylabs.com`) is the trusted GitHub Actions runner host for
Conary validation and test-harness operations.

## Server Details

- **SSH:** `ssh peter@forge.conarylabs.com`
- **OS:** Fedora 43
- **RAM:** 8GB
- **Disk:** 151GB
- **Role:** self-hosted GitHub Actions runner plus local `conary-test` execution

## Quick Setup

```bash
# On Forge, authenticate GitHub CLI as a repository admin:
gh auth login --hostname github.com

# Then install or refresh the runner host:
sudo bash /home/peter/Conary/deploy/setup-forge.sh

# Confirm the runner service:
systemctl status github-actions-runner --no-pager
gh auth status
```

`deploy/setup-forge.sh` installs Podman, ensures the Rust toolchain is present
for the runner user, downloads the GitHub Actions runner binaries, registers a
single trusted runner, and installs the checked-in systemd unit from
`deploy/systemd/github-actions-runner.service`.

## Runner Role

- The first rollout uses one runner with the custom label `forge-trusted`.
- Trusted lanes such as `merge-validation` and `scheduled-ops` should target
  this host explicitly.
- `pr-gate` stays on GitHub-hosted runners.
- No separate source-control or CI service is part of the target setup.

## Manual Validation Commands

```bash
# Run integration smoke checks locally on Forge:
cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora43 --phase 1

# Run Remi health checks:
./scripts/remi-health.sh --smoke
./scripts/remi-health.sh --full
```

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

**Local validation tools are missing:**
```bash
sudo -u peter -H bash -lc 'cargo --version && podman --version'
```

**Container builds fail locally:**
```bash
podman system prune -a
podman build -f tests/integration/remi/containers/Containerfile.fedora43 tests/integration/remi/
```
