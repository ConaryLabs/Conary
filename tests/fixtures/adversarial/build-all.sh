#!/usr/bin/env bash
# tests/fixtures/adversarial/build-all.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONARY_BIN="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"

echo "=== Building adversarial test fixtures ==="

echo "[1/4] Building corrupted fixtures..."
"$SCRIPT_DIR/build-corrupted.sh" "$CONARY_BIN"

echo "[2/4] Building malicious fixtures..."
"$SCRIPT_DIR/build-malicious.sh" "$CONARY_BIN"

echo "[3/4] Building dependency fixtures..."
"$SCRIPT_DIR/build-deps.sh" "$CONARY_BIN"

echo "[4/4] Generating large fixtures..."
"$SCRIPT_DIR/build-large.sh" "$CONARY_BIN"

echo "=== All fixtures built ==="
