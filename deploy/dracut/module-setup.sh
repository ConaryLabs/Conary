#!/bin/bash
# deploy/dracut/module-setup.sh
# Dracut module for composefs-native Conary boot

check() {
    require_binaries mount.composefs conary || return 1
    return 0
}

depends() {
    echo "fs-lib"
}

install() {
    inst_binary /usr/local/bin/conary
    inst_hook pre-mount 50 "$moddir/mount-conary.sh"
}
