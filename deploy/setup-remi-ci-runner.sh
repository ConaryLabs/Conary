#!/usr/bin/env bash
# deploy/setup-remi-ci-runner.sh -- Install the trusted Remi CI capacity runner.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: setup-remi-ci-runner.sh --registration-token-stdin
       setup-remi-ci-runner.sh --verify-only

Install or verify the dedicated, repository-scoped GitHub Actions runner on
the Ubuntu Remi host. Obtain a one-time registration token with:

  gh api -X POST repos/ConaryLabs/Conary/actions/runners/registration-token \
    --jq .token | sudo bash deploy/setup-remi-ci-runner.sh \
    --registration-token-stdin

The token is read before package installation, is not exposed in the sudo
command line, and is not written to disk by this script.
EOF
}

MODE=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --registration-token-stdin)
            [[ -z "$MODE" ]] || { usage >&2; exit 1; }
            MODE="install"
            shift
            ;;
        --verify-only)
            [[ -z "$MODE" ]] || { usage >&2; exit 1; }
            MODE="verify"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "[setup-remi-ci-runner] ERROR: unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

fail() {
    echo "[setup-remi-ci-runner] ERROR: $*" >&2
    exit 1
}

log() {
    echo "[setup-remi-ci-runner] $*"
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

[[ -n "$MODE" ]] || { usage >&2; exit 1; }
[[ "$(id -u)" -eq 0 ]] || fail "this installer must run as root"

RUNNER_USER="conary-ci"
RUNNER_ROOT="/data/conary-ci"
RUNNER_HOME="${RUNNER_ROOT}/actions-runner"
RUNNER_WORK="${RUNNER_ROOT}/work"
RUNNER_CACHE="${RUNNER_ROOT}/cache"
RUNNER_NAME="remi-ci-trusted-1"
RUNNER_LABELS="remi-ci-trusted"
RUNNER_REPOSITORY="ConaryLabs/Conary"
RUNNER_VERSION="2.337.0"
RUNNER_SHA256="70920811a4f8ad4328818682bca5c6469c1c942fab52448868071d0063816613"
RUST_VERSION="1.98.0"
SERVICE_NAME="github-actions-remi-ci-runner.service"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SERVICE_TEMPLATE="${SCRIPT_DIR}/systemd/${SERVICE_NAME}"
REGISTRATION_TOKEN=""
TEMP_ROOT=""

cleanup() {
    [[ -z "$TEMP_ROOT" || ! -d "$TEMP_ROOT" ]] || rm -rf -- "$TEMP_ROOT"
    REGISTRATION_TOKEN=""
}
trap cleanup EXIT

if [[ "$MODE" == "install" ]]; then
    IFS= read -r REGISTRATION_TOKEN || fail "registration token was not provided on stdin"
    [[ "$REGISTRATION_TOKEN" =~ ^[A-Za-z0-9_]+$ ]] ||
        fail "registration token has an unexpected format"
fi

os_id="$(awk -F= '$1 == "ID" { gsub(/"/, "", $2); print $2 }' /etc/os-release)"
[[ "$os_id" == "ubuntu" ]] || fail "this installer supports only the Ubuntu Remi host"
[[ -f "$SERVICE_TEMPLATE" ]] || fail "service template is missing: ${SERVICE_TEMPLATE}"

run_as_runner() {
    runuser -u "$RUNNER_USER" -- env \
        HOME="$RUNNER_ROOT" \
        PATH="${RUNNER_ROOT}/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
        RUSTUP_HOME="${RUNNER_ROOT}/.rustup" \
        "$@"
}

install_packages() {
    log "installing Ubuntu build, rootless-container, and KVM dependencies"
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends \
        build-essential ca-certificates clang cmake curl file fuse-overlayfs git \
        gperf jq libapt-pkg-dev liblzma-dev libseccomp-dev libssl-dev \
        linux-libc-dev musl-tools ovmf perl pkg-config podman qemu-system-x86 \
        qemu-utils ripgrep rustup sccache slirp4netns tar uidmap
}

ensure_runner_identity() {
    if getent passwd "$RUNNER_USER" >/dev/null; then
        actual_home="$(getent passwd "$RUNNER_USER" | cut -d: -f6)"
        [[ "$actual_home" == "$RUNNER_ROOT" ]] ||
            fail "existing ${RUNNER_USER} home is ${actual_home}, expected ${RUNNER_ROOT}"
    else
        useradd --create-home --home-dir "$RUNNER_ROOT" --shell /bin/bash "$RUNNER_USER"
    fi

    for forbidden_group in sudo admin docker; do
        if id -nG "$RUNNER_USER" | tr ' ' '\n' | grep -Fxq "$forbidden_group"; then
            fail "${RUNNER_USER} belongs to forbidden group ${forbidden_group}"
        fi
    done
    getent group kvm >/dev/null || fail "kvm group is missing after QEMU installation"
    usermod --append --groups kvm "$RUNNER_USER"
    grep -q "^${RUNNER_USER}:" /etc/subuid || fail "${RUNNER_USER} has no subordinate UID range"
    grep -q "^${RUNNER_USER}:" /etc/subgid || fail "${RUNNER_USER} has no subordinate GID range"

    install -d -m 0750 -o "$RUNNER_USER" -g "$RUNNER_USER" \
        "$RUNNER_HOME" "$RUNNER_WORK" "$RUNNER_CACHE"
}

ensure_rust() {
    if ! run_as_runner rustc --version 2>/dev/null | grep -q "^rustc ${RUST_VERSION} "; then
        run_as_runner rustup toolchain install "$RUST_VERSION" --profile minimal
    fi

    run_as_runner rustup default "$RUST_VERSION"
    run_as_runner rustup component add --toolchain "$RUST_VERSION" clippy rustfmt
    run_as_runner rustup target add --toolchain "$RUST_VERSION" x86_64-unknown-linux-musl
}

ensure_rootless_podman() {
    runner_uid="$(id -u "$RUNNER_USER")"
    runtime_dir="/run/user/${runner_uid}"
    podman_socket="${runtime_dir}/podman/podman.sock"

    loginctl enable-linger "$RUNNER_USER"
    systemctl start "user@${runner_uid}.service"
    run_as_runner env XDG_RUNTIME_DIR="$runtime_dir" \
        systemctl --user enable --now podman.socket
    run_as_runner test -S "$podman_socket"
    run_as_runner env XDG_RUNTIME_DIR="$runtime_dir" \
        DOCKER_HOST="unix://${podman_socket}" podman info >/dev/null
    curl --unix-socket "$podman_socket" -fsS http://d/v1.41/_ping >/dev/null ||
        curl --unix-socket "$podman_socket" -fsS http://d/_ping >/dev/null ||
        fail "rootless Podman API socket did not answer"
}

install_runner() {
    if [[ -f "${RUNNER_HOME}/.runner" ]]; then
        jq -e \
            --arg name "$RUNNER_NAME" \
            --arg url "https://github.com/${RUNNER_REPOSITORY}" \
            '.agentName == $name and .gitHubUrl == $url' \
            "${RUNNER_HOME}/.runner" >/dev/null ||
            fail "existing runner registration does not match ${RUNNER_NAME} and ${RUNNER_REPOSITORY}"
        log "retaining the matching existing runner registration"
        REGISTRATION_TOKEN=""
        return
    fi

    TEMP_ROOT="$(mktemp -d)"
    chmod 0755 "$TEMP_ROOT"
    archive="${TEMP_ROOT}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz"
    url="https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/$(basename "$archive")"
    curl --proto '=https' --tlsv1.2 -fL "$url" -o "$archive"
    printf '%s  %s\n' "$RUNNER_SHA256" "$archive" | sha256sum -c -
    chmod 0644 "$archive"
    run_as_runner tar -xzf "$archive" -C "$RUNNER_HOME"
    "${RUNNER_HOME}/bin/installdependencies.sh"

    run_as_runner "${RUNNER_HOME}/config.sh" \
        --url "https://github.com/${RUNNER_REPOSITORY}" \
        --token "$REGISTRATION_TOKEN" \
        --name "$RUNNER_NAME" \
        --labels "$RUNNER_LABELS" \
        --work "$RUNNER_WORK" \
        --unattended
    REGISTRATION_TOKEN=""
}

install_service() {
    runner_uid="$(id -u "$RUNNER_USER")"
    service_staging="$(mktemp)"
    sed "s/__RUNNER_UID__/${runner_uid}/g" "$SERVICE_TEMPLATE" > "$service_staging"
    install -m 0644 -o root -g root "$service_staging" "/etc/systemd/system/${SERVICE_NAME}"
    rm -f "$service_staging"
    systemctl daemon-reload
    systemctl enable --now "$SERVICE_NAME"
}

verify_setup() {
    require_cmd systemctl
    require_cmd runuser
    getent passwd "$RUNNER_USER" >/dev/null || fail "runner identity is missing"
    runner_uid="$(id -u "$RUNNER_USER")"
    podman_socket="/run/user/${runner_uid}/podman/podman.sock"

    [[ "$(systemctl is-active "$SERVICE_NAME")" == "active" ]] ||
        fail "runner service is not active"
    [[ "$(systemctl show "$SERVICE_NAME" -p User --value)" == "$RUNNER_USER" ]] ||
        fail "runner service identity is not ${RUNNER_USER}"
    [[ "$(systemctl show "$SERVICE_NAME" -p CPUQuotaPerSecUSec --value)" == "infinity" ]] ||
        fail "runner service has an artificial CPU quota"
    [[ "$(systemctl show "$SERVICE_NAME" -p MemoryMax --value)" == "infinity" ]] ||
        fail "runner service has an artificial memory ceiling"
    [[ -z "$(systemctl show "$SERVICE_NAME" -p AllowedCPUs --value)" ]] ||
        fail "runner service has an artificial CPU affinity restriction"
    [[ "$(systemctl show "$SERVICE_NAME" -p TasksMax --value)" == "infinity" ]] ||
        fail "runner service has an artificial process ceiling"
    [[ "$(run_as_runner nproc)" -ge 12 ]] || fail "runner sees fewer than 12 logical CPUs"
    [[ "$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo)" -ge $((48 * 1024 * 1024)) ]] ||
        fail "host exposes less than 48 GiB of memory"
    run_as_runner bash -c '[[ -r /dev/kvm && -w /dev/kvm ]]' ||
        fail "/dev/kvm is not readable and writable by the runner identity"
    run_as_runner test -S "$podman_socket"
    run_as_runner rustc --version | grep -Fq "rustc ${RUST_VERSION} "
    run_as_runner cargo clippy --version >/dev/null
    run_as_runner cargo fmt --version >/dev/null
    run_as_runner rustup target list --installed | grep -Fxq x86_64-unknown-linux-musl
    run_as_runner sccache --version >/dev/null
    if run_as_runner sudo -n true >/dev/null 2>&1; then
        fail "runner identity has non-interactive sudo authority"
    fi
    for authority_root in /conary/repository-keys /conary/deployment-backups; do
        if run_as_runner test -r "$authority_root"; then
            fail "runner identity can read production authority root ${authority_root}"
        fi
    done

    logical_cpus="$(run_as_runner nproc)"
    log "ok: ${RUNNER_NAME} has ${logical_cpus} CPUs, unrestricted memory, rootless Podman, KVM, Rust ${RUST_VERSION}, and sccache"
}

if [[ "$MODE" == "install" ]]; then
    for command in apt-get curl getent grep install loginctl runuser sed sha256sum systemctl tar useradd usermod; do
        require_cmd "$command"
    done
    install_packages
    ensure_runner_identity
    ensure_rust
    ensure_rootless_podman
    install_runner
    install_service
fi

verify_setup
