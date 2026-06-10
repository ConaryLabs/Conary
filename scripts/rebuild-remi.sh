#!/usr/bin/env bash
# scripts/rebuild-remi.sh -- Retired Remi rebuild helper.
set -euo pipefail

cat >&2 <<'EOF'
scripts/rebuild-remi.sh is retired for production Remi deploys.

Use the GitHub release/deploy workflow for normal releases. The privileged host
entry point is the root-owned helper installed as:

  /usr/local/sbin/conary-remi-deploy

To verify helper access:

  ssh peter@ssh.conary.io 'sudo -n /usr/local/sbin/conary-remi-deploy verify-access'

To bootstrap or repair helper access from a privileged shell:

  sudo scripts/install-remi-deploy-access.sh
EOF
exit 1
