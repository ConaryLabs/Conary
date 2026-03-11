#!/usr/bin/env bash
# tests/fixtures/adversarial/build-all.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONARY_BIN="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"

echo "=== Building adversarial test fixtures ==="

echo "[1/5] Building corrupted fixtures..."
"$SCRIPT_DIR/build-corrupted.sh" "$CONARY_BIN"

echo "[2/5] Building malicious fixtures..."
"$SCRIPT_DIR/build-malicious.sh" "$CONARY_BIN"

echo "[3/5] Building dependency fixtures..."
"$SCRIPT_DIR/build-deps.sh" "$CONARY_BIN"

if [ -x "$SCRIPT_DIR/build-boot-image.sh" ]; then
    BOOTSTRAP_WORK_DIR="${BOOTSTRAP_WORK_DIR:-/tmp/conary-bootstrap-v1}"
    BOOTSTRAP_ROOT="${BOOTSTRAP_ROOT:-$BOOTSTRAP_WORK_DIR/sysroot}"
    if [ "${AUTO_BUILD_BASE:-0}" = "1" ] || [ -d "$BOOTSTRAP_ROOT" ]; then
        echo "[4/5] Building QEMU boot image fixture..."
        "$SCRIPT_DIR/build-boot-image.sh" "$CONARY_BIN"
    else
        echo "[4/5] Skipping QEMU boot image fixture (set AUTO_BUILD_BASE=1 or prepare $BOOTSTRAP_ROOT)"
    fi
else
    echo "[4/5] Skipping QEMU boot image fixture (build-boot-image.sh missing)"
fi

if [ -x "$SCRIPT_DIR/build-large.sh" ]; then
    echo "[5/5] Generating large fixtures..."
    "$SCRIPT_DIR/build-large.sh" "$CONARY_BIN"
else
    echo "[5/5] Skipping large fixtures (build-large.sh not implemented yet)"
fi

echo "=== All fixtures built ==="
