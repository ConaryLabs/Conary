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

# Exact pinned texts: the repository's MIT file (with its copyright line), the
# canonical Apache-2.0 text, and the canonical AGPL-3.0 text. Any edit to a
# license file must update its digest here in the same change.
license_mit_sha256='3681cd22a07cf6ae3dbadde8a04fde421b28f30c9c378e8031874ab95025cd83'
license_apache_sha256='cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'
license_agpl_sha256='0d96a4ff68ad6d4b6f1f30f713b18d5184912ba8dd389f86aa7710db079abcb0'

require_exact_text() {
    local file="$1" expected="$2" actual
    [[ -f "$file" && ! -L "$file" ]] || fail "license text $file is missing"
    actual="$(sha256sum "$file" | cut -d ' ' -f 1)"
    [[ "$actual" == "$expected" ]] || fail "$file sha256 is $actual, expected $expected"
}
require_exact_text LICENSE-MIT "$license_mit_sha256"
require_exact_text LICENSE-APACHE "$license_apache_sha256"
require_exact_text apps/remi/LICENSE "$license_agpl_sha256"
[[ ! -e LICENSE ]] || fail "a bare LICENSE file would shadow the dual-license files; remove it"

require_match() {
    local file="$1" pattern="$2" description="$3"
    grep -qE -- "$pattern" "$file" || fail "$description missing in $file"
}
# An install line counts only when it is live: first non-blank text on the
# line, never behind a comment marker.
require_live_line() {
    local file="$1" pattern="$2" description="$3"
    grep -qE -- "^[[:space:]]*${pattern}" "$file" || fail "$description missing (or commented out) in $file"
}
require_match Cargo.toml '^license = "MIT OR Apache-2\.0"$' 'workspace license'
require_match apps/remi/Cargo.toml '^license = "AGPL-3\.0-or-later"$' 'remi license override'
# Metadata fields must be assigned exactly once so a later override cannot
# change the effective value; the PKGBUILD value is also evaluated as makepkg
# would see it.
require_unique_line() {
    local file="$1" pattern="$2" description="$3" count
    count="$(grep -cE -- "$pattern" "$file" || true)"
    [[ "$count" -eq 1 ]] || fail "$description must be assigned exactly once in $file (found $count)"
}
require_unique_line packaging/rpm/conary.spec '^License:' 'rpm License field'
require_match packaging/rpm/conary.spec '^License:[[:space:]]+MIT OR Apache-2\.0$' 'rpm License field'
require_unique_line packaging/arch/PKGBUILD '^[[:space:]]*license=' 'PKGBUILD license array'
effective_pkgbuild_license="$(bash -c 'set -eu; source "$1"; printf "%s\n" "${license[@]}"' _ packaging/arch/PKGBUILD 2>/dev/null | paste -sd ' ')"
[[ "$effective_pkgbuild_license" == "MIT Apache-2.0" ]] ||
    fail "PKGBUILD evaluates license to '${effective_pkgbuild_license}', expected 'MIT Apache-2.0'"
require_unique_line packaging/ccs/ccs.toml '^license[[:space:]]*=' 'ccs manifest license'
require_unique_line Cargo.toml '^license[[:space:]]*=' 'workspace license'
require_unique_line apps/remi/Cargo.toml '^license[[:space:]]*=' 'remi license'
require_live_line packaging/rpm/conary.spec '%license LICENSE-MIT LICENSE-APACHE$' 'rpm %license files'
require_match packaging/arch/PKGBUILD "^license=\\('MIT' 'Apache-2\\.0'\\)$" 'PKGBUILD license array'
require_match packaging/ccs/ccs.toml '^license = "MIT OR Apache-2\.0"$' 'ccs manifest license'
# DEP-5 paragraphs are blank-line separated; the License field must be read
# from the same paragraph as its Files field, never from elsewhere in the file.
dep5_paragraph_license() {
    local file="$1" files_glob="$2"
    awk -v files="Files: ${files_glob}" '
        /^$/ { in_paragraph = 0; next }
        $0 == files { in_paragraph = 1; next }
        in_paragraph && /^License: / { sub(/^License: /, ""); print; exit }
    ' "$file"
}
require_dep5_license() {
    local file="$1" files_glob="$2" expected="$3" actual
    actual="$(dep5_paragraph_license "$file" "$files_glob")"
    [[ "$actual" == "$expected" ]] ||
        fail "$file paragraph 'Files: $files_glob' declares License '${actual:-<none>}', expected '$expected'"
}
require_dep5_license packaging/deb/debian/copyright '*' 'MIT or Apache-2.0'
require_dep5_license packaging/deb/debian/copyright 'apps/remi/*' 'AGPL-3.0+'
require_match packaging/deb/debian/copyright '^License: Apache-2\.0$' 'debian copyright Apache text paragraph'
require_match packaging/deb/debian/copyright '^License: AGPL-3\.0\+$' 'debian copyright AGPL text paragraph'
# Every package builder must install both texts, source and destination.
require_live_line packaging/ccs/build.sh 'install -Dpm 0644 "\$REPO_ROOT/LICENSE-MIT" "\$STAGE/usr/share/licenses/\$NAME/LICENSE-MIT"' 'ccs bundle MIT license install'
require_live_line packaging/ccs/build.sh 'install -Dpm 0644 "\$REPO_ROOT/LICENSE-APACHE" "\$STAGE/usr/share/licenses/\$NAME/LICENSE-APACHE"' 'ccs bundle Apache license install'
require_live_line packaging/arch/PKGBUILD 'install -Dm644 LICENSE-MIT "\$pkgdir/usr/share/licenses/\$pkgname/LICENSE-MIT"' 'PKGBUILD MIT license install'
require_live_line packaging/arch/PKGBUILD 'install -Dm644 LICENSE-APACHE "\$pkgdir/usr/share/licenses/\$pkgname/LICENSE-APACHE"' 'PKGBUILD Apache license install'
require_live_line packaging/rpm/conary.spec 'install -Dpm 0644 LICENSE-MIT %\{buildroot\}%\{_datadir\}/licenses/%\{crate\}/LICENSE-MIT' 'rpm MIT license install'
require_live_line packaging/rpm/conary.spec 'install -Dpm 0644 LICENSE-APACHE %\{buildroot\}%\{_datadir\}/licenses/%\{crate\}/LICENSE-APACHE' 'rpm Apache license install'
require_live_line packaging/deb/debian/rules 'install -Dpm 0644 LICENSE-MIT ' 'debian rules MIT install'
require_live_line packaging/deb/debian/rules 'install -Dpm 0644 LICENSE-APACHE ' 'debian rules Apache install'
require_live_line packaging/deb/debian/rules 'dh_compress -X LICENSE-MIT -X LICENSE-APACHE' 'debian rules license compress exclusion'
# The release workflow is parsed as YAML: a required command must appear in a
# live step `run` script (comment lines stripped), never in dead text.
command -v python3 >/dev/null || fail "python3 is required to parse the release workflow"
python3 -I - .github/workflows/release-build.yml <<'PY' || fail "release workflow does not carry every license proof as a live step"
import sys
try:
    import yaml
except ImportError:
    sys.exit("PyYAML is required to parse the release workflow")
path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    workflow = yaml.safe_load(handle)
live = []
for job_id, job in (workflow.get("jobs") or {}).items():
    for step in job.get("steps") or []:
        run = step.get("run")
        if not isinstance(run, str):
            continue
        lines = [line for line in run.splitlines() if not line.lstrip().startswith("#")]
        live.append((job_id, "\n".join(lines)))
required = {
    "remi tarball AGPL text": 'apps/remi" LICENSE',
    "suite release AGPL asset": "copy_exact apps/remi/LICENSE LICENSE-AGPL-3.0-remi",
    "suite release license asset proof": "check-release-license-contents.sh suite suite-packages",
}
for kind in ["rpm", "deb", "arch", "ccs", "remi-tar", "client-tar"]:
    required[f"packaged-contents proof for {kind}"] = f"check-release-license-contents.sh {kind} "
missing = [name for name, needle in required.items() if not any(needle in text for _, text in live)]
if missing:
    sys.exit("missing live release proof: " + ", ".join(missing))
client_tar_jobs = {job for job, text in live if "check-release-license-contents.sh client-tar " in text}
if not {"build-conaryd", "build-conary-test"} <= client_tar_jobs:
    sys.exit("client-tar proofs must run in build-conaryd and build-conary-test, found: " + ", ".join(sorted(client_tar_jobs)))
PY
require_match scripts/remi-candidate-artifact.sh '-C "\$license_dir" LICENSE' 'candidate bundle AGPL text'
require_match README.md 'LICENSE-MIT' 'README dual-license link'
require_match README.md 'apps/remi/LICENSE' 'README Remi license link'

if grep -rEn --include=rules --include=*.sh --include=*.spec --include=PKGBUILD -- '(^|[[:space:]"/])LICENSE([[:space:]"]|$)' packaging >/dev/null; then
    fail "packaging still installs a bare LICENSE file: $(grep -rEn --include=rules --include=*.sh --include=*.spec --include=PKGBUILD -- '(^|[[:space:]"/])LICENSE([[:space:]"]|$)' packaging | head -n 1)"
fi

echo "License authority checks passed."
