#!/usr/bin/env bash
# scripts/test-release-license-contents.sh
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
checker="scripts/check-release-license-contents.sh"
failures=0

expect_pass() {
    local description="$1"
    shift
    if bash "$checker" "$@" >/dev/null 2>"$tmp/err"; then
        echo "ok: $description"
    else
        echo "FAIL: $description: $(head -n1 "$tmp/err")" >&2
        failures=$((failures + 1))
    fi
}
expect_fail() {
    local description="$1"
    shift
    if bash "$checker" "$@" >/dev/null 2>"$tmp/err"; then
        echo "FAIL: $description unexpectedly passed" >&2
        failures=$((failures + 1))
    else
        echo "ok: $description rejected ($(head -n1 "$tmp/err"))"
    fi
}

# --- deb: real dpkg-deb archives ---
make_deb() {
    local root="$1" out="$2"
    shift 2
    rm -rf "$root"
    mkdir -p "$root/DEBIAN" "$root/usr/share/doc/conary"
    printf 'Package: conary\nVersion: 1.0\nArchitecture: all\nMaintainer: t <t@t>\nDescription: fixture\n' > "$root/DEBIAN/control"
    local name
    for name in "$@"; do printf 'x\n' > "$root/usr/share/doc/conary/$name"; done
    dpkg-deb --root-owner-group --build "$root" "$out" >/dev/null
}
make_deb "$tmp/deb-ok" "$tmp/ok.deb" LICENSE-MIT LICENSE-APACHE
expect_pass "deb with both texts" deb "$tmp/ok.deb"
make_deb "$tmp/deb-bad" "$tmp/bad.deb" LICENSE-APACHE
expect_fail "deb missing LICENSE-MIT" deb "$tmp/bad.deb"

# --- arch: real zstd tarballs ---
make_arch() {
    local root="$1" out="$2"
    shift 2
    rm -rf "$root"
    mkdir -p "$root/usr/share/licenses/conary"
    printf 'pkgname = conary\n' > "$root/.PKGINFO"
    local name
    for name in "$@"; do printf 'x\n' > "$root/usr/share/licenses/conary/$name"; done
    tar --zstd -cf "$out" -C "$root" .PKGINFO usr
}
make_arch "$tmp/arch-ok" "$tmp/ok.pkg.tar.zst" LICENSE-MIT LICENSE-APACHE
expect_pass "arch with both texts" arch "$tmp/ok.pkg.tar.zst"
make_arch "$tmp/arch-bad" "$tmp/bad.pkg.tar.zst" LICENSE-MIT
expect_fail "arch missing LICENSE-APACHE" arch "$tmp/bad.pkg.tar.zst"

# --- rpm: the rpm listing tool is shimmed; the rest of the kernel is real ---
mkdir -p "$tmp/bin"
cat > "$tmp/bin/rpm" <<'SH'
#!/usr/bin/env bash
[[ "$1" == "-qlp" ]] || exit 2
cat "${2}.listing"
SH
chmod +x "$tmp/bin/rpm"
printf 'x' > "$tmp/ok.rpm"
printf '/usr/bin/conary\n/usr/share/licenses/conary/LICENSE-MIT\n/usr/share/licenses/conary/LICENSE-APACHE\n' > "$tmp/ok.rpm.listing"
PATH="$tmp/bin:$PATH" expect_pass "rpm with both texts" rpm "$tmp/ok.rpm"
printf 'x' > "$tmp/bad.rpm"
printf '/usr/bin/conary\n/usr/share/licenses/conary/LICENSE-APACHE\n' > "$tmp/bad.rpm.listing"
PATH="$tmp/bin:$PATH" expect_fail "rpm missing LICENSE-MIT" rpm "$tmp/bad.rpm"

# --- ccs: the conary inspector is shimmed ---
cat > "$tmp/bin/conary" <<'SH'
#!/usr/bin/env bash
[[ "$1 $2 $3" == "ccs inspect --files" ]] || exit 2
printf 'Files (3):\n\n'
cat "${4}.listing"
SH
chmod +x "$tmp/bin/conary"
printf 'x' > "$tmp/ok.ccs"
printf '  /usr/bin/conary\n  /usr/share/licenses/conary/LICENSE-MIT\n  /usr/share/licenses/conary/LICENSE-APACHE\n' > "$tmp/ok.ccs.listing"
expect_pass "ccs with both texts" ccs "$tmp/ok.ccs" "$tmp/bin/conary"
printf 'x' > "$tmp/bad.ccs"
printf '  /usr/bin/conary\n  /usr/share/licenses/conary/LICENSE-MIT\n' > "$tmp/bad.ccs.listing"
expect_fail "ccs missing LICENSE-APACHE" ccs "$tmp/bad.ccs" "$tmp/bin/conary"

# --- remi tarball: exact members and exact AGPL text ---
mkdir -p "$tmp/remi"
printf '#!/bin/sh\necho remi\n' > "$tmp/remi/remi-1.0.0-linux-x64"
cp apps/remi/LICENSE "$tmp/remi/LICENSE"
tar czf "$tmp/remi-1.0.0-linux-x64.tar.gz" -C "$tmp/remi" remi-1.0.0-linux-x64 LICENSE
expect_pass "remi tar with binary and AGPL text" remi-tar "$tmp/remi-1.0.0-linux-x64.tar.gz" apps/remi/LICENSE
tar czf "$tmp/remi-1.0.0-linux-x64.tar.gz" -C "$tmp/remi" remi-1.0.0-linux-x64
expect_fail "remi tar missing LICENSE" remi-tar "$tmp/remi-1.0.0-linux-x64.tar.gz" apps/remi/LICENSE
printf 'not the license\n' > "$tmp/remi/LICENSE"
tar czf "$tmp/remi-1.0.0-linux-x64.tar.gz" -C "$tmp/remi" remi-1.0.0-linux-x64 LICENSE
expect_fail "remi tar with the wrong LICENSE text" remi-tar "$tmp/remi-1.0.0-linux-x64.tar.gz" apps/remi/LICENSE

if [[ "$failures" -ne 0 ]]; then
    echo "$failures release license contents checks failed" >&2
    exit 1
fi
echo "release license contents tests passed"
