#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: build-selfhost-qcow2.sh [OPTIONS]

Build a Tier-2-complete self-hosting qcow2 and the deterministic inputs it
depends on.

Options:
  --work-dir PATH     Bootstrap work directory (default: /tmp/conary-selfhost-vm)
  --lfs-root PATH     Sysroot path for bootstrap phases (default: <work-dir>/lfs-root)
  --image-size SIZE   qcow2 size passed to `conary bootstrap image` (default: 16G)
  --jobs N            Parallel job count for bootstrap build phases
  --target ARCH       Target architecture (must be x86_64 for this milestone)
  --conary-bin PATH   Conary binary to invoke (default: <repo>/target/debug/conary)
  --help              Show this help text
EOF
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

WORK_DIR="${WORK_DIR:-/tmp/conary-selfhost-vm}"
LFS_ROOT=""
IMAGE_SIZE="${IMAGE_SIZE:-16G}"
TARGET_ARCH="${TARGET_ARCH:-x86_64}"
DEFAULT_CONARY_BIN="$PROJECT_DIR/target/debug/conary"
CONARY_BIN="${CONARY_BIN:-$DEFAULT_CONARY_BIN}"
JOBS=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --work-dir)
            WORK_DIR="$2"
            shift 2
            ;;
        --lfs-root)
            LFS_ROOT="$2"
            shift 2
            ;;
        --image-size)
            IMAGE_SIZE="$2"
            shift 2
            ;;
        --jobs)
            JOBS="$2"
            shift 2
            ;;
        --target)
            TARGET_ARCH="$2"
            shift 2
            ;;
        --conary-bin)
            CONARY_BIN="$2"
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

if [[ -z "$LFS_ROOT" ]]; then
    LFS_ROOT="$WORK_DIR/lfs-root"
fi

VM_SELFHOST_DIR="$WORK_DIR/vm-selfhost"
INPUTS_DIR="$VM_SELFHOST_DIR/inputs"
KEYS_DIR="$VM_SELFHOST_DIR/keys"
OUTPUT_DIR="$VM_SELFHOST_DIR/output"
LOGS_DIR="$VM_SELFHOST_DIR/logs"
WORKSPACE_TARBALL="$INPUTS_DIR/conary-workspace.tar.gz"
WORKSPACE_SHA256="$INPUTS_DIR/conary-workspace.tar.gz.sha256"
PRIVATE_KEY="$KEYS_DIR/selfhost_ed25519"
PUBLIC_KEY="$KEYS_DIR/selfhost_ed25519.pub"
OUTPUT_IMAGE="$OUTPUT_DIR/conaryos-selfhost-x86_64.qcow2"

log() {
    printf '[build-selfhost-qcow2] %s\n' "$*"
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

require_clean_tracked_files() {
    if ! git -C "$PROJECT_DIR" diff --quiet --ignore-submodules HEAD --; then
        echo "Tracked files are dirty. Commit or stash tracked changes before building the self-host image." >&2
        exit 1
    fi
    if ! git -C "$PROJECT_DIR" diff --cached --quiet --ignore-submodules --; then
        echo "Tracked files are staged but uncommitted. Commit them before building the self-host image." >&2
        exit 1
    fi
}

ensure_conary_bin() {
    if [[ -x "$CONARY_BIN" ]]; then
        return
    fi

    if [[ "$CONARY_BIN" != "$DEFAULT_CONARY_BIN" ]]; then
        echo "Custom conary binary does not exist or is not executable: $CONARY_BIN" >&2
        exit 1
    fi

    log "Building conary CLI at $CONARY_BIN"
    (
        cd "$PROJECT_DIR"
        cargo build --locked -p conary
    )
}

maybe_jobs_args() {
    if [[ -n "$JOBS" ]]; then
        printf -- '--jobs=%s' "$JOBS"
    fi
}

run_bootstrap() {
    log "Running: $CONARY_BIN bootstrap $*"
    "$CONARY_BIN" bootstrap "$@"
}

main() {
    require_cmd git gzip sha256sum ssh-keygen cargo

    if [[ "$TARGET_ARCH" != "x86_64" ]]; then
        echo "This self-hosting VM wrapper only supports x86_64 for the first milestone." >&2
        exit 1
    fi

    require_clean_tracked_files
    ensure_conary_bin

    mkdir -p "$INPUTS_DIR" "$KEYS_DIR" "$OUTPUT_DIR" "$LOGS_DIR"

    log "Creating deterministic workspace tarball at $WORKSPACE_TARBALL"
    git -C "$PROJECT_DIR" archive --format=tar --prefix=conary-workspace/ HEAD | gzip -n >"$WORKSPACE_TARBALL"
    sha256sum "$WORKSPACE_TARBALL" | awk '{print $1}' >"$WORKSPACE_SHA256"

    log "Generating ephemeral VM access keypair under $KEYS_DIR"
    rm -f "$PRIVATE_KEY" "$PUBLIC_KEY"
    ssh-keygen -q -t ed25519 -N "" -f "$PRIVATE_KEY" -C "conary-selfhost-vm"

    jobs_arg="$(maybe_jobs_args || true)"

    run_bootstrap init --work-dir "$WORK_DIR" --target "$TARGET_ARCH" ${jobs_arg:+$jobs_arg}
    run_bootstrap cross-tools --work-dir "$WORK_DIR" --lfs-root "$LFS_ROOT" ${jobs_arg:+$jobs_arg}
    run_bootstrap temp-tools --work-dir "$WORK_DIR" --lfs-root "$LFS_ROOT" ${jobs_arg:+$jobs_arg}
    run_bootstrap system --work-dir "$WORK_DIR" --lfs-root "$LFS_ROOT" ${jobs_arg:+$jobs_arg}
    run_bootstrap config --work-dir "$WORK_DIR" --lfs-root "$LFS_ROOT"
    run_bootstrap tier2 --work-dir "$WORK_DIR" --lfs-root "$LFS_ROOT" ${jobs_arg:+$jobs_arg}
    run_bootstrap guest-profile --work-dir "$WORK_DIR" --lfs-root "$LFS_ROOT" --public-key "$PUBLIC_KEY"

    log "Removing stale validation image at $OUTPUT_IMAGE"
    rm -f "$OUTPUT_IMAGE"

    run_bootstrap image \
        --work-dir "$WORK_DIR" \
        --output "$OUTPUT_IMAGE" \
        --format qcow2 \
        --size "$IMAGE_SIZE"

    log "Self-host qcow2 ready at $OUTPUT_IMAGE"
    log "Workspace tarball: $WORKSPACE_TARBALL"
    log "Workspace sha256: $WORKSPACE_SHA256"
}

main "$@"
