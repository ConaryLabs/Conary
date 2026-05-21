#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_SCRIPT="$SCRIPT_DIR/validate-selfhost-vm.sh"

TMPDIR_ROOT="$(mktemp -d)"
FAKEBIN="$TMPDIR_ROOT/fakebin"
FAKE_PROJECT="$TMPDIR_ROOT/project"
FAKE_SCRIPT_DIR="$FAKE_PROJECT/scripts/bootstrap-vm"
TARGET_SCRIPT="$FAKE_SCRIPT_DIR/validate-selfhost-vm.sh"
WORK_DIR="$TMPDIR_ROOT/work"
LOGS_DIR="$WORK_DIR/vm-selfhost/logs"
INPUTS_DIR="$WORK_DIR/vm-selfhost/inputs"
KEYS_DIR="$WORK_DIR/vm-selfhost/keys"
OUTPUT_DIR="$WORK_DIR/vm-selfhost/output"
WORKSPACE_TARBALL="$INPUTS_DIR/conary-workspace.tar.gz"
WORKSPACE_SHA256="$INPUTS_DIR/conary-workspace.tar.gz.sha256"
QEMU_LOG="$TMPDIR_ROOT/qemu.log"
SSH_LOG="$TMPDIR_ROOT/ssh.log"
SCP_LOG="$TMPDIR_ROOT/scp.log"
OVMF_CODE="$TMPDIR_ROOT/OVMF_CODE.fd"
OVMF_VARS_TEMPLATE="$TMPDIR_ROOT/OVMF_VARS.fd"

cleanup() {
    rm -rf "$TMPDIR_ROOT"
}
trap cleanup EXIT

mkdir -p "$FAKEBIN" "$FAKE_SCRIPT_DIR" "$LOGS_DIR" "$INPUTS_DIR" "$KEYS_DIR" "$OUTPUT_DIR"
cp "$SOURCE_SCRIPT" "$TARGET_SCRIPT"
printf 'fake-ovmf-code\n' >"$OVMF_CODE"
printf 'fake-ovmf-vars\n' >"$OVMF_VARS_TEMPLATE"
printf 'fake qcow2\n' >"$OUTPUT_DIR/conaryos-selfhost-x86_64.qcow2"
printf 'fake private key\n' >"$KEYS_DIR/selfhost_ed25519"
cat >"$FAKE_SCRIPT_DIR/guest-validate.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
chmod +x "$FAKE_SCRIPT_DIR/guest-validate.sh"

git -C "$FAKE_PROJECT" init -q
git -C "$FAKE_PROJECT" config user.email "conary-test@example.invalid"
git -C "$FAKE_PROJECT" config user.name "Conary Test"
git -C "$FAKE_PROJECT" add scripts/bootstrap-vm/validate-selfhost-vm.sh scripts/bootstrap-vm/guest-validate.sh
git -C "$FAKE_PROJECT" commit -qm "seed fake project"
(
    cd "$FAKE_PROJECT"
    LC_ALL=C git ls-files -z --cached --modified --others --exclude-standard \
        | sort -z \
        | tar \
            --null \
            --no-recursion \
            --files-from=- \
            --transform='s,^,conary-workspace/,' \
            --mtime='UTC 1970-01-01' \
            --owner=0 \
            --group=0 \
            --numeric-owner \
            -cf - \
        | gzip -n >"$WORKSPACE_TARBALL"
)
sha256sum "$WORKSPACE_TARBALL" | awk '{print $1}' >"$WORKSPACE_SHA256"

cat >"$FAKEBIN/qemu-system-x86_64" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$TEST_QEMU_LOG"
exit 0
EOF
chmod +x "$FAKEBIN/qemu-system-x86_64"

cat >"$FAKEBIN/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$TEST_SSH_LOG"
exit 0
EOF
chmod +x "$FAKEBIN/ssh"

cat >"$FAKEBIN/scp" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$TEST_SCP_LOG"
exit 0
EOF
chmod +x "$FAKEBIN/scp"

export PATH="$FAKEBIN:$PATH"
export TEST_QEMU_LOG="$QEMU_LOG"
export TEST_SSH_LOG="$SSH_LOG"
export TEST_SCP_LOG="$SCP_LOG"
export CONARY_BOOTSTRAP_OVMF_CODE="$OVMF_CODE"
export CONARY_BOOTSTRAP_OVMF_VARS_TEMPLATE="$OVMF_VARS_TEMPLATE"
export CONARY_BOOTSTRAP_QEMU_CPU="max"

bash "$TARGET_SCRIPT" \
    --work-dir "$WORK_DIR" \
    --repo-name fedora-remi \
    --repo-url https://remi.conary.io \
    --remi-endpoint https://remi.conary.io \
    --remi-distro fedora

assert_contains() {
    local file="$1"
    local needle="$2"
    if ! grep -Fq -- "$needle" "$file"; then
        echo "expected '$needle' in $file" >&2
        exit 1
    fi
}

assert_contains "$QEMU_LOG" "-machine q35"
assert_contains "$QEMU_LOG" "-drive if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
assert_contains "$QEMU_LOG" "-drive if=pflash,format=raw,file=$LOGS_DIR/OVMF_VARS.fd"
assert_contains "$QEMU_LOG" "-drive file=$OUTPUT_DIR/conaryos-selfhost-x86_64.qcow2,format=qcow2,if=virtio"
assert_contains "$QEMU_LOG" "-cpu max"

test -f "$LOGS_DIR/OVMF_VARS.fd"
cmp -s "$OVMF_VARS_TEMPLATE" "$LOGS_DIR/OVMF_VARS.fd"

assert_contains "$SSH_LOG" "-p 2222 root@127.0.0.1 true"
assert_contains "$SCP_LOG" "$INPUTS_DIR/conary-workspace.tar.gz"

printf 'badbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadb\n' \
    >"$WORKSPACE_SHA256"
: >"$QEMU_LOG"
STALE_STDERR="$TMPDIR_ROOT/stale.err"
if bash "$TARGET_SCRIPT" \
    --work-dir "$WORK_DIR" \
    --repo-name fedora-remi \
    --repo-url https://remi.conary.io \
    --remi-endpoint https://remi.conary.io \
    --remi-distro fedora 2>"$STALE_STDERR"; then
    echo "expected stale workspace validation to fail before QEMU" >&2
    exit 1
fi
assert_contains "$STALE_STDERR" "workspace tarball checksum mismatch"
if [[ -s "$QEMU_LOG" ]]; then
    echo "stale workspace validation launched QEMU unexpectedly" >&2
    exit 1
fi

echo "validate-selfhost-vm UEFI launch test passed"
