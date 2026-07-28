#!/bin/sh
# apps/conary/tests/fixtures/supported-host-generation-export/conary-qemu-test-access/stage-links.sh
#
# Create the unit-enablement symlinks in the CCS stage tree.
#
# These are shipped as ordinary package files, not produced by a preset or a
# scriptlet, so the fixture's enablement set is exactly what is written here.
# They live in this script rather than in the checked-in stage tree because the
# QEMU harness copies fixtures with `scp -r`, which materializes symlinks as
# copies of their targets; a checked-in link would silently arrive in the guest
# as a regular file. `conary ccs build` records them as payload symlinks.
#
# /etc/systemd/system is the enablement location systemctl writes to, so these
# never collide with the /usr paths systemd and openssh-server own. A dangling
# link is logged and skipped by systemd rather than being fatal, so the set is
# the full enablement systemctl would create, not a guessed minimum.
#
# Usage: sh stage-links.sh <stage-dir>

set -eu

stage="${1:?usage: stage-links.sh <stage-dir>}"
test -d "$stage" || {
    echo "stage-links.sh: stage directory $stage does not exist" >&2
    exit 1
}

link() {
    target="$1"
    link_path="$stage/$2"
    mkdir -p "$(dirname "$link_path")"
    rm -f "$link_path"
    ln -s "$target" "$link_path"
}

# Harness SSH.
link /usr/lib/systemd/system/sshd.service \
    etc/systemd/system/multi-user.target.wants/sshd.service

# DHCP on the QEMU user-mode NIC.
link /usr/lib/systemd/system/systemd-networkd.service \
    etc/systemd/system/multi-user.target.wants/systemd-networkd.service
link /usr/lib/systemd/system/systemd-networkd.socket \
    etc/systemd/system/sockets.target.wants/systemd-networkd.socket
link /usr/lib/systemd/system/systemd-networkd.service \
    etc/systemd/system/dbus-org.freedesktop.network1.service

# Resolution, enabled the same way systemctl would.
link /usr/lib/systemd/system/systemd-resolved.service \
    etc/systemd/system/multi-user.target.wants/systemd-resolved.service
link /usr/lib/systemd/system/systemd-resolved.service \
    etc/systemd/system/dbus-org.freedesktop.resolve1.service
