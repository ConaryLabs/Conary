#!/usr/bin/env bash
set -euo pipefail

[[ $EUID -ne 0 ]] && {
  echo "This script must be run as root" >&2
  exit 1
}

RUNNER_USER="${FORGE_RUNNER_USER:-peter}"

echo "[repair-forge-runtime] installing runtime dependencies"
dnf install -y \
  podman git curl tar jq gh ca-certificates \
  qemu-system-x86 qemu-img openssh-clients edk2-ovmf ripgrep

runner_uid="$(id -u "$RUNNER_USER")"
runtime_dir="/run/user/${runner_uid}"
podman_socket="${runtime_dir}/podman/podman.sock"

echo "[repair-forge-runtime] enabling linger for ${RUNNER_USER}"
loginctl enable-linger "$RUNNER_USER"
systemctl start "user@${runner_uid}.service"

echo "[repair-forge-runtime] enabling rootless Podman socket"
sudo -H -u "$RUNNER_USER" env XDG_RUNTIME_DIR="$runtime_dir" systemctl --user enable --now podman.socket
sudo -H -u "$RUNNER_USER" test -S "$podman_socket"
sudo -H -u "$RUNNER_USER" env DOCKER_HOST="unix://${podman_socket}" podman info >/dev/null
curl --unix-socket "$podman_socket" -fsS http://d/v1.41/_ping >/dev/null \
  || curl --unix-socket "$podman_socket" -fsS http://d/_ping >/dev/null

echo "[repair-forge-runtime] ok"
