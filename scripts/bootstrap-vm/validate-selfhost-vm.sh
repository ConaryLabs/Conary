#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: validate-selfhost-vm.sh [OPTIONS]

Boot the self-host qcow2 under QEMU, wait for SSH, copy the staged inputs into
the guest, and run the checked-in guest validation script.

Options:
  --work-dir PATH        Self-host work directory (default: /tmp/conary-selfhost-vm)
  --repo-name NAME       Repository name to configure inside the guest
  --repo-url URL         Repository metadata URL
  --remi-endpoint URL    Remi conversion endpoint URL
  --remi-distro DISTRO   Remi distro name (for example: fedora)
  --root-json PATH       Optional TUF root metadata to copy into the guest
  --ssh-port PORT        Host port forwarded to guest SSH (default: 2222)
  --memory MB            QEMU guest memory in MiB (default: 4096)
  --cpus N               QEMU vCPU count (default: 4)
  --help                 Show this help text

Environment overrides:
  CONARY_BOOTSTRAP_OVMF_CODE           Override the OVMF code firmware path
  CONARY_BOOTSTRAP_OVMF_VARS_TEMPLATE  Override the OVMF vars template path
  CONARY_BOOTSTRAP_QEMU_CPU            Override the QEMU CPU model (default: host with KVM, else max)
EOF
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

WORK_DIR="${WORK_DIR:-/tmp/conary-selfhost-vm}"
REPO_NAME=""
REPO_URL=""
REMI_ENDPOINT=""
REMI_DISTRO=""
ROOT_JSON=""
SSH_PORT="${SSH_PORT:-2222}"
MEMORY_MB="${MEMORY_MB:-4096}"
CPUS="${CPUS:-4}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --work-dir)
            WORK_DIR="$2"
            shift 2
            ;;
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
        --remi-distro)
            REMI_DISTRO="$2"
            shift 2
            ;;
        --root-json)
            ROOT_JSON="$2"
            shift 2
            ;;
        --ssh-port)
            SSH_PORT="$2"
            shift 2
            ;;
        --memory)
            MEMORY_MB="$2"
            shift 2
            ;;
        --cpus)
            CPUS="$2"
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
    echo "--repo-name, --repo-url, --remi-endpoint, and --remi-distro are required." >&2
    usage >&2
    exit 1
fi

VM_SELFHOST_DIR="$WORK_DIR/vm-selfhost"
INPUTS_DIR="$VM_SELFHOST_DIR/inputs"
KEYS_DIR="$VM_SELFHOST_DIR/keys"
OUTPUT_DIR="$VM_SELFHOST_DIR/output"
LOGS_DIR="$VM_SELFHOST_DIR/logs"
IMAGE="$OUTPUT_DIR/conaryos-selfhost-x86_64.qcow2"
SSH_KEY="$KEYS_DIR/selfhost_ed25519"
WORKSPACE_TARBALL="$INPUTS_DIR/conary-workspace.tar.gz"
WORKSPACE_SHA256="$INPUTS_DIR/conary-workspace.tar.gz.sha256"
GUEST_VALIDATE_LOCAL="$PROJECT_DIR/scripts/bootstrap-vm/guest-validate.sh"
SERIAL_LOG="$LOGS_DIR/qemu-serial.log"
SSH_LOG="$LOGS_DIR/ssh-probe.log"
GUEST_VALIDATE_LOG="$LOGS_DIR/guest-validate.log"
GUEST_INPUT_DIR="/var/lib/conary/bootstrap-inputs"
GUEST_VALIDATE_REMOTE="$GUEST_INPUT_DIR/guest-validate.sh"
OVMF_CODE="${CONARY_BOOTSTRAP_OVMF_CODE:-}"
OVMF_VARS_TEMPLATE="${CONARY_BOOTSTRAP_OVMF_VARS_TEMPLATE:-}"
OVMF_VARS_RUNTIME="$LOGS_DIR/OVMF_VARS.fd"
QEMU_CPU="${CONARY_BOOTSTRAP_QEMU_CPU:-}"
QEMU_PID=""

ssh_opts=(
    -i "$SSH_KEY"
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
)

log() {
    printf '[validate-selfhost-vm] %s\n' "$*"
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

cleanup() {
    if [[ -n "$QEMU_PID" ]]; then
        kill "$QEMU_PID" >/dev/null 2>&1 || true
        wait "$QEMU_PID" >/dev/null 2>&1 || true
    fi
}

resolve_ovmf_firmware() {
    local candidate

    if [[ -z "$OVMF_CODE" ]]; then
        for candidate in \
            /usr/share/OVMF/OVMF_CODE.fd \
            /usr/share/edk2/ovmf/OVMF_CODE.fd
        do
            if [[ -f "$candidate" ]]; then
                OVMF_CODE="$candidate"
                break
            fi
        done
    fi

    if [[ -z "$OVMF_VARS_TEMPLATE" ]]; then
        for candidate in \
            /usr/share/OVMF/OVMF_VARS.fd \
            /usr/share/edk2/ovmf/OVMF_VARS.fd
        do
            if [[ -f "$candidate" ]]; then
                OVMF_VARS_TEMPLATE="$candidate"
                break
            fi
        done
    fi

    if [[ ! -f "$OVMF_CODE" ]]; then
        echo "OVMF code firmware not found. Set CONARY_BOOTSTRAP_OVMF_CODE if your distro installs it elsewhere." >&2
        exit 1
    fi

    if [[ ! -f "$OVMF_VARS_TEMPLATE" ]]; then
        echo "OVMF vars template not found. Set CONARY_BOOTSTRAP_OVMF_VARS_TEMPLATE if your distro installs it elsewhere." >&2
        exit 1
    fi
}

resolve_qemu_cpu() {
    if [[ -n "$QEMU_CPU" ]]; then
        return
    fi

    if [[ -e /dev/kvm ]]; then
        QEMU_CPU="host"
    else
        # The self-host sysroot currently runs userland built with x86-64-v2
        # instructions, so the tiny qemu64 default is not representative enough
        # for truthful guest validation.
        QEMU_CPU="max"
    fi
}

start_qemu() {
    local accel_args=()
    if [[ -e /dev/kvm ]]; then
        accel_args=(-enable-kvm)
    fi

    : >"$SERIAL_LOG"
    : >"$SSH_LOG"
    : >"$GUEST_VALIDATE_LOG"
    cp "$OVMF_VARS_TEMPLATE" "$OVMF_VARS_RUNTIME"

    qemu-system-x86_64 \
        -machine q35 \
        -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE" \
        -drive "if=pflash,format=raw,file=$OVMF_VARS_RUNTIME" \
        -drive "file=$IMAGE,format=qcow2,if=virtio" \
        -m "$MEMORY_MB" \
        -smp "$CPUS" \
        -cpu "$QEMU_CPU" \
        -nographic \
        -no-reboot \
        -serial "file:$SERIAL_LOG" \
        -monitor none \
        -netdev "user,id=net0,hostfwd=tcp::${SSH_PORT}-:22" \
        -device e1000,netdev=net0 \
        "${accel_args[@]}" &
    QEMU_PID="$!"
}

wait_for_ssh() {
    local attempt
    for attempt in $(seq 1 60); do
        if ssh "${ssh_opts[@]}" -p "$SSH_PORT" root@127.0.0.1 true >>"$SSH_LOG" 2>&1; then
            return 0
        fi
        sleep 2
    done

    return 1
}

copy_guest_inputs() {
    ssh "${ssh_opts[@]}" -p "$SSH_PORT" root@127.0.0.1 "install -d -m 0755 '$GUEST_INPUT_DIR'"

    scp "${ssh_opts[@]}" -P "$SSH_PORT" \
        "$WORKSPACE_TARBALL" \
        "$WORKSPACE_SHA256" \
        "$GUEST_VALIDATE_LOCAL" \
        root@127.0.0.1:"$GUEST_INPUT_DIR"/

    if [[ -n "$ROOT_JSON" ]]; then
        scp "${ssh_opts[@]}" -P "$SSH_PORT" \
            "$ROOT_JSON" \
            root@127.0.0.1:"$GUEST_INPUT_DIR/root.json"
    fi
}

run_guest_validation() {
    ssh "${ssh_opts[@]}" -p "$SSH_PORT" root@127.0.0.1 \
        "chmod +x '$GUEST_VALIDATE_REMOTE' && bash '$GUEST_VALIDATE_REMOTE' \
            --repo-name '$REPO_NAME' \
            --repo-url '$REPO_URL' \
            --remi-endpoint '$REMI_ENDPOINT' \
            --remi-distro '$REMI_DISTRO'" | tee "$GUEST_VALIDATE_LOG"
}

main() {
    require_cmd qemu-system-x86_64 ssh scp

    mkdir -p "$LOGS_DIR"
    resolve_ovmf_firmware
    resolve_qemu_cpu

    for required_file in "$IMAGE" "$SSH_KEY" "$WORKSPACE_TARBALL" "$WORKSPACE_SHA256" "$GUEST_VALIDATE_LOCAL"; do
        if [[ ! -e "$required_file" ]]; then
            echo "Required file not found: $required_file" >&2
            exit 1
        fi
    done

    if [[ -n "$ROOT_JSON" && ! -f "$ROOT_JSON" ]]; then
        echo "root.json not found: $ROOT_JSON" >&2
        exit 1
    fi

    trap cleanup EXIT

    log "Booting $IMAGE under QEMU"
    start_qemu

    log "Waiting for guest SSH on localhost:$SSH_PORT"
    if ! wait_for_ssh; then
        echo "Guest SSH did not become reachable. See $SERIAL_LOG and $SSH_LOG." >&2
        exit 1
    fi

    log "Copying staged inputs into $GUEST_INPUT_DIR"
    copy_guest_inputs

    log "Running guest validation script"
    run_guest_validation

    log "Guest validation complete. Logs available under $LOGS_DIR"
}

main "$@"
