#!/usr/bin/env bash
# site/static/install-conary-preview.sh -- Verify and preview/apply a Conary release bootstrap.
set -euo pipefail

readonly DEFAULT_MANIFEST_URL="https://github.com/ConaryLabs/Conary/releases/latest/download/conary-bootstrap-v1.manifest"
# DER SubjectPublicKeyInfo for the Ed25519 release key embedded by conary-core.
readonly RELEASE_PUBLIC_KEY_DER_BASE64="MCowBQYDK2VwAyEACOqs0foIOJ043D8A0gvp3zBtpTZ+Za074fNunYAegAM="

apply=false
assume_yes=false
manifest_url="$DEFAULT_MANIFEST_URL"
release_public_key_der_base64="$RELEASE_PUBLIC_KEY_DER_BASE64"
artifact_base_url="https://github.com/ConaryLabs/Conary/releases/download"
artifact_base_overridden=false

die() {
    printf 'conary bootstrap: %s\n' "$1" >&2
    exit 1
}

if [[ -n "${CONARY_BOOTSTRAP_PUBLIC_KEY_DER_BASE64:-}" ]]; then
    [[ "${CONARY_BOOTSTRAP_TESTING:-}" == 1 ]] ||
        die "release-key override is available only to the test harness"
    release_public_key_der_base64="$CONARY_BOOTSTRAP_PUBLIC_KEY_DER_BASE64"
fi
if [[ -n "${CONARY_BOOTSTRAP_ARTIFACT_BASE_URL:-}" ]]; then
    [[ "${CONARY_BOOTSTRAP_TESTING:-}" == 1 ]] ||
        die "artifact-origin override is available only to the test harness"
    artifact_base_url="${CONARY_BOOTSTRAP_ARTIFACT_BASE_URL%/}"
    artifact_base_overridden=true
fi

usage() {
    cat <<'EOF'
Usage: install-conary-preview.sh [--apply --yes] [--manifest-url HTTPS_URL]

The default mode downloads and verifies the signed release authority and one
matching native package, then prints the exact non-mutating installation plan.
Installation requires both --apply and the explicit --yes confirmation.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --apply)
            apply=true
            shift
            ;;
        --yes)
            assume_yes=true
            shift
            ;;
        --manifest-url)
            [[ $# -ge 2 ]] || die "--manifest-url requires a value"
            manifest_url="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *) die "unknown argument: $1" ;;
    esac
done

if [[ "$manifest_url" == https://* ]]; then
    curl_protocol_args=(--proto '=https' --tlsv1.2)
elif [[ "${CONARY_BOOTSTRAP_TESTING:-}" == 1 && "$manifest_url" == http://127.0.0.1:* ]]; then
    curl_protocol_args=(--proto '=http')
else
    die "manifest URL must use HTTPS"
fi
if [[ "$artifact_base_overridden" == true && "$artifact_base_url" != http://127.0.0.1:* ]]; then
    die "test artifact origin must be loopback HTTP"
fi
if [[ "$assume_yes" == true && "$apply" != true ]]; then
    die "--yes is valid only with --apply"
fi
if [[ "$apply" == true && "$assume_yes" != true ]]; then
    die "installation requires explicit --apply --yes confirmation"
fi

for command in curl sha256sum base64 openssl stat uname mktemp readlink; do
    command -v "$command" >/dev/null 2>&1 || die "required command is unavailable: $command"
done

work_directory="$(mktemp -d)"
cleanup() {
    rm -rf -- "$work_directory"
}
trap cleanup EXIT

manifest_path="${work_directory}/conary-bootstrap-v1.manifest"
signature_path="${manifest_path}.sig"

download() {
    local url="$1"
    local output="$2"
    curl "${curl_protocol_args[@]}" --fail --location --silent --show-error \
        --output "$output" "$url"
}

download "$manifest_url" "$manifest_path"
download "${manifest_url}.sig" "$signature_path"
[[ -s "$manifest_path" && ! -L "$manifest_path" ]] || die "manifest download is empty or unsafe"
[[ -s "$signature_path" && ! -L "$signature_path" ]] || die "manifest signature is empty or unsafe"

public_key_der="${work_directory}/release-public-key.der"
signature_binary="${work_directory}/manifest.sig"
manifest_hash="${work_directory}/manifest.sha256"
printf '%s' "$release_public_key_der_base64" | base64 --decode > "$public_key_der" ||
    die "embedded release public key is malformed"
base64 --decode < "$signature_path" > "$signature_binary" ||
    die "manifest signature is not valid base64"
[[ "$(stat -c '%s' "$signature_binary")" == 64 ]] ||
    die "manifest signature is not an Ed25519 signature"
sha256sum "$manifest_path" | awk '{printf "%s", $1}' > "$manifest_hash"
openssl pkeyutl -verify -pubin -keyform DER -inkey "$public_key_der" \
    -rawin -in "$manifest_hash" -sigfile "$signature_binary" >/dev/null 2>&1 ||
    die "manifest signature verification failed"

schema=""
tag_name=""
suite_version=""
artifact_count=0
selected_count=0
selected_format=""
selected_basename=""
selected_size=""
selected_digest=""
fedora_count=0
ubuntu_count=0
arch_count=0
fedora_basename=""
ubuntu_basename=""
arch_basename=""

os_release_path="${CONARY_BOOTSTRAP_OS_RELEASE:-/etc/os-release}"
if [[ -n "${CONARY_BOOTSTRAP_OS_RELEASE:-}" && "${CONARY_BOOTSTRAP_TESTING:-}" != 1 ]]; then
    die "host-fact override is available only to the test harness"
fi
os_release_real="$(readlink -f -- "$os_release_path")"
if [[ "${CONARY_BOOTSTRAP_TESTING:-}" != 1 ]]; then
    [[ "$os_release_real" == /etc/os-release || "$os_release_real" == /usr/lib/os-release ]] ||
        die "resolved /etc/os-release is outside the system authority paths"
fi
[[ -f "$os_release_real" && ! -L "$os_release_real" ]] || die "cannot inspect a real /etc/os-release"

os_field() {
    local wanted="$1"
    local count=0
    local value=""
    local line raw
    while IFS= read -r line || [[ -n "$line" ]]; do
        case "$line" in
            "${wanted}="*)
                count=$((count + 1))
                raw="${line#*=}"
                if [[ "$raw" =~ ^\"([A-Za-z0-9._-]+)\"$ ]]; then
                    value="${BASH_REMATCH[1]}"
                elif [[ "$raw" =~ ^[A-Za-z0-9._-]+$ ]]; then
                    value="$raw"
                else
                    die "unsafe ${wanted} value in /etc/os-release"
                fi
                ;;
        esac
    done < "$os_release_real"
    [[ "$count" -le 1 ]] || die "ambiguous duplicate ${wanted} in /etc/os-release"
    printf '%s' "$value"
}

host_id="$(os_field ID)"
host_version="$(os_field VERSION_ID)"
host_build="$(os_field BUILD_ID)"
host_arch="$(uname -m)"
[[ "$host_arch" == x86_64 ]] || die "unsupported architecture: ${host_arch:-unknown}"
case "$host_id" in
    fedora)
        [[ "$host_version" == 44 ]] || die "unsupported Fedora version: ${host_version:-unknown}"
        host_release=44
        ;;
    ubuntu)
        [[ "$host_version" == 26.04 ]] || die "unsupported Ubuntu version: ${host_version:-unknown}"
        host_release=26.04
        ;;
    arch)
        [[ "$host_build" == rolling ]] ||
            die "unsupported or ambiguous Arch release facts"
        [[ -z "$host_version" || "$host_version" =~ ^[0-9]{8}\.[0-9]+\.[0-9]+$ ]] ||
            die "unsupported Arch snapshot identity: ${host_version}"
        host_release=rolling
        ;;
    *) die "unsupported host: ${host_id:-unknown}" ;;
esac

validate_artifact_row() {
    local value="$1"
    local separators="${value//[^|]/}"
    local row_host row_release row_arch row_format row_basename row_size row_digest
    [[ ${#separators} -eq 6 ]] || die "malformed artifact authority"
    IFS='|' read -r row_host row_release row_arch row_format row_basename row_size row_digest <<< "$value"
    [[ "$row_host" =~ ^(fedora|ubuntu|arch)$ ]] || die "invalid artifact host authority"
    [[ "$row_release" =~ ^(44|26\.04|rolling)$ ]] || die "invalid artifact host release"
    [[ "$row_arch" == x86_64 ]] || die "invalid artifact architecture"
    [[ "$row_format" =~ ^(rpm|deb|arch)$ ]] || die "invalid artifact format"
    [[ "$row_basename" =~ ^[A-Za-z0-9._+-]+$ && "$row_basename" != *..* ]] ||
        die "invalid artifact basename"
    [[ "$row_size" =~ ^[1-9][0-9]*$ ]] || die "invalid artifact size"
    [[ "$row_digest" =~ ^[0-9a-f]{64}$ ]] || die "invalid artifact SHA-256"

    case "${row_host}|${row_release}|${row_format}" in
        fedora\|44\|rpm)
            fedora_count=$((fedora_count + 1))
            fedora_basename="$row_basename"
            ;;
        ubuntu\|26.04\|deb)
            ubuntu_count=$((ubuntu_count + 1))
            ubuntu_basename="$row_basename"
            ;;
        arch\|rolling\|arch)
            arch_count=$((arch_count + 1))
            arch_basename="$row_basename"
            ;;
        *) die "inconsistent host and artifact format authority" ;;
    esac

    artifact_count=$((artifact_count + 1))
    if [[ "$row_host" == "$host_id" && "$row_release" == "$host_release" && "$row_arch" == "$host_arch" ]]; then
        selected_count=$((selected_count + 1))
        selected_format="$row_format"
        selected_basename="$row_basename"
        selected_size="$row_size"
        selected_digest="$row_digest"
    fi
}

while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -n "$line" && "$line" != *$'\r'* ]] || die "blank or non-canonical manifest line"
    case "$line" in
        schema=*)
            [[ -z "$schema" ]] || die "duplicate schema authority"
            schema="${line#schema=}"
            ;;
        tag_name=*)
            [[ -z "$tag_name" ]] || die "duplicate tag authority"
            tag_name="${line#tag_name=}"
            ;;
        suite_version=*)
            [[ -z "$suite_version" ]] || die "duplicate version authority"
            suite_version="${line#suite_version=}"
            ;;
        artifact=*) validate_artifact_row "${line#artifact=}" ;;
        *) die "unknown manifest field" ;;
    esac
done < "$manifest_path"

[[ "$schema" == conary-bootstrap-v1 ]] || die "unsupported bootstrap manifest schema"
[[ "$suite_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "invalid suite version authority"
[[ "$tag_name" == "v${suite_version}" ]] || die "tag and suite version authority disagree"
[[ "$artifact_count" -eq 3 ]] || die "manifest must declare exactly three artifacts"
[[ "$fedora_count" -eq 1 && "$ubuntu_count" -eq 1 && "$arch_count" -eq 1 ]] ||
    die "manifest must declare one exact artifact for every supported host"
[[ "$fedora_basename" == "conary-${suite_version}-1.fc44.x86_64.rpm" ]] ||
    die "Fedora artifact basename does not match suite authority"
[[ "$ubuntu_basename" == "conary_${suite_version}-1_amd64.deb" ]] ||
    die "Ubuntu artifact basename does not match suite authority"
[[ "$arch_basename" == "conary-${suite_version}-1-x86_64.pkg.tar.zst" ]] ||
    die "Arch artifact basename does not match suite authority"
[[ "$selected_count" -eq 1 ]] || die "manifest does not select exactly one artifact for this host"

if [[ "$artifact_base_overridden" == true ]]; then
    artifact_url="${artifact_base_url}/${selected_basename}"
else
    artifact_url="${artifact_base_url}/${tag_name}/${selected_basename}"
fi
artifact_path="${work_directory}/${selected_basename}"
download "$artifact_url" "$artifact_path"
[[ -f "$artifact_path" && ! -L "$artifact_path" ]] || die "artifact download is unsafe"
actual_size="$(stat -c '%s' "$artifact_path")"
actual_digest="$(sha256sum "$artifact_path" | awk '{print $1}')"
[[ "$actual_size" == "$selected_size" ]] || die "artifact size verification failed"
[[ "$actual_digest" == "$selected_digest" ]] || die "artifact SHA-256 verification failed"

case "$selected_format" in
    rpm) install_command=(sudo dnf install -y "$artifact_path") ;;
    deb) install_command=(sudo apt-get install -y -- "$artifact_path") ;;
    arch) install_command=(sudo pacman -U --noconfirm -- "$artifact_path") ;;
    *) die "internal unsupported package format" ;;
esac

printf 'Conary bootstrap plan\n'
printf '  release: %s (%s)\n' "$tag_name" "$suite_version"
printf '  host: %s %s %s\n' "$host_id" "$host_release" "$host_arch"
printf '  artifact: %s\n' "$selected_basename"
printf '  size: %s bytes\n' "$selected_size"
printf '  sha256: %s\n' "$selected_digest"
printf '  install:'
printf ' %q' "${install_command[@]}"
printf '\n'
printf '  initialization owner: native release package post-install hook\n'
printf '  health checks: conary --version; conary repo list\n'

if [[ "$apply" != true ]]; then
    printf 'Preview complete; no package transaction was invoked.\n'
    exit 0
fi

if command -v conary >/dev/null 2>&1 && [[ "$(conary --version 2>/dev/null || true)" == "conary ${suite_version}" ]]; then
    printf 'Exact Conary release is already installed; skipping the package transaction.\n'
else
    "${install_command[@]}" || die "native package transaction failed"
fi

[[ "$(conary --version 2>/dev/null || true)" == "conary ${suite_version}" ]] ||
    die "installed Conary version health check failed"
conary repo list >/dev/null || die "package-owned repository health check failed"
printf 'Conary %s is installed and package-owned repository state is inspectable.\n' "$suite_version"
