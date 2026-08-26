#!/usr/bin/env bash
# deploy/hetzner/remi-provision-data.sh -- Build Remi's destructive data tier.

set -euo pipefail

readonly APPLY_TOKEN="REMI_DATA_DESTROY_NVME_TAILS"
readonly DISK0="/dev/nvme0n1"
readonly DISK1="/dev/nvme1n1"
readonly SERIAL0="S3W6NA0M713460"
readonly SERIAL1="S3W6NA0M713525"
readonly DATA_MD="/dev/md2"
readonly DATA_VG="vgdata"
readonly VDO_POOL="vdopool"
readonly VDO_LV="data"
readonly VDO_PHYSICAL_SIZE="1536G"
readonly VDO_LOGICAL_SIZE="3072G"

usage() {
    cat <<EOF
Usage:
  sudo $0 --plan
  sudo $0 --apply $APPLY_TOKEN

--plan validates the installed host and prints the exact pending mutation.
--apply creates partition 3 on both fixed NVMe devices, RAID0 /dev/md2,
LVM VDO, XFS /data, bind mounts, and the root swapfile.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

trim() {
    sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

assert_disk() {
    local disk="$1"
    local expected_serial="$2"
    local actual_serial
    local table_label
    local partition_count

    [[ -b "$disk" ]] || die "missing block device: $disk"
    actual_serial="$(lsblk -dn -o SERIAL "$disk" | trim)"
    [[ "$actual_serial" == "$expected_serial" ]] ||
        die "$disk serial is $actual_serial, expected $expected_serial"

    table_label="$(sfdisk --json "$disk" | jq -r '.partitiontable.label')"
    [[ "$table_label" == "dos" ]] || die "$disk partition table is $table_label, expected dos"

    partition_count="$(sfdisk --json "$disk" | jq '.partitiontable.partitions | length')"
    [[ "$partition_count" == "2" ]] ||
        die "$disk has $partition_count partitions; expected exactly the installed p1 and p2"

    [[ -b "${disk}p1" && -b "${disk}p2" ]] || die "$disk is missing p1 or p2"
    [[ ! -e "${disk}p3" ]] || die "${disk}p3 already exists"
}

partition_end_sector() {
    sfdisk --json "$1" | jq -r '[.partitiontable.partitions[] | .start + .size] | max'
}

unallocated_sectors() {
    local disk="$1"
    local disk_sectors
    local used_end

    disk_sectors="$(blockdev --getsz "$disk")"
    used_end="$(partition_end_sector "$disk")"
    echo $((disk_sectors - used_end))
}

assert_installed_root() {
    local root_source
    local boot_source
    local os_id
    local os_version

    os_id="$(awk -F= '$1 == "ID" {gsub(/"/, "", $2); print $2}' /etc/os-release)"
    os_version="$(awk -F= '$1 == "VERSION_ID" {gsub(/"/, "", $2); print $2}' /etc/os-release)"
    [[ "$os_id:$os_version" == "ubuntu:26.04" ]] ||
        die "this provisioner requires Ubuntu 26.04"

    root_source="$(readlink -f "$(findmnt -n -o SOURCE /)")"
    boot_source="$(readlink -f "$(findmnt -n -o SOURCE /boot)")"
    [[ "$root_source" == "/dev/md1" ]] || die "root is $root_source, expected /dev/md1"
    [[ "$boot_source" == "/dev/md0" ]] || die "/boot is $boot_source, expected /dev/md0"

    grep -q '^md0 : active raid1 ' /proc/mdstat || die "md0 is not active RAID1"
    grep -q '^md1 : active raid1 ' /proc/mdstat || die "md1 is not active RAID1"
    grep -A1 '^md0 :' /proc/mdstat | grep -q '\[UU\]' || die "md0 is degraded"
    grep -A2 '^md1 :' /proc/mdstat | grep -q '\[UU\]' || die "md1 is degraded"
}

assert_unprovisioned() {
    [[ ! -e "$DATA_MD" ]] || die "$DATA_MD already exists"
    ! grep -q '^md2 :' /proc/mdstat || die "md2 is already assembled"
    ! pvs --noheadings "$DATA_MD" >/dev/null 2>&1 || die "$DATA_MD is already an LVM PV"
    ! vgs --noheadings "$DATA_VG" >/dev/null 2>&1 || die "volume group $DATA_VG already exists"
    ! findmnt -rn /data >/dev/null 2>&1 || die "/data is already mounted"
    ! grep -Eq '[[:space:]]/(data|work|conary)[[:space:]]' /etc/fstab ||
        die "/etc/fstab already contains a managed data or bind mount"
    [[ ! -e /swapfile ]] || die "/swapfile already exists"
    ! grep -Eq '^[^#]+[[:space:]]+none[[:space:]]+swap[[:space:]]' /etc/fstab ||
        die "/etc/fstab already contains swap"
}

install_monitoring() {
    local source_dir="$1"

    install -o root -g root -m 0755 \
        "$source_dir/remi-vdo-monitor.sh" /usr/local/sbin/remi-vdo-monitor
    install -o root -g root -m 0644 \
        "$source_dir/../systemd/remi-vdo-monitor.service" \
        /etc/systemd/system/remi-vdo-monitor.service
    install -o root -g root -m 0644 \
        "$source_dir/../systemd/remi-vdo-monitor.timer" \
        /etc/systemd/system/remi-vdo-monitor.timer
    systemctl daemon-reload
    systemctl enable --now remi-vdo-monitor.timer fstrim.timer
    systemctl start remi-vdo-monitor.service
}

main() {
    local mode="${1:---plan}"
    local token="${2:-}"
    local source_dir
    local disk0_tail
    local disk1_tail
    local disk0_start
    local disk1_start
    local data_uuid
    local md2_uuid
    local md2_line

    case "$mode" in
        --plan)
            [[ $# -eq 1 || $# -eq 0 ]] || { usage >&2; exit 2; }
            ;;
        --apply)
            [[ "$token" == "$APPLY_TOKEN" && $# -eq 2 ]] || {
                usage >&2
                exit 2
            }
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac

    ((EUID == 0)) || die "this provisioner must run as root; --plan remains read-only"

    for command in awk blkid blockdev fallocate findmnt jq lsblk lvcreate mdadm \
        mkfs.xfs mkswap parted partprobe pvs readlink sfdisk swapon udevadm \
        update-initramfs vgs; do
        require_command "$command"
    done

    assert_installed_root
    assert_disk "$DISK0" "$SERIAL0"
    assert_disk "$DISK1" "$SERIAL1"
    assert_unprovisioned

    disk0_start="$(partition_end_sector "$DISK0")"
    disk1_start="$(partition_end_sector "$DISK1")"
    disk0_tail="$(unallocated_sectors "$DISK0")"
    disk1_tail="$(unallocated_sectors "$DISK1")"
    [[ "$disk0_start" == "$disk1_start" ]] || die "data partition starts differ"
    [[ "$disk0_tail" == "$disk1_tail" ]] || die "unallocated tail sizes differ"
    ((disk0_tail > 1600000000)) || die "unallocated tails are unexpectedly small"

    cat <<EOF
Validated destructive data-tier plan:
  $DISK0 serial $SERIAL0: create p3 at sector $disk0_start using $disk0_tail remaining sectors
  $DISK1 serial $SERIAL1: create p3 at sector $disk1_start using $disk1_tail remaining sectors
  p3 + p3 -> $DATA_MD RAID0, 512 KiB chunk
  $DATA_MD -> $DATA_VG -> $VDO_POOL ($VDO_PHYSICAL_SIZE physical)
  $VDO_LV ($VDO_LOGICAL_SIZE logical) -> XFS /data
  /data/work -> /work; /data/conary -> /conary
  32 GiB /swapfile on mirrored root
EOF

    [[ "$mode" == "--apply" ]] || exit 0

    source_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    [[ -f "$source_dir/remi-vdo.profile" ]] || die "missing remi-vdo.profile"
    [[ -x "$source_dir/remi-vdo-monitor.sh" ]] || die "missing executable remi-vdo-monitor.sh"
    [[ -f "$source_dir/../systemd/remi-vdo-monitor.service" ]] || die "missing monitor service"
    [[ -f "$source_dir/../systemd/remi-vdo-monitor.timer" ]] || die "missing monitor timer"

    parted --script --align none "$DISK0" unit s mkpart primary "${disk0_start}s" 100%
    parted --script "$DISK0" set 3 raid on
    parted --script --align none "$DISK1" unit s mkpart primary "${disk1_start}s" 100%
    parted --script "$DISK1" set 3 raid on
    partprobe "$DISK0"
    partprobe "$DISK1"
    udevadm settle

    [[ -b "${DISK0}p3" && -b "${DISK1}p3" ]] || die "kernel did not expose both p3 devices"
    [[ "$(blockdev --getsz "${DISK0}p3")" == "$(blockdev --getsz "${DISK1}p3")" ]] ||
        die "created data partitions differ in size"

    mdadm --create "$DATA_MD" --metadata=1.2 --level=0 --raid-devices=2 \
        --chunk=512 "${DISK0}p3" "${DISK1}p3"
    udevadm settle
    mdadm --detail "$DATA_MD" | grep -q 'Raid Level : raid0' || die "md2 is not RAID0"

    md2_uuid="$(mdadm --detail "$DATA_MD" | awk '/UUID :/ {print $3}')"
    [[ -n "$md2_uuid" ]] || die "unable to read md2 UUID"
    if ! grep -q "UUID=$md2_uuid" /etc/mdadm/mdadm.conf; then
        md2_line="$(mdadm --detail --scan | grep "UUID=$md2_uuid")"
        [[ -n "$md2_line" ]] || die "unable to generate md2 mdadm configuration"
        printf '%s\n' "$md2_line" >> /etc/mdadm/mdadm.conf
    fi
    update-initramfs -u

    pvcreate --yes "$DATA_MD"
    vgcreate "$DATA_VG" "$DATA_MD"
    install -o root -g root -m 0644 "$source_dir/remi-vdo.profile" \
        /etc/lvm/profile/remi-vdo.profile
    lvcreate --yes --type vdo --name "$VDO_LV" --size "$VDO_PHYSICAL_SIZE" \
        --virtualsize "$VDO_LOGICAL_SIZE" --metadataprofile remi-vdo \
        "$DATA_VG/$VDO_POOL"

    mkfs.xfs -K -L remi-data "/dev/$DATA_VG/$VDO_LV"
    data_uuid="$(blkid -s UUID -o value "/dev/$DATA_VG/$VDO_LV")"
    [[ -n "$data_uuid" ]] || die "unable to read XFS UUID"

    sed -i -E \
        '\%^[^#]+[[:space:]]+/[[:space:]]+ext4[[:space:]]+% s/[[:space:]]0[[:space:]]+0$/ 0 1/' \
        /etc/fstab
    install -d -o root -g root -m 0755 /data /work /conary
    {
        printf 'UUID=%s /data xfs defaults,noatime 0 2\n' "$data_uuid"
        printf '/data/work /work none bind,x-systemd.requires-mounts-for=/data 0 0\n'
        printf '/data/conary /conary none bind,x-systemd.requires-mounts-for=/data 0 0\n'
    } >> /etc/fstab
    mount /data
    install -d -o root -g root -m 0755 /data/work /data/conary
    mount /work
    mount /conary

    fallocate -l 32G /swapfile
    chmod 0600 /swapfile
    mkswap /swapfile
    printf '/swapfile none swap sw 0 0\n' >> /etc/fstab
    swapon /swapfile

    systemctl daemon-reload
    findmnt --verify --verbose
    install_monitoring "$source_dir"

    echo "Remi data tier provisioned successfully."
    lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS
    lvs -a -o+devices,data_percent,vdo_compression,vdo_deduplication "$DATA_VG"
}

main "$@"
