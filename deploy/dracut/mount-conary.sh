#!/bin/bash
# deploy/dracut/mount-conary.sh
# Mount composefs generation at boot

CONARY_ROOT="/conary"

# Run conary's built-in recovery which handles the 4-step fallback:
#   1. Mount current generation if EROFS image is valid
#   2. Rebuild from DB state if image is missing/truncated
#   3. Scan for most recent intact EROFS image on disk
#   4. Error if nothing works
/usr/local/bin/conary system generation recover || {
    echo "FATAL: Conary generation recovery failed" >&2
    exit 1
}
