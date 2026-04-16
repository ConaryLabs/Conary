#!/usr/bin/env bash
# scripts/install-conaryd-on-forge.sh -- Install or roll back the Forge-local conaryd service.
set -euo pipefail

STAGING_DIR=""
VERSION=""
EXPECTED_SHA256=""
BUNDLE_PATH=""
UNIT_PATH=""
VERIFIER_PATH=""
PREVIOUS_VERSION=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --staging-dir)
            STAGING_DIR="${2:-}"
            shift 2
            ;;
        --bundle)
            BUNDLE_PATH="${2:-}"
            shift 2
            ;;
        --expected-version)
            VERSION="${2:-}"
            shift 2
            ;;
        --expected-sha256)
            EXPECTED_SHA256="${2:-}"
            shift 2
            ;;
        --unit-file)
            UNIT_PATH="${2:-}"
            shift 2
            ;;
        --verifier)
            VERIFIER_PATH="${2:-}"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

[[ -n "$STAGING_DIR" && -n "$BUNDLE_PATH" && -n "$VERSION" && -n "$EXPECTED_SHA256" && -n "$UNIT_PATH" && -n "$VERIFIER_PATH" ]] || {
    echo "missing required arguments" >&2
    exit 1
}

sudo -n true
test -d "$STAGING_DIR" || { echo "missing staging dir: $STAGING_DIR" >&2; exit 1; }
test -f "$BUNDLE_PATH" || { echo "missing bundle: $BUNDLE_PATH" >&2; exit 1; }
test -f "$UNIT_PATH" || { echo "missing unit file: $UNIT_PATH" >&2; exit 1; }
test -f "$VERIFIER_PATH" || { echo "missing verifier: $VERIFIER_PATH" >&2; exit 1; }
test -f /var/lib/conary/conary.db || { echo "missing /var/lib/conary/conary.db" >&2; exit 1; }

actual_sha="$(sha256sum "$BUNDLE_PATH" | awk '{print $1}')"
[[ "$actual_sha" == "$EXPECTED_SHA256" ]] || {
    echo "bundle hash mismatch: expected $EXPECTED_SHA256 got $actual_sha" >&2
    exit 1
}

tmpdir="$(mktemp -d "${STAGING_DIR}/install.XXXXXX")"
backup_bin="${tmpdir}/conaryd.previous"
backup_unit="${tmpdir}/conaryd.service.previous"
had_previous_bin=false
had_previous_unit=false

cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

if [[ -f /usr/local/bin/conaryd ]]; then
    cp /usr/local/bin/conaryd "$backup_bin"
    PREVIOUS_VERSION="$("$backup_bin" --version 2>/dev/null | awk '{print $2}' || true)"
    had_previous_bin=true
fi

if [[ -f /etc/systemd/system/conaryd.service ]]; then
    cp /etc/systemd/system/conaryd.service "$backup_unit"
    had_previous_unit=true
fi

tar xzf "$BUNDLE_PATH" -C "$tmpdir"
test -f "${tmpdir}/conaryd-${VERSION}-linux-x64" || {
    echo "bundle did not extract conaryd-${VERSION}-linux-x64" >&2
    exit 1
}

sudo -n install -m 0755 "${tmpdir}/conaryd-${VERSION}-linux-x64" /usr/local/bin/conaryd
sudo -n install -m 0644 "$UNIT_PATH" /etc/systemd/system/conaryd.service
sudo -n systemctl daemon-reload
if ! sudo -n systemctl restart conaryd; then
    systemctl status --no-pager conaryd || true
    exit 1
fi

if ! bash "$VERIFIER_PATH" --expected-version "$VERSION"; then
    if [[ "$had_previous_bin" == true ]]; then
        sudo -n install -m 0755 "$backup_bin" /usr/local/bin/conaryd
    else
        sudo -n rm -f /usr/local/bin/conaryd
    fi

    if [[ "$had_previous_unit" == true ]]; then
        sudo -n install -m 0644 "$backup_unit" /etc/systemd/system/conaryd.service
    else
        sudo -n rm -f /etc/systemd/system/conaryd.service
    fi

    sudo -n systemctl daemon-reload

    if [[ "$had_previous_bin" == true ]]; then
        sudo -n systemctl restart conaryd || true
        if [[ -n "$PREVIOUS_VERSION" ]]; then
            bash "$VERIFIER_PATH" --expected-version "$PREVIOUS_VERSION" || true
        fi
    else
        echo "no rollback target existed" >&2
    fi

    systemctl status --no-pager conaryd || true
    exit 1
fi
