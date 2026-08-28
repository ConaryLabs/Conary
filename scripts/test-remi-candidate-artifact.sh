#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
script="${repo_root}/scripts/remi-candidate-artifact.sh"
linker="${repo_root}/scripts/timed-linker.sh"
rustc_wrapper="${repo_root}/scripts/timed-rustc-wrapper.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fail() {
    echo "remi candidate artifact fixture: $*" >&2
    exit 1
}

fake_wrapper="${tmpdir}/fake-rustc-wrapper"
cat >"$fake_wrapper" <<'EOF'
#!/usr/bin/env bash
exit "${FAKE_RUSTC_STATUS:-0}"
EOF
chmod +x "$fake_wrapper"
wrapper_timings="${tmpdir}/wrapper-timings.tsv"
CONARY_REAL_RUSTC_WRAPPER="$fake_wrapper" \
CONARY_RUSTC_TIMINGS_PATH="$wrapper_timings" \
    "$rustc_wrapper" rustc --crate-name fixture --crate-type bin
IFS=$'\t' read -r wrapper_ms wrapper_status wrapper_crate wrapper_type \
    <"$wrapper_timings"
[[ "$wrapper_ms" =~ ^[0-9]+$ && "$wrapper_status" == 0 \
    && "$wrapper_crate" == fixture && "$wrapper_type" == bin ]] ||
    fail "timed Rust compiler wrapper did not retain attributable success evidence"
set +e
FAKE_RUSTC_STATUS=7 \
CONARY_REAL_RUSTC_WRAPPER="$fake_wrapper" \
CONARY_RUSTC_TIMINGS_PATH="$wrapper_timings" \
    "$rustc_wrapper" rustc --crate-name failed --crate-type rlib
wrapper_exit=$?
set -e
[[ "$wrapper_exit" == 7 ]] || fail "timed Rust compiler wrapper lost compiler failure status"
tail -n 1 "$wrapper_timings" | awk -F '\t' \
    'NF == 4 && $1 ~ /^[0-9]+$/ && $2 == 7 && $3 == "failed" && $4 == "rlib" { ok = 1 }
     END { exit !ok }' ||
    fail "timed Rust compiler wrapper did not retain attributable failure evidence"

fixture="${tmpdir}/fixture"
mkdir -p "$fixture/scripts" "$fixture/target/release"
cp "$script" "$fixture/scripts/remi-candidate-artifact.sh"
cp "$linker" "$fixture/scripts/timed-linker.sh"
cp "$rustc_wrapper" "$fixture/scripts/timed-rustc-wrapper.sh"
chmod +x "$fixture/scripts/"*.sh
cat >"$fixture/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = []

[workspace.package]
version = "1.2.3"
rust-version = "1.98.0"
EOF
cat >"$fixture/Cargo.lock" <<'EOF'
version = 4
EOF
cat >"$fixture/.gitignore" <<'EOF'
/target
EOF
cat >"$fixture/target/release/remi" <<'EOF'
#!/usr/bin/env bash
echo "remi 1.2.3"
EOF
chmod +x "$fixture/target/release/remi"

git -C "$fixture" init -q
git -C "$fixture" config user.name fixture
git -C "$fixture" config user.email fixture@example.invalid
git -C "$fixture" add Cargo.toml Cargo.lock .gitignore scripts
git -C "$fixture" commit -qm 'test: seed fixture'
commit="$(git -C "$fixture" rev-parse HEAD)"

timings="${tmpdir}/link-timings.tsv"
cat >"$timings" <<'EOF'
12	0	libdependency.so
34	0	remi-0123456789abcdef
2	1	failed-output
EOF
stats="${tmpdir}/sccache-stats.json"
cat >"$stats" <<'EOF'
{"compile_requests":10,"cache_hits":9,"cache_misses":1}
EOF
rustc_timings="${tmpdir}/rustc-timings.tsv"
cat >"$rustc_timings" <<'EOF'
5	0	dependency	rlib
81	0	remi	bin
3	1	probe	bin
EOF

package_once() {
    local output="$1"
    (
        cd "$fixture"
        GITHUB_REPOSITORY=ConaryLabs/Conary \
        GITHUB_WORKFLOW=build-remi-candidate \
        GITHUB_EVENT_NAME=push \
        RUNNER_OS=Linux \
        RUNNER_ARCH=X64 \
        ImageOS=ubuntu24 \
        ImageVersion=20260820.1 \
        CARGO_INCREMENTAL=0 \
        CARGO_PROFILE_DEV_DEBUG=0 \
        CARGO_PROFILE_TEST_DEBUG=0 \
        SCCACHE_VERSION=0.16.0 \
        SCCACHE_CACHE_BACKEND=local-disk-bulk-v1 \
        CONARY_COMPILER_CACHE_NAMESPACE=remi-release-local-v1-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef \
        CONARY_GIT_COMMIT="$commit" \
        CONARY_GIT_DIRTY=false \
        scripts/remi-candidate-artifact.sh package \
          target/release/remi "$output" 1.2.3 "$commit" 1234 10 20 25 30 \
          "$timings" "$rustc_timings" "$stats"
    )
}

first="${tmpdir}/first"
second="${tmpdir}/second"
package_once "$first" >/dev/null
package_once "$second" >/dev/null

cmp "$first/remi-1.2.3-linux-x64" "$second/remi-1.2.3-linux-x64"
cmp "$first/remi-1.2.3-linux-x64.tar.gz" "$second/remi-1.2.3-linux-x64.tar.gz"
jq -S 'del(.measurements.bundle_ms)' "$first/remi-candidate-manifest.json" \
    >"${tmpdir}/first-stable-manifest.json"
jq -S 'del(.measurements.bundle_ms)' "$second/remi-candidate-manifest.json" \
    >"${tmpdir}/second-stable-manifest.json"
cmp "${tmpdir}/first-stable-manifest.json" "${tmpdir}/second-stable-manifest.json"

(
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
      scripts/remi-candidate-artifact.sh verify "$first" "$commit" 1234 push \
      >"${tmpdir}/verified.json"
)
jq -e '
  .schema_version == 2
  and .version == "1.2.3"
  and (.binary_sha256 | test("^[0-9a-f]{64}$"))
  and (.bundle_sha256 | test("^[0-9a-f]{64}$"))
' "${tmpdir}/verified.json" >/dev/null
jq -e '
  .measurements.linker_invocations == 3
  and .measurements.linker_ms_total == 48
  and .measurements.successful_link_ms_total == 46
  and .measurements.remi_final_link_ms == 34
  and .measurements.compiler_cache_save_ms == 25
  and .measurements.rustc_invocations == 3
  and .measurements.rustc_ms_total == 89
  and .measurements.successful_rustc_ms_total == 86
  and .compiler_timing == {
    "slowest_ms": 81,
    "slowest_status": 0,
    "slowest_crate": "remi",
    "slowest_crate_type": "bin"
  }
  and .compiler_cache.backend == "local-disk-bulk-v1"
' "$first/remi-candidate-manifest.json" >/dev/null

rm "$second/remi-1.2.3-linux-x64"
(
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
      scripts/remi-candidate-artifact.sh verify "$second" "$commit" 1234 push \
      >/dev/null
)

cp "$second/remi-1.2.3-linux-x64.tar.gz" "${tmpdir}/bundle-backup"
printf '\ncorrupt\n' >>"$second/remi-1.2.3-linux-x64.tar.gz"
if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
      scripts/remi-candidate-artifact.sh verify "$second" "$commit" 1234 push
) >"${tmpdir}/bundle-tamper.out" 2>"${tmpdir}/bundle-tamper.err"; then
    fail "tampered downloaded bundle passed verification"
fi
grep -Fq 'candidate bundle digest does not match its manifest' \
    "${tmpdir}/bundle-tamper.err" ||
    fail "tamper failure did not name the downloaded bundle digest"
mv "${tmpdir}/bundle-backup" "$second/remi-1.2.3-linux-x64.tar.gz"

cp "$first/rustc-timings.tsv" "${tmpdir}/rustc-timings-backup"
printf '999\t0\tforged\tbin\n' >>"$first/rustc-timings.tsv"
if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
      scripts/remi-candidate-artifact.sh verify "$first" "$commit" 1234 push
) >"${tmpdir}/compiler-timing-tamper.out" \
    2>"${tmpdir}/compiler-timing-tamper.err"; then
    fail "tampered compiler timing evidence passed verification"
fi
grep -Fq 'compiler timing evidence digest does not match its manifest' \
    "${tmpdir}/compiler-timing-tamper.err" ||
    fail "compiler timing tamper failure did not name the evidence digest"
mv "${tmpdir}/rustc-timings-backup" "$first/rustc-timings.tsv"

cp "$first/remi-1.2.3-linux-x64" "${tmpdir}/binary-backup"
printf '\ncorrupt\n' >>"$first/remi-1.2.3-linux-x64"
if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
      scripts/remi-candidate-artifact.sh verify "$first" "$commit" 1234 push
) >"${tmpdir}/tamper.out" 2>"${tmpdir}/tamper.err"; then
    fail "tampered binary passed verification"
fi
grep -Fq 'candidate binary digest does not match its manifest' "${tmpdir}/tamper.err" ||
    fail "tamper failure did not name the binary digest"
mv "${tmpdir}/binary-backup" "$first/remi-1.2.3-linux-x64"

if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
      scripts/remi-candidate-artifact.sh verify "$first" \
        0000000000000000000000000000000000000000 1234 push
) >"${tmpdir}/commit.out" 2>"${tmpdir}/commit.err"; then
    fail "mismatched commit passed verification"
fi
grep -Fq 'candidate manifest bindings are invalid' "${tmpdir}/commit.err" ||
    fail "commit failure did not name the manifest binding"

cp "$first/remi-candidate-manifest.json" "${tmpdir}/manifest-backup"
jq '.build.rustflags = "-C target-cpu=native"' \
    "$first/remi-candidate-manifest.json" >"${tmpdir}/manifest-mutated"
mv "${tmpdir}/manifest-mutated" "$first/remi-candidate-manifest.json"
if (
    cd "$fixture"
    GITHUB_REPOSITORY=ConaryLabs/Conary \
      scripts/remi-candidate-artifact.sh verify "$first" "$commit" 1234 push
) >"${tmpdir}/policy.out" 2>"${tmpdir}/policy.err"; then
    fail "artifact with mismatched build policy passed verification"
fi
grep -Fq 'candidate manifest bindings are invalid' "${tmpdir}/policy.err" ||
    fail "build-policy failure did not name the manifest binding"
mv "${tmpdir}/manifest-backup" "$first/remi-candidate-manifest.json"

touch "$fixture/untracked"
if package_once "${tmpdir}/dirty" >"${tmpdir}/dirty.out" 2>"${tmpdir}/dirty.err"; then
    fail "dirty source checkout was packaged"
fi
grep -Fq 'candidate checkout must be clean before packaging' "${tmpdir}/dirty.err" ||
    fail "dirty-checkout failure was not typed"

echo "Remi candidate artifact fixtures passed."
