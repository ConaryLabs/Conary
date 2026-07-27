#!/usr/bin/env bash
# packaging/ccs/build.sh
#
# Build a CCS package of Conary for self-updates via Remi.
# Stages the release binary + man page + completions, then runs conary ccs build.
#
# Usage:
#   ./packaging/ccs/build.sh --version <version> --key <private-key>

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

usage() {
    echo "Usage: $0 --version <version> --key <private-key>" >&2
    exit 1
}

VERSION=""
SIGNING_KEY=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            [[ $# -ge 2 ]] || usage
            VERSION="$2"
            shift 2
            ;;
        --key)
            [[ $# -ge 2 ]] || usage
            SIGNING_KEY="$2"
            shift 2
            ;;
        *)
            usage
            ;;
    esac
done

[[ -n "$VERSION" && -n "$SIGNING_KEY" ]] || usage
if [[ ! -f "$SIGNING_KEY" || -L "$SIGNING_KEY" ]]; then
    echo "CCS release signing key must be a regular, non-symlink file: $SIGNING_KEY" >&2
    exit 1
fi

bash "$REPO_ROOT/scripts/release-matrix.sh" assert-owned-version conary "$VERSION"

NAME="conary"
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    if [[ "$CARGO_TARGET_DIR" = /* ]]; then
        TARGET_DIR="$CARGO_TARGET_DIR"
    else
        TARGET_DIR="$REPO_ROOT/$CARGO_TARGET_DIR"
    fi
else
    TARGET_DIR="$REPO_ROOT/target"
fi

echo "Building $NAME $VERSION CCS package"

# --- Build release binary ---
RELEASE_BIN="$TARGET_DIR/release/$NAME"
echo "[1/4] Building release binary..."
cargo build \
    --release \
    --manifest-path "$REPO_ROOT/apps/conary/Cargo.toml" \
    --target-dir "$TARGET_DIR"

# --- Stage files in install layout ---
echo "[2/4] Staging files..."
STAGE="$SCRIPT_DIR/stage"
rm -rf "$STAGE"

# Binary
install -Dpm 0755 "$RELEASE_BIN" "$STAGE/usr/bin/$NAME"

# Man page
install -Dpm 0644 "$REPO_ROOT/apps/conary/man/$NAME.1" "$STAGE/usr/share/man/man1/$NAME.1"

# Booted-generation activation
install -Dpm 0644 \
    "$REPO_ROOT/packaging/systemd/$NAME-generation-activation.service" \
    "$STAGE/usr/lib/systemd/system/$NAME-generation-activation.service"
install -d "$STAGE/usr/lib/systemd/system/multi-user.target.wants"
ln -s "../$NAME-generation-activation.service" \
    "$STAGE/usr/lib/systemd/system/multi-user.target.wants/$NAME-generation-activation.service"

# Shell completions
install -d "$STAGE/usr/share/bash-completion/completions"
install -d "$STAGE/usr/share/zsh/site-functions"
install -d "$STAGE/usr/share/fish/vendor_completions.d"
"$RELEASE_BIN" system completions bash > "$STAGE/usr/share/bash-completion/completions/$NAME"
"$RELEASE_BIN" system completions zsh  > "$STAGE/usr/share/zsh/site-functions/_$NAME"
"$RELEASE_BIN" system completions fish > "$STAGE/usr/share/fish/vendor_completions.d/$NAME.fish"

# License
install -Dpm 0644 "$REPO_ROOT/LICENSE" "$STAGE/usr/share/licenses/$NAME/LICENSE"

# Config and data directories
install -d "$STAGE/etc/$NAME"
install -d "$STAGE/var/lib/$NAME"

# Copy manifest into stage
cp "$SCRIPT_DIR/ccs.toml" "$STAGE/ccs.toml"

# --- Build CCS package ---
echo "[3/4] Building CCS package..."
OUTPUT="$SCRIPT_DIR/output"
mkdir -p "$OUTPUT"
find "$OUTPUT" -maxdepth 1 -name '*.ccs' -delete

"$RELEASE_BIN" ccs build "$STAGE" \
    --output "$OUTPUT" \
    --target ccs \
    --source "$STAGE" \
    --key "$SIGNING_KEY"

echo "[4/4] Done."
BUILT_CCS="$OUTPUT/${NAME}-${VERSION}-1.ccs"
EXPECTED_CCS="$OUTPUT/${NAME}-${VERSION}.ccs"
shopt -s nullglob
ccs_files=("$OUTPUT"/*.ccs)
if [[ ${#ccs_files[@]} -ne 1 || "${ccs_files[0]:-}" != "$BUILT_CCS" || ! -s "$BUILT_CCS" || -L "$BUILT_CCS" ]]; then
    echo "Expected exactly one CCS package at $BUILT_CCS; found ${#ccs_files[@]}" >&2
    exit 1
fi
mv -- "$BUILT_CCS" "$EXPECTED_CCS"

echo "CCS package written to: $EXPECTED_CCS"
ls -lh "$EXPECTED_CCS"
