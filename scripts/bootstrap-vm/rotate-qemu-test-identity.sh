#!/usr/bin/env bash
# scripts/bootstrap-vm/rotate-qemu-test-identity.sh
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: rotate-qemu-test-identity.sh --source PATH --output PATH --private-key PATH [OPTIONS]

Create a new versioned qcow2 test fixture by replacing only the guest root
authorized_keys file with a newly generated disposable test identity. The root
filesystem can also be expanded when current full-adoption proof needs more CAS
space. The source image is never modified. The output image and private/public
key files must not already exist.

Options:
  --source PATH       Existing source qcow2 image
  --output PATH       New versioned qcow2 image
  --private-key PATH  Host path for the generated private test key
  --disk-size-gib N   Expand the disk and final CONARY_ROOT partition to N GiB
  --comment TEXT      SSH public-key comment (default: conary-qemu-test)
  --help              Show this help text
EOF
}

SOURCE=""
OUTPUT=""
PRIVATE_KEY=""
DISK_SIZE_GIB=""
KEY_COMMENT="conary-qemu-test"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --source)
            SOURCE="$2"
            shift 2
            ;;
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        --disk-size-gib)
            DISK_SIZE_GIB="$2"
            shift 2
            ;;
        --comment)
            KEY_COMMENT="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

log() {
    printf '[rotate-qemu-test-identity] %s\n' "$*"
}

fail() {
    echo "[rotate-qemu-test-identity] ERROR: $*" >&2
    exit 1
}

require_cmd() {
    local command
    for command in "$@"; do
        command -v "$command" >/dev/null 2>&1 || fail "$command is required"
    done
}

[[ -n "$SOURCE" ]] || fail "--source is required"
[[ -n "$OUTPUT" ]] || fail "--output is required"
[[ -n "$PRIVATE_KEY" ]] || fail "--private-key is required"
[[ -f "$SOURCE" ]] || fail "source image does not exist: $SOURCE"
[[ "$SOURCE" != "$OUTPUT" ]] || fail "source and output paths must differ"
[[ ! -e "$OUTPUT" ]] || fail "output already exists: $OUTPUT"
[[ ! -e "$PRIVATE_KEY" ]] || fail "private key already exists: $PRIVATE_KEY"
[[ ! -e "$PRIVATE_KEY.pub" ]] || fail "public key already exists: $PRIVATE_KEY.pub"
[[ -z "$DISK_SIZE_GIB" || "$DISK_SIZE_GIB" =~ ^[1-9][0-9]*$ ]] \
    || fail "--disk-size-gib must be a positive integer"

require_cmd \
    cmp dd debugfs e2fsck fallocate grep install jq mktemp mv qemu-img resize2fs rm \
    sfdisk sgdisk ssh-keygen stat truncate

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/conary-qemu-identity.XXXXXX")"
output_tmp="${OUTPUT}.rotating.$$"

cleanup() {
    rm -rf "$work_dir"
    rm -f "$output_tmp"
}
trap cleanup EXIT

mkdir -p "$(dirname "$OUTPUT")" "$(dirname "$PRIVATE_KEY")"

raw_image="$work_dir/source.raw"
root_image="$work_dir/root.ext4"
generated_key="$work_dir/conaryos-test-key"
generated_public_key="$generated_key.pub"
verified_authorized_keys="$work_dir/authorized_keys"

log "Converting the source qcow2 to a temporary raw image"
qemu-img convert -O raw "$SOURCE" "$raw_image"

partition_json="$(sfdisk --json "$raw_image")"
sector_size="$(jq -er '.partitiontable.sectorsize' <<<"$partition_json")"
root_start="$(jq -er '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .start' <<<"$partition_json")"
root_size="$(jq -er '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .size' <<<"$partition_json")"
root_node="$(jq -er '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .node' <<<"$partition_json")"
root_type="$(jq -er '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .type' <<<"$partition_json")"
root_uuid="$(jq -er '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .uuid' <<<"$partition_json")"
root_name="$(jq -er '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .name' <<<"$partition_json")"
root_attrs="$(jq -r '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .attrs // ""' <<<"$partition_json")"
root_partition_number="${root_node#"$raw_image"}"

[[ "$sector_size" =~ ^[0-9]+$ ]] || fail "invalid root partition sector size"
[[ "$root_start" =~ ^[0-9]+$ ]] || fail "invalid root partition start"
[[ "$root_size" =~ ^[0-9]+$ ]] || fail "invalid root partition size"
[[ "$root_partition_number" =~ ^[1-9][0-9]*$ ]] || fail "cannot determine CONARY_ROOT partition number"

if [[ -n "$DISK_SIZE_GIB" ]]; then
    current_size_bytes="$(stat -c '%s' "$raw_image")"
    requested_size_bytes="$((DISK_SIZE_GIB * 1024 * 1024 * 1024))"
    [[ "$requested_size_bytes" -gt "$current_size_bytes" ]] \
        || fail "--disk-size-gib must exceed the source disk size"

    root_end="$((root_start + root_size))"
    last_partition_end="$(
        jq -er '[.partitiontable.partitions[] | .start + .size] | max' <<<"$partition_json"
    )"
    [[ "$root_end" -eq "$last_partition_end" ]] \
        || fail "CONARY_ROOT must be the final partition before disk expansion"

    log "Expanding the disk and CONARY_ROOT filesystem to ${DISK_SIZE_GIB} GiB"
    truncate -s "$requested_size_bytes" "$raw_image"
    sgdisk \
        -e \
        -d "$root_partition_number" \
        -n "${root_partition_number}:${root_start}:0" \
        -t "${root_partition_number}:${root_type}" \
        -c "${root_partition_number}:${root_name}" \
        -u "${root_partition_number}:${root_uuid}" \
        "$raw_image" >/dev/null
    if [[ -n "$root_attrs" ]]; then
        sfdisk --part-attrs "$raw_image" "$root_partition_number" "$root_attrs"
    fi

    partition_json="$(sfdisk --json "$raw_image")"
    root_size="$(jq -er '.partitiontable.partitions[] | select(.name == "CONARY_ROOT") | .size' <<<"$partition_json")"
fi

log "Extracting the CONARY_ROOT partition"
dd \
    if="$raw_image" \
    of="$root_image" \
    bs="$sector_size" \
    skip="$root_start" \
    count="$root_size" \
    conv=sparse \
    iflag=fullblock \
    status=none

if [[ -n "$DISK_SIZE_GIB" ]]; then
    e2fsck_status=0
    e2fsck -pf "$root_image" || e2fsck_status=$?
    [[ "$e2fsck_status" -le 1 ]] \
        || fail "CONARY_ROOT filesystem check failed with status $e2fsck_status"
    resize2fs "$root_image" >/dev/null
fi

if ! debugfs -R 'stat /root/.ssh/authorized_keys' "$root_image" 2>&1 | grep -q 'Inode:'; then
    fail "source image does not contain /root/.ssh/authorized_keys"
fi

log "Generating a new disposable guest SSH identity"
ssh-keygen -q -t ed25519 -N "" -C "$KEY_COMMENT" -f "$generated_key"

log "Replacing the guest authorized_keys file"
debugfs -w -R 'rm /root/.ssh/authorized_keys' "$root_image" >/dev/null 2>&1
debugfs -w -R "write $generated_public_key /root/.ssh/authorized_keys" "$root_image" >/dev/null 2>&1
debugfs -w -R 'set_inode_field /root/.ssh/authorized_keys mode 0100600' "$root_image" >/dev/null 2>&1
debugfs -w -R 'set_inode_field /root/.ssh/authorized_keys uid 0' "$root_image" >/dev/null 2>&1
debugfs -w -R 'set_inode_field /root/.ssh/authorized_keys gid 0' "$root_image" >/dev/null 2>&1
debugfs -R "dump /root/.ssh/authorized_keys $verified_authorized_keys" "$root_image" >/dev/null 2>&1
cmp "$generated_public_key" "$verified_authorized_keys"

log "Writing the updated root partition back into the raw image"
fallocate \
    --punch-hole \
    --keep-size \
    --offset "$((root_start * sector_size))" \
    --length "$((root_size * sector_size))" \
    "$raw_image"
dd \
    if="$root_image" \
    of="$raw_image" \
    bs="$sector_size" \
    seek="$root_start" \
    count="$root_size" \
    conv=notrunc,sparse \
    iflag=fullblock \
    status=none

log "Compressing the new versioned qcow2 image"
qemu-img convert -c -O qcow2 "$raw_image" "$output_tmp"
qemu-img check "$output_tmp" >/dev/null
chmod 0644 "$output_tmp"
mv "$output_tmp" "$OUTPUT"

install -m 0600 "$generated_key" "$PRIVATE_KEY"
install -m 0644 "$generated_public_key" "$PRIVATE_KEY.pub"

log "Created image: $OUTPUT"
log "Created private test key: $PRIVATE_KEY"
log "Created public test key: $PRIVATE_KEY.pub"
ssh-keygen -lf "$PRIVATE_KEY"
