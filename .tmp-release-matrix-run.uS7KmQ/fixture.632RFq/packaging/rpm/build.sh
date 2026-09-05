#!/usr/bin/env bash
# packaging/rpm/build.sh
#
# Build the Conary RPM. Vendors deps, creates source tarballs, runs rpmbuild.
#
# Usage:
#   ./packaging/rpm/build.sh           # Build locally (needs cargo + rpmbuild)
#   ./packaging/rpm/build.sh --podman  # Build inside a Fedora 44 container

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SPEC="$SCRIPT_DIR/conary.spec"
OUTPUT="$SCRIPT_DIR/output"

VERSION="$(bash "$REPO_ROOT/scripts/release-matrix.sh" workspace-version)"
bash "$REPO_ROOT/scripts/release-matrix.sh" assert-owned-version suite "$VERSION"
NAME="conary"
TARNAME="$NAME-$VERSION"

USE_PODMAN=false
for arg in "$@"; do
    case "$arg" in
        --podman) USE_PODMAN=true ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

require_unitdir_macro() {
    local unitdir
    unitdir="$(rpm --eval '%{_unitdir}')"
    if [[ "$unitdir" != "/usr/lib/systemd/system" ]]; then
        echo "Fedora systemd RPM macro authority is unavailable: %{_unitdir} resolved to $unitdir" >&2
        echo "Install the systemd-rpm-macros build dependency before building Conary." >&2
        exit 1
    fi
}

if ! $USE_PODMAN; then
    require_unitdir_macro
fi

echo "Building $NAME $VERSION RPM"

mkdir -p "$OUTPUT"
find "$OUTPUT" -maxdepth 1 -name '*.rpm' -delete

# --- Vendor dependencies ---
echo "[1/4] Vendoring dependencies..."
cd "$REPO_ROOT"
if [ ! -d vendor ] || [ -z "$(ls -A vendor 2>/dev/null)" ]; then
    cargo vendor vendor > /dev/null 2>&1
else
    echo "  Using existing vendor directory"
fi

# --- Create source tarballs ---
echo "[2/4] Creating source tarballs..."
TMPDIR=$(mktemp -d -p "${BUILDTMP:-${TMPDIR:-/tmp}}")
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

# Vendor tarball (copy to tmpdir first to avoid races with concurrent builds)
cp -a "$REPO_ROOT/vendor" "$TMPDIR/vendor"
tar czf "$TMPDIR/vendor.tar.gz" -C "$TMPDIR" vendor
rm -rf "$TMPDIR/vendor"

if $USE_PODMAN; then
    # --- Podman build ---
    echo "[3/4] Building in Podman container..."
    IMAGE="conary-rpm-builder"

    podman build -t "$IMAGE" -f "$SCRIPT_DIR/Containerfile.build" "$SCRIPT_DIR"

    podman run --rm \
        -v "$TMPDIR/$TARNAME.tar.gz:/rpmbuild/SOURCES/$TARNAME.tar.gz:ro,Z" \
        -v "$TMPDIR/vendor.tar.gz:/rpmbuild/SOURCES/vendor.tar.gz:ro,Z" \
        -v "$SPEC:/rpmbuild/SPECS/conary.spec:ro,Z" \
        -v "$OUTPUT:/output:Z" \
        "$IMAGE" \
        bash -c '
            rpmbuild -bb \
                --define "_topdir /rpmbuild" \
                /rpmbuild/SPECS/conary.spec && \
            cp /rpmbuild/RPMS/*/*.rpm /output/
        '

    echo "[4/4] Done."
else
    # --- Local rpmbuild ---
    echo "[3/4] Running rpmbuild..."
    RPMBUILD_DIR="$REPO_ROOT/packaging/rpm/rpmbuild"
    mkdir -p "$RPMBUILD_DIR"/{BUILD,RPMS,SRPMS,SOURCES,SPECS}
    find "$RPMBUILD_DIR/RPMS" -type f -name '*.rpm' -delete

    cp "$TMPDIR/$TARNAME.tar.gz" "$RPMBUILD_DIR/SOURCES/"
    cp "$TMPDIR/vendor.tar.gz" "$RPMBUILD_DIR/SOURCES/"
    cp "$SPEC" "$RPMBUILD_DIR/SPECS/"

    rpmbuild -bb --nodeps \
        --define "_topdir $RPMBUILD_DIR" \
        "$RPMBUILD_DIR/SPECS/conary.spec"

    # Copy RPMs to output/ for consistency with CI artifact paths
    find "$RPMBUILD_DIR/RPMS" -name '*.rpm' -exec cp {} "$OUTPUT/" \;

    echo "[4/4] Done."
fi

shopt -s nullglob
rpm_outputs=("$OUTPUT"/*.rpm)
versioned_rpm_outputs=("$OUTPUT/$NAME-$VERSION-"*.x86_64.rpm)
if [[ ${#rpm_outputs[@]} -ne 1 ||
      ${#versioned_rpm_outputs[@]} -ne 1 ||
      "${rpm_outputs[0]:-}" != "${versioned_rpm_outputs[0]:-}" ||
      ! -s "${rpm_outputs[0]:-}" ||
      -L "${rpm_outputs[0]:-}" ]]; then
    echo "Expected exactly one $NAME $VERSION x86_64 RPM, found ${#rpm_outputs[@]}" >&2
    exit 1
fi

echo "RPM written to: ${rpm_outputs[0]}"
ls -lh "${rpm_outputs[0]}"
