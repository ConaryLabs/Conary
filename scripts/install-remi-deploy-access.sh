#!/usr/bin/env bash
# scripts/install-remi-deploy-access.sh -- Install Remi deploy helper and sudo policy.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${EUID}" -ne 0 ]]; then
    echo "must run as root" >&2
    exit 1
fi

install -m 755 -o root -g root \
    "${repo_root}/deploy/remi-deploy-helper.sh" \
    /usr/local/sbin/conary-remi-deploy

install -m 440 -o root -g root \
    "${repo_root}/deploy/sudoers/remi" \
    /etc/sudoers.d/remi

visudo -cf /etc/sudoers.d/remi >/dev/null

echo "installed /usr/local/sbin/conary-remi-deploy and /etc/sudoers.d/remi"
