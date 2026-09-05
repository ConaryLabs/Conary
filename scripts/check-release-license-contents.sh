#!/usr/bin/env bash
# scripts/check-release-license-contents.sh
#
# Prove that a built release artifact actually carries its license texts.
# Runs inside the job that built the artifact, with that ecosystem's native
# listing tool, so the proof is over packaged contents rather than recipe text.
#
#   check-release-license-contents.sh rpm <file.rpm>
#   check-release-license-contents.sh deb <file.deb>
#   check-release-license-contents.sh arch <file.pkg.tar.zst>
#   check-release-license-contents.sh ccs <file.ccs> <conary-binary>
#   check-release-license-contents.sh remi-tar <remi-<version>-linux-x64.tar.gz> [<agpl-text>]
set -euo pipefail

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

kind="${1:-}"
artifact="${2:-}"
[[ -n "$kind" && -n "$artifact" ]] || fail "usage: $0 <rpm|deb|arch|ccs|remi-tar> <artifact> [tool-or-text]"
[[ -f "$artifact" && ! -L "$artifact" ]] || fail "artifact is not a plain file: $artifact"

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
        command -v rpm >/dev/null || fail "rpm is required to list $artifact"
        listing="$(rpm -qlp "$artifact")"
        require_entries "$listing" "/${client_license_dir}/LICENSE-MIT" "/${client_license_dir}/LICENSE-APACHE"
        ;;
    deb)
        command -v dpkg-deb >/dev/null || fail "dpkg-deb is required to list $artifact"
        listing="$(dpkg-deb -c "$artifact" | awk '{ print $6 }')"
        require_entries "$listing" "./${client_doc_dir}/LICENSE-MIT" "./${client_doc_dir}/LICENSE-APACHE"
        ;;
    arch)
        listing="$(tar --zstd -tf "$artifact")"
        require_entries "$listing" "${client_license_dir}/LICENSE-MIT" "${client_license_dir}/LICENSE-APACHE"
        ;;
    ccs)
        conary="${3:-}"
        [[ -n "$conary" && -x "$conary" ]] || fail "ccs listing needs an executable conary binary as the third argument"
        listing="$("$conary" ccs inspect --files "$artifact")"
        for entry in "/${client_license_dir}/LICENSE-MIT" "/${client_license_dir}/LICENSE-APACHE"; do
            grep -Fq -- "$entry" <<<"$listing" || fail "ccs artifact $artifact does not contain $entry"
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
    *)
        fail "unknown artifact kind: $kind"
        ;;
esac

echo "License contents verified for $kind artifact $artifact."
