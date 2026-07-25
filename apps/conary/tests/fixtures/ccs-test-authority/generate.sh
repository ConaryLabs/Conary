#!/usr/bin/env bash
# tests/fixtures/ccs-test-authority/generate.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../../.." && pwd)"
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
CONARY_BIN="${CONARY_BIN:-$CARGO_TARGET_ROOT/debug/conary}"
KEY_BASE="$SCRIPT_DIR/fixture-signing-key"

umask 077
"$CONARY_BIN" ccs keygen \
    --output "$KEY_BASE" \
    --key-id "conary-integration-fixtures" \
    --force

public_key="$(python3 - "$KEY_BASE.public" <<'PY'
import pathlib
import sys
import tomllib

path = pathlib.Path(sys.argv[1])
with path.open("rb") as stream:
    print(tomllib.load(stream)["key"])
PY
)"

cat > "$SCRIPT_DIR/trust-policy.toml" <<EOF
trusted_keys = ["$public_key"]
require_timestamp = true
max_signature_age = 0
EOF

cat > "$SCRIPT_DIR/expired-trust-policy.toml" <<EOF
trusted_keys = ["$public_key"]
require_timestamp = true
max_signature_age = 86400
EOF

chmod 0600 "$KEY_BASE.private"
chmod 0644 \
    "$KEY_BASE.public" \
    "$SCRIPT_DIR/trust-policy.toml" \
    "$SCRIPT_DIR/expired-trust-policy.toml"

echo "Generated disposable CCS integration fixture authority in $SCRIPT_DIR"
