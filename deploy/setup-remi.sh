#!/bin/bash
# setup-remi.sh - Deploy Remi server on Ubuntu 24.04 (Hetzner)
#
# Usage:
#   sudo ./setup-remi.sh [--zfs]
#
# Options:
#   --zfs    Set up ZFS storage (assumes mdadm RAID 0 already created)

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log() { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
error() { echo -e "${RED}[-]${NC} $1"; exit 1; }

# Check root
[[ $EUID -ne 0 ]] && error "This script must be run as root"

USE_ZFS=false
[[ "${1:-}" == "--zfs" ]] && USE_ZFS=true

# =============================================================================
# System Setup
# =============================================================================

log "Installing dependencies..."
apt-get update
apt-get install -y curl build-essential pkg-config libssl-dev

# Install ZFS if requested
if $USE_ZFS; then
    log "Installing ZFS..."
    apt-get install -y zfsutils-linux
fi

# =============================================================================
# Create Service User
# =============================================================================

log "Creating conary service user..."
if ! id -u conary &>/dev/null; then
    useradd -r -s /sbin/nologin -d /conary -c "Conary Package Manager" conary
fi

# =============================================================================
# Storage Setup
# =============================================================================

if $USE_ZFS; then
    log "Setting up ZFS storage..."

    # Check for existing pool
    if ! zpool list conary &>/dev/null; then
        # Find the RAID device
        if [[ -e /dev/md0 ]]; then
            RAID_DEV=/dev/md0
        else
            warn "No /dev/md0 found. Looking for available devices..."
            lsblk
            read -p "Enter device path for ZFS pool: " RAID_DEV
        fi

        # Create ZFS pool
        log "Creating ZFS pool on $RAID_DEV..."
        zpool create -o ashift=12 conary "$RAID_DEV"
    else
        log "ZFS pool 'conary' already exists"
    fi

    # Create datasets with optimized settings
    log "Creating ZFS datasets..."

    # Bulk storage - large records, compression
    zfs create -o compression=lz4 -o atime=off -o recordsize=128K conary/chunks 2>/dev/null || true
    zfs create -o compression=lz4 -o atime=off -o recordsize=128K conary/converted 2>/dev/null || true
    zfs create -o compression=lz4 -o atime=off -o recordsize=128K conary/built 2>/dev/null || true
    zfs create -o compression=lz4 -o atime=off -o recordsize=128K conary/bootstrap 2>/dev/null || true

    # Work directory - ephemeral
    zfs create conary/build 2>/dev/null || true

    # Metadata - optimized for SQLite
    zfs create -o recordsize=16K -o logbias=latency conary/metadata 2>/dev/null || true

    # Other directories
    zfs create conary/manifests 2>/dev/null || true
    zfs create conary/keys 2>/dev/null || true
    zfs create conary/cache 2>/dev/null || true

    # Set quotas
    log "Setting ZFS quotas..."
    zfs set quota=1500G conary/chunks
    zfs set quota=200G conary/converted
    zfs set quota=100G conary/built
    zfs set quota=50G conary/bootstrap
    zfs set quota=50G conary/build

    # Ensure mountpoint
    zfs set mountpoint=/conary conary
else
    # Traditional directory structure
    log "Creating directory structure..."
    mkdir -p /conary/{chunks,converted,built,bootstrap,build,metadata,manifests,keys,cache}
fi

# Set ownership
log "Setting permissions..."
chown -R conary:conary /conary
chmod 750 /conary
chmod 700 /conary/keys

# =============================================================================
# Configuration
# =============================================================================

log "Installing configuration..."
mkdir -p /etc/conary
if [[ ! -f /etc/conary/remi.toml ]]; then
    cp remi.toml /etc/conary/remi.toml
    chmod 644 /etc/conary/remi.toml
else
    warn "/etc/conary/remi.toml already exists, not overwriting"
fi

# =============================================================================
# Binary Installation
# =============================================================================

log "Installing Conary binary..."
if [[ -f target/release/conary ]]; then
    cp target/release/conary /usr/local/bin/
    chmod 755 /usr/local/bin/conary
elif [[ -f /usr/local/bin/conary ]]; then
    warn "Using existing /usr/local/bin/conary"
else
    warn "No binary found. Build with: cargo build --release --features server"
fi

# =============================================================================
# Systemd Services
# =============================================================================

log "Installing systemd services..."
cp systemd/remi.service /etc/systemd/system/
cp systemd/remi-builder.service /etc/systemd/system/
systemctl daemon-reload

# =============================================================================
# Initialize Database
# =============================================================================

log "Initializing Conary database..."
if [[ ! -f /conary/metadata/conary.db ]]; then
    sudo -u conary /usr/local/bin/conary system init -d /conary/metadata/conary.db || warn "Database init failed"
fi

# =============================================================================
# Enable Services
# =============================================================================

log "Enabling Remi service..."
systemctl enable remi

log ""
log "====================================="
log "Remi Setup Complete!"
log "====================================="
log ""
log "Start the server:"
log "  systemctl start remi"
log ""
log "View logs:"
log "  journalctl -u remi -f"
log ""
log "Configuration:"
log "  /etc/conary/remi.toml"
log ""
log "Storage:"
log "  /conary/"
if $USE_ZFS; then
    log ""
    log "ZFS Status:"
    zpool status conary
    log ""
    log "ZFS Datasets:"
    zfs list -r conary
fi
log ""
log "To enable the builder service:"
log "  Edit /etc/conary/remi.toml and set [builder] enabled = true"
log "  systemctl enable --now remi-builder"
