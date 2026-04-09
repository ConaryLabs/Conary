#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: forge-smoke.sh [--port PORT]

Lightweight Forge control-plane smoke check for conary-test.

Port resolution:
  1. --port PORT
  2. CONARY_TEST_PORT
  3. 9090
EOF
}

PORT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)
      PORT="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "${PORT}" ]]; then
  PORT="${CONARY_TEST_PORT:-9090}"
fi

if [[ -x "target/debug/conary-test" ]]; then
  CONARY_TEST_BIN="target/debug/conary-test"
elif command -v conary-test >/dev/null 2>&1; then
  CONARY_TEST_BIN="$(command -v conary-test)"
else
  echo "conary-test binary not found in target/debug or \$PATH" >&2
  exit 1
fi

HEALTH_URL="http://127.0.0.1:${PORT}/v1/health"
DEPLOY_URL="http://127.0.0.1:${PORT}/v1/deploy/status"

echo "[forge-smoke] probing ${HEALTH_URL}"
HEALTH_BODY="$(curl -fsS "${HEALTH_URL}")"
if [[ "${HEALTH_BODY}" != "ok" ]]; then
  echo "unexpected health body: ${HEALTH_BODY}" >&2
  exit 1
fi

echo "[forge-smoke] probing ${DEPLOY_URL}"
DEPLOY_ROUTE_JSON="$(curl -fsS "${DEPLOY_URL}")"
python3 - "${DEPLOY_ROUTE_JSON}" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
for key in ("binary", "runtime", "service"):
    if key not in payload:
        raise SystemExit(f"missing deploy route key: {key}")
PY

echo "[forge-smoke] checking conary-test health --json"
HEALTH_JSON="$(env -u REMI_ADMIN_TOKEN -u REMI_ADMIN_ENDPOINT "${CONARY_TEST_BIN}" --json health --port "${PORT}")"
python3 - "${HEALTH_JSON}" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
for key in ("mode", "deploy_status"):
    if key not in payload:
        raise SystemExit(f"missing health key: {key}")
PY

echo "[forge-smoke] checking conary-test deploy status --json"
DEPLOY_JSON="$("${CONARY_TEST_BIN}" --json deploy status --port "${PORT}")"
python3 - "${DEPLOY_JSON}" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
for key in ("checkout", "degraded", "reason"):
    if key not in payload:
        raise SystemExit(f"missing deploy status key: {key}")
for key in ("git_branch", "git_commit"):
    if key not in payload["checkout"]:
        raise SystemExit(f"missing checkout key: {key}")
PY

echo "[forge-smoke] ok"
