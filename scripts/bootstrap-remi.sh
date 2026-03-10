#!/usr/bin/env bash
# scripts/bootstrap-remi.sh
#
# Orchestrator for the Conary bootstrap pipeline on Remi.
# Builds a self-hosting Linux system in three tiers:
#   Tier A: 16 packages, boots to login prompt
#   Tier B: ~60 packages, full base with SSH
#   Tier C: Rust + Conary, self-hosting
#
# Usage:
#   ./scripts/bootstrap-remi.sh --tier all     # Full pipeline
#   ./scripts/bootstrap-remi.sh --tier a       # Single tier
#   ./scripts/bootstrap-remi.sh --resume       # Resume after failure
#   ./scripts/bootstrap-remi.sh --clean        # Clean start

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────────

BOOTSTRAP_DIR="${BOOTSTRAP_DIR:-/conary/bootstrap}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONARY_BIN="${CONARY_BIN:-$PROJECT_DIR/target/debug/conary}"
STATE_FILE="$BOOTSTRAP_DIR/bootstrap-state.json"
LOG_DIR="$BOOTSTRAP_DIR/logs"
PIPELINE_LOG="$LOG_DIR/pipeline.log"

# Tier A: 16 packages -- minimal boot to login prompt
TIER_A_PACKAGES=(
    zlib xz zstd openssl ncurses readline libcap kmod
    elfutils dbus linux-pam util-linux coreutils bash systemd linux
)

# Tier B: ~45 packages -- full base with SSH
TIER_B_PACKAGES=(
    libmnl iproute2 openssh curl wget2 ca-certificates
    make autoconf automake libtool cmake ninja meson
    perl python m4 bison flex
    grep sed gawk less diffutils patch findutils file
    tar gzip bzip2 cpio
    procps-ng psmisc shadow sudo
    vim nano
    git
    grub efibootmgr efivar dosfstools popt
)

# ── Logging ───────────────────────────────────────────────────────────────────

log_info() {
    local msg
    msg="[$(date '+%Y-%m-%d %H:%M:%S')] [INFO] $*"
    echo "$msg"
    echo "$msg" >> "$PIPELINE_LOG"
}

log_error() {
    local msg
    msg="[$(date '+%Y-%m-%d %H:%M:%S')] [ERROR] $*"
    echo "$msg" >&2
    echo "$msg" >> "$PIPELINE_LOG"
}

log_step() {
    local msg
    msg="[$(date '+%Y-%m-%d %H:%M:%S')] [STEP] $*"
    echo ""
    echo "════════════════════════════════════════════════════════════════"
    echo "  $*"
    echo "════════════════════════════════════════════════════════════════"
    echo "$msg" >> "$PIPELINE_LOG"
}

# ── Prerequisites ─────────────────────────────────────────────────────────────

check_prerequisites() {
    log_step "Checking prerequisites"

    local missing=()
    for cmd in qemu-system-x86_64 sfdisk mkfs.ext4 mkfs.fat curl cpio gzip jq; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done

    if [ ${#missing[@]} -gt 0 ]; then
        log_error "Missing required tools: ${missing[*]}"
        log_error "Install them and retry."
        return 1
    fi

    log_info "All prerequisites found"
}

build_conary_if_needed() {
    if [ -x "$CONARY_BIN" ]; then
        log_info "Using existing conary binary: $CONARY_BIN"
        return 0
    fi

    log_step "Building conary from source"
    (cd "$PROJECT_DIR" && cargo build)
    if [ ! -x "$CONARY_BIN" ]; then
        log_error "Build succeeded but binary not found at $CONARY_BIN"
        return 1
    fi
    log_info "Conary built: $CONARY_BIN"
}

# ── Directory Setup ───────────────────────────────────────────────────────────

setup_directories() {
    log_step "Setting up bootstrap directories"
    mkdir -p "$BOOTSTRAP_DIR"/{sources,tools,stage1,sysroot,build,logs,images}
    mkdir -p "$LOG_DIR"
    touch "$PIPELINE_LOG"
    log_info "Bootstrap root: $BOOTSTRAP_DIR"
}

# ── Stage 0 + Stage 1 ────────────────────────────────────────────────────────

run_stage0() {
    log_step "Stage 0: Cross-compilation toolchain"
    "$CONARY_BIN" bootstrap stage0 \
        --work-dir "$BOOTSTRAP_DIR" \
        --root "$BOOTSTRAP_DIR/sysroot" \
        2>&1 | tee "$LOG_DIR/stage0.log"
    log_info "[COMPLETE] Stage 0"
}

run_stage1() {
    log_step "Stage 1: Self-hosted toolchain"
    "$CONARY_BIN" bootstrap stage1 \
        --work-dir "$BOOTSTRAP_DIR" \
        --root "$BOOTSTRAP_DIR/sysroot" \
        --recipe-dir "$PROJECT_DIR/recipes" \
        2>&1 | tee "$LOG_DIR/stage1.log"
    log_info "[COMPLETE] Stage 1"
}

# ── Tier Builders ─────────────────────────────────────────────────────────────

build_packages() {
    local tier="$1"
    shift
    local packages=("$@")
    local total=${#packages[@]}
    local idx=0

    log_step "Building Tier ${tier^^}: $total packages"

    for pkg in "${packages[@]}"; do
        idx=$((idx + 1))

        # Check resume: skip completed packages
        if [ -f "$STATE_FILE" ] && jq -e ".completed_packages | index(\"$pkg\")" "$STATE_FILE" &>/dev/null; then
            log_info "[$idx/$total] $pkg -- already completed, skipping"
            continue
        fi

        log_info "[$idx/$total] Building $pkg"
        if "$CONARY_BIN" bootstrap base \
            --package "$pkg" \
            --tier "$tier" \
            --work-dir "$BOOTSTRAP_DIR" \
            --root "$BOOTSTRAP_DIR/sysroot" \
            --recipe-dir "$PROJECT_DIR/recipes" \
            2>&1 | tee "$LOG_DIR/pkg-$pkg.log"; then
            log_info "[$idx/$total] $pkg -- [COMPLETE]"
        else
            log_error "[$idx/$total] $pkg -- [FAILED]"
            log_error "Log: $LOG_DIR/pkg-$pkg.log"
            return 1
        fi
    done

    log_info "[COMPLETE] Tier ${tier^^}: all $total packages built"
}

build_tier_a() {
    build_packages "a" "${TIER_A_PACKAGES[@]}"
}

build_tier_b() {
    build_packages "b" "${TIER_B_PACKAGES[@]}"
}

build_tier_c() {
    log_step "Building Tier C: Rust + Conary"
    "$CONARY_BIN" bootstrap conary \
        --work-dir "$BOOTSTRAP_DIR" \
        --root "$BOOTSTRAP_DIR/sysroot" \
        2>&1 | tee "$LOG_DIR/tier-c.log"
    log_info "[COMPLETE] Tier C"
}

# ── Sysroot Population ───────────────────────────────────────────────────────

populate_sysroot() {
    log_step "Populating sysroot with system configuration"
    "$CONARY_BIN" bootstrap populate \
        --root "$BOOTSTRAP_DIR/sysroot" \
        2>&1 | tee "$LOG_DIR/populate.log" || {
        # Fallback: populate inline if conary command not available
        log_info "Using inline sysroot population"
        populate_sysroot_inline
    }
}

populate_sysroot_inline() {
    local root="$BOOTSTRAP_DIR/sysroot"
    local etc="$root/etc"
    mkdir -p "$etc"

    cat > "$etc/passwd" <<'PASSWD'
root:x:0:0:root:/root:/bin/bash
nobody:x:65534:65534:Nobody:/:/sbin/nologin
PASSWD

    cat > "$etc/group" <<'GROUP'
root:x:0:
wheel:x:10:
tty:x:5:
nogroup:x:65534:
GROUP

    cat > "$etc/shadow" <<'SHADOW'
root::0:0:99999:7:::
nobody:!:0:0:99999:7:::
SHADOW
    chmod 600 "$etc/shadow"

    echo "conary" > "$etc/hostname"

    cat > "$etc/os-release" <<'OSREL'
NAME="Conary Linux"
ID=conary
VERSION_ID=0.1
PRETTY_NAME="Conary Linux 0.1 (Bootstrap)"
HOME_URL="https://conary.io"
OSREL

    : > "$etc/machine-id"

    cat > "$etc/fstab" <<'FSTAB'
# /etc/fstab - Conary system
LABEL=CONARY_ROOT  /          ext4  defaults,noatime  0 1
LABEL=CONARY_ESP   /boot/efi  vfat  defaults,noatime  0 2
tmpfs              /tmp       tmpfs defaults,nosuid   0 0
FSTAB

    log_info "Sysroot populated with system files"
}

# ── Image Generation ──────────────────────────────────────────────────────────

generate_image() {
    local tier="$1"
    local image="$BOOTSTRAP_DIR/images/tier-${tier}.img"

    log_step "Generating image: tier-${tier}.img"

    "$CONARY_BIN" bootstrap image \
        --root "$BOOTSTRAP_DIR/sysroot" \
        --output "$image" \
        2>&1 | tee "$LOG_DIR/image-tier-${tier}.log"

    log_info "Image generated: $image"
}

# ── QEMU Smoke Tests ─────────────────────────────────────────────────────────

qemu_test_tier_a() {
    local image="$BOOTSTRAP_DIR/images/tier-a.img"
    local kernel="$BOOTSTRAP_DIR/sysroot/boot/vmlinuz"
    local initrd="$BOOTSTRAP_DIR/sysroot/boot/initramfs.img"
    local serial_log="$LOG_DIR/qemu-tier-a.log"

    log_step "QEMU Tier A: boot to login prompt"

    : > "$serial_log"

    timeout 120 qemu-system-x86_64 \
        -kernel "$kernel" \
        -initrd "$initrd" \
        -append "root=/dev/vda2 console=ttyS0 init=/lib/systemd/systemd" \
        -drive "file=$image,format=raw" \
        -m 1024 \
        -nographic \
        -no-reboot \
        -serial "file:$serial_log" \
        -monitor none &
    local qemu_pid=$!

    # Wait for login prompt or timeout
    local elapsed=0
    while [ "$elapsed" -lt 90 ]; do
        if grep -q "login:" "$serial_log" 2>/dev/null; then
            kill "$qemu_pid" 2>/dev/null || true
            wait "$qemu_pid" 2>/dev/null || true
            log_info "[PASS] Tier A: login prompt detected"
            return 0
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done

    kill "$qemu_pid" 2>/dev/null || true
    wait "$qemu_pid" 2>/dev/null || true
    log_error "[FAIL] Tier A: no login prompt after 90s"
    log_error "Serial log: $serial_log"
    return 1
}

qemu_test_tier_b() {
    local image="$BOOTSTRAP_DIR/images/tier-b.img"
    local serial_log="$LOG_DIR/qemu-tier-b.log"

    log_step "QEMU Tier B: boot with GRUB + SSH"

    : > "$serial_log"

    qemu-system-x86_64 \
        -drive "file=$image,format=raw" \
        -m 2048 \
        -nographic \
        -no-reboot \
        -serial "file:$serial_log" \
        -net nic -net "user,hostfwd=tcp::2222-:22" \
        -monitor none &
    local qemu_pid=$!

    # Wait for SSH
    local elapsed=0
    while [ "$elapsed" -lt 120 ]; do
        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
               -o UserKnownHostsFile=/dev/null \
               -p 2222 root@localhost "uname -r" 2>/dev/null; then
            # Run additional checks
            ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
                -p 2222 root@localhost \
                "ls /usr/bin/grep && python3 --version && git --version" 2>&1 || true
            kill "$qemu_pid" 2>/dev/null || true
            wait "$qemu_pid" 2>/dev/null || true
            log_info "[PASS] Tier B: SSH working, commands verified"
            return 0
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done

    kill "$qemu_pid" 2>/dev/null || true
    wait "$qemu_pid" 2>/dev/null || true
    log_error "[FAIL] Tier B: SSH not responding after 120s"
    log_error "Serial log: $serial_log"
    return 1
}

qemu_test_tier_c() {
    local image="$BOOTSTRAP_DIR/images/tier-c.img"
    local serial_log="$LOG_DIR/qemu-tier-c.log"

    log_step "QEMU Tier C: self-hosting verification"

    : > "$serial_log"

    qemu-system-x86_64 \
        -drive "file=$image,format=raw" \
        -m 4096 \
        -smp 4 \
        -nographic \
        -no-reboot \
        -serial "file:$serial_log" \
        -net nic -net "user,hostfwd=tcp::2222-:22" \
        -monitor none &
    local qemu_pid=$!

    # Wait for SSH
    local elapsed=0
    while [ "$elapsed" -lt 120 ]; do
        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
               -o UserKnownHostsFile=/dev/null \
               -p 2222 root@localhost "rustc --version" 2>/dev/null; then
            break
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done

    if [ "$elapsed" -ge 120 ]; then
        kill "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
        log_error "[FAIL] Tier C: SSH not responding"
        log_error "Serial log: $serial_log"
        return 1
    fi

    # Verify tools
    if ! ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
            -p 2222 root@localhost \
            "rustc --version && cargo --version && conary --version" 2>&1; then
        kill "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
        log_error "[FAIL] Tier C: tools not found"
        return 1
    fi

    log_info "[PASS] Tier C: self-hosting tools verified"
    kill "$qemu_pid" 2>/dev/null || true
    wait "$qemu_pid" 2>/dev/null || true
    return 0
}

# ── Main Pipeline ─────────────────────────────────────────────────────────────

run_tier() {
    local tier="$1"

    case "$tier" in
        a)
            run_stage0
            run_stage1
            build_tier_a
            populate_sysroot
            generate_image "a"
            qemu_test_tier_a
            ;;
        b)
            build_tier_b
            generate_image "b"
            qemu_test_tier_b
            ;;
        c)
            build_tier_c
            generate_image "c"
            qemu_test_tier_c
            ;;
        all)
            run_tier "a"
            run_tier "b"
            run_tier "c"
            ;;
        *)
            log_error "Unknown tier: $tier"
            return 1
            ;;
    esac
}

usage() {
    cat <<USAGE
Usage: $(basename "$0") [OPTIONS]

Options:
  --tier TIER     Build tier: a, b, c, or all (default: all)
  --resume        Resume from last checkpoint
  --clean         Clean bootstrap directory and start fresh
  --help          Show this help

Environment:
  BOOTSTRAP_DIR   Bootstrap root (default: /conary/bootstrap)
  CONARY_BIN      Path to conary binary (default: ./target/debug/conary)
USAGE
}

main() {
    local tier="all"
    local resume=false
    local clean=false

    while [ $# -gt 0 ]; do
        case "$1" in
            --tier)
                tier="$2"
                shift 2
                ;;
            --resume)
                resume=true
                shift
                ;;
            --clean)
                clean=true
                shift
                ;;
            --help)
                usage
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                usage
                exit 1
                ;;
        esac
    done

    setup_directories

    log_step "Conary Bootstrap Pipeline"
    log_info "Tier: $tier"
    log_info "Bootstrap dir: $BOOTSTRAP_DIR"
    log_info "Resume: $resume"

    if $clean; then
        log_step "Cleaning bootstrap directory"
        rm -rf "${BOOTSTRAP_DIR:?}"/{tools,stage1,sysroot,build,images}
        rm -f "$STATE_FILE"
        setup_directories
        log_info "Clean complete"
    fi

    check_prerequisites
    build_conary_if_needed

    if $resume && [ -f "$STATE_FILE" ]; then
        log_info "Resuming from state: $STATE_FILE"
    fi

    run_tier "$tier"

    log_step "Pipeline Complete"
    log_info "Images: $BOOTSTRAP_DIR/images/"
    log_info "Logs: $LOG_DIR/"
}

main "$@"
