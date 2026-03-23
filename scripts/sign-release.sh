#!/usr/bin/env bash
# scripts/sign-release.sh
# Sign a CCS self-update package for release.
# Requires RELEASE_SIGNING_KEY env var (hex-encoded 32-byte Ed25519 seed).
# Usage: sign-release.sh <path-to-ccs-file>
# Output: creates <path>.sig

set -euo pipefail

if [ $# -ne 1 ]; then
    echo "Usage: $0 <path-to-ccs-file>" >&2
    exit 1
fi

CCS_FILE="$1"
SIG_FILE="${CCS_FILE}.sig"

if [ ! -f "$CCS_FILE" ]; then
    echo "Error: CCS file not found: $CCS_FILE" >&2
    exit 1
fi

if [ -z "${RELEASE_SIGNING_KEY:-}" ]; then
    echo "Error: RELEASE_SIGNING_KEY not set" >&2
    exit 1
fi

SIGN_BIN="./target/release/examples/sign_hash"
if [ ! -f "$SIGN_BIN" ]; then
    echo "Building sign_hash helper..."
    cargo build --example sign_hash -p conary-core --release --quiet
fi

echo "Signing $CCS_FILE..."
"$SIGN_BIN" "$CCS_FILE" > "$SIG_FILE"

if [ ! -s "$SIG_FILE" ]; then
    echo "Error: signing produced empty output" >&2
    rm -f "$SIG_FILE"
    exit 1
fi

echo "Signature written to $SIG_FILE"
