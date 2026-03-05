#!/usr/bin/env bash
# packaging/deb/build.sh
#
# Build the Conary DEB. Vendors deps, copies source into a build tree, runs dpkg-buildpackage.
#
# Usage:
#   ./packaging/deb/build.sh           # Build locally (needs cargo + dpkg-buildpackage)
#   ./packaging/deb/build.sh --podman  # Build inside an Ubuntu Noble container

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
NAME="conary"

USE_PODMAN=false
for arg in "$@"; do
    case "$arg" in
        --podman) USE_PODMAN=true ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

echo "Building $NAME $VERSION DEB"

# --- Vendor dependencies ---
echo "[1/4] Vendoring dependencies..."
cd "$REPO_ROOT"
cargo vendor --locked vendor > /dev/null 2>&1

# --- Create build tree ---
echo "[2/4] Creating build tree..."
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

BUILDDIR="$TMPDIR/$NAME-$VERSION"
mkdir -p "$BUILDDIR"

# Copy source (excludes heavy dirs)
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
    . | tar xf - -C "$BUILDDIR"

# Copy vendored deps
cp -a vendor "$BUILDDIR/vendor"

# Copy debian directory into build tree
cp -a "$SCRIPT_DIR/debian" "$BUILDDIR/debian"

mkdir -p "$SCRIPT_DIR/output"

if $USE_PODMAN; then
    # --- Podman build ---
    echo "[3/4] Building in Podman container..."
    IMAGE="conary-deb-builder"

    podman build -t "$IMAGE" -f "$SCRIPT_DIR/Containerfile.build" "$SCRIPT_DIR"

    podman run --rm \
        -v "$BUILDDIR:/build/src:Z" \
        -v "$SCRIPT_DIR/output:/output:Z" \
        "$IMAGE" \
        bash -c '
            cd /build/src && \
            dpkg-buildpackage -us -uc -b && \
            cp /build/*.deb /output/
        '

    echo "[4/4] Done."
    echo "DEBs written to: $SCRIPT_DIR/output/"
    ls -lh "$SCRIPT_DIR/output/"*.deb 2>/dev/null || echo "(no DEBs found -- check build output)"
else
    # --- Local build ---
    echo "[3/4] Running dpkg-buildpackage..."
    cd "$BUILDDIR"
    dpkg-buildpackage -us -uc -b

    echo "[4/4] Done."
    cp "$TMPDIR"/*.deb "$SCRIPT_DIR/output/" 2>/dev/null || true
    echo "DEBs written to: $SCRIPT_DIR/output/"
    ls -lh "$SCRIPT_DIR/output/"*.deb 2>/dev/null || echo "(no DEBs found -- check build output)"
fi
