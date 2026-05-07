#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: forge-container-cleanup.sh

Reclaim inactive rootless Podman storage on the trusted Forge runner before
container-heavy validation jobs.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
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

RUNNER_UID="${FORGE_RUNNER_UID:-$(id -u)}"
PODMAN_SOCKET="${PODMAN_SOCKET:-/run/user/${RUNNER_UID}/podman/podman.sock}"

fail() {
  echo "[forge-container-cleanup] ERROR: $*" >&2
  exit 1
}

echo "[forge-container-cleanup] disk usage before cleanup"
df -h "${HOME}" || true

test -S "${PODMAN_SOCKET}" || fail "missing Podman socket: ${PODMAN_SOCKET}"
command -v podman >/dev/null 2>&1 || fail "podman is not installed"

echo "[forge-container-cleanup] podman usage before cleanup"
DOCKER_HOST="unix://${PODMAN_SOCKET}" podman system df || true

echo "[forge-container-cleanup] removing stale conary-test containers"
mapfile -t STALE_CONARY_TEST_CONTAINERS < <(
  DOCKER_HOST="unix://${PODMAN_SOCKET}" podman ps -a --format '{{.ID}} {{.Image}}' |
    awk '$2 ~ /(^|\/)conary-test-/ { print $1 }'
)
if (( ${#STALE_CONARY_TEST_CONTAINERS[@]} > 0 )); then
  DOCKER_HOST="unix://${PODMAN_SOCKET}" podman rm -f "${STALE_CONARY_TEST_CONTAINERS[@]}"
else
  echo "[forge-container-cleanup] no stale conary-test containers found"
fi

echo "[forge-container-cleanup] removing stale Podman build-storage containers"
mapfile -t STALE_STORAGE_CONTAINERS < <(
  DOCKER_HOST="unix://${PODMAN_SOCKET}" podman ps -a --external --format '{{.ID}}' 2>/dev/null || true
)
if (( ${#STALE_STORAGE_CONTAINERS[@]} > 0 )); then
  DOCKER_HOST="unix://${PODMAN_SOCKET}" podman rm -f "${STALE_STORAGE_CONTAINERS[@]}"
else
  echo "[forge-container-cleanup] no stale build-storage containers found"
fi

echo "[forge-container-cleanup] pruning inactive images, containers, and volumes"
if ! DOCKER_HOST="unix://${PODMAN_SOCKET}" podman system prune -af --volumes; then
  echo "[forge-container-cleanup] warning: podman system prune was incomplete" >&2
fi

echo "[forge-container-cleanup] podman usage after cleanup"
DOCKER_HOST="unix://${PODMAN_SOCKET}" podman system df || true

echo "[forge-container-cleanup] disk usage after cleanup"
df -h "${HOME}" || true
