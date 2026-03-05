#!/bin/bash
# packaging/dracut/90conary/conary-generator.sh
# Pre-pivot hook: bind-mount the selected Conary generation

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
        GEN_DIR=$(readlink -f /sysroot/conary/current)
    else
        exit 0  # No generation system configured
    fi
else
    GEN_DIR="/sysroot/conary/generations/${CONARY_GEN}"
fi

# Verify generation exists
if [ ! -d "$GEN_DIR" ]; then
    echo "conary: generation $CONARY_GEN not found, booting without generation" >&2
    exit 0
fi

# Bind-mount generation directories over sysroot
for dir in usr etc; do
    if [ -d "${GEN_DIR}/${dir}" ]; then
        mount --bind "${GEN_DIR}/${dir}" "/sysroot/${dir}"
    fi
done
