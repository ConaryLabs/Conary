#!/usr/bin/env bash
# tests/integration/remi/run.sh
# Orchestrator for Remi integration tests
#
# Usage:
#   ./tests/integration/remi/run.sh [--build] [--distro fedora43] [--binary path/to/conary]
#
# Options:
#   --build         Build conary binary before testing (cargo build)
#   --distro NAME   Distro to test (default: fedora43; also: ubuntu-noble, arch)
#   --binary PATH   Path to pre-built conary binary (default: target/debug/conary)
#   --no-cache      Rebuild container image from scratch
#   --keep          Keep results volume after run
#   --help          Show this help

set -euo pipefail

# ── Resolve paths ─────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# ── Defaults ──────────────────────────────────────────────────────────────────

DISTRO="fedora43"
BINARY=""
DO_BUILD=0
NO_CACHE=""
KEEP_RESULTS=0

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
        --no-cache)
            NO_CACHE="--no-cache"
            shift
            ;;
        --keep)
            KEEP_RESULTS=1
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

# Warn about disabled distros
case "$DISTRO" in
    ubuntu-noble)
        echo "[WARNING] Ubuntu Noble support is experimental."
        echo "  Binary built on Fedora 43 (glibc 2.41) may not run on Ubuntu Noble (glibc 2.39)."
        echo "  Consider building with --target x86_64-unknown-linux-musl."
        echo ""
        ;;
    arch)
        echo "[WARNING] Arch Linux support is experimental."
        echo "  Remi metadata not yet synced for Arch repos."
        echo ""
        ;;
esac

# ── Prerequisites ─────────────────────────────────────────────────────────────

if ! command -v podman &>/dev/null; then
    echo "podman is required but not found in PATH" >&2
    exit 1
fi

# ── Build binary ──────────────────────────────────────────────────────────────

if [ "$DO_BUILD" -eq 1 ]; then
    echo "[*] Building conary binary..."
    (cd "$PROJECT_ROOT" && cargo build 2>&1)
    echo "[*] Build complete"
    echo ""
fi

# Resolve binary path
if [ -z "$BINARY" ]; then
    BINARY="$PROJECT_ROOT/target/debug/conary"
fi

if [ ! -f "$BINARY" ]; then
    echo "Conary binary not found at: $BINARY" >&2
    echo "Build first with --build or specify path with --binary" >&2
    exit 1
fi

echo "[*] Using binary: $BINARY"
echo "[*] Binary size: $(du -h "$BINARY" | cut -f1)"
echo ""

# ── Prepare build context ────────────────────────────────────────────────────

# Podman build context is the remi test directory.
# We copy the binary into it temporarily.
BUILD_CONTEXT="$SCRIPT_DIR"
BINARY_COPY="$BUILD_CONTEXT/conary"

cleanup() {
    rm -f "$BINARY_COPY"
    if [ "$KEEP_RESULTS" -eq 0 ]; then
        podman volume rm "conary-test-results-${DISTRO}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

cp "$BINARY" "$BINARY_COPY"

# ── Build container image ────────────────────────────────────────────────────

IMAGE_NAME="conary-test-${DISTRO}"
echo "[*] Building container image: $IMAGE_NAME"

podman build \
    $NO_CACHE \
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
podman run \
    --rm \
    --name "conary-test-run-${DISTRO}" \
    -v "${VOLUME_NAME}:/results:Z" \
    -e "DISTRO=${DISTRO}" \
    "$IMAGE_NAME" || CONTAINER_EXIT=$?

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
