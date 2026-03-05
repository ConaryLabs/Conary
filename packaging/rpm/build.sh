#!/usr/bin/env bash
# packaging/rpm/build.sh
#
# Build the Conary RPM. Vendors deps, creates source tarballs, runs rpmbuild.
#
# Usage:
#   ./packaging/rpm/build.sh           # Build locally (needs cargo + rpmbuild)
#   ./packaging/rpm/build.sh --podman  # Build inside a Fedora 43 container

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SPEC="$SCRIPT_DIR/conary.spec"

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

echo "Building $NAME $VERSION RPM"

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

if $USE_PODMAN; then
    # --- Podman build ---
    echo "[3/4] Building in Podman container..."
    IMAGE="conary-rpm-builder"

    podman build -t "$IMAGE" -f "$SCRIPT_DIR/Containerfile.build" "$SCRIPT_DIR"

    podman run --rm \
        -v "$TMPDIR/$TARNAME.tar.gz:/rpmbuild/SOURCES/$TARNAME.tar.gz:ro,Z" \
        -v "$TMPDIR/vendor.tar.gz:/rpmbuild/SOURCES/vendor.tar.gz:ro,Z" \
        -v "$SPEC:/rpmbuild/SPECS/conary.spec:ro,Z" \
        -v "$REPO_ROOT/packaging/rpm/output:/output:Z" \
        "$IMAGE" \
        bash -c '
            rpmbuild -bb \
                --define "_topdir /rpmbuild" \
                /rpmbuild/SPECS/conary.spec && \
            cp /rpmbuild/RPMS/*/*.rpm /output/
        '

    echo "[4/4] Done."
    echo "RPMs written to: $REPO_ROOT/packaging/rpm/output/"
    ls -lh "$REPO_ROOT/packaging/rpm/output/"*.rpm 2>/dev/null || echo "(no RPMs found -- check build output)"
else
    # --- Local rpmbuild ---
    echo "[3/4] Running rpmbuild..."
    RPMBUILD_DIR="$REPO_ROOT/packaging/rpm/rpmbuild"
    mkdir -p "$RPMBUILD_DIR"/{BUILD,RPMS,SRPMS,SOURCES,SPECS}

    cp "$TMPDIR/$TARNAME.tar.gz" "$RPMBUILD_DIR/SOURCES/"
    cp "$TMPDIR/vendor.tar.gz" "$RPMBUILD_DIR/SOURCES/"
    cp "$SPEC" "$RPMBUILD_DIR/SPECS/"

    rpmbuild -bb --nodeps \
        --define "_topdir $RPMBUILD_DIR" \
        "$RPMBUILD_DIR/SPECS/conary.spec"

    echo "[4/4] Done."
    echo "RPMs written to: $RPMBUILD_DIR/RPMS/"
    find "$RPMBUILD_DIR/RPMS" -name '*.rpm' -exec ls -lh {} \;
fi
