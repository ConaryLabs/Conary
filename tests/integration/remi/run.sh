#!/usr/bin/env bash
# tests/integration/remi/run.sh
# Orchestrator for Remi integration tests
#
# Usage:
#   ./tests/integration/remi/run.sh [--build] [--distro fedora43] [--binary path/to/conary]
#   ./tests/integration/remi/run.sh --distro ubuntu-noble --package packaging/deb/output/conary_0.1.0-1_amd64.deb
#
# Options:
#   --build         Build conary binary before testing (cargo build)
#   --distro NAME   Distro to test (default: fedora43; also: ubuntu-noble, arch)
#   --binary PATH   Path to pre-built conary binary (default: target/debug/conary)
#   --package PATH  Path to native package (.rpm/.deb/.pkg.tar.zst) to install in container
#   --no-cache      Rebuild container image from scratch
#   --keep          Keep results volume after run
#   --phase2        Run Phase 2 (deep E2E) tests in addition to Phase 1
#   --help          Show this help

set -euo pipefail

# ── Resolve paths ─────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# ── Defaults ──────────────────────────────────────────────────────────────────

DISTRO="fedora43"
BINARY=""
PACKAGE=""
DO_BUILD=0
NO_CACHE=""
KEEP_RESULTS=0
PHASE2=0

# ── Parse arguments ───────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --build)
            DO_BUILD=1
            shift
            ;;
        --distro)
            DISTRO="$2"
            shift 2
            ;;
        --binary)
            BINARY="$2"
            shift 2
            ;;
        --package)
            PACKAGE="$2"
            shift 2
            ;;
        --no-cache)
            NO_CACHE="--no-cache"
            shift
            ;;
        --keep)
            KEEP_RESULTS=1
            shift
            ;;
        --phase2)
            PHASE2=1
            shift
            ;;
        --help)
            sed -n '3,/^$/s/^# //p' "$0"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            echo "Run with --help for usage" >&2
            exit 1
            ;;
    esac
done

# ── Validate distro ──────────────────────────────────────────────────────────

CONTAINERFILE="$SCRIPT_DIR/containers/Containerfile.${DISTRO}"
if [ ! -f "$CONTAINERFILE" ]; then
    echo "No Containerfile for distro '$DISTRO'" >&2
    echo "Available:" >&2
    ls "$SCRIPT_DIR/containers/" | sed 's/Containerfile\./  /g' >&2
    exit 1
fi

# ── Prerequisites ─────────────────────────────────────────────────────────────

if ! command -v podman &>/dev/null; then
    echo "podman is required but not found in PATH" >&2
    exit 1
fi

# ── Determine install mode ───────────────────────────────────────────────────

BUILD_CONTEXT="$SCRIPT_DIR"
INSTALL_MODE="binary"
CLEANUP_FILES=()

cleanup() {
    for f in "${CLEANUP_FILES[@]}"; do
        rm -rf "$f"
    done
    if [ "$KEEP_RESULTS" -eq 0 ]; then
        podman volume rm "conary-test-results-${DISTRO}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

if [ -n "$PACKAGE" ]; then
    # ── Package mode: install native package in container ────────────────
    INSTALL_MODE="package"

    if [ ! -f "$PACKAGE" ]; then
        echo "Package file not found: $PACKAGE" >&2
        exit 1
    fi

    PKG_BASENAME="$(basename "$PACKAGE")"
    cp "$PACKAGE" "$BUILD_CONTEXT/$PKG_BASENAME"
    CLEANUP_FILES+=("$BUILD_CONTEXT/$PKG_BASENAME")

    echo "Building $DISTRO (package mode)"
    echo "[*] Using package: $PACKAGE"
    echo "[*] Package size: $(du -h "$PACKAGE" | cut -f1)"
    echo ""
else
    # ── Binary mode: copy pre-built binary into container ────────────────
    if [ "$DO_BUILD" -eq 1 ]; then
        echo "[*] Building conary binary..."
        (cd "$PROJECT_ROOT" && cargo build 2>&1)
        echo "[*] Build complete"
        echo ""
    fi

    if [ -z "$BINARY" ]; then
        BINARY="$PROJECT_ROOT/target/debug/conary"
    fi

    if [ ! -f "$BINARY" ]; then
        echo "Conary binary not found at: $BINARY" >&2
        echo "Build first with --build or specify path with --binary" >&2
        exit 1
    fi

    cp "$BINARY" "$BUILD_CONTEXT/conary"
    # Strip debug symbols to reduce size (podman COPY fails on very large files)
    strip "$BUILD_CONTEXT/conary" 2>/dev/null || true
    CLEANUP_FILES+=("$BUILD_CONTEXT/conary")

    echo "Building $DISTRO (binary mode)"
    echo "[*] Using binary: $BINARY"
    echo "[*] Binary size: $(du -h "$BUILD_CONTEXT/conary" | cut -f1) (stripped)"
    echo ""
fi

# ── Copy config and fixtures into build context ─────────────────────────────
# config.toml is already in BUILD_CONTEXT (same dir), no copy needed.

mkdir -p "$BUILD_CONTEXT/fixtures"
FIXTURES_SRC="$PROJECT_ROOT/tests/fixtures"
if [ -d "$FIXTURES_SRC" ]; then
    cp -r "$FIXTURES_SRC/recipes" "$BUILD_CONTEXT/fixtures/recipes" 2>/dev/null || true
    mkdir -p "$BUILD_CONTEXT/fixtures/pkgbuild"
    cp "$PROJECT_ROOT/packaging/arch/PKGBUILD" "$BUILD_CONTEXT/fixtures/pkgbuild/" 2>/dev/null || true
fi
CLEANUP_FILES+=("$BUILD_CONTEXT/fixtures")

# ── Build container image ────────────────────────────────────────────────────

IMAGE_NAME="conary-test-${DISTRO}"
echo "[*] Building container image: $IMAGE_NAME"

podman build \
    $NO_CACHE \
    --build-arg "INSTALL_MODE=$INSTALL_MODE" \
    -t "$IMAGE_NAME" \
    -f "$CONTAINERFILE" \
    "$BUILD_CONTEXT" 2>&1

echo "[*] Image built"
echo ""

# ── Create results volume ────────────────────────────────────────────────────

VOLUME_NAME="conary-test-results-${DISTRO}"
podman volume rm "$VOLUME_NAME" 2>/dev/null || true
podman volume create "$VOLUME_NAME" >/dev/null

# ── Run tests ─────────────────────────────────────────────────────────────────

echo "[*] Running tests in container..."
echo ""

CONTAINER_EXIT=0
CONTAINER_CMD="python3 /opt/remi-tests/runner/test_runner.py"
if [ "$PHASE2" -eq 1 ]; then
    CONTAINER_CMD="$CONTAINER_CMD --phase2"
fi

podman run \
    --rm \
    --name "conary-test-run-${DISTRO}" \
    -v "${VOLUME_NAME}:/results:Z" \
    -e "DISTRO=${DISTRO}" \
    "$IMAGE_NAME" $CONTAINER_CMD || CONTAINER_EXIT=$?

echo ""

# ── Extract results ──────────────────────────────────────────────────────────

RESULTS_DIR="$SCRIPT_DIR/results"
mkdir -p "$RESULTS_DIR"

# Copy results from volume using a temporary container
podman run --rm \
    -v "${VOLUME_NAME}:/results:ro" \
    -v "${RESULTS_DIR}:/out:Z" \
    docker.io/library/alpine:latest \
    sh -c "cp /results/*.json /out/ 2>/dev/null || echo 'No results found'"

# ── Print summary ────────────────────────────────────────────────────────────

RESULTS_FILE="$RESULTS_DIR/${DISTRO}.json"
if [ -f "$RESULTS_FILE" ]; then
    echo ""
    echo "Results saved to: $RESULTS_FILE"

    # Parse and display summary table if jq-like parsing is possible
    # (fallback to raw display if needed)
    if command -v python3 &>/dev/null; then
        python3 -c "
import json, sys
with open('$RESULTS_FILE') as f:
    data = json.load(f)
s = data['summary']
print()
print(f\"{'ID':<6} {'Name':<30} {'Status':<8} {'Duration'}\")
print('-' * 60)
for t in data['tests']:
    status = t['status'].upper()
    dur = f\"{t.get('duration_ms', 0)}ms\" if 'duration_ms' in t else '-'
    extra = ''
    if t['status'] == 'fail':
        extra = f\"  {t.get('message', '')[:60]}\"
    elif t['status'] == 'skip':
        extra = f\"  {t.get('reason', '')}\"
    print(f\"{t['id']:<6} {t['name']:<30} {status:<8} {dur}{extra}\")
print()
print(f\"Total: {s['total']}  Passed: {s['passed']}  Failed: {s['failed']}  Skipped: {s['skipped']}\")
" 2>/dev/null || cat "$RESULTS_FILE"
    else
        cat "$RESULTS_FILE"
    fi
fi

exit "$CONTAINER_EXIT"
