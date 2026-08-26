#!/usr/bin/env bash
# deploy/hetzner/remi-vdo-monitor.sh -- Report authoritative Remi VDO usage.

set -euo pipefail

readonly VDO_POOL="${REMI_VDO_POOL:-vgdata/vdopool}"
readonly WARN_PERCENT="${REMI_VDO_WARN_PERCENT:-75}"
readonly CRITICAL_PERCENT="${REMI_VDO_CRITICAL_PERCENT:-85}"

for value in "$WARN_PERCENT" "$CRITICAL_PERCENT"; do
    if [[ ! "$value" =~ ^[0-9]+$ ]] || ((value < 1 || value > 99)); then
        echo "invalid VDO threshold: $value" >&2
        exit 2
    fi
done

if ((WARN_PERCENT >= CRITICAL_PERCENT)); then
    echo "VDO warning threshold must be lower than critical threshold" >&2
    exit 2
fi

data_percent="$({
    lvs --noheadings --nosuffix -o data_percent "$VDO_POOL" 2>/dev/null || true
} | tr -d '[:space:]')"

if [[ ! "$data_percent" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
    echo "unable to read physical VDO usage for $VDO_POOL" >&2
    exit 2
fi

whole_percent="${data_percent%%.*}"
printf 'VDO pool %s physical usage: %s%%\n' "$VDO_POOL" "$data_percent"

if ((whole_percent >= CRITICAL_PERCENT)); then
    echo "critical: VDO physical usage reached ${CRITICAL_PERCENT}%" >&2
    exit 1
fi

if ((whole_percent >= WARN_PERCENT)); then
    echo "warning: VDO physical usage reached ${WARN_PERCENT}%" >&2
fi
