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

# --- deb: real dpkg-deb archives with the real texts ---
make_deb() {
    local root="$1" out="$2" mit="$3" apache="$4"
    rm -rf "$root"
    mkdir -p "$root/DEBIAN" "$root/usr/share/doc/conary"
    printf 'Package: conary\nVersion: 1.0\nArchitecture: all\nMaintainer: t <t@t>\nDescription: fixture\n' > "$root/DEBIAN/control"
    [[ -z "$mit" ]] || cp "$mit" "$root/usr/share/doc/conary/LICENSE-MIT"
    [[ -z "$apache" ]] || cp "$apache" "$root/usr/share/doc/conary/LICENSE-APACHE"
    dpkg-deb --root-owner-group --build "$root" "$out" >/dev/null
}
head -c 2000 LICENSE-APACHE > "$tmp/truncated-apache"
make_deb "$tmp/deb-ok" "$tmp/ok.deb" LICENSE-MIT LICENSE-APACHE
expect_pass "deb with both exact texts" deb "$tmp/ok.deb" LICENSE-MIT LICENSE-APACHE
make_deb "$tmp/deb-missing" "$tmp/missing.deb" "" LICENSE-APACHE
expect_fail "deb missing LICENSE-MIT" deb "$tmp/missing.deb" LICENSE-MIT LICENSE-APACHE
make_deb "$tmp/deb-trunc" "$tmp/trunc.deb" LICENSE-MIT "$tmp/truncated-apache"
expect_fail "deb with a truncated Apache text" deb "$tmp/trunc.deb" LICENSE-MIT LICENSE-APACHE

# --- arch: real zstd tarballs ---
make_arch() {
    local root="$1" out="$2" mit="$3" apache="$4"
    rm -rf "$root"
    mkdir -p "$root/usr/share/licenses/conary"
    printf 'pkgname = conary\n' > "$root/.PKGINFO"
    [[ -z "$mit" ]] || cp "$mit" "$root/usr/share/licenses/conary/LICENSE-MIT"
    [[ -z "$apache" ]] || cp "$apache" "$root/usr/share/licenses/conary/LICENSE-APACHE"
    tar --zstd -cf "$out" -C "$root" .PKGINFO usr
}
make_arch "$tmp/arch-ok" "$tmp/ok.pkg.tar.zst" LICENSE-MIT LICENSE-APACHE
expect_pass "arch with both exact texts" arch "$tmp/ok.pkg.tar.zst" LICENSE-MIT LICENSE-APACHE
make_arch "$tmp/arch-missing" "$tmp/missing.pkg.tar.zst" LICENSE-MIT ""
expect_fail "arch missing LICENSE-APACHE" arch "$tmp/missing.pkg.tar.zst" LICENSE-MIT LICENSE-APACHE
make_arch "$tmp/arch-trunc" "$tmp/trunc.pkg.tar.zst" LICENSE-MIT "$tmp/truncated-apache"
expect_fail "arch with a truncated Apache text" arch "$tmp/trunc.pkg.tar.zst" LICENSE-MIT LICENSE-APACHE

# --- rpm: rpm2cpio is shimmed to emit a real cpio archive; cpio is real ---
mkdir -p "$tmp/bin"
cat > "$tmp/bin/rpm2cpio" <<'SH'
#!/usr/bin/env bash
cat "${1}.cpio"
SH
chmod +x "$tmp/bin/rpm2cpio"
make_rpm_cpio() {
    local root="$1" out="$2" mit="$3" apache="$4"
    rm -rf "$root"
    mkdir -p "$root/usr/share/licenses/conary" "$root/usr/bin"
    printf 'x' > "$root/usr/bin/conary"
    [[ -z "$mit" ]] || cp "$mit" "$root/usr/share/licenses/conary/LICENSE-MIT"
    [[ -z "$apache" ]] || cp "$apache" "$root/usr/share/licenses/conary/LICENSE-APACHE"
    printf 'x' > "$out"
    ( cd "$root" && find . -type f | cpio -o --quiet -H newc > "$out.cpio" )
}
make_rpm_cpio "$tmp/rpm-ok" "$tmp/ok.rpm" LICENSE-MIT LICENSE-APACHE
PATH="$tmp/bin:$PATH" expect_pass "rpm with both exact texts" rpm "$tmp/ok.rpm" LICENSE-MIT LICENSE-APACHE
make_rpm_cpio "$tmp/rpm-missing" "$tmp/missing.rpm" "" LICENSE-APACHE
PATH="$tmp/bin:$PATH" expect_fail "rpm missing LICENSE-MIT" rpm "$tmp/missing.rpm" LICENSE-MIT LICENSE-APACHE
make_rpm_cpio "$tmp/rpm-trunc" "$tmp/trunc.rpm" LICENSE-MIT "$tmp/truncated-apache"
PATH="$tmp/bin:$PATH" expect_fail "rpm with a truncated Apache text" rpm "$tmp/trunc.rpm" LICENSE-MIT LICENSE-APACHE

# --- ccs: the conary inspector and verifier are shimmed; sizes must match ---
mit_size="$(wc -c < LICENSE-MIT)"; apache_size="$(wc -c < LICENSE-APACHE)"
cat > "$tmp/bin/conary" <<'SH'
#!/usr/bin/env bash
case "$1 $2" in
  "ccs verify") [[ -f "${3}.verify-ok" ]] ;;
  "ccs inspect") printf 'Files (3):\n\n'; cat "${4}.listing" ;;
  *) exit 2 ;;
esac
SH
chmod +x "$tmp/bin/conary"
printf 'x' > "$tmp/ok.ccs"; : > "$tmp/ok.ccs.verify-ok"
printf -- '-rw-r--r-- %s /usr/share/licenses/conary/LICENSE-MIT\n-rw-r--r-- %s /usr/share/licenses/conary/LICENSE-APACHE\n' "$mit_size" "$apache_size" > "$tmp/ok.ccs.listing"
expect_pass "ccs with both texts at the reference sizes" ccs "$tmp/ok.ccs" "$tmp/bin/conary" LICENSE-MIT LICENSE-APACHE
printf 'x' > "$tmp/short.ccs"; : > "$tmp/short.ccs.verify-ok"
printf -- '-rw-r--r-- %s /usr/share/licenses/conary/LICENSE-MIT\n-rw-r--r-- 2000 /usr/share/licenses/conary/LICENSE-APACHE\n' "$mit_size" > "$tmp/short.ccs.listing"
expect_fail "ccs with a wrong-size Apache text" ccs "$tmp/short.ccs" "$tmp/bin/conary" LICENSE-MIT LICENSE-APACHE
printf 'x' > "$tmp/unverified.ccs"
cp "$tmp/ok.ccs.listing" "$tmp/unverified.ccs.listing"
expect_fail "ccs that fails its own verification" ccs "$tmp/unverified.ccs" "$tmp/bin/conary" LICENSE-MIT LICENSE-APACHE

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

# --- client tarballs: binary plus both texts, digest-checked ---
mkdir -p "$tmp/client"
printf '#!/bin/sh\necho conaryd\n' > "$tmp/client/conaryd-1.0.0-linux-x64"
cp LICENSE-MIT LICENSE-APACHE "$tmp/client/"
tar czf "$tmp/conaryd-1.0.0-linux-x64.tar.gz" -C "$tmp/client" conaryd-1.0.0-linux-x64 LICENSE-MIT LICENSE-APACHE
expect_pass "client tar with both texts" client-tar "$tmp/conaryd-1.0.0-linux-x64.tar.gz" LICENSE-MIT LICENSE-APACHE
tar czf "$tmp/conaryd-1.0.0-linux-x64.tar.gz" -C "$tmp/client" conaryd-1.0.0-linux-x64 LICENSE-MIT
expect_fail "client tar missing LICENSE-APACHE" client-tar "$tmp/conaryd-1.0.0-linux-x64.tar.gz" LICENSE-MIT LICENSE-APACHE
printf 'wrong\n' > "$tmp/client/LICENSE-MIT"
tar czf "$tmp/conaryd-1.0.0-linux-x64.tar.gz" -C "$tmp/client" conaryd-1.0.0-linux-x64 LICENSE-MIT LICENSE-APACHE
expect_fail "client tar with the wrong MIT text" client-tar "$tmp/conaryd-1.0.0-linux-x64.tar.gz" LICENSE-MIT LICENSE-APACHE

# --- suite assets: the three texts published next to the standalone binaries ---
mkdir -p "$tmp/suite"
cp LICENSE-MIT LICENSE-APACHE "$tmp/suite/"
cp apps/remi/LICENSE "$tmp/suite/LICENSE-AGPL-3.0-remi"
expect_pass "suite with all three texts" suite "$tmp/suite" LICENSE-MIT LICENSE-APACHE apps/remi/LICENSE
rm "$tmp/suite/LICENSE-AGPL-3.0-remi"
expect_fail "suite missing the Remi AGPL text" suite "$tmp/suite" LICENSE-MIT LICENSE-APACHE apps/remi/LICENSE

if [[ "$failures" -ne 0 ]]; then
    echo "$failures release license contents checks failed" >&2
    exit 1
fi
echo "release license contents tests passed"
