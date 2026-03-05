#!/usr/bin/env bash
# deploy/deploy-sites.sh
#
# Deploy the two SvelteKit frontends to the Remi server.
#
#   site/  -> conary.io       -> /conary/site on remi
#   web/   -> packages.conary.io -> /conary/web  on remi
#
# Usage:
#   ./deploy/deploy-sites.sh          # Deploy both
#   ./deploy/deploy-sites.sh site     # Deploy conary.io only
#   ./deploy/deploy-sites.sh packages # Deploy packages.conary.io only

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REMI_HOST="remi"

deploy_site() {
    echo "[site] Building conary.io from site/..."
    (cd "$REPO_ROOT/site" && npm run build)
    echo "[site] Deploying to $REMI_HOST:/conary/site/"
    rsync -avz --delete "$REPO_ROOT/site/build/" "$REMI_HOST:/conary/site/"
    echo "[site] conary.io deployed."
}

deploy_packages() {
    echo "[packages] Building packages.conary.io from web/..."
    (cd "$REPO_ROOT/web" && npm run build)
    echo "[packages] Deploying to $REMI_HOST:/conary/web/"
    rsync -avz --delete "$REPO_ROOT/web/build/" "$REMI_HOST:/conary/web/"
    echo "[packages] packages.conary.io deployed."
}

case "${1:-both}" in
    site)      deploy_site ;;
    packages)  deploy_packages ;;
    both)      deploy_site; deploy_packages ;;
    *)         echo "Usage: $0 [site|packages|both]"; exit 1 ;;
esac
