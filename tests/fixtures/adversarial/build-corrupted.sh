#!/usr/bin/env bash
# tests/fixtures/adversarial/build-corrupted.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONARY_BIN="${1:-${CONARY_BIN:-$(pwd)/target/debug/conary}}"
NATIVE_OUTPUT_DIR="$SCRIPT_DIR/corrupted/native/output"

build_valid_fixture() {
    local fixture_dir="$1"
    mkdir -p "$fixture_dir/output"
    rm -f "$fixture_dir/output/"*.ccs
    "$CONARY_BIN" ccs build "$fixture_dir/ccs.toml" \
        --source "$fixture_dir/stage" \
        --output "$fixture_dir/output/"
}

mutate_ccs() {
    local source_ccs="$1"
    local output_ccs="$2"
    local mutator="$3"
    local tmpdir
    tmpdir="$(mktemp -d)"
    tar -xzf "$source_ccs" -C "$tmpdir"
    "$mutator" "$tmpdir"
    tar -czf "$output_ccs" -C "$tmpdir" .
    rm -rf "$tmpdir"
}

mutate_bad_checksum() {
    local root="$1"
    python3 - "$root/components/runtime.json" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
data["files"][0]["hash"] = "0" * 64
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

mutate_truncated() {
    local root="$1"
    local object
    object="$(find "$root/objects" -type f | head -1)"
    python3 - "$object" <<'PY'
import os
import sys
path = sys.argv[1]
size = os.path.getsize(path)
with open(path, "r+b") as f:
    f.truncate(max(1, size // 2))
PY
}

mutate_size_lie() {
    local root="$1"
    python3 - "$root/components/runtime.json" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
data["files"][0]["size"] = 1073741824
data["size"] = 1073741824
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

mutate_tampered() {
    local root="$1"
    local object
    object="$(find "$root/objects" -type f | head -1)"
    python3 - "$object" <<'PY'
import sys
path = sys.argv[1]
with open(path, "r+b") as f:
    data = bytearray(f.read())
    if not data:
        raise SystemExit("object payload is empty")
    data[0] = ord("X") if data[0] != ord("X") else ord("Y")
    f.seek(0)
    f.write(data)
    f.truncate()
PY
}

build_native_rpm() {
    local tmpdir="$1"
    local output_pkg="$2"
    mkdir -p "$tmpdir"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,BUILDROOT,tmp}
    cat > "$tmpdir/SPECS/adversarial-native.spec" <<'SPEC'
Name: adversarial-native
Version: 1.0.0
Release: 1
Summary: Adversarial native fixture
License: MIT
BuildArch: noarch

%description
Adversarial native fixture.

%install
mkdir -p %{buildroot}/usr/share/adversarial-native
printf 'hello native\n' > %{buildroot}/usr/share/adversarial-native/hello.txt

%files
/usr/share/adversarial-native/hello.txt
SPEC

    rpmbuild \
        --define "_topdir $tmpdir" \
        --define "_tmppath $tmpdir/tmp" \
        --define "__os_install_post %{nil}" \
        -bb "$tmpdir/SPECS/adversarial-native.spec" >/dev/null

    cp "$(find "$tmpdir/RPMS" -name '*.rpm' | head -1)" "$output_pkg"
}

build_native_deb() {
    local tmpdir="$1"
    local output_pkg="$2"
    mkdir -p "$tmpdir/deb/DEBIAN" "$tmpdir/deb/usr/share/adversarial-native"
    cat > "$tmpdir/deb/DEBIAN/control" <<'CONTROL'
Package: adversarial-native
Version: 1.0.0-1
Section: utils
Priority: optional
Architecture: all
Maintainer: Conary Tests <tests@example.invalid>
Description: Adversarial native fixture
CONTROL
    printf 'hello native\n' > "$tmpdir/deb/usr/share/adversarial-native/hello.txt"
    dpkg-deb --build "$tmpdir/deb" "$output_pkg" >/dev/null
}

build_native_arch() {
    local tmpdir="$1"
    local output_pkg="$2"
    mkdir -p "$tmpdir/pkg/usr/share/adversarial-native"
    cat > "$tmpdir/pkg/.PKGINFO" <<'PKGINFO'
pkgname = adversarial-native
pkgver = 1.0.0-1
pkgdesc = Adversarial native fixture
url = https://example.invalid
builddate = 1735689600
packager = Conary Tests <tests@example.invalid>
size = 13
arch = any
license = MIT
PKGINFO
    printf 'hello native\n' > "$tmpdir/pkg/usr/share/adversarial-native/hello.txt"
    tar --format=gnu -C "$tmpdir/pkg" -cf "$tmpdir/adversarial-native.pkg.tar" .
    zstd -q -f "$tmpdir/adversarial-native.pkg.tar" -o "$output_pkg"
}

truncate_native_package() {
    local source_pkg="$1"
    local output_pkg="$2"
    python3 - "$source_pkg" "$output_pkg" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1])
dest = Path(sys.argv[2])
data = source.read_bytes()
cut = max(256, len(data) // 4)
dest.write_bytes(data[:-cut])
PY
}

build_native_fixtures() {
    mkdir -p "$NATIVE_OUTPUT_DIR"
    rm -f "$NATIVE_OUTPUT_DIR"/native-package-corrupted.*

    local tmpdir
    tmpdir="$(mktemp -d /tmp/conary-native-XXXXXX)"
    trap 'rm -rf "$tmpdir"' RETURN

    local rpm_valid="$tmpdir/native-valid.rpm"
    local deb_valid="$tmpdir/native-valid.deb"
    local arch_valid="$tmpdir/native-valid.pkg.tar.zst"

    build_native_rpm "$tmpdir/rpm" "$rpm_valid"
    build_native_deb "$tmpdir/deb-src" "$deb_valid"
    build_native_arch "$tmpdir/arch" "$arch_valid"

    truncate_native_package "$rpm_valid" "$NATIVE_OUTPUT_DIR/native-package-corrupted.rpm"
    truncate_native_package "$deb_valid" "$NATIVE_OUTPUT_DIR/native-package-corrupted.deb"
    truncate_native_package "$arch_valid" "$NATIVE_OUTPUT_DIR/native-package-corrupted.pkg.tar.zst"
}

echo "Building valid corrupted-fixture bases..."
for fixture in bad-checksum truncated size-lie tampered; do
    build_valid_fixture "$SCRIPT_DIR/corrupted/$fixture"
done

echo "Corrupting bad-checksum fixture..."
bad_src="$(find "$SCRIPT_DIR/corrupted/bad-checksum/output" -maxdepth 1 -name '*.ccs' | head -1)"
bad_dst="$SCRIPT_DIR/corrupted/bad-checksum/output/bad-checksum-corrupted.ccs"
mutate_ccs "$bad_src" "$bad_dst" mutate_bad_checksum

echo "Corrupting truncated fixture..."
trunc_src="$(find "$SCRIPT_DIR/corrupted/truncated/output" -maxdepth 1 -name '*.ccs' | head -1)"
trunc_dst="$SCRIPT_DIR/corrupted/truncated/output/truncated-corrupted.ccs"
mutate_ccs "$trunc_src" "$trunc_dst" mutate_truncated

echo "Corrupting size-lie fixture..."
size_src="$(find "$SCRIPT_DIR/corrupted/size-lie/output" -maxdepth 1 -name '*.ccs' | head -1)"
size_dst="$SCRIPT_DIR/corrupted/size-lie/output/size-lie-corrupted.ccs"
mutate_ccs "$size_src" "$size_dst" mutate_size_lie

echo "Corrupting tampered fixture..."
tampered_src="$(find "$SCRIPT_DIR/corrupted/tampered/output" -maxdepth 1 -name '*.ccs' | head -1)"
tampered_dst="$SCRIPT_DIR/corrupted/tampered/output/tampered-corrupted.ccs"
mutate_ccs "$tampered_src" "$tampered_dst" mutate_tampered

echo "Building corrupted native package fixtures..."
build_native_fixtures

echo "[OK] Corrupted fixtures built:"
printf '  %s\n' "$bad_dst" "$trunc_dst" "$size_dst" "$tampered_dst"
printf '  %s\n' \
    "$NATIVE_OUTPUT_DIR/native-package-corrupted.rpm" \
    "$NATIVE_OUTPUT_DIR/native-package-corrupted.deb" \
    "$NATIVE_OUTPUT_DIR/native-package-corrupted.pkg.tar.zst"
