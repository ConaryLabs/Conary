#!/usr/bin/env bash
# tests/fixtures/adversarial/build-boot-image.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONARY_BIN="${1:-${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}}"

IMAGE_VERSION="${IMAGE_VERSION:-v1}"
IMAGE_NAME="minimal-boot-${IMAGE_VERSION}.qcow2"
OUTPUT_DIR="${OUTPUT_DIR:-$SCRIPT_DIR/output}"
OUTPUT_PATH="$OUTPUT_DIR/$IMAGE_NAME"

BOOTSTRAP_WORK_DIR="${BOOTSTRAP_WORK_DIR:-/tmp/conary-bootstrap-${IMAGE_VERSION}}"
BOOTSTRAP_ROOT="${BOOTSTRAP_ROOT:-$BOOTSTRAP_WORK_DIR/sysroot}"
BOOTSTRAP_RECIPE_DIR="${BOOTSTRAP_RECIPE_DIR:-$PROJECT_ROOT/recipes/core}"
TARGET_ARCH="${TARGET_ARCH:-x86_64}"
IMAGE_SIZE="${IMAGE_SIZE:-4G}"
AUTO_BUILD_BASE="${AUTO_BUILD_BASE:-0}"
SKIP_VERIFY="${SKIP_VERIFY:-0}"

if [ ! -x "$CONARY_BIN" ]; then
    echo "FATAL: conary binary not found or not executable: $CONARY_BIN" >&2
    exit 1
fi

run_conary() {
    echo "+ $*"
    "$CONARY_BIN" "$@"
}

ensure_bootstrap_initialized() {
    if [ -d "$BOOTSTRAP_WORK_DIR" ]; then
        return
    fi

    echo "Initializing bootstrap work directory at $BOOTSTRAP_WORK_DIR..."
    run_conary bootstrap init \
        --work-dir "$BOOTSTRAP_WORK_DIR" \
        --target "$TARGET_ARCH"
}

ensure_bootstrap_base() {
    if [ -d "$BOOTSTRAP_ROOT" ]; then
        return
    fi

    if [ "$AUTO_BUILD_BASE" != "1" ]; then
        echo "FATAL: bootstrap sysroot not found at $BOOTSTRAP_ROOT" >&2
        echo "Set AUTO_BUILD_BASE=1 to build the base system automatically," >&2
        echo "or point BOOTSTRAP_WORK_DIR / BOOTSTRAP_ROOT at existing bootstrap output." >&2
        exit 1
    fi

    echo "Bootstrap sysroot missing; building base system into $BOOTSTRAP_ROOT..."
    ensure_bootstrap_initialized

    args=(
        bootstrap base
        --work-dir "$BOOTSTRAP_WORK_DIR"
        --root "$BOOTSTRAP_ROOT"
        --recipe-dir "$BOOTSTRAP_RECIPE_DIR"
    )
    if [ "$SKIP_VERIFY" = "1" ]; then
        args+=(--skip-verify)
    fi

    run_conary "${args[@]}"
}

build_image() {
    mkdir -p "$OUTPUT_DIR"
    rm -f "$OUTPUT_PATH"

    echo "Building minimal QEMU boot image..."
    echo "  Bootstrap work dir: $BOOTSTRAP_WORK_DIR"
    echo "  Bootstrap root:     $BOOTSTRAP_ROOT"
    echo "  Output image:       $OUTPUT_PATH"
    echo "  Image size:         $IMAGE_SIZE"

    run_conary bootstrap image \
        --work-dir "$BOOTSTRAP_WORK_DIR" \
        --output "$OUTPUT_PATH" \
        --format qcow2 \
        --size "$IMAGE_SIZE"

    echo ""
    echo "[OK] Boot image built:"
    printf '  %s\n' "$OUTPUT_PATH"
    sha256sum "$OUTPUT_PATH"
}

ensure_bootstrap_initialized
ensure_bootstrap_base
build_image
