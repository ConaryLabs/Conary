#!/usr/bin/env bash
# scripts/build-qemu-guest-image.sh
set -euo pipefail

# Pinned Fedora Cloud Base input. The image is an official Fedora release
# artifact; the checksum is the one Fedora publishes for it. Changing the
# release means changing all three values together and re-running the build.
FEDORA_RELEASE="44"
FEDORA_IMAGE="Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2"
FEDORA_IMAGE_SHA256="28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f"
FEDORA_IMAGE_URL="https://download.fedoraproject.org/pub/fedora/linux/releases/${FEDORA_RELEASE}/Cloud/x86_64/images/${FEDORA_IMAGE}"

# The generation builder, the QEMU suites' preflight, and Group P's ISO carrier
# together require this toolchain inside the guest. Every entry is checked by
# name after installation, so a renamed or dropped Fedora package fails the
# build instead of surfacing as a suite preflight failure later.
GUEST_PACKAGES=(
    sqlite
    cpio
    dracut
    kmod
    systemd-udev
    systemd-boot-unsigned
    qemu-img
    e2fsprogs
    dosfstools
    composefs
    xorriso
    mtools
    openssh-server
)

# Driver packages are installed at the running kernel's exact NEVRA. Installing
# them unpinned can pull a newer kernel's modules, which `depmod` and `modprobe`
# would then not find for the kernel that is actually booted.
GUEST_KERNEL_MODULE_PACKAGES=(
    kernel-modules
    kernel-modules-extra
)

# Commands the phase-3 QEMU manifests probe with `command -v` before they build
# a generation. Group P adds xorriso/mmd/mcopy.
REQUIRED_COMMANDS=(
    sqlite3
    cpio
    dracut
    depmod
    systemd-repart
    qemu-img
    mkfs.ext4
    mkfs.vfat
    composefs-info
    xorriso
    mmd
    mcopy
)

# Exact packaged assets the generation builder consumes from the adopted live
# root. A command-only preflight allowed `fedora44-guest-v1` to pass image
# construction while TGE04/TGE05 failed after a full adoption and dracut run:
# Fedora Cloud does not install systemd-boot's EFI binary by default.
REQUIRED_FILES=(
    /usr/lib/systemd/boot/efi/systemd-bootx64.efi
)

# Filesystem and device drivers the suites depend on: erofs and overlay carry
# the composefs generation root, iso9660 carries the Group P ISO export, and
# e1000/virtio are the harness's NIC and scratch disk.
REQUIRED_MODULES=(
    erofs
    overlay
    isofs
    e1000
    virtio_blk
    virtio_net
    virtio_pci
)

# Same discovery order as apps/conary-test/src/engine/qemu.rs so an image built
# here boots on the firmware the harness will actually pick.
OVMF_CODE_PATHS=(
    /usr/share/edk2/x64/OVMF_CODE.4m.fd
    /usr/share/edk2/x64/OVMF_CODE.fd
    /usr/share/edk2/ovmf/OVMF_CODE.fd
    /usr/share/OVMF/OVMF_CODE_4M.fd
    /usr/share/OVMF/OVMF_CODE.fd
    /usr/share/edk2-ovmf/x64/OVMF_CODE.fd
    /usr/share/qemu/OVMF_CODE.fd
)

usage() {
    cat <<'EOF'
Usage: build-qemu-guest-image.sh --output PATH --private-key PATH [OPTIONS]

Build a phase-3 QEMU guest fixture image from an official Fedora Cloud Base
qcow2 plus provisioning. This is the regeneration path for the guest images the
phase-3 QEMU suites boot; it replaces the bootstrap-built conaryOS
`minimal-boot-*` lineage, which cannot be rebuilt once the bootstrap pipeline is
deleted.

The base image is downloaded once into a cache and verified against a pinned
SHA-256. The output image is grown, provisioned over SSH with the generation
toolchain, checked against the suites' own preflight, and powered off cleanly.
The base image is never modified and the output path must not already exist.

Options:
  --output PATH         New guest qcow2 image to create
  --private-key PATH    Disposable harness SSH identity. Its public half is
                        derived with `ssh-keygen -y` and authorized for guest
                        root, so the key the build provisions and the key the
                        build logs in with cannot disagree.
  --disk-size-gib N     Guest disk size in GiB (default: 20)
  --cache-dir PATH      Base-image download cache
                        (default: ${XDG_CACHE_HOME:-$HOME/.cache}/conary-test/base-images)
  --base-image PATH     Use this Fedora Cloud Base qcow2 instead of downloading
  --ssh-port N          Host port forwarded to guest SSH during the build
                        (default: 2222)
  --memory-mb N         Guest memory during the build (default: 4096)
  --no-compress         Skip the final compressing qcow2 convert
  --help                Show this help text

Host requirements: qemu-img, qemu-system-x86_64, mkfs.vfat, mmd, mcopy, curl,
sha256sum, ssh, scp, ssh-keygen, and an OVMF firmware image. KVM is used when
/dev/kvm is usable and TCG otherwise.
EOF
}

OUTPUT=""
PRIVATE_KEY=""
DISK_SIZE_GIB="20"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/conary-test/base-images"
BASE_IMAGE=""
SSH_PORT="2222"
MEMORY_MB="4096"
COMPRESS="yes"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        --disk-size-gib)
            DISK_SIZE_GIB="$2"
            shift 2
            ;;
        --cache-dir)
            CACHE_DIR="$2"
            shift 2
            ;;
        --base-image)
            BASE_IMAGE="$2"
            shift 2
            ;;
        --ssh-port)
            SSH_PORT="$2"
            shift 2
            ;;
        --memory-mb)
            MEMORY_MB="$2"
            shift 2
            ;;
        --no-compress)
            COMPRESS="no"
            shift
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

fail() {
    echo "error: $*" >&2
    exit 1
}

step() {
    echo
    echo "== $* =="
}

[[ -n "$OUTPUT" ]] || fail "--output is required"
[[ -n "$PRIVATE_KEY" ]] || fail "--private-key is required"
[[ ! -e "$OUTPUT" ]] || fail "output image already exists: $OUTPUT"
[[ -f "$PRIVATE_KEY" ]] || fail "private key not found: $PRIVATE_KEY"
[[ "$DISK_SIZE_GIB" =~ ^[0-9]+$ ]] || fail "--disk-size-gib must be an integer"
[[ "$SSH_PORT" =~ ^[0-9]+$ ]] || fail "--ssh-port must be an integer"
[[ "$MEMORY_MB" =~ ^[0-9]+$ ]] || fail "--memory-mb must be an integer"

for tool in qemu-img qemu-system-x86_64 mkfs.vfat mmd mcopy curl sha256sum ssh scp ssh-keygen; do
    command -v "$tool" >/dev/null || fail "host is missing required tool: $tool"
done

OVMF_CODE=""
for candidate in "${OVMF_CODE_PATHS[@]}"; do
    if [[ -f "$candidate" ]]; then
        OVMF_CODE="$candidate"
        break
    fi
done
[[ -n "$OVMF_CODE" ]] || fail "no OVMF firmware found in: ${OVMF_CODE_PATHS[*]}"

ACCEL="tcg"
if [[ -r /dev/kvm && -w /dev/kvm ]]; then
    ACCEL="kvm"
fi

PUBLIC_KEY_CONTENT="$(ssh-keygen -y -f "$PRIVATE_KEY")" ||
    fail "could not derive a public key from $PRIVATE_KEY"
[[ -n "$PUBLIC_KEY_CONTENT" ]] || fail "derived public key is empty: $PRIVATE_KEY"

WORK_DIR="$(mktemp -d)"
QEMU_PID=""

cleanup() {
    if [[ -n "$QEMU_PID" ]] && kill -0 "$QEMU_PID" 2>/dev/null; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

SSH_OPTS=(
    -i "$PRIVATE_KEY"
    -o IdentitiesOnly=yes
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
    -o ConnectTimeout=5
    -o BatchMode=yes
)

guest_ssh() {
    ssh "${SSH_OPTS[@]}" -p "$SSH_PORT" root@127.0.0.1 "$@"
}

guest_scp_to() {
    scp "${SSH_OPTS[@]}" -P "$SSH_PORT" "$1" "root@127.0.0.1:$2"
}

# `reboot` inside a guest restarts the VM without exiting QEMU, so the build
# never waits on the QEMU pid except after an explicit poweroff, and even then
# only with a bounded poll.
wait_for_qemu_exit() {
    local deadline=$((SECONDS + 180))
    while kill -0 "$QEMU_PID" 2>/dev/null; do
        if ((SECONDS > deadline)); then
            echo "guest did not power off within 180s; terminating QEMU" >&2
            kill "$QEMU_PID" 2>/dev/null || true
            wait "$QEMU_PID" 2>/dev/null || true
            QEMU_PID=""
            return 1
        fi
        sleep 2
    done
    wait "$QEMU_PID" 2>/dev/null || true
    QEMU_PID=""
}

start_guest() {
    local seed="${1:-}"
    local -a args=(
        -m "$MEMORY_MB"
        -smp 4
        -accel "$ACCEL"
        -nographic
        -serial "file:$WORK_DIR/serial.log"
        -monitor none
        -netdev "user,id=net0,hostfwd=tcp::${SSH_PORT}-:22"
        -device e1000,netdev=net0
        -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
        -drive "file=$OUTPUT,format=qcow2"
    )
    if [[ -n "$seed" ]]; then
        args+=(-drive "file=$seed,format=raw,if=virtio,media=cdrom")
    fi
    qemu-system-x86_64 "${args[@]}" </dev/null >>"$WORK_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
}

wait_for_ssh() {
    local attempts="${1:-150}"
    local i
    for ((i = 1; i <= attempts; i++)); do
        if guest_ssh true 2>/dev/null; then
            echo "guest ssh ready after ~$((i * 4))s"
            return 0
        fi
        kill -0 "$QEMU_PID" 2>/dev/null || {
            echo "--- serial log ---" >&2
            tail -60 "$WORK_DIR/serial.log" >&2 || true
            fail "QEMU exited before SSH became available"
        }
        sleep 4
    done
    echo "--- serial log ---" >&2
    tail -60 "$WORK_DIR/serial.log" >&2 || true
    fail "guest SSH never became available"
}

step "resolve base image"
mkdir -p "$CACHE_DIR"
if [[ -z "$BASE_IMAGE" ]]; then
    BASE_IMAGE="$CACHE_DIR/$FEDORA_IMAGE"
    if [[ ! -f "$BASE_IMAGE" ]]; then
        echo "downloading $FEDORA_IMAGE_URL"
        curl -fSL --retry 3 -o "$BASE_IMAGE.part" "$FEDORA_IMAGE_URL"
        mv "$BASE_IMAGE.part" "$BASE_IMAGE"
    fi
fi
[[ -f "$BASE_IMAGE" ]] || fail "base image not found: $BASE_IMAGE"

echo "verifying $BASE_IMAGE"
ACTUAL_SHA256="$(sha256sum "$BASE_IMAGE" | cut -d' ' -f1)"
if [[ "$ACTUAL_SHA256" != "$FEDORA_IMAGE_SHA256" ]]; then
    fail "base image checksum mismatch
  expected $FEDORA_IMAGE_SHA256
  actual   $ACTUAL_SHA256"
fi
echo "base image sha256 ok: $ACTUAL_SHA256"

step "create and grow the guest image"
cp --reflink=auto "$BASE_IMAGE" "$OUTPUT"
chmod u+w "$OUTPUT"
qemu-img resize "$OUTPUT" "${DISK_SIZE_GIB}G"
qemu-img info "$OUTPUT"

step "build the NoCloud provisioning seed"
SEED="$WORK_DIR/seed.img"
cat >"$WORK_DIR/meta-data" <<EOF
instance-id: conary-qemu-guest
local-hostname: conary-qemu-guest
EOF
# cloud-init only opens root SSH access and grows the root filesystem. Package
# installation and verification run over SSH afterwards so their real output is
# part of the build log rather than buried in the guest's cloud-init journal.
cat >"$WORK_DIR/user-data" <<EOF
#cloud-config
disable_root: false
ssh_pwauth: false
growpart:
  mode: auto
  devices: ['/']
  ignore_growroot_disabled: false
resize_rootfs: true
users:
  - name: root
    lock_passwd: true
    ssh_authorized_keys:
      - $PUBLIC_KEY_CONTENT
write_files:
  - path: /etc/ssh/sshd_config.d/00-conary-qemu-test.conf
    permissions: '0600'
    content: |
      PermitRootLogin prohibit-password
      PasswordAuthentication no
EOF
truncate -s 4M "$SEED"
mkfs.vfat -n CIDATA "$SEED" >/dev/null
mcopy -i "$SEED" "$WORK_DIR/meta-data" ::meta-data
mcopy -i "$SEED" "$WORK_DIR/user-data" ::user-data

step "first boot: cloud-init applies access and root growth"
start_guest "$SEED"
wait_for_ssh
guest_ssh 'cloud-init status --wait || true'
guest_ssh 'set -x; systemctl is-system-running || true; df -h /; lsblk'

step "provision the generation toolchain"
guest_ssh "set -euxo pipefail; dnf -y install ${GUEST_PACKAGES[*]}"

step "provision drivers for the running kernel"
guest_ssh "set -euxo pipefail
KVER=\$(uname -r)
PKGS=\"\"
for pkg in ${GUEST_KERNEL_MODULE_PACKAGES[*]}; do
    PKGS=\"\$PKGS \$pkg-\$KVER\"
done
dnf -y install \$PKGS
depmod -a \"\$KVER\""

step "preflight: commands the phase-3 QEMU manifests require"
guest_ssh "set -eu
for cmd in ${REQUIRED_COMMANDS[*]}; do
    if command -v \"\$cmd\" >/dev/null; then
        printf 'ok      %-16s %s\n' \"\$cmd\" \"\$(command -v \"\$cmd\")\"
    else
        printf 'MISSING %s\n' \"\$cmd\"
        missing=1
    fi
done
test -d /usr/lib/dracut && echo 'ok      /usr/lib/dracut' || { echo 'MISSING /usr/lib/dracut'; missing=1; }
test -z \"\${missing:-}\""

step "preflight: packaged generation boot assets"
guest_ssh "set -eu
for path in ${REQUIRED_FILES[*]}; do
    if test -f \"\$path\"; then
        printf 'ok      %s\n' \"\$path\"
    else
        printf 'MISSING %s\n' \"\$path\"
        missing=1
    fi
done
test -z \"\${missing:-}\""

step "preflight: kernel modules the suites depend on"
guest_ssh "set -eu
KVER=\$(uname -r)
BUILTIN=/lib/modules/\$KVER/modules.builtin
for mod in ${REQUIRED_MODULES[*]}; do
    if modinfo \"\$mod\" >/dev/null 2>&1; then
        printf 'ok      %-12s loadable\n' \"\$mod\"
    elif grep -q \"/\$mod\\.ko\" \"\$BUILTIN\" 2>/dev/null; then
        printf 'ok      %-12s builtin\n' \"\$mod\"
    else
        printf 'MISSING %s\n' \"\$mod\"
        missing=1
    fi
done
test -z \"\${missing:-}\""

step "finalize the image"
# cloud-init has no datasource on a harness boot, so leaving it enabled costs
# every suite a datasource search on every boot. The access it configured is
# already persisted in /root/.ssh/authorized_keys.
guest_ssh 'set -eux
touch /etc/cloud/cloud-init.disabled
test -s /root/.ssh/authorized_keys
dnf clean all
rm -rf /var/cache/dnf/*
journalctl --rotate || true
journalctl --vacuum-time=1s || true
fstrim -av || true
sync'

step "power off the guest"
guest_ssh 'systemctl poweroff' || true
wait_for_qemu_exit

step "verification boot without the seed"
start_guest
wait_for_ssh
guest_ssh 'set -eu
echo "uptime: $(uptime -p)"
echo "os-release: $(. /etc/os-release; echo "$NAME $VERSION_ID")"
echo "kernel: $(uname -r)"
echo "system state: $(systemctl is-system-running || true)"
df -h / | tail -1'
guest_ssh 'systemctl poweroff' || true
wait_for_qemu_exit

step "check and compress"
qemu-img check "$OUTPUT"
if [[ "$COMPRESS" == "yes" ]]; then
    qemu-img convert -c -O qcow2 "$OUTPUT" "$OUTPUT.compressed"
    mv "$OUTPUT.compressed" "$OUTPUT"
    qemu-img check "$OUTPUT"
fi

step "result"
qemu-img info "$OUTPUT"
echo "bytes:  $(stat -c '%s' "$OUTPUT")"
echo "sha256: $(sha256sum "$OUTPUT" | cut -d' ' -f1)"
echo "GUEST-IMAGE-BUILD-DONE"
