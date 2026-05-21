#!/bin/bash
# packaging/dracut/90conary/conary-generator.sh
# Pre-pivot hook: mount Conary generation via composefs

SYSROOT="${CONARY_SYSROOT:-/sysroot}"
CMDLINE_FILE="${CONARY_CMDLINE_FILE:-/proc/cmdline}"

expose_generation_usr() {
    usr_source="${SYSROOT}/conary/mnt/usr"
    usr_target="${SYSROOT}/usr"

    mkdir -p "$usr_target"
    if mount --bind "$usr_source" "$usr_target"; then
        mount -o remount,ro "$usr_target" 2>/dev/null || true
        return 0
    fi

    # composefs is overlay-backed; some initramfs environments cannot bind a
    # subdirectory from that mount. Exported generation images create /usr as
    # an empty carrier-root placeholder, so replace only that empty directory.
    if rmdir "$usr_target" 2>/dev/null && ln -s conary/mnt/usr "$usr_target"; then
        return 0
    fi

    echo "conary: failed to expose generation /usr at $usr_target" >&2
    return 1
}

ensure_root_symlink() {
    link_path="${SYSROOT}/$1"
    link_target="$2"

    if [ -e "$link_path" ] || [ -L "$link_path" ]; then
        return 0
    fi

    ln -s "$link_target" "$link_path" || {
        echo "conary: failed to create $link_path -> $link_target" >&2
        return 1
    }
}

read_kernel_value() {
    local key="$1"

    if [ ! -r "$CMDLINE_FILE" ]; then
        return 0
    fi

    for opt in $(cat "$CMDLINE_FILE"); do
        case "$opt" in
            "$key"=*)
                printf '%s\n' "${opt#*=}"
                return 0
                ;;
        esac
    done
}

read_kernel_generation() {
    read_kernel_value conary.generation
}

read_current_generation() {
    local current_link="${SYSROOT}/conary/current"
    local raw_target

    if [ ! -L "$current_link" ]; then
        return 0
    fi

    raw_target=$(readlink "$current_link") || return 0
    basename "$raw_target"
}

prepare_etc_state_base() {
    if [ "$CONARY_CARRIER" = "readonly" ]; then
        mkdir -p "${SYSROOT}/run"
        if ! grep -q " ${SYSROOT}/run " /proc/mounts 2>/dev/null; then
            mount -t tmpfs tmpfs "${SYSROOT}/run" -o mode=0755,nosuid,nodev || {
                echo "conary: failed to mount runtime tmpfs for readonly carrier" >&2
                return 1
            }
        fi
        ETC_STATE_BASE="${SYSROOT}/run/conary/etc-state"
    else
        ETC_STATE_BASE="${SYSROOT}/conary/etc-state"
    fi

    mkdir -p "$ETC_STATE_BASE"
}

prepare_readonly_var_state() {
    [ "$CONARY_CARRIER" = "readonly" ] || return 0

    mkdir -p "${SYSROOT}/var"
    if ! grep -q " ${SYSROOT}/var " /proc/mounts 2>/dev/null; then
        mount -t tmpfs tmpfs "${SYSROOT}/var" -o mode=0755,nosuid,nodev || {
            echo "conary: failed to mount var tmpfs for readonly carrier" >&2
            return 1
        }
    fi
    mkdir -p "${SYSROOT}/var/cache" "${SYSROOT}/var/lib/sshd" "${SYSROOT}/var/log" "${SYSROOT}/var/tmp"
    chmod 1777 "${SYSROOT}/var/tmp"
}

CONARY_CARRIER="$(read_kernel_value conary.carrier)"
CONARY_GEN="$(read_kernel_generation)"
if [ -z "$CONARY_GEN" ]; then
    CONARY_GEN="$(read_current_generation)"
fi

if [ -z "$CONARY_GEN" ]; then
    exit 0  # No generation system configured
fi

prepare_readonly_var_state || exit 1

GEN_DIR="${SYSROOT}/conary/generations/${CONARY_GEN}"
EROFS_IMG="${GEN_DIR}/root.erofs"
CAS_DIR="${SYSROOT}/conary/objects"

# Check for EROFS image (composefs format)
[ -f "$EROFS_IMG" ] || {
    echo "conary: generation ${CONARY_GEN} is missing root.erofs at ${EROFS_IMG}" >&2
    exit 1
}

# Mount composefs at staging point
mkdir -p "${SYSROOT}/conary/mnt"
mount -t composefs "$EROFS_IMG" "${SYSROOT}/conary/mnt" \
    -o "basedir=${CAS_DIR},verity_check=1" 2>/dev/null || \
mount -t composefs "$EROFS_IMG" "${SYSROOT}/conary/mnt" \
    -o "basedir=${CAS_DIR}" || {
    echo "conary: composefs mount failed for $EROFS_IMG" >&2
    exit 1
}

# Bind-mount /usr from composefs tree (read-only)
if [ -d "${SYSROOT}/conary/mnt/usr" ]; then
    expose_generation_usr || exit 1
fi

ensure_root_symlink bin usr/bin || exit 1
ensure_root_symlink lib usr/lib || exit 1
ensure_root_symlink lib64 usr/lib64 || exit 1
ensure_root_symlink sbin usr/sbin || exit 1

# Overlayfs for /etc (writable upper on immutable composefs lower)
if [ -d "${SYSROOT}/conary/mnt/etc" ]; then
    prepare_etc_state_base || exit 1
    ETC_UPPER="${ETC_STATE_BASE}/${CONARY_GEN}"
    ETC_WORK="${ETC_STATE_BASE}/${CONARY_GEN}-work"
    mkdir -p "$ETC_UPPER" "$ETC_WORK"
    mount -t overlay overlay "${SYSROOT}/etc" \
        -o "lowerdir=${SYSROOT}/conary/mnt/etc,upperdir=${ETC_UPPER},workdir=${ETC_WORK}"
fi
