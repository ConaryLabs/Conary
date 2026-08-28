#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
script="${repo_root}/scripts/remi-candidate-artifact.sh"
linker="${repo_root}/scripts/timed-linker.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fail() {
    echo "remi candidate artifact fixture: $*" >&2
    exit 1
}

fixture="${tmpdir}/fixture"
mkdir -p "$fixture/scripts" "$fixture/target/release"
cp "$script" "$fixture/scripts/remi-candidate-artifact.sh"
cp "$linker" "$fixture/scripts/timed-linker.sh"
chmod +x "$fixture/scripts/"*.sh
cat >"$fixture/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = []

[workspace.package]
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
        SCCACHE_VERSION=0.17.0 \
        SCCACHE_GHA_VERSION=remi-release-v1 \
        CONARY_GIT_COMMIT="$commit" \
        CONARY_GIT_DIRTY=false \
        scripts/remi-candidate-artifact.sh package \
          target/release/remi "$output" 1.2.3 "$commit" 1234 10 20 30 \
          "$timings" "$stats"
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
  .schema_version == 1
  and .version == "1.2.3"
  and (.binary_sha256 | test("^[0-9a-f]{64}$"))
  and (.bundle_sha256 | test("^[0-9a-f]{64}$"))
' "${tmpdir}/verified.json" >/dev/null
jq -e '
  .measurements.linker_invocations == 3
  and .measurements.linker_ms_total == 48
  and .measurements.successful_link_ms_total == 46
  and .measurements.remi_final_link_ms == 34
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

touch "$fixture/untracked"
if package_once "${tmpdir}/dirty" >"${tmpdir}/dirty.out" 2>"${tmpdir}/dirty.err"; then
    fail "dirty source checkout was packaged"
fi
grep -Fq 'candidate checkout must be clean before packaging' "${tmpdir}/dirty.err" ||
    fail "dirty-checkout failure was not typed"

echo "Remi candidate artifact fixtures passed."
