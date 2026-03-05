#!/bin/bash
# packaging/dracut/90conary/module-setup.sh
# Dracut module for Conary generation switching

check() {
    # Only include if conary generations exist
    [ -d /conary/generations ] && return 0
    return 255
}

depends() {
    return 0
}

install() {
    inst_hook pre-pivot 90 "$moddir/conary-generator.sh"
}
