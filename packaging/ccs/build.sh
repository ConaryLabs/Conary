#!/usr/bin/env bash
# packaging/ccs/build.sh
#
# Build a CCS package of Conary for self-updates via Remi.
# Stages the release binary + man page + completions, then runs conary ccs build.
#
# Usage:
#   ./packaging/ccs/build.sh                    # Build from local release binary
#   ./packaging/ccs/build.sh --from-rpm <rpm>   # Extract from RPM build

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
NAME="conary"

echo "Building $NAME $VERSION CCS package"

# --- Build release binary if needed ---
RELEASE_BIN="$REPO_ROOT/target/release/$NAME"
if [ ! -f "$RELEASE_BIN" ]; then
    echo "[1/4] Building release binary..."
    cargo build --release -p conary --manifest-path "$REPO_ROOT/Cargo.toml"
else
    echo "[1/4] Using existing release binary: $RELEASE_BIN"
fi

# --- Stage files in install layout ---
echo "[2/4] Staging files..."
STAGE="$SCRIPT_DIR/stage"
rm -rf "$STAGE"

# Binary
install -Dpm 0755 "$RELEASE_BIN" "$STAGE/usr/bin/$NAME"

# Man page
install -Dpm 0644 "$REPO_ROOT/man/$NAME.1" "$STAGE/usr/share/man/man1/$NAME.1"

# Shell completions
install -d "$STAGE/usr/share/bash-completion/completions"
install -d "$STAGE/usr/share/zsh/site-functions"
install -d "$STAGE/usr/share/fish/vendor_completions.d"
"$RELEASE_BIN" system completions bash > "$STAGE/usr/share/bash-completion/completions/$NAME"
"$RELEASE_BIN" system completions zsh  > "$STAGE/usr/share/zsh/site-functions/_$NAME"
"$RELEASE_BIN" system completions fish > "$STAGE/usr/share/fish/vendor_completions.d/$NAME.fish"

# Licenses
install -Dpm 0644 "$REPO_ROOT/LICENSE-MIT" "$STAGE/usr/share/licenses/$NAME/LICENSE-MIT"
install -Dpm 0644 "$REPO_ROOT/LICENSE-APACHE" "$STAGE/usr/share/licenses/$NAME/LICENSE-APACHE"

# Config and data directories
install -d "$STAGE/etc/$NAME"
install -d "$STAGE/var/lib/$NAME"

# Copy manifest into stage
cp "$SCRIPT_DIR/ccs.toml" "$STAGE/ccs.toml"

# --- Build CCS package ---
echo "[3/4] Building CCS package..."
OUTPUT="$SCRIPT_DIR/output"
mkdir -p "$OUTPUT"

"$RELEASE_BIN" ccs build "$STAGE" \
    --output "$OUTPUT" \
    --target ccs \
    --source "$STAGE"

echo "[4/4] Done."
find "$OUTPUT" -name '*.ccs' -exec ls -lh {} \;
