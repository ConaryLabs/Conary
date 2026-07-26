#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: guest-validate.sh [OPTIONS]

Run the checked-in self-host validation flow inside the guest.

Options:
  --repo-name NAME       Repository name to configure for validation
  --repo-url URL         Repository metadata URL
  --remi-endpoint URL    Remi conversion endpoint URL
  --source-profile ID    Exact public source profile ID
  --help                 Show this help text
EOF
}

REPO_NAME=""
REPO_URL=""
REMI_ENDPOINT=""
REMI_DISTRO=""
PUBLIC_REMI_ENDPOINT="https://remi.conary.io"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo-name)
            REPO_NAME="$2"
            shift 2
            ;;
        --repo-url)
            REPO_URL="$2"
            shift 2
            ;;
        --remi-endpoint)
            REMI_ENDPOINT="$2"
            shift 2
            ;;
        --source-profile)
            REMI_DISTRO="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ -z "$REPO_NAME" || -z "$REPO_URL" || -z "$REMI_ENDPOINT" || -z "$REMI_DISTRO" ]]; then
    echo "--repo-name, --repo-url, --remi-endpoint, and --source-profile are required." >&2
    usage >&2
    exit 1
fi
if [[ "$REPO_NAME" != "remi-$REMI_DISTRO" ]]; then
    echo "--repo-name must be 'remi-$REMI_DISTRO' so clean-host validation exercises the packaged source feed." >&2
    exit 1
fi

INPUTS_DIR="/var/lib/conary/bootstrap-inputs"
WORKSPACE_TARBALL="$INPUTS_DIR/conary-workspace.tar.gz"
WORKSPACE_SHA256="$INPUTS_DIR/conary-workspace.tar.gz.sha256"
WORKSPACE_DIR="$INPUTS_DIR/conary-workspace"
ROOT_JSON="$INPUTS_DIR/root.json"
SMOKE_RECIPE="$WORKSPACE_DIR/recipes/bootstrap-smoke/simple-hello.toml"
SMOKE_OUTPUT="/var/tmp/conary-smoke-output"
SMOKE_CACHE="/var/tmp/conary-smoke-cache"

log() {
    printf '[guest-validate] %s\n' "$*"
}

require_cmd() {
    local cmd
    for cmd in "$@"; do
        if ! command -v "$cmd" >/dev/null 2>&1; then
            echo "Missing required command: $cmd" >&2
            exit 1
        fi
    done
}

verify_workspace_inputs() {
    local expected_sha256
    local actual_sha256

    [[ -f "$WORKSPACE_TARBALL" ]] || {
        echo "Missing workspace tarball: $WORKSPACE_TARBALL" >&2
        exit 1
    }
    [[ -f "$WORKSPACE_SHA256" ]] || {
        echo "Missing workspace sha256 sidecar: $WORKSPACE_SHA256" >&2
        exit 1
    }

    expected_sha256="$(tr -d ' \n' < "$WORKSPACE_SHA256")"
    actual_sha256="$(sha256sum "$WORKSPACE_TARBALL" | awk '{print $1}')"

    if [[ "$expected_sha256" != "$actual_sha256" ]]; then
        echo "Workspace tarball checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
        exit 1
    fi
}

unpack_workspace() {
    rm -rf "$WORKSPACE_DIR"
    tar -xzf "$WORKSPACE_TARBALL" -C "$INPUTS_DIR"
    [[ -d "$WORKSPACE_DIR" ]] || {
        echo "Expected unpacked workspace at $WORKSPACE_DIR" >&2
        exit 1
    }
}

check_for_baked_private_key() {
    if [[ -d /root/.ssh ]] && find /root/.ssh -maxdepth 1 -type f -name 'id_*' ! -name '*.pub' | grep -q .; then
        echo "Found reusable operator/test private SSH key under /root/.ssh" >&2
        exit 1
    fi
}

main() {
    require_cmd conary cargo tar sha256sum
    verify_workspace_inputs
    unpack_workspace
    check_for_baked_private_key

    conary system init
    if [[ "$REPO_URL" != "$PUBLIC_REMI_ENDPOINT" || "$REMI_ENDPOINT" != "$PUBLIC_REMI_ENDPOINT" ]]; then
        conary repo remove "$REPO_NAME"
        conary repo add \
            "$REPO_NAME" \
            "$REPO_URL" \
            --package-format json \
            --default-strategy remi \
            --remi-endpoint "$REMI_ENDPOINT" \
            --source-profile "$REMI_DISTRO"
    fi

    repo_output="$(conary repo list --all)"
    remi_count="$(printf '%s\n' "$repo_output" | grep -Ec "^[[:space:]]+\\[[x ]\\][[:space:]]+$REPO_NAME[[:space:]]")"
    if [[ "$remi_count" -ne 1 ]]; then
        echo "Expected packaged source '$REPO_NAME', found $remi_count rows" >&2
        exit 1
    fi

    if [[ -f "$ROOT_JSON" ]]; then
        conary trust init "$REPO_NAME" --root "$ROOT_JSON"
    fi

    conary repo sync "$REPO_NAME" --force
    conary query label list

    conary install tree --repo "$REPO_NAME" --yes --sandbox always
    conary remove tree --sandbox always --yes

    rm -rf "$SMOKE_OUTPUT" "$SMOKE_CACHE"
    conary cook \
        "$SMOKE_RECIPE" \
        --output "$SMOKE_OUTPUT" \
        --source-cache "$SMOKE_CACHE"

    (
        cd "$WORKSPACE_DIR"
        cargo build --locked
        target/debug/conary --version
        target/debug/conary query label list
        target/debug/conary install tree --repo "$REPO_NAME" --yes --sandbox always
        target/debug/conary remove tree --sandbox always --yes
    )

    check_for_baked_private_key
    log "Guest self-host validation completed successfully"
}

main "$@"
