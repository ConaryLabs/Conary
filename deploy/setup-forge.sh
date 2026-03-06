#!/usr/bin/env bash
# deploy/setup-forge.sh -- Install Forgejo + Runner on Fedora 43
#
# Usage:
#   ssh peter@forge.conarylabs.com
#   sudo bash /home/peter/Conary/deploy/setup-forge.sh
#
# Prerequisites:
#   - Fedora 43 with Podman installed
#   - Rust 1.93+ installed (in /home/peter/.cargo/bin/)
#   - Git installed
#
# What this installs:
#   - Forgejo (native binary) on port 3000
#   - Forgejo Runner (native binary, host executor) registered with the instance
#   - systemd services for both
#   - app.ini with SQLite, Actions enabled
#   - Admin user, DB migration, runner registration
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[-]${NC} $1"; exit 1; }

[[ $EUID -ne 0 ]] && error "This script must be run as root"

FORGEJO_VERSION="14.0.2"
RUNNER_VERSION="12.7.1"
FORGEJO_USER="forgejo"
FORGEJO_HOME="/var/lib/forgejo"
FORGEJO_BIN="/usr/local/bin/forgejo"
RUNNER_BIN="/usr/local/bin/forgejo-runner"
RUNNER_HOME="/var/lib/forgejo-runner"
ADMIN_USER="${FORGE_ADMIN_USER:-peter}"
ADMIN_EMAIL="${FORGE_ADMIN_EMAIL:-peter@conary.io}"
ADMIN_PASS="${FORGE_ADMIN_PASS:-}"
DOMAIN="${FORGE_DOMAIN:-forge.conarylabs.com}"

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
curl -fSL "https://code.forgejo.org/forgejo/runner/releases/download/v${RUNNER_VERSION}/forgejo-runner-${RUNNER_VERSION}-linux-amd64" \
    -o "$RUNNER_BIN"
chmod +x "$RUNNER_BIN"

# ── Create directories ──────────────────────────────────────────────────
mkdir -p "$FORGEJO_HOME"/{custom,data,log}
mkdir -p /etc/forgejo
mkdir -p "$RUNNER_HOME"
chown -R "$FORGEJO_USER":"$FORGEJO_USER" "$FORGEJO_HOME"
chown -R "$FORGEJO_USER":"$FORGEJO_USER" "$RUNNER_HOME"
chown "$FORGEJO_USER":"$FORGEJO_USER" /etc/forgejo
chmod 770 /etc/forgejo

# ── Generate app.ini ─────────────────────────────────────────────────────
log "Writing app.ini..."
SECRET_KEY=$(openssl rand -hex 32)
INTERNAL_TOKEN=$(openssl rand -hex 64)

cat > /etc/forgejo/app.ini << APPEOF
APP_NAME = Conary Forge
RUN_USER = ${FORGEJO_USER}
WORK_PATH = ${FORGEJO_HOME}

[server]
HTTP_PORT = 3000
ROOT_URL = http://${DOMAIN}:3000/
DOMAIN = ${DOMAIN}
SSH_PORT = 22
SSH_DOMAIN = ${DOMAIN}

[database]
DB_TYPE = sqlite3
PATH = ${FORGEJO_HOME}/data/forgejo.db

[repository]
ROOT = ${FORGEJO_HOME}/repos

[security]
INSTALL_LOCK = true
SECRET_KEY = ${SECRET_KEY}
INTERNAL_TOKEN = ${INTERNAL_TOKEN}

[actions]
ENABLED = true

[log]
ROOT_PATH = ${FORGEJO_HOME}/data/log
MODE = console
LEVEL = Info
APPEOF

chown "$FORGEJO_USER":"$FORGEJO_USER" /etc/forgejo/app.ini
chmod 660 /etc/forgejo/app.ini

# ── Run database migration ───────────────────────────────────────────────
log "Migrating database..."
sudo -u "$FORGEJO_USER" "$FORGEJO_BIN" migrate --config /etc/forgejo/app.ini

# ── Create admin user ────────────────────────────────────────────────────
if [[ -n "$ADMIN_PASS" ]]; then
    log "Creating admin user: ${ADMIN_USER}..."
    sudo -u "$FORGEJO_USER" "$FORGEJO_BIN" admin user create \
        --config /etc/forgejo/app.ini \
        --username "$ADMIN_USER" \
        --password "$ADMIN_PASS" \
        --email "$ADMIN_EMAIL" \
        --admin
else
    warn "No FORGE_ADMIN_PASS set -- skipping admin user creation"
    warn "Create one manually: forgejo admin user create --config /etc/forgejo/app.ini --username peter --password <pass> --email peter@conary.io --admin"
fi

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

# ── Start Forgejo ────────────────────────────────────────────────────────
systemctl daemon-reload
systemctl enable --now forgejo
sleep 2

# Verify Forgejo is responding
if curl -sf http://localhost:3000/api/v1/version > /dev/null 2>&1; then
    log "Forgejo is running"
else
    error "Forgejo failed to start -- check: journalctl -u forgejo"
fi

# ── Register runner ──────────────────────────────────────────────────────
log "Generating runner registration token..."
RUNNER_TOKEN=$(sudo -u "$FORGEJO_USER" "$FORGEJO_BIN" actions generate-runner-token --config /etc/forgejo/app.ini 2>/dev/null)

log "Registering runner..."
cd "$RUNNER_HOME"
sudo -u "$FORGEJO_USER" "$RUNNER_BIN" register \
    --instance http://localhost:3000 \
    --token "$RUNNER_TOKEN" \
    --labels linux-native \
    --name forge-runner \
    --no-interactive

# ── Configure runner for host executor ───────────────────────────────────
log "Generating runner config (host executor)..."
sudo -u "$FORGEJO_USER" "$RUNNER_BIN" generate-config > "$RUNNER_HOME/config.yaml"
sed -i 's/^  labels: \[\]/  labels: ["linux-native:host"]/' "$RUNNER_HOME/config.yaml"

# Set env vars so runner jobs can find Rust toolchain
# Use forgejo's own CARGO_HOME (writable) but peter's RUSTUP_HOME (read-only toolchain)
sed -i '/A_TEST_ENV_NAME_1/,/A_TEST_ENV_NAME_2/c\    RUSTUP_HOME: /home/peter/.rustup\n    CARGO_HOME: /var/lib/forgejo/.cargo\n    PATH: /home/peter/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin' "$RUNNER_HOME/config.yaml"

# Create writable cargo home for forgejo user
mkdir -p /var/lib/forgejo/.cargo/registry
chown -R "$FORGEJO_USER":"$FORGEJO_USER" /var/lib/forgejo/.cargo

# Ensure forgejo can read peter's rustup toolchains
chmod o+rx /home/peter/.rustup /home/peter/.cargo
chmod -R o+rX /home/peter/.rustup/toolchains 2>/dev/null || true
chmod o+r /home/peter/.rustup/settings.toml 2>/dev/null || true

# ── Runner systemd service ──────────────────────────────────────────────
log "Creating Runner systemd service..."
cat > /etc/systemd/system/forgejo-runner.service << 'SVCEOF'
[Unit]
Description=Forgejo Runner
After=forgejo.service
Wants=forgejo.service

[Service]
Type=simple
User=forgejo
WorkingDirectory=/var/lib/forgejo-runner
ExecStart=/usr/local/bin/forgejo-runner daemon --config /var/lib/forgejo-runner/config.yaml
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
SVCEOF

systemctl daemon-reload
systemctl enable --now forgejo-runner
sleep 2

# ── Make Rust toolchain accessible to forgejo user ───────────────────────
log "Symlinking Rust toolchain..."
for bin in cargo rustc clippy-driver cargo-clippy; do
    if [[ -f "/home/peter/.cargo/bin/$bin" ]]; then
        ln -sf "/home/peter/.cargo/bin/$bin" "/usr/local/bin/$bin"
    fi
done

# ── Verify ───────────────────────────────────────────────────────────────
log ""
log "Setup complete!"
log ""
log "  Forgejo:  http://${DOMAIN}:3000  (v${FORGEJO_VERSION})"
log "  Runner:   forge-runner (v${RUNNER_VERSION}, host executor)"
log "  Admin:    ${ADMIN_USER}"
log ""
log "Next steps:"
log "  1. Mirror the GitHub repo via API or web UI:"
log "     curl -X POST http://localhost:3000/api/v1/repos/migrate \\"
log "       -H 'Authorization: token <TOKEN>' \\"
log "       -H 'Content-Type: application/json' \\"
log "       -d '{\"clone_addr\":\"https://github.com/ConaryLabs/Conary.git\",\"repo_name\":\"Conary\",\"repo_owner\":\"peter\",\"service\":\"git\",\"mirror\":true,\"mirror_interval\":\"10m\"}'"
log ""
warn "DNS: Point ${DOMAIN} to this server's IP"
warn "TLS: Set up Caddy/nginx reverse proxy with Let's Encrypt for HTTPS"
