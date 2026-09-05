#!/usr/bin/env bash
# scripts/check-release-license-contents.sh
#
# Prove that a built release artifact actually carries its license texts.
# Runs inside the job that built the artifact, with that ecosystem's native
# listing tool, so the proof is over packaged contents rather than recipe text.
#
#   check-release-license-contents.sh rpm <file.rpm> <LICENSE-MIT> <LICENSE-APACHE>
#   check-release-license-contents.sh deb <file.deb> <LICENSE-MIT> <LICENSE-APACHE>
#   check-release-license-contents.sh arch <file.pkg.tar.zst> <LICENSE-MIT> <LICENSE-APACHE>
#   check-release-license-contents.sh ccs <file.ccs> <conary-binary> <LICENSE-MIT> <LICENSE-APACHE> <trust-policy.toml>
#   check-release-license-contents.sh remi-tar <remi-<version>-linux-x64.tar.gz> [<agpl-text>]
#   check-release-license-contents.sh client-tar <product-<version>-linux-x64.tar.gz> <LICENSE-MIT> <LICENSE-APACHE>
#   check-release-license-contents.sh suite <suite-packages-dir> <LICENSE-MIT> <LICENSE-APACHE> <remi-agpl-text>
set -euo pipefail

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

kind="${1:-}"
artifact="${2:-}"
[[ -n "$kind" && -n "$artifact" ]] || fail "usage: $0 <rpm|deb|arch|ccs|remi-tar> <artifact> [tool-or-text]"
if [[ "$kind" == suite ]]; then
    [[ -d "$artifact" ]] || fail "suite directory is missing: $artifact"
else
    [[ -f "$artifact" && ! -L "$artifact" ]] || fail "artifact is not a plain file: $artifact"
fi

sha256_of() { sha256sum "$1" | cut -d ' ' -f 1; }
require_same_text() {
    local actual_file="$1" expected_file="$2" label="$3"
    [[ -f "$expected_file" ]] || fail "$label reference text $expected_file is missing"
    [[ -f "$actual_file" && ! -L "$actual_file" ]] || fail "$label $actual_file is missing"
    [[ "$(sha256_of "$actual_file")" == "$(sha256_of "$expected_file")" ]] || fail "$label $actual_file differs from $expected_file"
}

client_license_dir='usr/share/licenses/conary'
client_doc_dir='usr/share/doc/conary'

require_entries() {
    local listing="$1"
    shift
    local entry
    for entry in "$@"; do
        grep -Fxq -- "$entry" <<<"$listing" || fail "$kind artifact $artifact does not contain $entry"
    done
}

case "$kind" in
    rpm)
        mit_text="${3:-}"; apache_text="${4:-}"
        [[ -n "$mit_text" && -n "$apache_text" ]] || fail "rpm needs the MIT and Apache texts as the third and fourth arguments"
        command -v rpm2cpio >/dev/null && command -v cpio >/dev/null || fail "rpm2cpio and cpio are required to extract $artifact"
        for pair in "LICENSE-MIT:$mit_text" "LICENSE-APACHE:$apache_text"; do
            member="${pair%%:*}"; reference="${pair#*:}"
            # cpio member names may carry a leading "./" (rpm2cpio) or not.
            actual="$(rpm2cpio "$artifact" | cpio -i --quiet --to-stdout "*${client_license_dir}/${member}" 2>/dev/null | sha256sum | cut -d ' ' -f 1)"
            [[ "$actual" == "$(sha256_of "$reference")" ]] || fail "rpm artifact $artifact member /${client_license_dir}/${member} is missing or differs from $reference"
        done
        ;;
    deb)
        mit_text="${3:-}"; apache_text="${4:-}"
        [[ -n "$mit_text" && -n "$apache_text" ]] || fail "deb needs the MIT and Apache texts as the third and fourth arguments"
        command -v dpkg-deb >/dev/null || fail "dpkg-deb is required to extract $artifact"
        for pair in "LICENSE-MIT:$mit_text" "LICENSE-APACHE:$apache_text"; do
            member="${pair%%:*}"; reference="${pair#*:}"
            actual="$(dpkg-deb --fsys-tarfile "$artifact" | tar -xO "./${client_doc_dir}/${member}" 2>/dev/null | sha256sum | cut -d ' ' -f 1)"
            [[ "$actual" == "$(sha256_of "$reference")" ]] || fail "deb artifact $artifact member ./${client_doc_dir}/${member} is missing or differs from $reference"
        done
        ;;
    arch)
        mit_text="${3:-}"; apache_text="${4:-}"
        [[ -n "$mit_text" && -n "$apache_text" ]] || fail "arch needs the MIT and Apache texts as the third and fourth arguments"
        for pair in "LICENSE-MIT:$mit_text" "LICENSE-APACHE:$apache_text"; do
            member="${pair%%:*}"; reference="${pair#*:}"
            actual="$(tar --zstd -xOf "$artifact" "${client_license_dir}/${member}" 2>/dev/null | sha256sum | cut -d ' ' -f 1)"
            [[ "$actual" == "$(sha256_of "$reference")" ]] || fail "arch artifact $artifact member ${client_license_dir}/${member} is missing or differs from $reference"
        done
        ;;
    ccs)
        # CCS payload files carry a signed content digest in the JSON
        # inspection; compare it to the reference text under the same trust
        # policy the build verified with.
        conary="${3:-}"; mit_text="${4:-}"; apache_text="${5:-}"; policy="${6:-}"
        [[ -n "$conary" && -x "$conary" && -n "$mit_text" && -n "$apache_text" && -n "$policy" ]] ||
            fail "ccs needs the conary binary, the MIT and Apache texts, and the trust policy as arguments"
        [[ -f "$policy" && ! -L "$policy" ]] || fail "ccs trust policy $policy is missing"
        "$conary" ccs verify "$artifact" --policy "$policy" >/dev/null || fail "ccs artifact $artifact fails its own verification"
        # `ccs inspect` reads the package untrusted and takes no policy; only
        # `ccs verify` does. Verification above proves the signed digests the
        # inspection reports.
        inspection="$("$conary" ccs inspect "$artifact" --files --format json)" || fail "ccs artifact $artifact could not be inspected"
        for pair in "LICENSE-MIT:$mit_text" "LICENSE-APACHE:$apache_text"; do
            member="${pair%%:*}"; reference="${pair#*:}"
            actual="$(jq -r --arg path "/${client_license_dir}/${member}" '[.files[] | select(.path == $path) | .content.sha256] | if length == 1 then .[0] else "" end' <<<"$inspection")"
            [[ "$actual" == "$(sha256_of "$reference")" ]] || fail "ccs artifact $artifact member /${client_license_dir}/${member} is missing or its signed digest differs from $reference"
        done
        ;;
    remi-tar)
        listing="$(tar -tzf "$artifact" | sort)"
        base="$(basename "$artifact" .tar.gz)"
        expected="$(printf '%s\n%s\n' LICENSE "$base" | sort)"
        [[ "$listing" == "$expected" ]] ||
            fail "remi bundle $artifact members are not exactly $base and LICENSE: $(tr '\n' ' ' <<<"$listing")"
        agpl_text="${3:-}"
        if [[ -n "$agpl_text" ]]; then
            [[ -f "$agpl_text" ]] || fail "AGPL text $agpl_text is missing"
            bundled="$(tar -xOzf "$artifact" LICENSE | sha256sum | cut -d ' ' -f 1)"
            expected_sha="$(sha256sum "$agpl_text" | cut -d ' ' -f 1)"
            [[ "$bundled" == "$expected_sha" ]] || fail "remi bundle LICENSE differs from $agpl_text"
        fi
        ;;
    client-tar)
        mit_text="${3:-}"; apache_text="${4:-}"
        [[ -n "$mit_text" && -n "$apache_text" ]] || fail "client-tar needs the MIT and Apache texts as the third and fourth arguments"
        listing="$(tar -tzf "$artifact" | sort)"
        base="$(basename "$artifact" .tar.gz)"
        expected="$(printf '%s\n%s\n%s\n' LICENSE-APACHE LICENSE-MIT "$base" | sort)"
        [[ "$listing" == "$expected" ]] ||
            fail "client bundle $artifact members are not exactly $base, LICENSE-MIT, LICENSE-APACHE: $(tr '\n' ' ' <<<"$listing")"
        for pair in "LICENSE-MIT:$mit_text" "LICENSE-APACHE:$apache_text"; do
            member="${pair%%:*}"; reference="${pair#*:}"
            [[ "$(tar -xOzf "$artifact" "$member" | sha256sum | cut -d ' ' -f 1)" == "$(sha256_of "$reference")" ]] ||
                fail "client bundle $artifact member $member differs from $reference"
        done
        ;;
    suite)
        mit_text="${3:-}"; apache_text="${4:-}"; agpl_text="${5:-}"
        [[ -n "$mit_text" && -n "$apache_text" && -n "$agpl_text" ]] || fail "suite needs the MIT, Apache, and AGPL texts as arguments"
        require_same_text "$artifact/LICENSE-MIT" "$mit_text" "suite asset"
        require_same_text "$artifact/LICENSE-APACHE" "$apache_text" "suite asset"
        require_same_text "$artifact/LICENSE-AGPL-3.0-remi" "$agpl_text" "suite asset"
        ;;
    *)
        fail "unknown artifact kind: $kind"
        ;;
esac

echo "License contents verified for $kind artifact $artifact."
