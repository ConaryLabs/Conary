#!/usr/bin/env bash
# scripts/remi-ci-runner-preflight.sh -- Validate the isolated trusted runner.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: remi-ci-runner-preflight.sh [--mode rust|container|qemu]

Validate the non-privileged execution boundary and tools required by the
trusted Remi CI benchmark lane.
EOF
}

MODE="rust"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)
            MODE="${2:-}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "[remi-ci-preflight] ERROR: unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

case "$MODE" in
    rust|container|qemu)
        ;;
    *)
        echo "[remi-ci-preflight] ERROR: invalid mode: ${MODE}" >&2
        exit 1
        ;;
esac

fail() {
    echo "[remi-ci-preflight] ERROR: $*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

require_positive_integer() {
    local name="$1"
    local value="$2"
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "${name} must be a positive integer"
}

check_cgroup_v2_limits() {
    [[ -f /sys/fs/cgroup/cgroup.controllers ]] || return

    local cgroup_relative cgroup_dir setting value
    cgroup_relative="$(awk -F: '$1 == "0" { print $3 }' /proc/self/cgroup)"
    [[ -n "$cgroup_relative" ]] || fail "cannot determine the runner cgroup"
    cgroup_dir="/sys/fs/cgroup${cgroup_relative}"

    while [[ "$cgroup_dir" == /sys/fs/cgroup* ]]; do
        for setting in cpu.max memory.max; do
            [[ -f "${cgroup_dir}/${setting}" ]] || continue
            read -r value _ < "${cgroup_dir}/${setting}"
            [[ "$value" == "max" ]] ||
                fail "runner inherits artificial ${setting} limit ${value} from ${cgroup_dir}"
        done
        [[ "$cgroup_dir" == "/sys/fs/cgroup" ]] && break
        cgroup_dir="$(dirname "$cgroup_dir")"
    done
}

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

expected_identity="${CONARY_CI_RUNNER_IDENTITY:-conary-ci}"
actual_identity="$(id -un)"
[[ "$actual_identity" == "$expected_identity" ]] ||
    fail "runner identity is '${actual_identity}', expected dedicated identity '${expected_identity}'"
[[ "$(id -u)" -ne 0 ]] || fail "runner must not execute as root"

if command -v sudo >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1; then
    fail "runner identity has non-interactive sudo authority"
fi
if id -nG | tr ' ' '\n' | grep -Fxq docker; then
    fail "runner identity belongs to the root-equivalent docker group"
fi

for variable in \
    RELEASE_SIGNING_KEY \
    REMI_BOOTSTRAP_SSH_TARGET \
    REMI_SSH_KEY \
    REMI_SSH_TARGET; do
    [[ -z "${!variable:-}" ]] || fail "production variable ${variable} is present"
done

for authority_root in /conary/repository-keys /conary/deployment-backups; do
    [[ ! -r "$authority_root" ]] ||
        fail "runner identity can read production authority root ${authority_root}"
done

for command in cargo git jq rg rustc sccache; do
    require_cmd "$command"
done
cargo fmt --version >/dev/null
cargo clippy --version >/dev/null

minimum_logical_cpus="${CONARY_CI_MIN_LOGICAL_CPUS:-12}"
minimum_memory_gib="${CONARY_CI_MIN_MEMORY_GIB:-48}"
require_positive_integer CONARY_CI_MIN_LOGICAL_CPUS "$minimum_logical_cpus"
require_positive_integer CONARY_CI_MIN_MEMORY_GIB "$minimum_memory_gib"
logical_cpus="$(nproc)"
memory_kib="$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo)"
[[ "$logical_cpus" -ge "$minimum_logical_cpus" ]] ||
    fail "runner exposes ${logical_cpus} logical CPUs; expected at least ${minimum_logical_cpus}"
[[ "$memory_kib" -ge $((minimum_memory_gib * 1024 * 1024)) ]] ||
    fail "runner exposes less than ${minimum_memory_gib} GiB of memory"
check_cgroup_v2_limits

workspace_rust_version="$(
    awk '
        /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
        in_workspace_package && /^\[/ { exit }
        in_workspace_package && /^rust-version[[:space:]]*=/ {
            value = $0
            sub(/^[^=]*=[[:space:]]*"/, "", value)
            sub(/"[[:space:]]*$/, "", value)
            print value
            exit
        }
    ' Cargo.toml
)"
[[ -n "$workspace_rust_version" ]] || fail "workspace Rust version is missing"
[[ "$(rustc --version)" == "rustc ${workspace_rust_version} "* ]] ||
    fail "rustc does not match workspace version ${workspace_rust_version}"

cache_root="${CONARY_CI_CACHE_ROOT:-${HOME}/.cache/conary-ci}"
[[ "$cache_root" == /* ]] || fail "CONARY_CI_CACHE_ROOT must be absolute"
mkdir -p "$cache_root"
[[ -d "$cache_root" && -w "$cache_root" ]] ||
    fail "runner cache root is not a writable directory: ${cache_root}"

if [[ "$MODE" == "container" || "$MODE" == "qemu" ]]; then
    for command in curl file gperf make musl-gcc podman rustup sha256sum tar; do
        require_cmd "$command"
    done
    rustup target list --installed | grep -Fxq x86_64-unknown-linux-musl ||
        fail "x86_64-unknown-linux-musl Rust target is not installed"
    runner_uid="$(id -u)"
    podman_socket="${PODMAN_SOCKET:-/run/user/${runner_uid}/podman/podman.sock}"
    [[ -S "$podman_socket" ]] ||
        fail "rootless Podman socket is missing: ${podman_socket}"
    DOCKER_HOST="unix://${podman_socket}" podman info >/dev/null
    curl --unix-socket "$podman_socket" -fsS http://d/v1.41/_ping >/dev/null ||
        curl --unix-socket "$podman_socket" -fsS http://d/_ping >/dev/null ||
        fail "Podman socket did not answer the Docker-compatible API ping"
fi

if [[ "$MODE" == "qemu" ]]; then
    require_cmd qemu-img
    require_cmd qemu-system-x86_64
    require_cmd scp
    [[ -r /dev/kvm && -w /dev/kvm ]] ||
        fail "/dev/kvm is not readable and writable by the runner identity"
fi

echo "[remi-ci-preflight] ok (${MODE}; ${logical_cpus} logical CPUs; no cgroup CPU or memory limit)"
