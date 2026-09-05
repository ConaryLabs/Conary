#!/usr/bin/env bash
# scripts/test-conary-generator.sh -- Exercise initramfs composefs verification policy.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
generator="$repo_root/packaging/dracut/90conary/conary-generator.sh"
verity_policy="$repo_root/packaging/dracut/90conary/conary-verity.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/conary-generator-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

fake_bin="$test_root/bin"
mkdir -p "$fake_bin"
cat > "$fake_bin/mount" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOUNT_LOG"
# Accept only the option grammar of the pinned composefs helper: `verity` as
# a whole `-o` option. The obsolete `verity_check=1` must never be emitted.
composefs=0
options=""
previous=""
for argument in "$@"; do
    [[ "$previous" == "-t" && "$argument" == "composefs" ]] && composefs=1
    [[ "$previous" == "-o" ]] && options="$argument"
    previous="$argument"
done
if [[ "$options" == *"verity_check"* ]]; then
    echo "obsolete verity_check option" >&2
    exit 32
fi
if [[ "$composefs" -eq 1 && ",$options," == *",verity,"* ]]; then
    exit "${VERIFIED_MOUNT_STATUS:-0}"
fi
exit 0
SH
chmod +x "$fake_bin/mount"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

prepare_case() {
    local name="$1"
    local cmdline="$2"
    local case_root="$test_root/$name"

    mkdir -p \
        "$case_root/sysroot/conary/generations/1" \
        "$case_root/sysroot/conary/etc-state/1" \
        "$case_root/sysroot/conary/objects"
    : > "$case_root/sysroot/conary/generations/1/root.erofs"
    printf '%s\n' "$cmdline" > "$case_root/cmdline"
    : > "$case_root/mount.log"
    printf '%s\n' "$case_root"
}

run_generator() {
    local case_root="$1"
    local verified_status="$2"

    PATH="$fake_bin:$PATH" \
    MOUNT_LOG="$case_root/mount.log" \
    VERIFIED_MOUNT_STATUS="$verified_status" \
    CONARY_VERITY_POLICY_PATH="$verity_policy" \
    CONARY_SYSROOT="$case_root/sysroot" \
    CONARY_CMDLINE_FILE="$case_root/cmdline" \
        bash "$generator" > "$case_root/stdout" 2> "$case_root/stderr"
}

case_root="$(prepare_case verified-default 'quiet conary.generation=1')"
run_generator "$case_root" 0 || fail "default verified activation failed"
grep -qE -- '-o [^ ]*,verity( |$)' "$case_root/mount.log" ||
    fail "default activation did not require fs-verity"
[[ "$(grep -c -- '-t composefs' "$case_root/mount.log")" -eq 1 ]] ||
    fail "default activation attempted more than one composefs mount"

case_root="$(prepare_case verified-failure 'quiet conary.generation=1')"
if run_generator "$case_root" 1; then
    fail "verified mount failure silently downgraded"
fi
grep -qE -- '-o [^ ]*,verity( |$)' "$case_root/mount.log" ||
    fail "failed verified activation did not attempt fs-verity"
[[ "$(grep -c -- '-t composefs' "$case_root/mount.log")" -eq 1 ]] ||
    fail "failed verified activation attempted an unverified fallback"
grep -q 'composefs mount failed' "$case_root/stderr" ||
    fail "verified mount failure did not report a fatal activation error"

case_root="$(prepare_case explicit-off 'quiet conary.generation=1 conary.verity=off')"
run_generator "$case_root" 1 || fail "explicit unverified activation failed"
if grep -qE -- '-o [^ ]*,verity( |$)' "$case_root/mount.log"; then
    fail "explicit unverified activation still requested fs-verity"
fi
grep -q 'conary.verity=off disables composefs fs-verity verification' "$case_root/stderr" ||
    fail "explicit unverified activation did not print its downgrade"

case_root="$(prepare_case explicit-on 'quiet conary.generation=1 conary.verity=on')"
run_generator "$case_root" 0 || fail "explicit verified activation failed"
grep -qE -- '-o [^ ]*,verity( |$)' "$case_root/mount.log" ||
    fail "conary.verity=on did not require fs-verity"

case_root="$(prepare_case duplicate-last-on \
    'quiet conary.generation=1 conary.verity=off conary.verity=on')"
run_generator "$case_root" 0 || fail "last conary.verity=on was not authoritative"
grep -qE -- '-o [^ ]*,verity( |$)' "$case_root/mount.log" ||
    fail "last conary.verity=on did not require fs-verity"

case_root="$(prepare_case duplicate-last-off \
    'quiet conary.generation=1 conary.verity=on conary.verity=off')"
run_generator "$case_root" 1 || fail "last conary.verity=off was not authoritative"
if grep -qE -- '-o [^ ]*,verity( |$)' "$case_root/mount.log"; then
    fail "earlier conary.verity=on overrode the final explicit opt-out"
fi
grep -q 'conary.verity=off disables composefs fs-verity verification' "$case_root/stderr" ||
    fail "final explicit opt-out did not print its downgrade"

case_root="$(prepare_case invalid-value 'quiet conary.generation=1 conary.verity=maybe')"
if run_generator "$case_root" 0; then
    fail "invalid conary.verity value was accepted"
fi
[[ ! -s "$case_root/mount.log" ]] ||
    fail "invalid conary.verity value reached the mount boundary"
grep -q "invalid conary.verity value 'maybe'" "$case_root/stderr" ||
    fail "invalid conary.verity value did not report its grammar"

for previous in absent on off; do
    cmdline='quiet conary.generation=1'
    if [[ "$previous" != absent ]]; then
        cmdline+=" conary.verity=$previous"
    fi
    case_root="$(prepare_case "empty-value-after-$previous" "$cmdline conary.verity=")"
    if run_generator "$case_root" 0; then
        fail "empty conary.verity value was accepted after $previous"
    fi
    [[ ! -s "$case_root/mount.log" ]] ||
        fail "empty conary.verity value reached the mount boundary"
    grep -q "invalid conary.verity value ''" "$case_root/stderr" ||
        fail "empty conary.verity value did not report its grammar"
done

case_root="$(prepare_case corrected-empty-value \
    'quiet conary.generation=1 conary.verity= conary.verity=on')"
run_generator "$case_root" 0 || fail "last valid verity argument did not override empty value"
grep -qE -- '-o [^ ]*,verity( |$)' "$case_root/mount.log" ||
    fail "corrected empty verity value did not require fs-verity"

echo "conary generator verity policy tests passed"
