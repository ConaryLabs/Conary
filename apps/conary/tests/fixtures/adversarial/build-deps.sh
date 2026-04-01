#!/usr/bin/env bash
# tests/fixtures/adversarial/build-deps.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONARY_BIN="${1:-${CONARY_BIN:-$(pwd)/target/debug/conary}}"
DEPS_DIR="$SCRIPT_DIR/deps"

fixtures=(
    dep-base-v1
    dep-base-v2
    dep-liba-v1
    dep-liba-v2
    dep-libb-v1
    dep-app-v1
    dep-circular-a-v1
    dep-circular-b-v1
    dep-virtual-provider-v1
    dep-virtual-consumer-v1
    dep-or-a-v1
    dep-or-b-v1
    dep-or-consumer-v1
    dep-unresolvable-v1
)

echo "Building dependency fixture packages..."
for fixture in "${fixtures[@]}"; do
    fixture_dir="$DEPS_DIR/$fixture"
    mkdir -p "$fixture_dir/output"
    rm -f "$fixture_dir/output/"*.ccs
    "$CONARY_BIN" ccs build "$fixture_dir/ccs.toml" \
        --source "$fixture_dir/stage" \
        --output "$fixture_dir/output/"
done

echo "Writing SHA256SUMS..."
find "$DEPS_DIR" -path '*/output/*.ccs' -type f -print0 \
    | sort -z \
    | xargs -0 sha256sum > "$DEPS_DIR/SHA256SUMS"

echo "[OK] Dependency fixtures built"
