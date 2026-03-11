#!/usr/bin/env bash
# scripts/publish-test-fixtures.sh
# Build and publish test fixture CCS packages to Remi.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURE_DIR="$PROJECT_ROOT/tests/fixtures/conary-test-fixture"
ADVERSARIAL_DIR="$PROJECT_ROOT/tests/fixtures/adversarial"
REMI_ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"
CONARY_BIN="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"

publish_phase2_fixtures() {
    bash "$FIXTURE_DIR/build-all.sh"

    echo ""
    echo "Publishing Phase 2 fixtures to Remi package endpoints ($REMI_ENDPOINT)..."
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
}

publish_adversarial_fixtures() {
    echo ""
    echo "Building adversarial fixtures..."
    bash "$ADVERSARIAL_DIR/build-corrupted.sh" "$CONARY_BIN"
    bash "$ADVERSARIAL_DIR/build-malicious.sh" "$CONARY_BIN"
    bash "$ADVERSARIAL_DIR/build-deps.sh" "$CONARY_BIN"

    echo ""
    echo "Publishing adversarial fixtures to $REMI_ENDPOINT/test-fixtures/adversarial/..."
    while IFS= read -r -d '' pkg; do
        name="$(basename "$pkg")"
        printf "  %s... " "$name"
        curl -sf -T "$pkg" "$REMI_ENDPOINT/test-fixtures/adversarial/$name" \
            && echo "OK" \
            || echo "WARN (upload failed)"
    done < <(find \
        "$ADVERSARIAL_DIR/corrupted" \
        "$ADVERSARIAL_DIR/malicious" \
        "$ADVERSARIAL_DIR/deps" \
        -path '*/output/*.ccs' -type f -print0 | sort -z)
}

publish_phase2_fixtures
publish_adversarial_fixtures

echo "[OK] Done"
