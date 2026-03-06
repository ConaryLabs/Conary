#!/usr/bin/env bash
# deploy/setup-forge.sh -- Install Forgejo + Runner on Fedora 43
#
# Usage:
#   ssh peter@forge.conarylabs.com
#   sudo ./deploy/setup-forge.sh
#
# Prerequisites:
#   - Fedora 43 with Podman installed
#   - Rust 1.93+ installed
#   - Git installed
#
# What this installs:
#   - Forgejo (native binary) on port 3000
#   - Forgejo Runner (native binary) registered with the instance
#   - systemd services for both
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[-]${NC} $1"; exit 1; }

[[ $EUID -ne 0 ]] && error "This script must be run as root"

FORGEJO_VERSION="10.0.1"
RUNNER_VERSION="6.3.1"
FORGEJO_USER="forgejo"
FORGEJO_HOME="/var/lib/forgejo"
FORGEJO_BIN="/usr/local/bin/forgejo"
RUNNER_BIN="/usr/local/bin/forgejo-runner"

# ── Create forgejo user ──────────────────────────────────────────────────
if ! id "$FORGEJO_USER" &>/dev/null; then
    log "Creating forgejo user..."
    useradd --system --shell /bin/bash --home-dir "$FORGEJO_HOME" --create-home "$FORGEJO_USER"
fi

# ── Install Forgejo binary ───────────────────────────────────────────────
log "Downloading Forgejo ${FORGEJO_VERSION}..."
curl -fSL "https://codeberg.org/forgejo/forgejo/releases/download/v${FORGEJO_VERSION}/forgejo-${FORGEJO_VERSION}-linux-amd64" \
    -o "$FORGEJO_BIN"
chmod +x "$FORGEJO_BIN"

# ── Install Forgejo Runner binary ────────────────────────────────────────
log "Downloading Forgejo Runner ${RUNNER_VERSION}..."
curl -fSL "https://codeberg.org/forgejo/runner/releases/download/v${RUNNER_VERSION}/forgejo-runner-${RUNNER_VERSION}-linux-amd64" \
    -o "$RUNNER_BIN"
chmod +x "$RUNNER_BIN"

# ── Create directories ──────────────────────────────────────────────────
mkdir -p "$FORGEJO_HOME"/{custom,data,log}
mkdir -p /etc/forgejo
chown -R "$FORGEJO_USER":"$FORGEJO_USER" "$FORGEJO_HOME"
chown root:"$FORGEJO_USER" /etc/forgejo
chmod 770 /etc/forgejo

# ── Forgejo systemd service ─────────────────────────────────────────────
log "Creating Forgejo systemd service..."
cat > /etc/systemd/system/forgejo.service << 'SVCEOF'
[Unit]
Description=Forgejo
After=network.target

[Service]
Type=simple
User=forgejo
Group=forgejo
WorkingDirectory=/var/lib/forgejo
ExecStart=/usr/local/bin/forgejo web --config /etc/forgejo/app.ini
Restart=always
RestartSec=3
Environment=USER=forgejo HOME=/var/lib/forgejo FORGEJO_WORK_DIR=/var/lib/forgejo

[Install]
WantedBy=multi-user.target
SVCEOF

# ── Runner systemd service ──────────────────────────────────────────────
log "Creating Runner systemd service..."
cat > /etc/systemd/system/forgejo-runner.service << 'SVCEOF'
[Unit]
Description=Forgejo Runner
After=forgejo.service
Wants=forgejo.service

[Service]
Type=simple
User=peter
WorkingDirectory=/home/peter
ExecStart=/usr/local/bin/forgejo-runner daemon
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
SVCEOF

# ── Enable and start Forgejo ────────────────────────────────────────────
systemctl daemon-reload
systemctl enable forgejo
systemctl start forgejo

log "Forgejo installed and running on port 3000"
log ""
log "Next steps:"
log "  1. Visit http://forge.conarylabs.com:3000 to complete web setup"
log "  2. Create an admin account"
log "  3. Mirror the GitHub repo:"
log "     - New Migration > GitHub > https://github.com/ConaryLabs/Conary"
log "     - Enable 'Mirror' option"
log "  4. Generate a runner registration token:"
log "     - Site Administration > Actions > Runners > Create new Runner"
log "  5. Register the runner:"
log "     forgejo-runner register --instance http://localhost:3000 --token <TOKEN> --labels linux-native --name forge-runner"
log "  6. Start the runner:"
log "     systemctl enable --now forgejo-runner"
log ""
warn "DNS: Point forge.conarylabs.com to this server's IP"
warn "TLS: Set up Caddy/nginx reverse proxy with Let's Encrypt for HTTPS"
