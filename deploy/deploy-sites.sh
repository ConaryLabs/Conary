#!/usr/bin/env bash
# deploy/deploy-sites.sh
#
# Deploy the two SvelteKit frontends to the Remi server.
#
# Both frontends currently live on the same Remi host, but they remain split as
# separate build outputs and deploy roots:
#   site/  -> conary.io          -> /conary/site on remi
#   web/   -> remi.conary.io -> /conary/web  on remi
#
# Usage:
#   ./deploy/deploy-sites.sh          # Deploy both
#   ./deploy/deploy-sites.sh site     # Deploy conary.io only
#   ./deploy/deploy-sites.sh packages # Deploy remi.conary.io only
#                                     # (`packages` is a historical subcommand name)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REMI_HOST="${REMI_HOST:-peter@ssh.conary.io}"
REMI_DEPLOY_HELPER="${REMI_DEPLOY_HELPER:-/usr/local/sbin/conary-remi-deploy}"

remote_quote() {
    printf '%q' "$1"
}

cleanup_remote_stage() {
    local remote_stage="$1"
    local quoted_stage
    quoted_stage="$(remote_quote "$remote_stage")"
    ssh "$REMI_HOST" "rm -rf -- $quoted_stage" >/dev/null 2>&1 || true
}

stage_and_publish() {
    local label="$1"
    local build_dir="$2"
    local target="$3"
    local remote_stage quoted_stage quoted_helper

    test -f "${build_dir}/index.html" || {
        echo "[$label] Missing ${build_dir}/index.html after build" >&2
        exit 1
    }

    remote_stage="$(ssh "$REMI_HOST" "mktemp -d /tmp/conary-${target}.deploy.XXXXXX")"
    if ! rsync -avz --delete "${build_dir}/" "$REMI_HOST:${remote_stage}/"; then
        cleanup_remote_stage "$remote_stage"
        exit 1
    fi

    quoted_stage="$(remote_quote "$remote_stage")"
    quoted_helper="$(remote_quote "$REMI_DEPLOY_HELPER")"
    if ! ssh "$REMI_HOST" "sudo -n $quoted_helper deploy-site $target $quoted_stage"; then
        cleanup_remote_stage "$remote_stage"
        exit 1
    fi
}

deploy_site() {
    echo "[site] Building conary.io from site/..."
    (cd "$REPO_ROOT/site" && npm run build)
    echo "[site] Deploying to $REMI_HOST:/conary/site/"
    stage_and_publish "site" "$REPO_ROOT/site/build" "site"
    echo "[site] conary.io deployed."
}

deploy_packages() {
    echo "[packages] Building remi.conary.io from web/..."
    (cd "$REPO_ROOT/web" && npm run build)
    echo "[packages] Deploying to $REMI_HOST:/conary/web/"
    stage_and_publish "packages" "$REPO_ROOT/web/build" "web"
    echo "[packages] remi.conary.io deployed."
}

case "${1:-both}" in
    site)      deploy_site ;;
    packages)  deploy_packages ;;
    both)      deploy_site; deploy_packages ;;
    *)         echo "Usage: $0 [site|packages|both]"; exit 1 ;;
esac
