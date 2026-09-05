#!/usr/bin/env bash
# scripts/test-install-conary-preview.sh -- Focused release-bootstrap protocol tests.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="${REPO_ROOT}/site/static/install-conary-preview.sh"
MANIFEST_BUILDER="${REPO_ROOT}/scripts/bootstrap-manifest.sh"
TEST_ROOT="$(mktemp -d)"

cleanup() {
    rm -rf -- "$TEST_ROOT"
}
trap cleanup EXIT

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

assert_contains() {
    local text="$1"
    local expected="$2"
    [[ "$text" == *"$expected"* ]] || fail "expected output to contain: $expected"
}

embedded_key_base64="$(
    sed -nE 's/^readonly RELEASE_PUBLIC_KEY_DER_BASE64="([^"]+)"$/\1/p' "$INSTALLER"
)"
embedded_key_hex="$(
    printf '%s' "$embedded_key_base64" |
        base64 --decode |
        tail -c 32 |
        od -An -v -tx1 |
        tr -d ' \n'
)"
core_release_key="$(
    sed -n '/pub const TRUSTED_UPDATE_KEYS/,/];/p' \
        "${REPO_ROOT}/crates/conary-core/src/self_update.rs" |
        grep -Eo '[0-9a-f]{64}'
)"
[[ -n "$embedded_key_base64" && "$embedded_key_hex" == "$core_release_key" ]] ||
    fail "installer release key does not match conary-core release authority"

downloads="${TEST_ROOT}/downloads"
release_files="${TEST_ROOT}/release-files"
mock_bin="${TEST_ROOT}/bin"
mkdir -p "$downloads" "$release_files" "$mock_bin"

version=9.8.7
tag="v${version}"
rpm="conary-${version}-1.fc44.x86_64.rpm"
deb="conary_${version}-1_amd64.deb"
arch="conary-${version}-1-x86_64.pkg.tar.zst"
printf 'rpm release bytes\n' > "${release_files}/${rpm}"
printf 'deb release bytes\n' > "${release_files}/${deb}"
printf 'arch release bytes\n' > "${release_files}/${arch}"

manifest="${downloads}/conary-bootstrap-v1.manifest"
bash "$MANIFEST_BUILDER" "$tag" "$version" "$release_files" "$manifest"
cp "${release_files}/${rpm}" "${downloads}/${rpm}"
cp "${release_files}/${deb}" "${downloads}/${deb}"
cp "${release_files}/${arch}" "${downloads}/${arch}"

private_key="${TEST_ROOT}/release.private.pem"
public_key_der="${TEST_ROOT}/release.public.der"
openssl genpkey -algorithm ED25519 -out "$private_key" >/dev/null 2>&1
openssl pkey -in "$private_key" -pubout -outform DER -out "$public_key_der"
public_key_base64="$(base64 -w0 "$public_key_der")"

sign_manifest() {
    local hash_file="${TEST_ROOT}/manifest.sha256"
    local signature_file="${TEST_ROOT}/manifest.sig"
    sha256sum "$manifest" | awk '{printf "%s", $1}' > "$hash_file"
    openssl pkeyutl -sign -inkey "$private_key" -rawin \
        -in "$hash_file" -out "$signature_file"
    base64 -w0 "$signature_file" > "${manifest}.sig"
}
sign_manifest

cat > "${mock_bin}/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
output=""
url=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --output)
            output="$2"
            shift 2
            ;;
        --proto|--tlsv1.2)
            shift 2
            ;;
        --fail|--location|--silent|--show-error)
            shift
            ;;
        *)
            url="$1"
            shift
            ;;
    esac
done
[[ -n "$output" && -n "$url" ]]
basename="${url##*/}"
cp "${FIXTURE_DOWNLOAD_DIR}/${basename}" "$output"
EOF

cat > "${mock_bin}/uname" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == -m ]]
printf '%s\n' "${MOCK_UNAME:-x86_64}"
EOF

cat > "${mock_bin}/sudo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%q ' "$@" >> "$MOCK_INSTALL_LOG"
printf '\n' >> "$MOCK_INSTALL_LOG"
[[ "${MOCK_INSTALL_FAIL:-0}" != 1 ]] || exit 42
: > "$MOCK_INSTALLED_STATE"
EOF

cat > "${mock_bin}/conary" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ -f "$MOCK_INSTALLED_STATE" ]] || exit 1
case "${1:-}" in
    --version) printf 'conary %s\n' "$MOCK_VERSION" ;;
    repo)
        [[ "${2:-}" == list ]]
        [[ "${MOCK_HEALTH_FAIL:-0}" != 1 ]]
        printf 'remi\n'
        ;;
    *) exit 2 ;;
esac
EOF
chmod +x "${mock_bin}/curl" "${mock_bin}/uname" "${mock_bin}/sudo" "${mock_bin}/conary"

fedora_os="${TEST_ROOT}/fedora-os-release"
ubuntu_os="${TEST_ROOT}/ubuntu-os-release"
arch_os="${TEST_ROOT}/arch-os-release"
arch_snapshot_os="${TEST_ROOT}/arch-snapshot-os-release"
unsupported_os="${TEST_ROOT}/unsupported-os-release"
printf 'ID=fedora\nVERSION_ID=44\n' > "$fedora_os"
printf 'ID="ubuntu"\nVERSION_ID="26.04"\n' > "$ubuntu_os"
printf 'ID=arch\nBUILD_ID=rolling\n' > "$arch_os"
printf 'ID=arch\nBUILD_ID=rolling\nVERSION_ID=20260712.0.555161\n' > "$arch_snapshot_os"
printf 'ID=debian\nVERSION_ID=13\n' > "$unsupported_os"

install_log="${TEST_ROOT}/install.log"
installed_state="${TEST_ROOT}/installed"

run_installer() {
    local os_release="$1"
    shift
    set +e
    output="$({
        env \
            PATH="${mock_bin}:$PATH" \
            FIXTURE_DOWNLOAD_DIR="$downloads" \
            MOCK_INSTALL_LOG="$install_log" \
            MOCK_INSTALLED_STATE="$installed_state" \
            MOCK_VERSION="$version" \
            MOCK_UNAME="${MOCK_UNAME:-x86_64}" \
            MOCK_INSTALL_FAIL="${MOCK_INSTALL_FAIL:-0}" \
            MOCK_HEALTH_FAIL="${MOCK_HEALTH_FAIL:-0}" \
            CONARY_BOOTSTRAP_TESTING=1 \
            CONARY_BOOTSTRAP_OS_RELEASE="$os_release" \
            CONARY_BOOTSTRAP_PUBLIC_KEY_DER_BASE64="$public_key_base64" \
            "$INSTALLER" --manifest-url https://fixtures.invalid/conary-bootstrap-v1.manifest "$@"
    } 2>&1)"
    status=$?
    set -e
}

assert_manifest_contract() {
    [[ "$(sed -n '1p' "$manifest")" == schema=conary-bootstrap-v1 ]] ||
        fail "manifest schema is not canonical"
    [[ "$(rg -c '^artifact=' "$manifest")" == 3 ]] ||
        fail "manifest must contain three artifact rows"
    assert_contains "$(<"$manifest")" "tag_name=${tag}"
    assert_contains "$(<"$manifest")" "suite_version=${version}"
}
assert_manifest_contract

run_installer "$fedora_os"
[[ "$status" -eq 0 ]] || fail "Fedora preview failed: $output"
assert_contains "$output" "host: fedora 44 x86_64"
assert_contains "$output" "artifact: $rpm"
assert_contains "$output" "sudo dnf install -y"
[[ "$output" != *"sudo dnf install -y --"* ]] || fail "Fedora plan used unsupported DNF5 separator"
assert_contains "$output" "initialization owner: native release package post-install hook"
assert_contains "$output" "health checks: conary --version; conary repo list"
assert_contains "$output" "Preview complete; no package transaction was invoked."
[[ ! -e "$install_log" && ! -e "$installed_state" ]] || fail "preview mutated installation state"

fedora_os_link="${TEST_ROOT}/fedora-os-release-link"
ln -s "$fedora_os" "$fedora_os_link"
run_installer "$fedora_os_link"
[[ "$status" -eq 0 ]] || fail "canonical os-release symlink failed: $output"

run_installer "$ubuntu_os"
[[ "$status" -eq 0 ]] || fail "Ubuntu preview failed: $output"
assert_contains "$output" "host: ubuntu 26.04 x86_64"
assert_contains "$output" "artifact: $deb"
assert_contains "$output" "sudo apt-get install -y"

run_installer "$arch_os"
[[ "$status" -eq 0 ]] || fail "Arch preview failed: $output"
assert_contains "$output" "host: arch rolling x86_64"
assert_contains "$output" "artifact: $arch"

run_installer "$arch_snapshot_os"
[[ "$status" -eq 0 ]] || fail "Arch snapshot preview failed: $output"
assert_contains "$output" "host: arch rolling x86_64"
assert_contains "$output" "artifact: $arch"
assert_contains "$output" "sudo pacman -U --noconfirm"

cp "$manifest" "${TEST_ROOT}/valid.manifest"
cp "${manifest}.sig" "${TEST_ROOT}/valid.manifest.sig"
printf '\n' >> "$manifest"
run_installer "$fedora_os"
[[ "$status" -ne 0 ]] || fail "tampered manifest unexpectedly passed"
assert_contains "$output" "manifest signature verification failed"
cp "${TEST_ROOT}/valid.manifest" "$manifest"

printf 'unknown=value\n' >> "$manifest"
sign_manifest
run_installer "$fedora_os"
[[ "$status" -ne 0 ]] || fail "signed unknown manifest field unexpectedly passed"
assert_contains "$output" "unknown manifest field"
cp "${TEST_ROOT}/valid.manifest" "$manifest"

printf 'tag_name=%s\n' "$tag" >> "$manifest"
sign_manifest
run_installer "$fedora_os"
[[ "$status" -ne 0 ]] || fail "duplicate tag authority unexpectedly passed"
assert_contains "$output" "duplicate tag authority"
cp "${TEST_ROOT}/valid.manifest" "$manifest"
cp "${TEST_ROOT}/valid.manifest.sig" "${manifest}.sig"

run_installer "$unsupported_os"
[[ "$status" -ne 0 ]] || fail "unsupported host unexpectedly passed"
assert_contains "$output" "unsupported host: debian"

MOCK_UNAME=aarch64 run_installer "$fedora_os"
[[ "$status" -ne 0 ]] || fail "unsupported architecture unexpectedly passed"
assert_contains "$output" "unsupported architecture: aarch64"

duplicate_os="${TEST_ROOT}/duplicate-os-release"
printf 'ID=fedora\nID=ubuntu\nVERSION_ID=44\n' > "$duplicate_os"
run_installer "$duplicate_os"
[[ "$status" -ne 0 ]] || fail "ambiguous host facts unexpectedly passed"
assert_contains "$output" "ambiguous duplicate ID"

cp "${downloads}/${rpm}" "${TEST_ROOT}/valid.rpm"
printf 'tampered artifact\n' > "${downloads}/${rpm}"
run_installer "$fedora_os"
[[ "$status" -ne 0 ]] || fail "wrong artifact unexpectedly passed"
assert_contains "$output" "artifact SHA-256 verification failed"
cp "${TEST_ROOT}/valid.rpm" "${downloads}/${rpm}"

mv "${manifest}.sig" "${TEST_ROOT}/held.manifest.sig"
run_installer "$fedora_os"
[[ "$status" -ne 0 ]] || fail "missing signature download unexpectedly passed"
assert_contains "$output" "No such file or directory"
mv "${TEST_ROOT}/held.manifest.sig" "${manifest}.sig"

rm -f -- "$install_log" "$installed_state"
MOCK_INSTALL_FAIL=1 run_installer "$fedora_os" --apply --yes
[[ "$status" -ne 0 ]] || fail "failed native package command unexpectedly passed"
assert_contains "$output" "native package transaction failed"

rm -f -- "$install_log" "$installed_state"
run_installer "$fedora_os" --apply --yes
[[ "$status" -eq 0 ]] || fail "successful apply failed: $output"
assert_contains "$output" "package-owned repository state is inspectable"
[[ "$(wc -l < "$install_log")" -eq 1 ]] || fail "apply did not invoke exactly one package transaction"

run_installer "$fedora_os" --apply --yes
[[ "$status" -eq 0 ]] || fail "idempotent apply failed: $output"
assert_contains "$output" "already installed; skipping the package transaction"
[[ "$(wc -l < "$install_log")" -eq 1 ]] || fail "idempotent apply repeated the package transaction"

MOCK_HEALTH_FAIL=1 run_installer "$fedora_os" --apply --yes
[[ "$status" -ne 0 ]] || fail "repository health failure unexpectedly passed"
assert_contains "$output" "package-owned repository health check failed"

run_installer "$fedora_os" --apply
[[ "$status" -ne 0 ]] || fail "apply without confirmation unexpectedly passed"
assert_contains "$output" "requires explicit --apply --yes confirmation"

printf 'release bootstrap installer tests passed\n'
