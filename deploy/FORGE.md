# Forge Server Setup

Forge (`forge.conarylabs.com`) runs Forgejo for CI/CD with a native Forgejo Runner.

## Server Details

- **SSH:** `ssh peter@forge.conarylabs.com`
- **OS:** Fedora 43
- **RAM:** 8GB
- **Disk:** 151GB
- **Software:** Rust 1.94, Podman 5.7, Forgejo 14.0.2, Forgejo Runner 12.7.1

## Quick Setup

```bash
# On forge server (full automated install):
sudo FORGE_ADMIN_PASS='YourPassword' bash /home/peter/Conary/deploy/setup-forge.sh

# Then mirror the GitHub repo:
TOKEN=$(curl -s -X POST http://localhost:3000/api/v1/users/peter/tokens \
  -u 'peter:YourPassword' -H 'Content-Type: application/json' \
  -d '{"name":"setup","scopes":["all"]}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["sha1"])')

curl -X POST http://localhost:3000/api/v1/repos/migrate \
  -H "Authorization: token $TOKEN" -H 'Content-Type: application/json' \
  -d '{"clone_addr":"https://github.com/ConaryLabs/Conary.git","repo_name":"Conary","repo_owner":"peter","service":"git","mirror":true,"mirror_interval":"10m"}'
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
cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora43 --phase 1

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
cat /var/lib/forgejo-runner/.runner  # Check registration
cat /var/lib/forgejo-runner/config.yaml  # Verify labels: ["linux-native:host"]
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
