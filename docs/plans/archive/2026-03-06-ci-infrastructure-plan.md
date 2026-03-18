# CI & Validation Infrastructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Set up Forgejo CI on the Forge server with 3 automated workflows: fast gate (build/test/clippy), integration test matrix (3 distros), and scheduled Remi health monitoring.

**Architecture:** Forgejo installed natively on forge.conarylabs.com with a native Forgejo Runner. Three workflow YAML files in `.forgejo/workflows/` trigger on push and schedule. A new `scripts/remi-health.sh` provides reusable Remi endpoint verification. A setup script and docs guide Forge server configuration.

**Tech Stack:** Forgejo Actions (GitHub Actions-compatible YAML), Bash, Podman, curl

---

### Task 1: Create Remi health check script

**Files:**
- Create: `scripts/remi-health.sh`

**Context:** This script is reused by both the CI fast gate (smoke mode) and the scheduled health workflow (full mode). It hits Remi endpoints and reports pass/fail.

**Step 1: Write the script**

```bash
#!/usr/bin/env bash
# scripts/remi-health.sh -- Remi server health verification
#
# Usage:
#   ./scripts/remi-health.sh [--smoke|--full] [--endpoint URL]
#
# Modes:
#   --smoke   Quick health + metadata check (~5s)
#   --full    All endpoints + test conversion (~60s)
set -euo pipefail

ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"
MODE="smoke"

for arg in "$@"; do
    case "$arg" in
        --smoke) MODE="smoke" ;;
        --full)  MODE="full" ;;
        --endpoint=*) ENDPOINT="${arg#*=}" ;;
        --endpoint) shift; ENDPOINT="${1:-$ENDPOINT}" ;;
    esac
done

PASS=0
FAIL=0

check() {
    local name="$1" url="$2" expect="${3:-200}"
    local http_code
    http_code=$(curl -sf -o /dev/null -w '%{http_code}' --max-time 10 "$url" 2>/dev/null || echo "000")

    if [[ "$http_code" == "$expect" ]]; then
        printf "  [PASS] %-40s %s\n" "$name" "$http_code"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s %s (expected %s)\n" "$name" "$http_code" "$expect"
        FAIL=$((FAIL + 1))
    fi
}

check_contains() {
    local name="$1" url="$2" needle="$3"
    local body
    body=$(curl -sf --max-time 10 "$url" 2>/dev/null || echo "")

    if echo "$body" | grep -qF "$needle"; then
        printf "  [PASS] %-40s contains '%s'\n" "$name" "$needle"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s missing '%s'\n" "$name" "$needle"
        FAIL=$((FAIL + 1))
    fi
}

echo "Remi Health Check ($MODE)"
echo "Endpoint: $ENDPOINT"
echo ""

# ── Smoke checks (always run) ────────────────────────────────────────────
echo "=== Core Endpoints ==="
check "health"           "$ENDPOINT/health"
check "stats overview"   "$ENDPOINT/v1/stats/overview"

echo ""
echo "=== Metadata (per distro) ==="
for distro in fedora ubuntu arch; do
    check "metadata ($distro)" "$ENDPOINT/v1/${distro}/metadata"
done

# ── Full checks (--full only) ────────────────────────────────────────────
if [[ "$MODE" == "full" ]]; then
    echo ""
    echo "=== Sparse Index ==="
    # Check that a known package exists in the sparse index
    check "sparse index (curl)" "$ENDPOINT/v1/packages/curl"

    echo ""
    echo "=== Search ==="
    check_contains "search (curl)" "$ENDPOINT/v1/search?q=curl" "curl"

    echo ""
    echo "=== OCI Distribution ==="
    check "OCI catalog" "$ENDPOINT/v2/_catalog"

    echo ""
    echo "=== Conversion (async) ==="
    # Submit a conversion request and check we get 200 or 202
    local conv_code
    conv_code=$(curl -sf -o /dev/null -w '%{http_code}' --max-time 30 \
        -X POST "$ENDPOINT/v1/convert/fedora/curl" 2>/dev/null || echo "000")
    if [[ "$conv_code" == "200" ]] || [[ "$conv_code" == "202" ]]; then
        printf "  [PASS] %-40s %s\n" "conversion submit" "$conv_code"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s %s (expected 200 or 202)\n" "conversion submit" "$conv_code"
        FAIL=$((FAIL + 1))
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────────
echo ""
TOTAL=$((PASS + FAIL))
echo "Results: $PASS/$TOTAL passed"

if [[ "$FAIL" -gt 0 ]]; then
    echo "[FAILED] $FAIL checks failed"
    exit 1
fi

echo "[OK] All checks passed"
exit 0
```

Note: The `local` keyword inside the `if` block for the conversion check needs to be moved outside. Fix during implementation: declare `conv_code` before the if block.

**Step 2: Make executable and test locally**

```bash
chmod +x scripts/remi-health.sh
./scripts/remi-health.sh --smoke
```

Expected: All smoke checks pass against packages.conary.io.

```bash
./scripts/remi-health.sh --full
```

Expected: All full checks pass (conversion may return 200 if already cached or 202 if queued).

**Step 3: Commit**

```bash
git add scripts/remi-health.sh
git commit -m "feat: Add Remi health check script (smoke + full modes)"
```

---

### Task 2: Create CI fast gate workflow

**Files:**
- Create: `.forgejo/workflows/ci.yaml`

**Context:** Forgejo Actions uses GitHub Actions-compatible YAML. The runner label is `linux-native`. This runs on every push to main. Jobs run in parallel.

**Step 1: Write the workflow file**

```yaml
# .forgejo/workflows/ci.yaml
# Fast CI gate -- runs on every push to main (~5 min)
name: CI

on:
  push:
    branches: [main]

jobs:
  build:
    runs-on: linux-native
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: cargo build

  test:
    runs-on: linux-native
    steps:
      - uses: actions/checkout@v4
      - name: Test
        run: cargo test

  clippy:
    runs-on: linux-native
    steps:
      - uses: actions/checkout@v4
      - name: Clippy
        run: cargo clippy -- -D warnings

  remi-smoke:
    runs-on: linux-native
    steps:
      - uses: actions/checkout@v4
      - name: Remi smoke check
        run: ./scripts/remi-health.sh --smoke
```

**Step 2: Commit**

```bash
mkdir -p .forgejo/workflows
git add .forgejo/workflows/ci.yaml
git commit -m "feat: Add CI fast gate workflow (build, test, clippy, Remi smoke)"
```

---

### Task 3: Create integration test matrix workflow

**Files:**
- Create: `.forgejo/workflows/integration.yaml`

**Context:** Runs after CI passes. Uses the existing `tests/integration/remi/run.sh` harness with Podman. Each distro is a separate job. The `--build` flag compiles conary from source inside the workflow, then the run script copies the binary into a Podman container.

**Step 1: Write the workflow file**

```yaml
# .forgejo/workflows/integration.yaml
# Integration test matrix -- 3 distros via Podman (~15 min)
name: Integration Tests

on:
  push:
    branches: [main]

jobs:
  integration:
    runs-on: linux-native
    strategy:
      fail-fast: false
      matrix:
        distro: [fedora43, ubuntu-noble, arch]
    name: Integration (${{ matrix.distro }})
    steps:
      - uses: actions/checkout@v4

      - name: Build conary
        run: cargo build

      - name: Run integration tests (${{ matrix.distro }})
        run: ./tests/integration/remi/run.sh --distro ${{ matrix.distro }}

      - name: Upload results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: results-${{ matrix.distro }}
          path: tests/integration/remi/results/${{ matrix.distro }}.json
```

**Step 2: Commit**

```bash
git add .forgejo/workflows/integration.yaml
git commit -m "feat: Add integration test matrix workflow (Fedora, Ubuntu, Arch)"
```

---

### Task 4: Create Remi health monitoring workflow

**Files:**
- Create: `.forgejo/workflows/remi-health.yaml`

**Context:** Scheduled cron job every 6 hours. Also triggerable manually. Runs the full Remi health check.

**Step 1: Write the workflow file**

```yaml
# .forgejo/workflows/remi-health.yaml
# Scheduled Remi server health monitoring
name: Remi Health

on:
  schedule:
    - cron: '0 */6 * * *'  # Every 6 hours
  workflow_dispatch: {}      # Manual trigger

jobs:
  health:
    runs-on: linux-native
    steps:
      - uses: actions/checkout@v4
      - name: Full Remi health check
        run: ./scripts/remi-health.sh --full
```

**Step 2: Commit**

```bash
git add .forgejo/workflows/remi-health.yaml
git commit -m "feat: Add scheduled Remi health monitoring workflow (every 6h)"
```

---

### Task 5: Create Forge setup script

**Files:**
- Create: `deploy/setup-forge.sh`

**Context:** This script installs Forgejo and the Forgejo Runner on `forge.conarylabs.com` (Fedora 43). It's meant to be run once via SSH as root. Follow the pattern of the existing `deploy/setup-remi.sh`.

**Step 1: Write the setup script**

```bash
#!/usr/bin/env bash
# deploy/setup-forge.sh -- Install Forgejo + Runner on Fedora 43
#
# Usage:
#   ssh peter@forge.conarylabs.com
#   sudo ./deploy/setup-forge.sh
#
# Prerequisites:
#   - Fedora 43 with Podman installed
#   - Rust 1.93+ installed
#   - Git installed
#
# What this installs:
#   - Forgejo (native binary) on port 3000
#   - Forgejo Runner (native binary) registered with the instance
#   - systemd services for both
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[-]${NC} $1"; exit 1; }

[[ $EUID -ne 0 ]] && error "This script must be run as root"

FORGEJO_VERSION="10.0.1"
RUNNER_VERSION="6.3.1"
FORGEJO_USER="forgejo"
FORGEJO_HOME="/var/lib/forgejo"
FORGEJO_BIN="/usr/local/bin/forgejo"
RUNNER_BIN="/usr/local/bin/forgejo-runner"

# ── Create forgejo user ──────────────────────────────────────────────────
if ! id "$FORGEJO_USER" &>/dev/null; then
    log "Creating forgejo user..."
    useradd --system --shell /bin/bash --home-dir "$FORGEJO_HOME" --create-home "$FORGEJO_USER"
fi

# ── Install Forgejo binary ───────────────────────────────────────────────
log "Downloading Forgejo ${FORGEJO_VERSION}..."
curl -fSL "https://codeberg.org/forgejo/forgejo/releases/download/v${FORGEJO_VERSION}/forgejo-${FORGEJO_VERSION}-linux-amd64" \
    -o "$FORGEJO_BIN"
chmod +x "$FORGEJO_BIN"

# ── Install Forgejo Runner binary ────────────────────────────────────────
log "Downloading Forgejo Runner ${RUNNER_VERSION}..."
curl -fSL "https://codeberg.org/forgejo/runner/releases/download/v${RUNNER_VERSION}/forgejo-runner-${RUNNER_VERSION}-linux-amd64" \
    -o "$RUNNER_BIN"
chmod +x "$RUNNER_BIN"

# ── Create directories ──────────────────────────────────────────────────
mkdir -p "$FORGEJO_HOME"/{custom,data,log}
mkdir -p /etc/forgejo
chown -R "$FORGEJO_USER":"$FORGEJO_USER" "$FORGEJO_HOME"
chown root:"$FORGEJO_USER" /etc/forgejo
chmod 770 /etc/forgejo

# ── Forgejo systemd service ─────────────────────────────────────────────
log "Creating Forgejo systemd service..."
cat > /etc/systemd/system/forgejo.service << 'EOF'
[Unit]
Description=Forgejo
After=network.target

[Service]
Type=simple
User=forgejo
Group=forgejo
WorkingDirectory=/var/lib/forgejo
ExecStart=/usr/local/bin/forgejo web --config /etc/forgejo/app.ini
Restart=always
RestartSec=3
Environment=USER=forgejo HOME=/var/lib/forgejo FORGEJO_WORK_DIR=/var/lib/forgejo

[Install]
WantedBy=multi-user.target
EOF

# ── Runner systemd service ──────────────────────────────────────────────
log "Creating Runner systemd service..."
cat > /etc/systemd/system/forgejo-runner.service << 'EOF'
[Unit]
Description=Forgejo Runner
After=forgejo.service
Wants=forgejo.service

[Service]
Type=simple
User=peter
WorkingDirectory=/home/peter
ExecStart=/usr/local/bin/forgejo-runner daemon
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# ── Enable and start Forgejo ────────────────────────────────────────────
systemctl daemon-reload
systemctl enable forgejo
systemctl start forgejo

log "Forgejo installed and running on port 3000"
log ""
log "Next steps:"
log "  1. Visit http://forge.conarylabs.com:3000 to complete web setup"
log "  2. Create an admin account"
log "  3. Mirror the GitHub repo:"
log "     - New Migration > GitHub > https://github.com/ConaryLabs/Conary"
log "     - Enable 'Mirror' option"
log "  4. Generate a runner registration token:"
log "     - Site Administration > Actions > Runners > Create new Runner"
log "  5. Register the runner:"
log "     forgejo-runner register --instance http://localhost:3000 --token <TOKEN> --labels linux-native --name forge-runner"
log "  6. Start the runner:"
log "     systemctl enable --now forgejo-runner"
log ""
warn "DNS: Point forge.conarylabs.com to this server's IP"
warn "TLS: Set up Caddy/nginx reverse proxy with Let's Encrypt for HTTPS"
```

**Step 2: Make executable**

```bash
chmod +x deploy/setup-forge.sh
```

**Step 3: Commit**

```bash
git add deploy/setup-forge.sh
git commit -m "feat: Add Forge server setup script (Forgejo + Runner)"
```

---

### Task 6: Create Forge setup documentation

**Files:**
- Create: `deploy/FORGE.md`

**Step 1: Write the documentation**

```markdown
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
```

**Step 2: Commit**

```bash
git add deploy/FORGE.md
git commit -m "docs: Add Forge server setup documentation"
```

---

### Task 7: Verify all files and final commit

**Step 1: Verify directory structure**

```bash
ls -la .forgejo/workflows/
ls -la scripts/remi-health.sh
ls -la deploy/setup-forge.sh deploy/FORGE.md
```

Expected: All 6 new files present and executable where appropriate.

**Step 2: Verify Remi health script works**

```bash
./scripts/remi-health.sh --smoke
```

Expected: All checks pass.

**Step 3: Verify cargo build (no breakage)**

```bash
cargo build
```

Expected: success.

**Step 4: Verify workflow YAML is valid**

```bash
# Basic YAML syntax check
python3 -c "import yaml; yaml.safe_load(open('.forgejo/workflows/ci.yaml'))" && echo "ci.yaml: valid"
python3 -c "import yaml; yaml.safe_load(open('.forgejo/workflows/integration.yaml'))" && echo "integration.yaml: valid"
python3 -c "import yaml; yaml.safe_load(open('.forgejo/workflows/remi-health.yaml'))" && echo "remi-health.yaml: valid"
```

Expected: All 3 valid.
