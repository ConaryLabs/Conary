#!/usr/bin/env bash
# scripts/test-build-static-conary.sh
set -euo pipefail

# Fast, hermetic checks for the kernel-header probe that
# scripts/build-static-conary.sh depends on. The probe decides which include
# roots reach the musl compiler, and getting it wrong fails the static build
# before a single object is compiled -- which is exactly what happened on
# Ubuntu runners, where linux/ and asm/ do not share a root.
#
# These tests never compile anything. The slow proof is a real static build.

repo_root="$(git rev-parse --show-toplevel)"
build_script="$repo_root/scripts/build-static-conary.sh"
probe="$repo_root/scripts/kernel-header-roots.sh"

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

bash -n "$build_script" || fail "build-static-conary.sh has a syntax error"
bash -n "$probe" || fail "kernel-header-roots.sh has a syntax error"

# shellcheck source=scripts/kernel-header-roots.sh
source "$probe"

# Build a fake header tree. Each argument is "<root>:<header>".
make_tree() {
    local name="$1"
    shift
    local spec root header
    for spec in "$@"; do
        root="$tmp/$name/${spec%%:*}"
        header="${spec#*:}"
        mkdir -p "$root/$(dirname "$header")"
        : > "$root/$header"
    done
}

candidates_for() {
    printf '%s\n' "$tmp/$1/usr/include" "$tmp/$1/usr/include/x86_64-linux-gnu"
}

roots_for() {
    local name="$1"
    local -a candidates
    mapfile -t candidates < <(candidates_for "$name")
    kernel_header_roots "${candidates[@]}"
}

# Render the flags for a newline-separated list of roots.
flags_for() {
    local -a roots
    mapfile -t roots <<< "$1"
    kernel_header_include_flags "${roots[@]}"
}

# --- Arch and Fedora: both headers under one root -> one flag, as before ---
make_tree together \
    "usr/include:linux/audit.h" \
    "usr/include:asm/unistd.h"
actual="$(roots_for together)"
expected="$tmp/together/usr/include"
[[ "$actual" == "$expected" ]] ||
    fail "a single shared root should resolve to itself, got: $actual"
flags="$(flags_for "$actual")"
[[ "$flags" == "-idirafter $expected" ]] ||
    fail "a single root should render one -idirafter, got: $flags"

# --- Debian and Ubuntu: linux/ exact, asm/ multiarch only -> two roots ---
make_tree split \
    "usr/include:linux/audit.h" \
    "usr/include/x86_64-linux-gnu:asm/unistd.h"
actual="$(roots_for split)"
expected="$(printf '%s\n%s' "$tmp/split/usr/include" "$tmp/split/usr/include/x86_64-linux-gnu")"
[[ "$actual" == "$expected" ]] ||
    fail "split headers should resolve to both roots, exact first, got: $actual"
flags="$(flags_for "$actual")"
[[ "$flags" == "-idirafter $tmp/split/usr/include -idirafter $tmp/split/usr/include/x86_64-linux-gnu" ]] ||
    fail "split headers should render one -idirafter per root, got: $flags"

# --- Reversed split: candidate order still wins over discovery order ---
make_tree reversed \
    "usr/include:asm/unistd.h" \
    "usr/include/x86_64-linux-gnu:linux/audit.h"
actual="$(roots_for reversed)"
expected="$(printf '%s\n%s' "$tmp/reversed/usr/include" "$tmp/reversed/usr/include/x86_64-linux-gnu")"
[[ "$actual" == "$expected" ]] ||
    fail "roots must be emitted in candidate order, got: $actual"

# --- Everything multiarch: one root, and not the empty exact directory ---
make_tree multiarch \
    "usr/include/x86_64-linux-gnu:linux/audit.h" \
    "usr/include/x86_64-linux-gnu:asm/unistd.h"
actual="$(roots_for multiarch)"
expected="$tmp/multiarch/usr/include/x86_64-linux-gnu"
[[ "$actual" == "$expected" ]] ||
    fail "multiarch-only headers should resolve to the multiarch root, got: $actual"

# --- A missing header fails closed and says which one, and from where ---
make_tree partial "usr/include:linux/audit.h"
if roots_for partial > "$tmp/out" 2> "$tmp/err"; then
    fail "a missing header must not resolve"
fi
grep -q 'asm/unistd.h not found under any of:' "$tmp/err" ||
    fail "the failure must name the missing header: $(cat "$tmp/err")"
grep -q 'linux/audit.h found under' "$tmp/err" ||
    fail "the failure should report what it did find: $(cat "$tmp/err")"
grep -q 'linux-libc-dev' "$tmp/err" ||
    fail "the failure should name the package that supplies the headers"

make_tree empty "usr/include:unrelated.h"
if roots_for empty > "$tmp/out" 2> "$tmp/err"; then
    fail "an empty header tree must not resolve"
fi
for header in 'linux/audit.h' 'asm/unistd.h'; do
    grep -q "$header not found under any of:" "$tmp/err" ||
        fail "the failure must name every missing header, missing $header"
done

if kernel_header_roots > "$tmp/out" 2> "$tmp/err"; then
    fail "no candidate directories must not resolve"
fi

# --- The build script consumes the probe rather than its own copy ---
grep -q 'source "$(dirname "${BASH_SOURCE\[0\]}")/kernel-header-roots.sh"' "$build_script" ||
    fail "build-static-conary.sh must source the shared probe"
for needle in \
    'CFLAGS="-O2 $KERNEL_HEADER_FLAGS"' \
    'CFLAGS_x86_64_unknown_linux_musl="$KERNEL_HEADER_FLAGS"' \
    '--with-test-harness' \
    'cargo build "${cargo_packages[@]}" --target "$TARGET" --locked' \
    'cargo test -p conary-test --lib --target "$TARGET" --locked' \
    'conary-test-library-tests' \
    'CONARY_STATIC_BUILD_METRICS_PATH' \
    'static_dependency_cache_hit' \
    'static_runtime_build_ms' \
    'library_test_build_ms'; do
    grep -qF -- "$needle" "$build_script" ||
        fail "build-static-conary.sh must pass the resolved flags: $needle"
done
if grep -q 'KERNEL_HEADER_DIR' "$build_script"; then
    fail "build-static-conary.sh still uses the single-root variable"
fi

# --- Running the probe directly reports this host, for diagnosis ---
bash "$probe" > "$tmp/host.out" 2> "$tmp/host.err" ||
    fail "the probe should resolve on this host: $(cat "$tmp/host.err")"
grep -q '^roots: /' "$tmp/host.out" || fail "direct execution should print roots"
grep -q '^flags: -idirafter /' "$tmp/host.out" || fail "direct execution should print flags"

echo "build-static-conary tests passed."
