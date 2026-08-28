#!/usr/bin/env bash
# scripts/test-native-matrix-artifact.sh
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
script="$repo_root/scripts/native-matrix-artifact.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fail() {
    echo "native matrix artifact fixture: $*" >&2
    exit 1
}

for tool in cc git jq sha256sum tar file; do
    command -v "$tool" >/dev/null || fail "missing fixture tool: $tool"
done

fixture="$tmpdir/fixture"
mkdir -p \
    "$fixture/scripts" \
    "$fixture/.github/actions/build-static-conary" \
    "$fixture/target/x86_64-unknown-linux-musl/debug"
cp "$script" "$fixture/scripts/native-matrix-artifact.sh"
cp "$repo_root/scripts/build-static-conary.sh" "$fixture/scripts/"
cp "$repo_root/scripts/kernel-header-roots.sh" "$fixture/scripts/"
cp "$repo_root/.github/actions/build-static-conary/action.yml" \
    "$fixture/.github/actions/build-static-conary/"
chmod +x "$fixture/scripts/"*.sh

printf '%s\n' \
    '[workspace]' \
    'resolver = "2"' \
    'members = []' \
    '' \
    '[workspace.package]' \
    'version = "1.2.3"' \
    'rust-version = "1.98.0"' >"$fixture/Cargo.toml"
printf '%s\n' 'version = 4' >"$fixture/Cargo.lock"
printf '%s\n' '/target' >"$fixture/.gitignore"
printf '%s\n' 'int main(void) { return 0; }' | \
    cc -static -x c -o "$fixture/target/x86_64-unknown-linux-musl/debug/conary" -
for name in conary-test conary-test-library-tests; do
    cp "$fixture/target/x86_64-unknown-linux-musl/debug/conary" \
        "$fixture/target/x86_64-unknown-linux-musl/debug/$name"
done
chmod +x "$fixture/target/x86_64-unknown-linux-musl/debug/"*

git -C "$fixture" init -q
git -C "$fixture" config user.name fixture
git -C "$fixture" config user.email fixture@example.invalid
git -C "$fixture" add Cargo.toml Cargo.lock .gitignore scripts .github
git -C "$fixture" commit -qm 'test: seed fixture'
commit="$(git -C "$fixture" rev-parse HEAD)"
stats="$tmpdir/sccache-stats.json"
printf '%s\n' \
    '{"version":"0.16.0","cache_location":"Local disk: /tmp/native-matrix-sccache","stats":{"compile_requests":10,"cache_hits":{"counts":{"Rust":9}},"cache_misses":{"counts":{"Rust":1}},"cache_errors":{"counts":{}},"cache_writes":1,"cache_read_errors":0,"cache_write_errors":0,"cache_timeouts":0}}' \
    >"$stats"
metrics="$tmpdir/static-build-metrics.json"
printf '%s\n' \
    '{"schema_version":1,"static_dependency_cache_hit":true,"static_dependency_ms":1,"static_runtime_build_ms":20,"library_test_build_ms":9,"with_test_harness":true}' \
    >"$metrics"

package_once() {
    local output="$1"
    (
        cd "$fixture"
        GITHUB_REPOSITORY=ConaryLabs/Conary \
        GITHUB_WORKFLOW=pr-gate \
        GITHUB_EVENT_NAME=pull_request \
        RUNNER_OS=Linux \
        RUNNER_ARCH=X64 \
        ImageOS=ubuntu24 \
        ImageVersion=20260820.1 \
        CARGO_INCREMENTAL=0 \
        CARGO_PROFILE_DEV_DEBUG=0 \
        CARGO_PROFILE_TEST_DEBUG=0 \
        RUSTFLAGS='' \
        CARGO_ENCODED_RUSTFLAGS='' \
        SCCACHE_VERSION=0.16.0 \
        SCCACHE_CACHE_BACKEND=local-disk-bulk-v1 \
        SCCACHE_CACHE_NAMESPACE=native-matrix-musl-local-v1-0000000000000000000000000000000000000000000000000000000000000000 \
            scripts/native-matrix-artifact.sh package \
                "$output" "$commit" 1234 10 20 0 30 "$stats" \
                "$metrics"
    )
}

first="$tmpdir/first"
second="$tmpdir/second"
package_once "$first" >/dev/null
package_once "$second" >/dev/null
cmp "$first/native-matrix-artifacts.tar.gz" "$second/native-matrix-artifacts.tar.gz"
jq -S 'del(.measurements.bundle_ms)' "$first/native-matrix-artifact-manifest.json" \
    >"$tmpdir/first-manifest.json"
jq -S 'del(.measurements.bundle_ms)' "$second/native-matrix-artifact-manifest.json" \
    >"$tmpdir/second-manifest.json"
cmp "$tmpdir/first-manifest.json" "$tmpdir/second-manifest.json"

rm -rf "$fixture/target"
(
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
        scripts/native-matrix-artifact.sh verify \
            "$first" "$commit" 1234 pull_request >"$tmpdir/verified.json"
)
jq -e \
    '.schema_version == 1 and .workflow_run_id == 1234 and (.bundle_sha256 | test("^[0-9a-f]{64}$"))' \
    "$tmpdir/verified.json" >/dev/null
for name in conary conary-test conary-test-library-tests; do
    [[ -x "$fixture/target/x86_64-unknown-linux-musl/debug/$name" ]] ||
        fail "verification did not restore $name"
done

cp "$first/native-matrix-artifacts.tar.gz" "$tmpdir/bundle-backup"
printf '%s\n' corrupt >>"$first/native-matrix-artifacts.tar.gz"
if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
        scripts/native-matrix-artifact.sh verify "$first" "$commit" 1234 pull_request
) >"$tmpdir/corrupt.out" 2>"$tmpdir/corrupt.err"; then
    fail "corrupt bundle passed verification"
fi
grep -Fq 'bundle digest does not match its manifest' "$tmpdir/corrupt.err" ||
    fail "corrupt-bundle failure did not name the digest"
mv "$tmpdir/bundle-backup" "$first/native-matrix-artifacts.tar.gz"

if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
        scripts/native-matrix-artifact.sh verify "$first" \
            0000000000000000000000000000000000000000 1234 pull_request
) >"$tmpdir/commit.out" 2>"$tmpdir/commit.err"; then
    fail "mismatched commit passed verification"
fi
grep -Eq 'checkout does not match|manifest bindings are invalid' "$tmpdir/commit.err" ||
    fail "commit mismatch did not name its binding"

cp "$first/native-matrix-artifact-manifest.json" "$tmpdir/manifest-backup"
jq '.build.cargo_incremental = "1"' \
    "$first/native-matrix-artifact-manifest.json" >"$tmpdir/mutated-manifest.json"
mv "$tmpdir/mutated-manifest.json" "$first/native-matrix-artifact-manifest.json"
if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
        scripts/native-matrix-artifact.sh verify "$first" "$commit" 1234 pull_request
) >"$tmpdir/policy.out" 2>"$tmpdir/policy.err"; then
    fail "mismatched build policy passed verification"
fi
grep -Fq 'manifest bindings are invalid' "$tmpdir/policy.err" ||
    fail "policy mismatch did not name its binding"
mv "$tmpdir/manifest-backup" "$first/native-matrix-artifact-manifest.json"

touch "$fixture/untracked"
if package_once "$tmpdir/dirty" >"$tmpdir/dirty.out" 2>"$tmpdir/dirty.err"; then
    fail "dirty checkout was packaged"
fi
grep -Fq 'artifact checkout must be clean' "$tmpdir/dirty.err" ||
    fail "dirty-checkout failure was not typed"

echo "Native matrix artifact fixtures passed."
