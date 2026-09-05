#!/usr/bin/env bash
# scripts/test-license-authority.sh
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

bash scripts/check-license-authority.sh >/dev/null || {
    echo "FAIL: the live tree must pass the license authority check" >&2
    exit 1
}
echo "ok: live tree passes"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Build a minimal fixture workspace that mirrors the live authority files.
make_fixture() {
    local root="$1"
    rm -rf "$root"
    mkdir -p "$root"
    cp -r Cargo.toml Cargo.lock LICENSE-MIT LICENSE-APACHE README.md "$root/"
    mkdir -p "$root/packaging/rpm" "$root/packaging/arch" "$root/packaging/ccs" "$root/packaging/deb/debian" "$root/.github/workflows" "$root/scripts"
    cp packaging/rpm/conary.spec "$root/packaging/rpm/"
    cp packaging/arch/PKGBUILD "$root/packaging/arch/"
    cp packaging/ccs/ccs.toml packaging/ccs/build.sh "$root/packaging/ccs/"
    cp packaging/deb/debian/copyright "$root/packaging/deb/debian/"
    cp .github/workflows/release-build.yml "$root/.github/workflows/"
    cp scripts/check-license-authority.sh "$root/scripts/"
    for manifest in apps/*/Cargo.toml crates/*/Cargo.toml; do
        mkdir -p "$root/$(dirname "$manifest")/src"
        cp "$manifest" "$root/$manifest"
        : > "$root/$(dirname "$manifest")/src/lib.rs"
    done
    mkdir -p "$root/apps/remi/src/bin"
    printf 'fn main() {}\n' > "$root/apps/remi/src/bin/remi.rs"
    cp apps/remi/LICENSE "$root/apps/remi/LICENSE"
    ( cd "$root" && git init -q && git add -A >/dev/null 2>&1 && git -c user.email=t@t -c user.name=t commit -qm fixture >/dev/null 2>&1 )
}

expect_failure() {
    local description="$1" root="$2"
    if LICENSE_AUTHORITY_ROOT="$root" bash scripts/check-license-authority.sh >/dev/null 2>"$tmp/err"; then
        echo "FAIL: $description unexpectedly passed" >&2
        exit 1
    fi
    echo "ok: $description rejected ($(head -n1 "$tmp/err"))"
}

# Fixture manifests are not buildable; the checker only needs cargo metadata,
# which requires bin/lib targets to exist, provided above.
root="$tmp/fixture"
make_fixture "$root"
LICENSE_AUTHORITY_ROOT="$root" bash scripts/check-license-authority.sh >/dev/null || {
    echo "FAIL: baseline fixture must pass" >&2
    exit 1
}
echo "ok: baseline fixture passes"

make_fixture "$root"
sed -i 's/^license = "AGPL-3.0-or-later"$/license = "MIT OR Apache-2.0"/' "$root/apps/remi/Cargo.toml"
expect_failure "remi declared permissive" "$root"

make_fixture "$root"
sed -i 's/^license = "MIT OR Apache-2.0"$/license = "AGPL-3.0-or-later"/' "$root/Cargo.toml"
expect_failure "client declared copyleft" "$root"

make_fixture "$root"
rm "$root/LICENSE-APACHE"
expect_failure "missing Apache text" "$root"

make_fixture "$root"
cp "$root/LICENSE-MIT" "$root/LICENSE"
expect_failure "bare LICENSE file" "$root"

make_fixture "$root"
# Corrupt the Apache text below its heading: the first lines still match.
sed -i '120,140d' "$root/LICENSE-APACHE"
expect_failure "truncated Apache text" "$root"

make_fixture "$root"
sed -i 's/Version 3, 19 November 2007/Version 3, 19 November 2008/' "$root/apps/remi/LICENSE"
expect_failure "altered AGPL text" "$root"

make_fixture "$root"
sed -i '/^Files: apps\/remi\/\*$/,/^$/d' "$root/packaging/deb/debian/copyright"
expect_failure "missing Remi Debian stanza" "$root"

make_fixture "$root"
sed -i 's/^License:        MIT OR Apache-2.0$/License:        MIT/' "$root/packaging/rpm/conary.spec"
expect_failure "rpm License drift" "$root"

make_fixture "$root"
sed -i 's/^License: MIT or Apache-2.0$/License: MIT/' "$root/packaging/deb/debian/copyright"
expect_failure "debian copyright drift" "$root"

echo "license authority tests passed"
