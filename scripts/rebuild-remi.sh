#!/usr/bin/env bash
# scripts/rebuild-remi.sh -- Pull, build, and restart Remi server
#
# Runs ON Remi itself. Expects to be called from /root/conary-src/
# (or wherever the repo clone lives).
#
# Usage:
#   ./scripts/rebuild-remi.sh [--skip-pull] [--skip-smoke]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

SKIP_PULL=false
SKIP_SMOKE=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-pull)  SKIP_PULL=true; shift ;;
        --skip-smoke) SKIP_SMOKE=true; shift ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

BINARY_SRC="target/release/remi"
BINARY_DST="/usr/local/bin/remi"

echo "=== Remi rebuild started at $(date -u +%Y-%m-%dT%H:%M:%SZ) ==="

# Step 1: Pull latest
if [[ "$SKIP_PULL" == false ]]; then
    echo "[1/5] Pulling latest from origin..."
    git pull --ff-only origin main
else
    echo "[1/5] Skipping pull (--skip-pull)"
fi

# Step 2: Fetch deps if needed
echo "[2/5] Fetching dependencies..."
cargo fetch --quiet

# Step 3: Build the Remi app
echo "[3/5] Building release (this may take a few minutes)..."
cargo build --release -p remi

# Step 4: Stop service, copy binary, start service
echo "[4/5] Restarting remi service..."
systemctl stop remi
cp "$BINARY_SRC" "$BINARY_DST"
systemctl start remi

# Brief pause for service to fully start
sleep 2

# Step 5: Smoke test
if [[ "$SKIP_SMOKE" == false ]]; then
    echo "[5/5] Running smoke test..."
    if ./scripts/remi-health.sh --smoke --endpoint http://localhost:8081; then
        echo "=== Remi rebuild complete [OK] ==="
    else
        echo "=== Remi rebuild complete [SMOKE TEST FAILED] ===" >&2
        echo "Service is running but health check failed. Check logs: journalctl -u remi -n 50" >&2
        exit 1
    fi
else
    echo "[5/5] Skipping smoke test (--skip-smoke)"
    echo "=== Remi rebuild complete ==="
fi
