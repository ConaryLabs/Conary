#!/usr/bin/env bash
# scripts/conaryd-health.sh -- Verify Forge-local conaryd health over the Unix socket.
set -euo pipefail

EXPECTED_VERSION=""
SOCKET_PATH="/run/conary/conaryd.sock"
SERVICE_NAME="conaryd"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --expected-version)
            EXPECTED_VERSION="${2:-}"
            shift 2
            ;;
        --expected-version=*)
            EXPECTED_VERSION="${1#*=}"
            shift
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

[[ -n "$EXPECTED_VERSION" ]] || {
    echo "--expected-version is required" >&2
    exit 1
}

if [[ "$(systemctl is-active "$SERVICE_NAME" 2>/dev/null || true)" != "active" ]]; then
    echo "service-not-running: ${SERVICE_NAME}" >&2
    exit 1
fi

PAYLOAD="$(sudo -n /usr/bin/curl --fail --silent --show-error \
    --unix-socket "$SOCKET_PATH" http://localhost/health)"

python3 - "$EXPECTED_VERSION" "$PAYLOAD" <<'PY'
import json
import sys

expected = sys.argv[1]
payload = json.loads(sys.argv[2])

if payload.get("status") != "healthy":
    raise SystemExit(f"unexpected status: {payload!r}")
if payload.get("version") != expected:
    raise SystemExit(
        f"version mismatch: expected {expected}, got {payload.get('version')}"
    )
PY

echo "[conaryd-health] ok"
