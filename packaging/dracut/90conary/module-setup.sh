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
    # Own the complete executable contract of the runtime scripts instead of
    # relying on incidental tools pulled in by neighboring dracut modules.
    inst_multiple \
        basename \
        cat \
        chmod \
        cp \
        grep \
        head \
        ln \
        mkdir \
        modprobe \
        mount \
        mount.composefs \
        readlink \
        rmdir \
        sleep \
        switch_root || return 1
    # conary-init resolves stable /dev/disk links first and guards blkid as a
    # fallback, so it is the only runtime executable that remains optional.
    inst_multiple -o blkid
}
