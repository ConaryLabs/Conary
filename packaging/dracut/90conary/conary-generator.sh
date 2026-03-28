#!/bin/bash
# packaging/dracut/90conary/conary-generator.sh
# Pre-pivot hook: mount Conary generation via composefs

SYSROOT="${CONARY_SYSROOT:-/sysroot}"
CMDLINE_FILE="${CONARY_CMDLINE_FILE:-/proc/cmdline}"

read_kernel_generation() {
    if [ ! -r "$CMDLINE_FILE" ]; then
        return 0
    fi

    for opt in $(cat "$CMDLINE_FILE"); do
        case "$opt" in
            conary.generation=*)
                printf '%s\n' "${opt#conary.generation=}"
                return 0
                ;;
        esac
    done
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

CONARY_GEN="$(read_kernel_generation)"
if [ -z "$CONARY_GEN" ]; then
    CONARY_GEN="$(read_current_generation)"
fi

if [ -z "$CONARY_GEN" ]; then
    exit 0  # No generation system configured
fi

GEN_DIR="${SYSROOT}/conary/generations/${CONARY_GEN}"
EROFS_IMG="${GEN_DIR}/root.erofs"
CAS_DIR="${SYSROOT}/conary/objects"

# Check for EROFS image (composefs format)
if [ ! -f "$EROFS_IMG" ]; then
    # Fall back to legacy bind-mount if generation dir exists but has no EROFS image
    if [ -d "$GEN_DIR" ]; then
        for dir in usr etc; do
            if [ -d "${GEN_DIR}/${dir}" ]; then
                mount --bind "${GEN_DIR}/${dir}" "${SYSROOT}/${dir}"
            fi
        done
    else
        echo "conary: generation not found at $GEN_DIR" >&2
    fi
    exit 0
fi

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
    mount --bind "${SYSROOT}/conary/mnt/usr" "${SYSROOT}/usr"
    mount -o remount,ro "${SYSROOT}/usr"
fi

# Overlayfs for /etc (writable upper on immutable composefs lower)
if [ -d "${SYSROOT}/conary/mnt/etc" ]; then
    ETC_UPPER="${SYSROOT}/conary/etc-state/${CONARY_GEN}"
    ETC_WORK="${SYSROOT}/conary/etc-state/${CONARY_GEN}-work"
    mkdir -p "$ETC_UPPER" "$ETC_WORK"
    mount -t overlay overlay "${SYSROOT}/etc" \
        -o "lowerdir=${SYSROOT}/conary/mnt/etc,upperdir=${ETC_UPPER},workdir=${ETC_WORK}"
fi
