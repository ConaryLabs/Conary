#!/bin/sh
# packaging/dracut/90conary/conary-init.sh
# Minimal Conary initramfs entrypoint for exported composefs generations.

PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH

SYSROOT="${CONARY_SYSROOT:-/sysroot}"
CMDLINE_FILE="${CONARY_CMDLINE_FILE:-/proc/cmdline}"

msg() {
    echo "conary-init: $*" >&2
}

die() {
    msg "$*"
    exec sh
}

is_mounted() {
    grep -q " $1 " /proc/mounts 2>/dev/null
}

mount_once() {
    fstype="$1"
    source="$2"
    target="$3"
    options="$4"

    mkdir -p "$target"
    is_mounted "$target" && return 0
    mount -t "$fstype" -o "$options" "$source" "$target"
}

cmdline_value() {
    key="$1"

    [ -r "$CMDLINE_FILE" ] || return 0
    for opt in $(cat "$CMDLINE_FILE"); do
        case "$opt" in
            "$key"=*)
                printf '%s\n' "${opt#*=}"
                return 0
                ;;
        esac
    done
}

resolve_root_device() {
    root_spec="$1"

    case "$root_spec" in
        PARTLABEL=*)
            label="${root_spec#PARTLABEL=}"
            if [ -e "/dev/disk/by-partlabel/$label" ]; then
                readlink -f "/dev/disk/by-partlabel/$label"
                return 0
            fi
            if command -v blkid >/dev/null 2>&1; then
                blkid -t "PARTLABEL=$label" -o device 2>/dev/null | head -n 1
                return 0
            fi
            ;;
        UUID=*)
            uuid="${root_spec#UUID=}"
            if [ -e "/dev/disk/by-uuid/$uuid" ]; then
                readlink -f "/dev/disk/by-uuid/$uuid"
                return 0
            fi
            if command -v blkid >/dev/null 2>&1; then
                blkid -t "UUID=$uuid" -o device 2>/dev/null | head -n 1
                return 0
            fi
            ;;
        LABEL=*)
            label="${root_spec#LABEL=}"
            if [ -e "/dev/disk/by-label/$label" ]; then
                readlink -f "/dev/disk/by-label/$label"
                return 0
            fi
            if command -v blkid >/dev/null 2>&1; then
                blkid -t "LABEL=$label" -o device 2>/dev/null | head -n 1
                return 0
            fi
            ;;
        /dev/*)
            printf '%s\n' "$root_spec"
            return 0
            ;;
    esac
}

mount_runtime_api() {
    mount_once proc proc /proc nosuid,noexec,nodev || die "failed to mount /proc"
    mount_once sysfs sysfs /sys nosuid,noexec,nodev || die "failed to mount /sys"
    mount_once devtmpfs devtmpfs /dev mode=0755,nosuid || die "failed to mount /dev"
    mount_once tmpfs tmpfs /run mode=0755,nosuid,nodev || die "failed to mount /run"
    mkdir -p /dev/pts
    is_mounted /dev/pts || mount -t devpts -o gid=5,mode=620 devpts /dev/pts 2>/dev/null || true
}

load_root_modules() {
    for module in virtio_pci virtio_blk sd_mod ahci ata_piix ext4 iso9660 erofs overlay composefs; do
        modprobe "$module" 2>/dev/null || true
    done
}

mount_root() {
    root_spec="$(cmdline_value root)"
    root_spec="${root_spec:-PARTLABEL=CONARY_ROOT}"
    rootfstype="$(cmdline_value rootfstype)"
    rootfstype="${rootfstype:-ext4}"
    rootflags="$(cmdline_value rootflags)"
    carrier="$(cmdline_value conary.carrier)"
    if [ -z "$rootflags" ]; then
        if [ "$carrier" = "readonly" ]; then
            rootflags="ro"
        else
            rootflags="rw"
        fi
    fi

    mkdir -p "$SYSROOT"
    for _attempt in 1 2 3 4 5 6 7 8 9 10; do
        root_device="$(resolve_root_device "$root_spec")"
        root_device="${root_device:-$root_spec}"
        if mount -t "$rootfstype" -o "$rootflags" "$root_device" "$SYSROOT"; then
            return 0
        fi
        sleep 1
    done

    die "failed to mount root $root_spec as $rootfstype"
}

mount_readonly_runtime_state() {
    carrier="$(cmdline_value conary.carrier)"
    [ "$carrier" = "readonly" ] || return 0

    mkdir -p "$SYSROOT/run"
    mount_once tmpfs tmpfs "$SYSROOT/run" mode=0755,nosuid,nodev || \
        die "failed to mount readonly carrier runtime tmpfs"
    mkdir -p "$SYSROOT/run/conary/etc-state"

    mount_once tmpfs tmpfs "$SYSROOT/var" mode=0755,nosuid,nodev || \
        die "failed to mount readonly carrier var tmpfs"
    mkdir -p "$SYSROOT/var/cache" "$SYSROOT/var/lib/sshd" "$SYSROOT/var/log" "$SYSROOT/var/tmp"
    chmod 1777 "$SYSROOT/var/tmp"
}

mount_runtime_api
load_root_modules
mount_root
mount_readonly_runtime_state

if [ -x /sbin/conary-generator ]; then
    /sbin/conary-generator || die "generation activation failed"
else
    die "missing /sbin/conary-generator"
fi

exec switch_root "$SYSROOT" /sbin/init
die "switch_root failed"
