#!/usr/bin/env bash
# packaging/arch/build.sh
#
# Build the Conary Arch package. Vendors deps, creates source tarballs, runs makepkg.
#
# Usage:
#   ./packaging/arch/build.sh           # Build locally (needs cargo + makepkg)
#   ./packaging/arch/build.sh --podman  # Build inside an Arch Linux container

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
NAME="conary"
TARNAME="$NAME-$VERSION"

USE_PODMAN=false
for arg in "$@"; do
    case "$arg" in
        --podman) USE_PODMAN=true ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

echo "Building $NAME $VERSION Arch package"

# --- Vendor dependencies ---
echo "[1/4] Vendoring dependencies..."
cd "$REPO_ROOT"
cargo vendor --locked vendor > /dev/null 2>&1

# --- Create source tarballs ---
echo "[2/4] Creating source tarballs..."
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Source tarball (excludes heavy dirs)
mkdir -p "$TMPDIR/$TARNAME"
tar cf - \
    --exclude=target \
    --exclude=vendor \
    --exclude=web/node_modules \
    --exclude=site/node_modules \
    --exclude='*.db' \
    --exclude='*.db-shm' \
    --exclude='*.db-wal' \
    --exclude='.claude' \
    --exclude='.git' \
    . | tar xf - -C "$TMPDIR/$TARNAME"

tar czf "$TMPDIR/$TARNAME.tar.gz" -C "$TMPDIR" "$TARNAME"

# Vendor tarball
tar czf "$TMPDIR/vendor.tar.gz" -C "$REPO_ROOT" vendor

mkdir -p "$SCRIPT_DIR/output"

if $USE_PODMAN; then
    # --- Podman build ---
    echo "[3/4] Building in Podman container..."
    IMAGE="conary-arch-builder"

    podman build -t "$IMAGE" -f "$SCRIPT_DIR/Containerfile.build" "$SCRIPT_DIR"

    podman run --rm \
        -v "$TMPDIR/$TARNAME.tar.gz:/build/$TARNAME.tar.gz:ro,Z" \
        -v "$TMPDIR/vendor.tar.gz:/build/vendor.tar.gz:ro,Z" \
        -v "$SCRIPT_DIR/PKGBUILD:/build/PKGBUILD:ro,Z" \
        -v "$SCRIPT_DIR/conary.install:/build/conary.install:ro,Z" \
        -v "$SCRIPT_DIR/output:/output:Z" \
        "$IMAGE" \
        bash -c '
            cd /build && \
            makepkg -sf --noconfirm --skipchecksums && \
            cp /build/*.pkg.tar.zst /output/
        '

    echo "[4/4] Done."
    echo "Package written to: $SCRIPT_DIR/output/"
    ls -lh "$SCRIPT_DIR/output/"*.pkg.tar.zst 2>/dev/null || echo "(no package found -- check build output)"
else
    # --- Local makepkg ---
    echo "[3/4] Running makepkg..."
    BUILDDIR="$TMPDIR/makepkg-build"
    mkdir -p "$BUILDDIR"

    cp "$TMPDIR/$TARNAME.tar.gz" "$BUILDDIR/"
    cp "$TMPDIR/vendor.tar.gz" "$BUILDDIR/"
    cp "$SCRIPT_DIR/PKGBUILD" "$BUILDDIR/"
    cp "$SCRIPT_DIR/conary.install" "$BUILDDIR/"

    cd "$BUILDDIR"
    makepkg -sf --noconfirm --skipchecksums

    cp "$BUILDDIR"/*.pkg.tar.zst "$SCRIPT_DIR/output/" 2>/dev/null || true

    echo "[4/4] Done."
    echo "Package written to: $SCRIPT_DIR/output/"
    ls -lh "$SCRIPT_DIR/output/"*.pkg.tar.zst 2>/dev/null || echo "(no package found -- check build output)"
fi
