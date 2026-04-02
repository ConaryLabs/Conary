#!/usr/bin/env bash
# deploy/setup-forge.sh -- Configure Forge as a trusted GitHub Actions runner host
#
# Usage:
#   ssh peter@forge.conarylabs.com
#   gh auth login --hostname github.com
#   sudo bash /home/peter/Conary/deploy/setup-forge.sh
#
# What this configures:
#   - Podman and basic build dependencies
#   - Rust toolchain for the runner user when missing
#   - GitHub Actions runner binaries under /home/peter/actions-runner
#   - Repository- or org-level runner registration via GitHub API
#   - A checked-in systemd service definition for the runner
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[-]${NC} $1"; exit 1; }

[[ $EUID -ne 0 ]] && error "This script must be run as root"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SERVICE_TEMPLATE="${SCRIPT_DIR}/systemd/github-actions-runner.service"

RUNNER_USER="${FORGE_RUNNER_USER:-peter}"
RUNNER_HOME="${FORGE_RUNNER_HOME:-/home/${RUNNER_USER}/actions-runner}"
RUNNER_WORKDIR="${FORGE_RUNNER_WORKDIR:-${RUNNER_HOME}/_work}"
RUNNER_VERSION="${GITHUB_RUNNER_VERSION:-2.333.1}"
RUNNER_NAME="${GITHUB_RUNNER_NAME:-forge-trusted-1}"
RUNNER_LABELS="${GITHUB_RUNNER_LABELS:-forge-trusted}"
RUNNER_ARCH="${GITHUB_RUNNER_ARCH:-x64}"
RUNNER_SCOPE="${GITHUB_RUNNER_SCOPE:-repo}"
GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-ConaryLabs/Conary}"
GITHUB_OWNER="${GITHUB_OWNER:-${GITHUB_REPOSITORY%%/*}}"
GITHUB_RUNNER_GROUP="${GITHUB_RUNNER_GROUP:-}"
REGISTRATION_TOKEN="${GITHUB_RUNNER_REGISTRATION_TOKEN:-}"
RUNNER_URL_BASE="https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}"
RUNNER_TARBALL="actions-runner-linux-${RUNNER_ARCH}-${RUNNER_VERSION}.tar.gz"
RUNNER_DOWNLOAD_URL="${RUNNER_URL_BASE}/${RUNNER_TARBALL}"

require_cmd() {
    local cmd="$1"
    command -v "$cmd" >/dev/null 2>&1 || error "Missing required command: ${cmd}"
}

runner_shell() {
    sudo -H -u "$RUNNER_USER" bash -lc "$1"
}

registration_token() {
    if [[ -n "$REGISTRATION_TOKEN" ]]; then
        printf '%s\n' "$REGISTRATION_TOKEN"
        return
    fi

    if [[ "$RUNNER_SCOPE" == "org" ]]; then
        runner_shell "gh api -X POST orgs/${GITHUB_OWNER}/actions/runners/registration-token --jq .token"
    else
        runner_shell "gh api -X POST repos/${GITHUB_REPOSITORY}/actions/runners/registration-token --jq .token"
    fi
}

runner_url() {
    if [[ "$RUNNER_SCOPE" == "org" ]]; then
        echo "https://github.com/${GITHUB_OWNER}"
    else
        echo "https://github.com/${GITHUB_REPOSITORY}"
    fi
}

install_packages() {
    log "Installing host dependencies..."
    dnf install -y podman git curl tar jq gh ca-certificates
}

ensure_runner_user() {
    getent passwd "$RUNNER_USER" >/dev/null 2>&1 || error "Runner user ${RUNNER_USER} does not exist"
}

ensure_rust() {
    if runner_shell "command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1"; then
        log "Rust toolchain already present for ${RUNNER_USER}"
        return
    fi

    log "Installing Rust toolchain for ${RUNNER_USER}..."
    runner_shell "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
}

install_runner_files() {
    log "Installing GitHub Actions runner ${RUNNER_VERSION}..."
    install -d -m 0755 -o "$RUNNER_USER" -g "$RUNNER_USER" "$RUNNER_HOME"
    install -d -m 0755 -o "$RUNNER_USER" -g "$RUNNER_USER" "$RUNNER_WORKDIR"

    local tarball="/tmp/${RUNNER_TARBALL}"
    curl -fSL "$RUNNER_DOWNLOAD_URL" -o "$tarball"
    runner_shell "cd '${RUNNER_HOME}' && tar xzf '${tarball}'"
    rm -f "$tarball"
}

configure_runner() {
    local token url config_args

    require_cmd systemctl
    if [[ -z "$REGISTRATION_TOKEN" ]]; then
        require_cmd gh
    fi

    if systemctl is-active --quiet github-actions-runner; then
        log "Stopping existing runner service before reconfiguration..."
        systemctl stop github-actions-runner
    fi

    token="$(registration_token)"
    [[ -n "$token" ]] || error "Failed to obtain a runner registration token from GitHub"

    url="$(runner_url)"
    config_args=(
        "./config.sh"
        "--url" "$url"
        "--token" "$token"
        "--name" "$RUNNER_NAME"
        "--labels" "$RUNNER_LABELS"
        "--work" "$RUNNER_WORKDIR"
        "--unattended"
        "--replace"
    )

    if [[ -n "$GITHUB_RUNNER_GROUP" && "$RUNNER_SCOPE" == "org" ]]; then
        config_args+=("--runnergroup" "$GITHUB_RUNNER_GROUP")
    fi

    log "Registering runner ${RUNNER_NAME} against ${url}..."
    runner_shell "cd '${RUNNER_HOME}' && $(printf "%q " "${config_args[@]}")"
}

install_service() {
    [[ -f "$SERVICE_TEMPLATE" ]] || error "Missing service template: ${SERVICE_TEMPLATE}"

    log "Installing checked-in systemd service..."
    sed \
        -e "s|__RUNNER_USER__|${RUNNER_USER}|g" \
        -e "s|__RUNNER_HOME__|${RUNNER_HOME}|g" \
        "$SERVICE_TEMPLATE" > /etc/systemd/system/github-actions-runner.service

    systemctl daemon-reload
    systemctl enable --now github-actions-runner
}

verify_setup() {
    log "Verifying runner host setup..."
    runner_shell "command -v cargo >/dev/null 2>&1"
    runner_shell "command -v podman >/dev/null 2>&1"
    if [[ -z "$REGISTRATION_TOKEN" ]]; then
        runner_shell "gh auth status >/dev/null 2>&1"
    else
        warn "Skipped persistent gh auth verification because a one-time registration token was supplied."
    fi
    systemctl is-active --quiet github-actions-runner || error "github-actions-runner service is not active"

    log ""
    log "Setup complete!"
    log ""
    log "  Runner host: ${RUNNER_NAME}"
    log "  Scope:       ${RUNNER_SCOPE}"
    log "  Labels:      ${RUNNER_LABELS}"
    log "  Work dir:    ${RUNNER_WORKDIR}"
    log "  Service:     github-actions-runner"
    log ""
    warn "Keep this runner reserved for trusted lanes such as merge-validation and scheduled-ops."
    warn "Use GitHub-hosted runners for untrusted pull request workflows."
}

install_packages
ensure_runner_user
ensure_rust
install_runner_files
configure_runner
install_service
verify_setup
