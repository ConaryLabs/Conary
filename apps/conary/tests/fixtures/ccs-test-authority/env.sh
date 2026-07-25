#!/usr/bin/env bash
# tests/fixtures/ccs-test-authority/env.sh

FIXTURE_CCS_AUTHORITY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURE_CCS_KEY="$FIXTURE_CCS_AUTHORITY_DIR/fixture-signing-key.private"
FIXTURE_CCS_PUBLIC_KEY="$FIXTURE_CCS_AUTHORITY_DIR/fixture-signing-key.public"
FIXTURE_CCS_POLICY="$FIXTURE_CCS_AUTHORITY_DIR/trust-policy.toml"
FIXTURE_CCS_EXPIRED_POLICY="$FIXTURE_CCS_AUTHORITY_DIR/expired-trust-policy.toml"

export FIXTURE_CCS_AUTHORITY_DIR
export FIXTURE_CCS_KEY
export FIXTURE_CCS_PUBLIC_KEY
export FIXTURE_CCS_POLICY
export FIXTURE_CCS_EXPIRED_POLICY

fixture_ccs_require_authority() {
    local authority_path
    for authority_path in \
        "$FIXTURE_CCS_KEY" \
        "$FIXTURE_CCS_PUBLIC_KEY" \
        "$FIXTURE_CCS_POLICY" \
        "$FIXTURE_CCS_EXPIRED_POLICY"
    do
        if [ ! -f "$authority_path" ]; then
            echo "FATAL: missing CCS integration fixture authority file: $authority_path" >&2
            echo "Run $FIXTURE_CCS_AUTHORITY_DIR/generate.sh and rebuild the CCS fixtures." >&2
            return 1
        fi
    done
}
