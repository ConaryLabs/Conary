#!/usr/bin/env bash
# scripts/test-dev-build.sh -- Prove compiler-cache sharing without target sharing.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
script="${repo_root}/scripts/dev-build.sh"
tmp="$(mktemp -d)"
trap 'rm -rf -- "$tmp"' EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_contains() {
    local output="$1" expected="$2"
    [[ "$output" == *"$expected"* ]] ||
        fail "expected output to contain [$expected], got: $output"
}

fixture="${tmp}/repo"
linked="${tmp}/linked"
fake_bin="${tmp}/bin"
mkdir -p "$fixture" "$fake_bin"
git -C "$fixture" init -q
git -C "$fixture" config user.email test@example.com
git -C "$fixture" config user.name test
printf 'fixture\n' >"$fixture/tracked"
git -C "$fixture" add tracked
git -C "$fixture" commit -qm initial

cat >"$fake_bin/sccache" <<'FAKE'
#!/usr/bin/env bash
case "${1:-}" in
    --show-stats)
        echo 'Compile requests                  7'
        ;;
    --stop-server)
        echo 'Compile requests                  7'
        ;;
    *)
        echo "unexpected fake sccache invocation: $*" >&2
        exit 91
        ;;
esac
FAKE
chmod +x "$fake_bin/sccache"

cat >"$fake_bin/cargo" <<'FAKE'
#!/usr/bin/env bash
printf 'cargo-argv='
printf '<%s>' "$@"
printf '\n'
FAKE
chmod +x "$fake_bin/cargo"

auto_out="$(
    cd "$fixture"
    env -u RUSTC_WRAPPER -u SCCACHE_DIR -u CARGO_TARGET_DIR \
        PATH="$fake_bin:$PATH" \
        "$script" run -- bash -c '
            printf "wrapper=%s\n" "$RUSTC_WRAPPER"
            printf "cache=%s\n" "$SCCACHE_DIR"
            printf "cache_size=%s\n" "$SCCACHE_CACHE_SIZE"
            printf "target_set=%s\n" "${CARGO_TARGET_DIR+x}"
        ' 2>&1
)"
expected_cache="${fixture}/.git/conary-dev/sccache"
assert_contains "$auto_out" 'compiler-cache=sccache'
assert_contains "$auto_out" "wrapper=${fake_bin}/sccache"
assert_contains "$auto_out" "cache=${expected_cache}"
assert_contains "$auto_out" 'cache_size=10G'
assert_contains "$auto_out" 'target_set='
[[ -f "${expected_cache}/.conary-sccache-cache-v1" ]] ||
    fail "auto mode did not create the exact shared-cache marker"

caller_out="$(
    cd "$fixture"
    RUSTC_WRAPPER=/caller/wrapper \
    SCCACHE_DIR=/caller/cache \
    CARGO_TARGET_DIR=private-target \
        "$script" --cache off run -- bash -c '
            printf "wrapper=%s\n" "$RUSTC_WRAPPER"
            printf "cache=%s\n" "$SCCACHE_DIR"
            printf "target=%s\n" "$CARGO_TARGET_DIR"
        ' 2>&1
)"
assert_contains "$caller_out" 'compiler-cache=caller-wrapper'
assert_contains "$caller_out" 'wrapper=/caller/wrapper'
assert_contains "$caller_out" 'cache=/caller/cache'
assert_contains "$caller_out" 'target=private-target'

caller_disabled_out="$(
    cd "$fixture"
    RUSTC_WRAPPER='' "$script" run -- bash -c \
        'printf "wrapper_length=%s\n" "${#RUSTC_WRAPPER}"' 2>&1
)"
assert_contains "$caller_disabled_out" 'compiler-cache=caller-disabled'
assert_contains "$caller_disabled_out" 'wrapper_length=0'

off_out="$(
    cd "$fixture"
    env -u RUSTC_WRAPPER CONARY_COMPILER_CACHE=off \
        "$script" run -- bash -c \
        'printf "wrapper_set=%s\n" "${RUSTC_WRAPPER+x}"' 2>&1
)"
assert_contains "$off_out" 'compiler-cache=disabled'
assert_contains "$off_out" 'wrapper_set='

iterate_out="$(
    cd "$fixture"
    env -u RUSTC_WRAPPER -u SCCACHE_DIR -u CARGO_TARGET_DIR \
        PATH="$fake_bin:$PATH" \
        "$script" iterate -- build -p conary --locked 2>&1
)"
assert_contains "$iterate_out" 'compiler-cache=sccache'
assert_contains "$iterate_out" 'cargo-profile=fast-release authority=development-only'
assert_contains "$iterate_out" \
    'cargo-argv=<build><--profile><fast-release><-p><conary><--locked>'

iterate_separator_out="$(
    cd "$fixture"
    env -u RUSTC_WRAPPER -u SCCACHE_DIR -u CARGO_TARGET_DIR \
        PATH="$fake_bin:$PATH" \
        "$script" iterate -- test -p conary -- --profile downstream --release 2>&1
)"
assert_contains "$iterate_separator_out" \
    'cargo-argv=<test><--profile><fast-release><-p><conary><--><--profile><downstream><--release>'

if iterate_release_out="$(
    cd "$fixture"
    PATH="$fake_bin:$PATH" "$script" iterate -- build --release 2>&1
)"; then
    fail "iterate unexpectedly accepted --release"
fi
assert_contains "$iterate_release_out" \
    'iterate owns --profile fast-release; remove: --release'

if iterate_profile_out="$(
    cd "$fixture"
    PATH="$fake_bin:$PATH" \
        "$script" iterate -- build --profile release 2>&1
)"; then
    fail "iterate unexpectedly accepted a caller profile"
fi
assert_contains "$iterate_profile_out" \
    'iterate owns --profile fast-release; remove: --profile'

if iterate_action_out="$(
    cd "$fixture"
    PATH="$fake_bin:$PATH" "$script" iterate -- metadata 2>&1
)"; then
    fail "iterate unexpectedly accepted a non-compiling Cargo action"
fi
assert_contains "$iterate_action_out" \
    'iterate does not support cargo action: metadata'

if required_out="$(
    cd "$fixture"
    env -u RUSTC_WRAPPER CONARY_SCCACHE="${tmp}/missing-sccache" \
        "$script" --cache required run -- true 2>&1
)"; then
    fail "required mode unexpectedly accepted missing sccache"
fi
assert_contains "$required_out" 'compiler cache is required but sccache is unavailable'

missing_out="$(
    cd "$fixture"
    env -u RUSTC_WRAPPER CONARY_SCCACHE="${tmp}/missing-sccache" \
        "$script" run -- bash -c \
        'printf "wrapper_set=%s\n" "${RUSTC_WRAPPER+x}"' 2>&1
)"
assert_contains "$missing_out" 'compiler-cache=unavailable'
assert_contains "$missing_out" 'wrapper_set='

status_out="$(
    cd "$fixture"
    SCCACHE_DIR="$expected_cache" PATH="$fake_bin:$PATH" "$script" status
)"
assert_contains "$status_out" 'Compile requests                  7'
assert_contains "$status_out" 'target-cleanup=never'
assert_contains "$status_out" 'Cache disk bytes:'

missing_status_out="$(
    cd "$fixture"
    CONARY_SCCACHE="${tmp}/missing-sccache" \
    SCCACHE_DIR="${tmp}/absent-cache" "$script" status
)"
assert_contains "$missing_status_out" 'sccache status: unavailable'
assert_contains "$missing_status_out" 'compiler-cache=unavailable'
assert_contains "$missing_status_out" 'Cache disk bytes: 0'

cleanup_cache="${tmp}/cleanup-cache"
preserved_target="${tmp}/preserved-target"
mkdir -p "$preserved_target"
printf 'keep\n' >"$preserved_target/sentinel"
(
    cd "$fixture"
    env -u RUSTC_WRAPPER SCCACHE_DIR="$cleanup_cache" PATH="$fake_bin:$PATH" \
        "$script" run -- true >/dev/null
)
printf 'cached\n' >"$cleanup_cache/object"
if (
    cd "$fixture"
    SCCACHE_DIR="$cleanup_cache" PATH="$fake_bin:$PATH" "$script" clean
) >/dev/null 2>&1; then
    fail "cache cleanup unexpectedly ran without --yes"
fi
(
    cd "$fixture"
    SCCACHE_DIR="$cleanup_cache" PATH="$fake_bin:$PATH" \
        "$script" clean --yes >/dev/null
)
[[ ! -e "$cleanup_cache/object" ]] || fail "cache cleanup retained a cache object"
[[ -f "$cleanup_cache/.conary-sccache-cache-v1" ]] ||
    fail "cache cleanup removed its safety marker"
[[ -f "$preserved_target/sentinel" ]] || fail "cache cleanup touched a caller target"

git -C "$fixture" worktree add -q -b linked-test "$linked"
main_cache_out="${tmp}/main-cache"
linked_cache_out="${tmp}/linked-cache"
(
    cd "$fixture"
    env -u RUSTC_WRAPPER -u SCCACHE_DIR -u CARGO_TARGET_DIR \
        PATH="$fake_bin:$PATH" "$script" run -- bash -c '
            printf "%s\n" "$SCCACHE_DIR" >"$1"
            mkdir -p "${CARGO_TARGET_DIR:-$PWD/target}"
            sleep 0.2
            printf "main\n" >"${CARGO_TARGET_DIR:-$PWD/target}/owner"
        ' bash "$main_cache_out" >/dev/null
) &
main_pid=$!
(
    cd "$linked"
    env -u RUSTC_WRAPPER -u SCCACHE_DIR -u CARGO_TARGET_DIR \
        PATH="$fake_bin:$PATH" "$script" run -- bash -c '
            printf "%s\n" "$SCCACHE_DIR" >"$1"
            mkdir -p "${CARGO_TARGET_DIR:-$PWD/target}"
            sleep 0.2
            printf "linked\n" >"${CARGO_TARGET_DIR:-$PWD/target}/owner"
        ' bash "$linked_cache_out" >/dev/null
) &
linked_pid=$!
wait "$main_pid"
wait "$linked_pid"

[[ "$(cat "$main_cache_out")" == "$(cat "$linked_cache_out")" ]] ||
    fail "linked worktrees did not select one shared compiler cache"
[[ "$(cat "$fixture/target/owner")" == main ]] ||
    fail "main worktree did not retain its isolated target"
[[ "$(cat "$linked/target/owner")" == linked ]] ||
    fail "linked worktree did not retain its isolated target"
[[ "$fixture/target" != "$linked/target" ]] || fail "target paths unexpectedly collapsed"

mkdir -p "$fixture/apps/remi" "$fixture/apps/conary-test" \
    "$linked/apps/remi" "$linked/apps/conary-test"
install -m 0644 "$repo_root/apps/remi/build.rs" "$fixture/apps/remi/build.rs"
install -m 0644 "$repo_root/apps/conary-test/build.rs" \
    "$fixture/apps/conary-test/build.rs"
install -m 0644 "$repo_root/apps/remi/build.rs" "$linked/apps/remi/build.rs"
install -m 0644 "$repo_root/apps/conary-test/build.rs" \
    "$linked/apps/conary-test/build.rs"
rustc --edition 2024 "$repo_root/apps/remi/build.rs" \
    -o "$tmp/remi-build-metadata"
rustc --edition 2024 "$repo_root/apps/conary-test/build.rs" \
    -o "$tmp/conary-test-build-metadata"

assert_watch_paths_exist() {
    local output="$1" manifest_dir="$2" line watched
    while IFS= read -r line; do
        [[ "$line" == cargo:rerun-if-changed=* ]] || continue
        watched="${line#cargo:rerun-if-changed=}"
        if [[ "$watched" != /* ]]; then
            watched="${manifest_dir}/${watched}"
        fi
        [[ -e "$watched" ]] ||
            fail "build metadata emitted a permanently missing watch path: $watched"
    done <<<"$output"
}

linked_index="$(git -C "$linked" rev-parse --git-path index)"
touch "$linked/tracked"
index_mtime_before="$(stat -c '%y' "$linked_index")"
remi_metadata_out="$(
    CARGO_MANIFEST_DIR="$linked/apps/remi" "$tmp/remi-build-metadata"
)"
index_mtime_after="$(stat -c '%y' "$linked_index")"
[[ "$index_mtime_before" == "$index_mtime_after" ]] ||
    fail "Remi build metadata mutated the Git index that it watches"
assert_watch_paths_exist "$remi_metadata_out" "$linked/apps/remi"
linked_git_dir="$(git -C "$linked" rev-parse --git-dir)"
assert_contains "$remi_metadata_out" \
    "cargo:rerun-if-changed=${linked_git_dir}/HEAD"
assert_contains "$remi_metadata_out" \
    "cargo:rerun-if-changed=${linked_git_dir}/index"
assert_contains "$remi_metadata_out" 'cargo:rustc-env=CONARY_GIT_DIRTY=false'

conary_test_metadata_out="$(
    CARGO_MANIFEST_DIR="$linked/apps/conary-test" "$tmp/conary-test-build-metadata"
)"
assert_watch_paths_exist "$conary_test_metadata_out" "$linked/apps/conary-test"

main_conary_test_metadata_out="$(
    CARGO_MANIFEST_DIR="$fixture/apps/conary-test" "$tmp/conary-test-build-metadata"
)"
assert_watch_paths_exist "$main_conary_test_metadata_out" \
    "$fixture/apps/conary-test"
if grep -Fxq "cargo:rerun-if-changed=${fixture}/.git" \
    <<<"$main_conary_test_metadata_out"; then
    fail "main-worktree metadata recursively watched the common Git directory"
fi

[[ ! -e "$repo_root/tools/sccache-wrapper.sh" ]] ||
    fail "the retired compiler fallback wrapper still exists"

echo "dev-build environment tests passed"
