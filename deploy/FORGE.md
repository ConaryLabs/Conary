# Forge Server Setup

Forge (`forge.conarylabs.com`) runs Forgejo for CI/CD with a native Forgejo Runner.

## Server Details

- **SSH:** `ssh peter@forge.conarylabs.com`
- **OS:** Fedora 43
- **RAM:** 8GB
- **Disk:** 151GB
- **Software:** Rust 1.93, Podman, Forgejo, Forgejo Runner

## Quick Setup

```bash
# On forge server:
sudo ./deploy/setup-forge.sh

# Complete web setup at http://forge.conarylabs.com:3000
# Mirror GitHub repo, generate runner token, register runner
```

See `deploy/setup-forge.sh` for detailed steps.

## CI Workflows

| Workflow | Trigger | Duration | What it does |
|----------|---------|----------|-------------|
| `ci.yaml` | Push to main | ~5 min | cargo build, test, clippy, Remi smoke |
| `integration.yaml` | Push to main | ~15 min | 37-test suite on Fedora/Ubuntu/Arch via Podman |
| `remi-health.yaml` | Every 6 hours | ~60s | Full Remi endpoint verification |

## Runner

The Forgejo Runner runs natively on the host (not in a container) with label `linux-native`. It has direct access to:
- Rust toolchain for cargo build/test/clippy
- Podman for integration test containers
- Network for Remi health checks

## Manual Test Commands

```bash
# Run integration tests locally (on Forge):
./tests/integration/remi/run.sh --build --distro fedora43

# Run Remi health check:
./scripts/remi-health.sh --smoke   # Quick (~5s)
./scripts/remi-health.sh --full    # Comprehensive (~60s)
```

## Troubleshooting

**Forgejo won't start:**
```bash
journalctl -u forgejo -f
```

**Runner not picking up jobs:**
```bash
journalctl -u forgejo-runner -f
forgejo-runner list  # Check registration
```

**Integration tests fail to build container:**
```bash
podman system prune -a  # Clean stale images
podman build -f tests/integration/remi/containers/Containerfile.fedora43 tests/integration/remi/
```

**Remi health check fails:**
```bash
curl -v https://packages.conary.io/health  # Check connectivity
```
