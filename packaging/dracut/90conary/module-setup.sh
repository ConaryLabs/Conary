#!/bin/bash
# packaging/dracut/90conary/module-setup.sh
# Dracut module for Conary generation switching (composefs)

check() {
    # Only include if conary generations exist
    [ -d "${dracutsysrootdir-}/conary/generations" ] && return 0
    [ -d /conary/generations ] && return 0
    return 255
}

depends() {
    echo "bash base rootfs-block fs-lib"
    return 0
}

install_conary_script() {
    src="$1"
    dest="$2"

    mkdir -p "${initdir}/$(dirname "$dest")"
    cp "$src" "${initdir}/${dest}"
    chmod 0755 "${initdir}/${dest}"
}

install() {
    install_conary_script "$moddir/conary-init.sh" "/init"
    install_conary_script "$moddir/conary-generator.sh" "/sbin/conary-generator"
    install_conary_script "$moddir/conary-generator.sh" \
        "/var/lib/dracut/hooks/pre-pivot/90-conary-generator.sh"
    inst_multiple -o blkid grep head modprobe switch_root
    # Include mount.composefs if available
    inst_multiple -o mount.composefs
}
