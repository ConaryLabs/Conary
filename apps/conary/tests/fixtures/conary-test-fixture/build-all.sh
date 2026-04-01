#!/usr/bin/env bash
# tests/fixtures/conary-test-fixture/build-all.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONARY="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"

for ver in v1 v2; do
    echo "Building conary-test-fixture $ver..."
    mkdir -p "$SCRIPT_DIR/$ver/output"
    "$CONARY" ccs build "$SCRIPT_DIR/$ver/ccs.toml" \
        --source "$SCRIPT_DIR/$ver/stage" \
        --output "$SCRIPT_DIR/$ver/output/"
done

echo ""
echo "Checksums for config.toml:"
echo "  v1 hello: $(sha256sum "$SCRIPT_DIR/v1/stage/usr/share/conary-test/hello.txt" | awk '{print $1}')"
echo "  v2 hello: $(sha256sum "$SCRIPT_DIR/v2/stage/usr/share/conary-test/hello.txt" | awk '{print $1}')"
echo "  v2 added: $(sha256sum "$SCRIPT_DIR/v2/stage/usr/share/conary-test/added.txt" | awk '{print $1}')"
