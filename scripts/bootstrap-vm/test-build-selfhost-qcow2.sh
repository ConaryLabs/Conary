#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TARGET_SCRIPT="$SCRIPT_DIR/build-selfhost-qcow2.sh"

TMPDIR_ROOT="$(mktemp -d)"
FAKEBIN="$TMPDIR_ROOT/fakebin"
FIXTURE_SRC="$TMPDIR_ROOT/archive-src"
WORK_DIR="$TMPDIR_ROOT/work"
CONARY_LOG="$TMPDIR_ROOT/conary.log"
ROOTFUL_LOG="$TMPDIR_ROOT/rootful.log"
FAKE_CONARY="$TMPDIR_ROOT/fake-conary"

cleanup() {
    rm -rf "$TMPDIR_ROOT"
}
trap cleanup EXIT

mkdir -p "$FAKEBIN" "$FIXTURE_SRC" "$WORK_DIR"
printf 'hello bootstrap\n' >"$FIXTURE_SRC/hello.txt"

cat >"$FAKEBIN/git" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "-C" ]]; then
    shift 2
fi

case "${1:-}" in
    diff)
        exit 0
        ;;
    archive)
        tar -cf - --transform 's,^,conary-workspace/,' -C "$TEST_GIT_ARCHIVE_SRC" hello.txt
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
export TEST_GIT_ARCHIVE_SRC="$FIXTURE_SRC"
export TEST_CONARY_LOG="$CONARY_LOG"
export TEST_ROOTFUL_LOG="$ROOTFUL_LOG"
export CONARY_BOOTSTRAP_ROOTFUL_RUNNER="$FAKEBIN/fake-rootful-runner"

bash "$TARGET_SCRIPT" --work-dir "$WORK_DIR" --conary-bin "$FAKE_CONARY"

assert_contains() {
    local file="$1"
    local needle="$2"
    if ! grep -Fq "$needle" "$file"; then
        echo "expected '$needle' in $file" >&2
        exit 1
    fi
}

assert_not_contains() {
    local file="$1"
    local needle="$2"
    if grep -Fq "$needle" "$file"; then
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
assert_contains "$CONARY_LOG" "bootstrap image --work-dir $WORK_DIR --output $WORK_DIR/vm-selfhost/output/conaryos-selfhost-x86_64.qcow2 --format qcow2 --size 16G"

assert_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap temp-tools --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap system --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap tier2 --work-dir $WORK_DIR --lfs-root $WORK_DIR/lfs-root"
assert_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap image --work-dir $WORK_DIR --output $WORK_DIR/vm-selfhost/output/conaryos-selfhost-x86_64.qcow2 --format qcow2 --size 16G"
assert_contains "$ROOTFUL_LOG" "chown -R"

assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap init"
assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap cross-tools"
assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap config"
assert_not_contains "$ROOTFUL_LOG" "$FAKE_CONARY bootstrap guest-profile"

test -f "$WORK_DIR/vm-selfhost/inputs/conary-workspace.tar.gz"
test -f "$WORK_DIR/vm-selfhost/inputs/conary-workspace.tar.gz.sha256"
test -f "$WORK_DIR/vm-selfhost/keys/selfhost_ed25519"
test -f "$WORK_DIR/vm-selfhost/keys/selfhost_ed25519.pub"

echo "build-selfhost-qcow2 rootful handoff test passed"
