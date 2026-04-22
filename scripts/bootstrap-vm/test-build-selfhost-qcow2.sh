#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_SCRIPT="$SCRIPT_DIR/build-selfhost-qcow2.sh"

TMPDIR_ROOT="$(mktemp -d)"
FAKEBIN="$TMPDIR_ROOT/fakebin"
FAKE_PROJECT="$TMPDIR_ROOT/project"
FAKE_SCRIPT_DIR="$FAKE_PROJECT/scripts/bootstrap-vm"
TARGET_SCRIPT="$FAKE_SCRIPT_DIR/build-selfhost-qcow2.sh"
WORK_DIR="$TMPDIR_ROOT/work"
CONARY_LOG="$TMPDIR_ROOT/conary.log"
ROOTFUL_LOG="$TMPDIR_ROOT/rootful.log"
GIT_LOG="$TMPDIR_ROOT/git.log"
TAR_LIST="$TMPDIR_ROOT/tar.list"
FAKE_CONARY="$TMPDIR_ROOT/fake-conary"

cleanup() {
    rm -rf "$TMPDIR_ROOT"
}
trap cleanup EXIT

mkdir -p "$FAKEBIN" "$FAKE_PROJECT" "$FAKE_SCRIPT_DIR" "$WORK_DIR"
cp "$SOURCE_SCRIPT" "$TARGET_SCRIPT"
printf 'hello bootstrap\n' >"$FAKE_PROJECT/hello.txt"
printf 'modified working tree\n' >"$FAKE_PROJECT/modified.txt"
printf 'untracked working tree\n' >"$FAKE_PROJECT/untracked.txt"

cat >"$FAKEBIN/git" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"$TEST_GIT_LOG"

if [[ "$1" == "-C" ]]; then
    shift 2
fi

case "${1:-}" in
    ls-files)
        printf 'hello.txt\0modified.txt\0untracked.txt\0'
        ;;
    diff|archive)
        echo "unexpected git invocation: $*" >&2
        exit 1
        ;;
    *)
        echo "unexpected git invocation: $*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$FAKEBIN/git"

cat >"$FAKEBIN/fake-rootful-runner" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$TEST_ROOTFUL_LOG"
"$@"
EOF
chmod +x "$FAKEBIN/fake-rootful-runner"

cat >"$FAKE_CONARY" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$TEST_CONARY_LOG"
exit 0
EOF
chmod +x "$FAKE_CONARY"

export PATH="$FAKEBIN:$PATH"
export TEST_CONARY_LOG="$CONARY_LOG"
export TEST_ROOTFUL_LOG="$ROOTFUL_LOG"
export TEST_GIT_LOG="$GIT_LOG"
export CONARY_BOOTSTRAP_ROOTFUL_RUNNER="$FAKEBIN/fake-rootful-runner"

bash "$TARGET_SCRIPT" --work-dir "$WORK_DIR" --conary-bin "$FAKE_CONARY"

assert_contains() {
    local file="$1"
    local needle="$2"
    if ! grep -Fq -- "$needle" "$file"; then
        echo "expected '$needle' in $file" >&2
        exit 1
    fi
}

assert_not_contains() {
    local file="$1"
    local needle="$2"
    if grep -Fq -- "$needle" "$file"; then
        echo "did not expect '$needle' in $file" >&2
        exit 1
    fi
}

assert_contains "$CONARY_LOG" "bootstrap init --work-dir $WORK_DIR --target x86_64"
assert_contains "$CONARY_LOG" "bootstrap cross-tools --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$CONARY_LOG" "bootstrap temp-tools --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$CONARY_LOG" "bootstrap system --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$CONARY_LOG" "bootstrap config --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$CONARY_LOG" "bootstrap tier2 --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$CONARY_LOG" "bootstrap guest-profile --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root --public-key $WORK_DIR/vm-selfhost/keys/selfhost_ed25519.pub"
assert_contains "$CONARY_LOG" "bootstrap image --work-dir $WORK_DIR --output $WORK_DIR/vm-selfhost/output/conaryos-selfhost-x86_64.qcow2 --format qcow2 --size 32G"

assert_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap temp-tools --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap system --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap tier2 --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$ROOTFUL_LOG" "chown -R"

assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap init"
assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap cross-tools"
assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap config"
assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap guest-profile"
assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap image --work-dir $WORK_DIR --output $WORK_DIR/vm-selfhost/output/conaryos-selfhost-x86_64.qcow2 --format qcow2 --size 32G"

assert_contains "$GIT_LOG" "ls-files -z --cached --modified --others --exclude-standard"
assert_not_contains "$GIT_LOG" " archive "
assert_not_contains "$GIT_LOG" " diff "

test -f "$WORK_DIR/vm-selfhost/inputs/conary-workspace.tar.gz"
test -f "$WORK_DIR/vm-selfhost/inputs/conary-workspace.tar.gz.sha256"
test -f "$WORK_DIR/vm-selfhost/keys/selfhost_ed25519"
test -f "$WORK_DIR/vm-selfhost/keys/selfhost_ed25519.pub"

tar -tzf "$WORK_DIR/vm-selfhost/inputs/conary-workspace.tar.gz" >"$TAR_LIST"
assert_contains "$TAR_LIST" "conary-workspace/hello.txt"
assert_contains "$TAR_LIST" "conary-workspace/modified.txt"
assert_contains "$TAR_LIST" "conary-workspace/untracked.txt"

actual_sha256="$(sha256sum "$WORK_DIR/vm-selfhost/inputs/conary-workspace.tar.gz" | awk '{print $1}')"
expected_sha256="$(tr -d ' \n' < "$WORK_DIR/vm-selfhost/inputs/conary-workspace.tar.gz.sha256")"
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "workspace tarball checksum sidecar mismatch" >&2
    exit 1
fi

echo "build-selfhost-qcow2 working-tree tarball test passed"
