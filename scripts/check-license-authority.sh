#!/usr/bin/env bash
# scripts/check-license-authority.sh
#
# Pin every workspace crate, license file, and packaging field to the
# licensing decision in issue #900: the client and libraries are
# MIT OR Apache-2.0; the Remi server is AGPL-3.0-or-later.
set -euo pipefail

repo_root="${LICENSE_AUTHORITY_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

client_license='MIT OR Apache-2.0'
remi_license='AGPL-3.0-or-later'

# Expected license per workspace package; every member must appear here.
expected_license_for() {
    case "$1" in
        remi) printf '%s\n' "$remi_license" ;;
        conary | conaryd | conary-test | conary-core | conary-bootstrap | conary-mcp | conary-agent-contract | conary-xtask)
            printf '%s\n' "$client_license" ;;
        *) return 1 ;;
    esac
}

command -v cargo >/dev/null || fail "cargo is required"
command -v jq >/dev/null || fail "jq is required"

metadata="$(cargo metadata --no-deps --format-version 1 --offline 2>/dev/null || cargo metadata --no-deps --format-version 1)"
while IFS=$'\t' read -r name license; do
    expected="$(expected_license_for "$name")" ||
        fail "workspace package $name has no licensing decision; add it to check-license-authority.sh"
    [[ "$license" == "$expected" ]] ||
        fail "package $name declares license '$license'; the decision is '$expected'"
done < <(jq -r '.packages[] | [.name, (.license // "")] | @tsv' <<<"$metadata")

require_first_line() {
    local file="$1" expected="$2" actual
    [[ -f "$file" && ! -L "$file" ]] || fail "license text $file is missing"
    actual="$(sed -n '/[^[:space:]]/{s/^[[:space:]]*//;s/[[:space:]]*$//;p;q}' "$file")"
    [[ "$actual" == "$expected" ]] || fail "$file first line is '$actual', expected '$expected'"
}
require_first_line LICENSE-MIT 'MIT License'
require_first_line LICENSE-APACHE 'Apache License'
require_first_line apps/remi/LICENSE 'GNU AFFERO GENERAL PUBLIC LICENSE'
[[ ! -e LICENSE ]] || fail "a bare LICENSE file would shadow the dual-license files; remove it"

require_match() {
    local file="$1" pattern="$2" description="$3"
    grep -qE -- "$pattern" "$file" || fail "$description missing in $file"
}
require_match Cargo.toml '^license = "MIT OR Apache-2\.0"$' 'workspace license'
require_match apps/remi/Cargo.toml '^license = "AGPL-3\.0-or-later"$' 'remi license override'
require_match packaging/rpm/conary.spec '^License:[[:space:]]+MIT OR Apache-2\.0$' 'rpm License field'
require_match packaging/rpm/conary.spec '^%license LICENSE-MIT LICENSE-APACHE$' 'rpm %license files'
require_match packaging/arch/PKGBUILD "^license=\\('MIT' 'Apache-2\\.0'\\)$" 'PKGBUILD license array'
require_match packaging/ccs/ccs.toml '^license = "MIT OR Apache-2\.0"$' 'ccs manifest license'
require_match packaging/deb/debian/copyright '^License: MIT or Apache-2\.0$' 'debian copyright Files stanza'
require_match packaging/deb/debian/copyright '^License: Apache-2\.0$' 'debian copyright Apache text'
require_match packaging/ccs/build.sh 'LICENSE-APACHE' 'ccs bundle Apache license install'
require_match .github/workflows/release-build.yml 'apps/remi" LICENSE' 'remi tarball AGPL text'
require_match README.md 'LICENSE-MIT' 'README dual-license link'
require_match README.md 'apps/remi/LICENSE' 'README Remi license link'

echo "License authority checks passed."
