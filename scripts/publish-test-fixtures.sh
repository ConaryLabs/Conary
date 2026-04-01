#!/usr/bin/env bash
# scripts/publish-test-fixtures.sh
# Build and publish test fixture CCS packages to Remi.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURE_DIR="$PROJECT_ROOT/tests/fixtures/conary-test-fixture"
ADVERSARIAL_DIR="$PROJECT_ROOT/tests/fixtures/adversarial"
REMI_ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"
REMI_ADMIN_ENDPOINT="${REMI_ADMIN_ENDPOINT:-}"
REMI_ADMIN_TOKEN="${REMI_ADMIN_TOKEN:-}"
CONARY_BIN="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"

if [ -z "$REMI_ADMIN_ENDPOINT" ]; then
    echo "FATAL: REMI_ADMIN_ENDPOINT is required for admin uploads." >&2
    echo "Set it to a direct admin origin or SSH tunnel base URL; do not rely on ${REMI_ENDPOINT%/}:8082 behind the Cloudflare proxy." >&2
    exit 1
fi

publish_phase2_fixtures() {
    bash "$FIXTURE_DIR/build-all.sh"

    if [ -z "$REMI_ADMIN_TOKEN" ]; then
        echo "FATAL: REMI_ADMIN_TOKEN is required to publish Phase 2 fixtures" >&2
        exit 1
    fi

    echo ""
    echo "Publishing Phase 2 fixtures to Remi admin package endpoints ($REMI_ADMIN_ENDPOINT)..."
    for ver in v1 v2; do
        pkg=$(ls "$FIXTURE_DIR/$ver/output/"*.ccs 2>/dev/null | head -1)
        [ -z "$pkg" ] && { echo "FATAL: No CCS for $ver" >&2; exit 1; }

        for distro in fedora ubuntu arch; do
            printf "  %s -> %s... " "$ver" "$distro"
            curl -sf -X POST \
                -H "Authorization: Bearer $REMI_ADMIN_TOKEN" \
                --data-binary "@$pkg" \
                "$REMI_ADMIN_ENDPOINT/v1/admin/packages/$distro" \
                && echo "OK" \
                || echo "WARN (upload failed)"
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
    echo "Publishing adversarial fixtures to $REMI_ADMIN_ENDPOINT/v1/admin/test-fixtures/adversarial/..."
    while IFS= read -r -d '' pkg; do
        relative_path="${pkg#"$ADVERSARIAL_DIR"/}"
        printf "  %s... " "$relative_path"
        curl -sf -X PUT \
            -H "Authorization: Bearer $REMI_ADMIN_TOKEN" \
            --data-binary "@$pkg" \
            "$REMI_ADMIN_ENDPOINT/v1/admin/test-fixtures/$relative_path" \
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
