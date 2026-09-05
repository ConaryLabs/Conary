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
    cp packaging/deb/debian/rules "$root/packaging/deb/debian/"
    mkdir -p "$root/recipes/tier2" && cp recipes/tier2/conary.toml "$root/recipes/tier2/"
    cp .github/workflows/release-build.yml "$root/.github/workflows/"
    cp scripts/check-license-authority.sh scripts/remi-candidate-artifact.sh "$root/scripts/"
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
# Only the Remi paragraph's own License line changes; the AGPL text paragraph
# later in the file still carries a License: AGPL-3.0+ header.
sed -i '/^Files: apps\/remi\/\*$/,/^$/ s/^License: AGPL-3.0+$/License: MIT/' "$root/packaging/deb/debian/copyright"
expect_failure "Remi paragraph relicensed while the AGPL text paragraph remains" "$root"

make_fixture "$root"
sed -i '/^Files: \*$/,/^$/ s/^License: MIT or Apache-2.0$/License: AGPL-3.0+/' "$root/packaging/deb/debian/copyright"
expect_failure "client paragraph declared AGPL" "$root"

make_fixture "$root"
sed -i 's#install -Dpm 0644 LICENSE-MIT debian/conary/usr/share/doc/conary/LICENSE-MIT#install -Dpm 0644 LICENSE debian/conary/usr/share/doc/conary/LICENSE#' "$root/packaging/deb/debian/rules"
expect_failure "debian rules installing a bare LICENSE" "$root"

make_fixture "$root"
sed -i '/LICENSE-MIT/d' "$root/packaging/ccs/build.sh"
expect_failure "ccs bundle missing the MIT install" "$root"

make_fixture "$root"
sed -i 's|^install -Dpm 0644 "$REPO_ROOT/LICENSE-MIT"|#&|' "$root/packaging/ccs/build.sh"
expect_failure "ccs bundle MIT install commented out" "$root"

make_fixture "$root"
sed -i 's|^\(\s*\)install -Dm644 LICENSE-APACHE|\1# install -Dm644 LICENSE-APACHE|' "$root/packaging/arch/PKGBUILD"
expect_failure "PKGBUILD Apache install commented out" "$root"

make_fixture "$root"
sed -i '/dh_compress -X LICENSE-MIT/d' "$root/packaging/deb/debian/rules"
expect_failure "debian rules compressing the license texts" "$root"

make_fixture "$root"
sed -i '/check-release-license-contents.sh rpm /d' "$root/.github/workflows/release-build.yml"
expect_failure "release-build without the rpm contents proof" "$root"

make_fixture "$root"
sed -i 's|^\(\s*\)run: bash scripts/check-release-license-contents.sh rpm |\1run: "# disabled" # bash scripts/check-release-license-contents.sh rpm |' "$root/.github/workflows/release-build.yml"
expect_failure "release-build rpm proof commented out inside the step" "$root"

make_fixture "$root"
python3 - "$root/.github/workflows/release-build.yml" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
needle = "run: bash scripts/check-release-license-contents.sh deb packaging/deb/output/*.deb LICENSE-MIT LICENSE-APACHE"
assert needle in text
# Move the proof into dead text: a YAML comment line that still contains the command.
text = text.replace(needle, "run: 'true'\n        # " + needle, 1)
open(path, "w", encoding="utf-8").write(text)
PY
expect_failure "release-build deb proof present only as a YAML comment" "$root"

make_fixture "$root"
sed -i 's|^\(\s*\)run: bash scripts/check-release-license-contents.sh rpm \(.*\)$|\1run: echo "bash scripts/check-release-license-contents.sh rpm \2"|' "$root/.github/workflows/release-build.yml"
expect_failure "release-build rpm proof only echoed, not executed" "$root"

make_fixture "$root"
python3 - "$root/.github/workflows/release-build.yml" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
needle = "        run: bash scripts/check-release-license-contents.sh arch packaging/arch/output/*.pkg.tar.zst LICENSE-MIT LICENSE-APACHE\n"
assert needle in text
# Execute the arch proof, but from the deb job instead of its owner.
text = text.replace(needle, "        run: 'true'\n", 1)
deb = "        run: bash scripts/check-release-license-contents.sh deb packaging/deb/output/*.deb LICENSE-MIT LICENSE-APACHE\n"
assert deb in text
text = text.replace(deb, deb + "      - name: Misplaced arch proof\n        run: bash scripts/check-release-license-contents.sh arch packaging/arch/output/*.pkg.tar.zst LICENSE-MIT LICENSE-APACHE\n", 1)
open(path, "w", encoding="utf-8").write(text)
PY
expect_failure "release-build arch proof executed by the wrong job" "$root"

make_fixture "$root"
sed -i 's|^\(\s*\)run: bash scripts/check-release-license-contents.sh ccs |\1continue-on-error: true\n\1run: bash scripts/check-release-license-contents.sh ccs |' "$root/.github/workflows/release-build.yml"
expect_failure "release-build ccs proof allowed to fail" "$root"

make_fixture "$root"
sed -i "s|^\(\s*\)if: \${{ hashFiles('scripts/check-release-license-contents.sh') != '' }}$|\1if: \${{ github.event_name == 'never' }}|" "$root/.github/workflows/release-build.yml"
expect_failure "release-build proofs under a condition other than the tree guard" "$root"

make_fixture "$root"
sed -i 's|^\(\s*\)if \[\[ -f scripts/check-release-license-contents.sh \]\]; then$|\1if true; then|' "$root/.github/workflows/release-build.yml"
expect_failure "release-build license packaging without the tree guard" "$root"

make_fixture "$root"
sed -i 's|^\(\s*run: bash scripts/check-release-license-contents.sh rpm .*\)$|\1 \|\| true|' "$root/.github/workflows/release-build.yml"
expect_failure "release-build rpm proof masked with || true" "$root"

make_fixture "$root"
sed -i 's|^\(\s*run: bash scripts/check-release-license-contents.sh deb .*\)$|\1; true|' "$root/.github/workflows/release-build.yml"
expect_failure "release-build deb proof followed by a masking command" "$root"

make_fixture "$root"
sed -i 's|^\(\s*\)run: bash scripts/check-release-license-contents.sh arch |\1shell: bash {0}\n\1run: bash scripts/check-release-license-contents.sh arch |' "$root/.github/workflows/release-build.yml"
expect_failure "release-build arch proof under a shell without errexit" "$root"

make_fixture "$root"
sed -i 's|^\(\s*\)run: bash scripts/check-release-license-contents.sh rpm \(.*\)$|\1run: \|\n\1  exit 0\n\1  bash scripts/check-release-license-contents.sh rpm \2|' "$root/.github/workflows/release-build.yml"
expect_failure "release-build rpm proof unreachable after an earlier exit" "$root"

make_fixture "$root"
printf '\nFiles: apps/remi/*\nCopyright: 2024-2026 Conary Contributors\nLicense: MIT\n' >> "$root/packaging/deb/debian/copyright"
expect_failure "duplicate Remi Debian paragraph with a conflicting license" "$root"

make_fixture "$root"
sed -i 's/^license = "MIT OR Apache-2.0"$/license = "MIT"/' "$root/recipes/tier2/conary.toml"
expect_failure "tier2 recipe license drift" "$root"

make_fixture "$root"
sed -i '/install -Dm644 LICENSE-APACHE %(destdir)s/d' "$root/recipes/tier2/conary.toml"
expect_failure "tier2 recipe missing the Apache install" "$root"

make_fixture "$root"
sed -i '/install -Dm644 LICENSE-APACHE/d' "$root/packaging/arch/PKGBUILD"
expect_failure "PKGBUILD missing the Apache install" "$root"

make_fixture "$root"
sed -i '/install -Dm644 LICENSE-MIT/d' "$root/packaging/arch/PKGBUILD"
expect_failure "PKGBUILD missing the MIT install" "$root"

make_fixture "$root"
sed -i '/install -Dpm 0644 LICENSE-APACHE %{buildroot}/d' "$root/packaging/rpm/conary.spec"
expect_failure "rpm spec missing the Apache install" "$root"

make_fixture "$root"
printf "license=('GPL')\n" >> "$root/packaging/arch/PKGBUILD"
expect_failure "PKGBUILD license overridden by a later assignment" "$root"

make_fixture "$root"
printf 'License:        GPL-3.0-or-later\n' >> "$root/packaging/rpm/conary.spec"
expect_failure "rpm spec with a second License field" "$root"

make_fixture "$root"
printf '\n[package]\nlicense = "GPL-3.0-or-later"\n' >> "$root/packaging/ccs/ccs.toml"
expect_failure "ccs manifest with a second license key" "$root"

make_fixture "$root"
sed -i 's/^License:        MIT OR Apache-2.0$/License:        MIT/' "$root/packaging/rpm/conary.spec"
expect_failure "rpm License drift" "$root"

make_fixture "$root"
sed -i 's/^License: MIT or Apache-2.0$/License: MIT/' "$root/packaging/deb/debian/copyright"
expect_failure "debian copyright drift" "$root"

echo "license authority tests passed"
