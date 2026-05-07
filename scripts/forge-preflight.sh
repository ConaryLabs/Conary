#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: forge-preflight.sh [--mode container|qemu]

Read-only runtime checks for trusted Forge lanes.
EOF
}

MODE="container"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="${2:-}"
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

case "$MODE" in
  container|qemu)
    ;;
  *)
    echo "invalid mode: ${MODE}" >&2
    usage >&2
    exit 1
    ;;
esac

RUNNER_UID="${FORGE_RUNNER_UID:-$(id -u)}"
PODMAN_SOCKET="${PODMAN_SOCKET:-/run/user/${RUNNER_UID}/podman/podman.sock}"

fail() {
  echo "[forge-preflight] ERROR: $*" >&2
  exit 1
}

echo "[forge-preflight] checking Podman socket: ${PODMAN_SOCKET}"
test -S "${PODMAN_SOCKET}" || fail "missing Podman socket; enable linger and podman.socket for the runner user"

echo "[forge-preflight] checking Podman CLI"
command -v podman >/dev/null 2>&1 || fail "podman is not installed"
DOCKER_HOST="unix://${PODMAN_SOCKET}" podman info >/dev/null

echo "[forge-preflight] checking Docker-compatible Podman API"
curl --unix-socket "${PODMAN_SOCKET}" -fsS http://d/v1.41/_ping >/dev/null \
  || curl --unix-socket "${PODMAN_SOCKET}" -fsS http://d/_ping >/dev/null \
  || fail "Podman socket did not answer Docker-compatible API ping"

if [[ "${MODE}" == "qemu" ]]; then
  echo "[forge-preflight] checking QEMU tools"
  command -v qemu-system-x86_64 >/dev/null 2>&1 || fail "qemu-system-x86_64 is not installed"
  command -v qemu-img >/dev/null 2>&1 || fail "qemu-img is not installed"
  command -v scp >/dev/null 2>&1 || fail "scp is not installed"
  command -v rg >/dev/null 2>&1 || fail "ripgrep is not installed"
  test -e /dev/kvm || echo "[forge-preflight] warning: /dev/kvm missing; QEMU tests may be slow or fail" >&2
fi

echo "[forge-preflight] ok"
