#!/usr/bin/env bash
# scripts/publish-release.sh
#
# Build and publish a Conary release to Remi.
# Builds CCS + native packages, uploads to self-update dir + package API + releases dir.
#
# Usage:
#   ./scripts/publish-release.sh                    # Build and publish current version
#   ./scripts/publish-release.sh --version 0.3.0    # Override version
#   ./scripts/publish-release.sh --skip-build       # Use existing artifacts
#   ./scripts/publish-release.sh --dry-run          # Show what would happen

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

REMI_HOST="${REMI_HOST:-remi}"
REMI_ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"

DRY_RUN=false
SKIP_BUILD=false
VERSION=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true ;;
        --skip-build) SKIP_BUILD=true ;;
        --version)
            shift
            VERSION="${1:?--version requires a value}"
            ;;
        *)
            echo "Unknown option: $1" >&2
            echo "Usage: $0 [--version VERSION] [--skip-build] [--dry-run]" >&2
            exit 1
            ;;
    esac
    shift
done

# --- Step 1: Determine version ---
if [[ -z "$VERSION" ]]; then
    VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
fi

echo "=== Conary Release: v${VERSION} ==="
echo "  Remi host: $REMI_HOST"
echo "  Remi endpoint: $REMI_ENDPOINT"
echo "  Dry run: $DRY_RUN"
echo "  Skip build: $SKIP_BUILD"
echo ""

# Artifact paths
CCS_PKG="$REPO_ROOT/packaging/ccs/output/conary-${VERSION}.ccs"
RPM_PKG="$REPO_ROOT/packaging/rpm/output/conary-${VERSION}-1.fc43.x86_64.rpm"
DEB_PKG="$REPO_ROOT/packaging/deb/output/conary_${VERSION}-1_amd64.deb"
ARCH_PKG="$REPO_ROOT/packaging/arch/output/conary-${VERSION}-1-x86_64.pkg.tar.zst"

# --- Step 2-4: Build artifacts ---
if [[ "$SKIP_BUILD" == "true" ]]; then
    echo "[SKIP] Using existing artifacts (--skip-build)"
else
    # Step 2: Build release binary
    echo "[1/4] Building release binary..."
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "  [DRY RUN] Would run: cargo build --release"
    else
        cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
    fi

    # Step 3: Build CCS package
    echo "[2/4] Building CCS package..."
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "  [DRY RUN] Would run: packaging/ccs/build.sh"
    else
        bash "$REPO_ROOT/packaging/ccs/build.sh"
    fi

    # Step 4: Build native packages via Podman
    echo "[3/4] Building native packages (RPM, DEB, Arch)..."
    for fmt in rpm deb arch; do
        build_script="$REPO_ROOT/packaging/$fmt/build.sh"
        if [[ -f "$build_script" ]]; then
            echo "  Building $fmt..."
            if [[ "$DRY_RUN" == "true" ]]; then
                echo "  [DRY RUN] Would run: packaging/$fmt/build.sh --podman"
            else
                bash "$build_script" --podman || echo "  [WARN] $fmt build failed (continuing)"
            fi
        else
            echo "  [WARN] No build script for $fmt"
        fi
    done
fi

echo ""

# --- Step 5: Verify artifacts ---
echo "[4/4] Verifying artifacts..."

if [[ "$DRY_RUN" == "false" ]] && [[ ! -f "$CCS_PKG" ]]; then
    echo "[FATAL] CCS package not found: $CCS_PKG" >&2
    exit 1
fi

ARTIFACTS=("$CCS_PKG")
ARTIFACT_NAMES=("conary-${VERSION}.ccs")

for pkg_path in "$RPM_PKG" "$DEB_PKG" "$ARCH_PKG"; do
    pkg_name="$(basename "$pkg_path")"
    if [[ "$DRY_RUN" == "true" ]] || [[ -f "$pkg_path" ]]; then
        ARTIFACTS+=("$pkg_path")
        ARTIFACT_NAMES+=("$pkg_name")
        echo "  [OK] $pkg_name"
    else
        echo "  [WARN] Not found: $pkg_name (skipping)"
    fi
done

echo "  CCS: $CCS_PKG"
echo "  Total artifacts: ${#ARTIFACTS[@]}"
echo ""

# --- Step 6: Upload to Remi via SSH ---
echo "=== Uploading to Remi ==="

REMOTE_RELEASE_DIR="/conary/releases/${VERSION}"
REMOTE_SELF_UPDATE="/conary/self-update/conary-${VERSION}.ccs"

if [[ "$DRY_RUN" == "true" ]]; then
    echo "  [DRY RUN] Would create: $REMOTE_RELEASE_DIR"
    echo "  [DRY RUN] Would copy CCS to: $REMOTE_SELF_UPDATE"
    for name in "${ARTIFACT_NAMES[@]}"; do
        echo "  [DRY RUN] Would copy: $name -> $REMOTE_RELEASE_DIR/$name"
    done
    echo "  [DRY RUN] Would generate SHA256SUMS in $REMOTE_RELEASE_DIR"
    echo "  [DRY RUN] Would update /conary/releases/latest -> $VERSION"
else
    # Create release directory
    ssh "$REMI_HOST" "mkdir -p $REMOTE_RELEASE_DIR"

    # Copy CCS to self-update directory
    ssh "$REMI_HOST" "mkdir -p /conary/self-update"
    scp "$CCS_PKG" "${REMI_HOST}:${REMOTE_SELF_UPDATE}"
    echo "  [OK] CCS -> $REMOTE_SELF_UPDATE"

    # Copy all artifacts to release directory
    for artifact in "${ARTIFACTS[@]}"; do
        if [[ -f "$artifact" ]]; then
            scp "$artifact" "${REMI_HOST}:${REMOTE_RELEASE_DIR}/"
            echo "  [OK] $(basename "$artifact") -> $REMOTE_RELEASE_DIR/"
        fi
    done

    # Generate SHA256SUMS and update latest symlink
    ssh "$REMI_HOST" bash -s "$VERSION" <<'REMOTE_EOF'
        set -euo pipefail
        VERSION="$1"
        RELEASE_DIR="/conary/releases/${VERSION}"

        cd "$RELEASE_DIR"
        sha256sum -- * > SHA256SUMS
        echo "  Generated SHA256SUMS"

        ln -sfn "$VERSION" /conary/releases/latest
        echo "  Updated /conary/releases/latest -> $VERSION"
REMOTE_EOF
fi

echo ""

# --- Step 7: POST CCS to Remi API for each distro ---
# TODO: Enable when Remi has a package upload endpoint (POST /v1/{distro}/packages).
# For now, CCS is served via the self-update directory and releases directory.
echo "=== Publishing CCS to Remi API ==="
echo "  [SKIP] Package upload API not yet available on Remi (served via self-update + releases)"

echo ""

# --- Step 8: Smoke test ---
echo "=== Smoke test ==="

if [[ "$DRY_RUN" == "true" ]]; then
    echo "  [DRY RUN] Would verify $REMI_ENDPOINT/v1/ccs/conary/latest returns version $VERSION"
else
    latest_response=$(curl -sf "$REMI_ENDPOINT/v1/ccs/conary/latest" 2>/dev/null) || latest_response=""

    if echo "$latest_response" | grep -q "$VERSION"; then
        echo "  [OK] Self-update API reports version $VERSION"
    else
        echo "  [WARN] Version mismatch or endpoint unavailable"
        echo "  Response: $latest_response"
    fi
fi

echo ""
echo "=== Release v${VERSION} complete ==="
