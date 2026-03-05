#!/bin/bash
# packaging/dracut/90conary/conary-generator.sh
# Pre-pivot hook: mount Conary generation via composefs

# Read conary.generation=N from kernel cmdline
CONARY_GEN=""
for opt in $(cat /proc/cmdline); do
    case "$opt" in
        conary.generation=*)
            CONARY_GEN="${opt#conary.generation=}"
            ;;
    esac
done

# Fall back to /conary/current symlink
if [ -z "$CONARY_GEN" ]; then
    if [ -L /sysroot/conary/current ]; then
        # Resolve symlink relative to /sysroot (target is absolute e.g. /conary/generations/N)
        RAW_TARGET=$(readlink /sysroot/conary/current)
        GEN_DIR="/sysroot${RAW_TARGET}"
    else
        exit 0  # No generation system configured
    fi
else
    GEN_DIR="/sysroot/conary/generations/${CONARY_GEN}"
fi

EROFS_IMG="${GEN_DIR}/root.erofs"
CAS_DIR="/sysroot/conary/objects"

# Check for EROFS image (composefs format)
if [ ! -f "$EROFS_IMG" ]; then
    # Fall back to legacy bind-mount if generation dir exists but has no EROFS image
    if [ -d "$GEN_DIR" ]; then
        for dir in usr etc; do
            if [ -d "${GEN_DIR}/${dir}" ]; then
                mount --bind "${GEN_DIR}/${dir}" "/sysroot/${dir}"
            fi
        done
    else
        echo "conary: generation not found at $GEN_DIR" >&2
    fi
    exit 0
fi

# Mount composefs at staging point
mkdir -p /sysroot/conary/mnt
mount -t composefs "$EROFS_IMG" /sysroot/conary/mnt \
    -o "basedir=${CAS_DIR},verity_check=1" 2>/dev/null || \
mount -t composefs "$EROFS_IMG" /sysroot/conary/mnt \
    -o "basedir=${CAS_DIR}" || {
    echo "conary: composefs mount failed for $EROFS_IMG" >&2
    exit 1
}

# Bind-mount /usr from composefs tree (read-only)
if [ -d /sysroot/conary/mnt/usr ]; then
    mount --bind /sysroot/conary/mnt/usr /sysroot/usr
    mount -o remount,ro /sysroot/usr
fi

# Overlayfs for /etc (writable upper on immutable composefs lower)
if [ -d /sysroot/conary/mnt/etc ]; then
    mkdir -p /sysroot/conary/etc-state/upper /sysroot/conary/etc-state/work
    mount -t overlay overlay /sysroot/etc \
        -o "lowerdir=/sysroot/conary/mnt/etc,upperdir=/sysroot/conary/etc-state/upper,workdir=/sysroot/conary/etc-state/work"
fi
