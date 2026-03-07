#!/usr/bin/env bash
# scripts/publish-test-fixtures.sh
# Build and publish test fixture CCS packages to Remi.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURE_DIR="$PROJECT_ROOT/tests/fixtures/conary-test-fixture"
REMI_ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"

bash "$FIXTURE_DIR/build-all.sh"

echo ""
echo "Publishing to Remi ($REMI_ENDPOINT)..."
for ver in v1 v2; do
    pkg=$(ls "$FIXTURE_DIR/$ver/output/"*.ccs 2>/dev/null | head -1)
    [ -z "$pkg" ] && { echo "FATAL: No CCS for $ver" >&2; exit 1; }

    for distro in fedora ubuntu arch; do
        printf "  %s -> %s... " "$ver" "$distro"
        curl -sf -X POST "$REMI_ENDPOINT/v1/$distro/packages" \
            -F "package=@$pkg" -F "format=ccs" \
            && echo "OK" \
            || echo "WARN (may already exist)"
    done
done
echo "[OK] Done"
