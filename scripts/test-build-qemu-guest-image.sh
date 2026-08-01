#!/usr/bin/env bash
# scripts/test-build-qemu-guest-image.sh
set -euo pipefail

# Fast, hermetic checks for scripts/build-qemu-guest-image.sh. These cover
# argument handling, host preflight, and the contract that the image build's
# tool and module lists match what the phase-3 QEMU manifests actually probe.
# They deliberately never boot a guest; the slow proof is a real image build.

repo_root="$(git rev-parse --show-toplevel)"
script="$repo_root/scripts/build-qemu-guest-image.sh"
manifest_dir="$repo_root/apps/conary/tests/integration/remi/manifests"

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

run_script() {
    "$script" "$@" >"$tmp/out" 2>"$tmp/err"
}

[[ -x "$script" ]] || fail "expected $script to be executable"

bash -n "$script" || fail "build-qemu-guest-image.sh has a syntax error"

# --help succeeds and names the required options.
run_script --help || fail "--help should exit 0"
for needle in --output --private-key --disk-size-gib --base-image; do
    grep -q -- "$needle" "$tmp/out" || fail "--help should document $needle"
done

# Missing required arguments fail with a legible message rather than a
# half-built image.
if run_script --output "$tmp/img.qcow2"; then
    fail "missing --private-key should fail"
fi
grep -q -- "--private-key is required" "$tmp/err" ||
    fail "expected a --private-key required error, got: $(cat "$tmp/err")"

if run_script --private-key /dev/null; then
    fail "missing --output should fail"
fi
grep -q -- "--output is required" "$tmp/err" ||
    fail "expected an --output required error, got: $(cat "$tmp/err")"

# An unknown option is rejected instead of silently ignored.
if run_script --output "$tmp/img.qcow2" --private-key /dev/null --nope; then
    fail "unknown argument should fail"
fi
grep -q "Unknown argument: --nope" "$tmp/err" ||
    fail "expected an unknown-argument error, got: $(cat "$tmp/err")"

# The build never overwrites an existing image.
: >"$tmp/existing.qcow2"
if run_script --output "$tmp/existing.qcow2" --private-key /dev/null; then
    fail "an existing output image should fail"
fi
grep -q "output image already exists" "$tmp/err" ||
    fail "expected an existing-output error, got: $(cat "$tmp/err")"

# A key path that is not a usable private key fails before any QEMU work.
if run_script --output "$tmp/img.qcow2" --private-key "$tmp/absent-key"; then
    fail "a missing private key should fail"
fi
grep -q "private key not found" "$tmp/err" ||
    fail "expected a missing-key error, got: $(cat "$tmp/err")"

# Numeric options are validated. This needs a real key file so validation gets
# past the key-existence check.
ssh-keygen -q -t ed25519 -N '' -C test-only -f "$tmp/key" ||
    fail "could not generate a throwaway test key"
if run_script --output "$tmp/img.qcow2" --private-key "$tmp/key" --disk-size-gib big; then
    fail "a non-numeric --disk-size-gib should fail"
fi
grep -q -- "--disk-size-gib must be an integer" "$tmp/err" ||
    fail "expected a --disk-size-gib error, got: $(cat "$tmp/err")"

# Contract check: every command the phase-3 QEMU manifests probe in their
# preflight must be a command this build verifies. Testing the contract rather
# than a pinned list means a manifest that starts requiring a new tool fails
# here instead of failing a suite run hours later.
manifest_commands="$(
    grep -ho 'for cmd in [^;]*; do command -v' "$manifest_dir"/phase3-*.toml |
        sed 's/^for cmd in //; s/; do command -v$//' |
        tr ' ' '\n' |
        sort -u
)"
[[ -n "$manifest_commands" ]] ||
    fail "found no preflight command lists in $manifest_dir; the contract check is not looking at anything"

script_commands="$(
    sed -n '/^REQUIRED_COMMANDS=(/,/^)/p' "$script" |
        sed '1d;$d' |
        tr -d ' ' |
        sort -u
)"
[[ -n "$script_commands" ]] || fail "could not read REQUIRED_COMMANDS from $script"

while read -r cmd; do
    [[ -n "$cmd" ]] || continue
    grep -qx "$cmd" <<<"$script_commands" ||
        fail "phase-3 manifests require '$cmd' in their preflight, but build-qemu-guest-image.sh does not verify it"
done <<<"$manifest_commands"

# The adopted-live-root export proofs need a real EFI source in the fixture.
# Pin both the Fedora package and the exact file the generation builder reads,
# then prove the slow builder checks every declared file before promotion.
script_packages="$(
    sed -n '/^GUEST_PACKAGES=(/,/^)/p' "$script" |
        sed '1d;$d' |
        tr -d ' ' |
        sort -u
)"
grep -qx "systemd-boot-unsigned" <<<"$script_packages" ||
    fail "the QEMU guest package set must install systemd-boot-unsigned"

script_files="$(
    sed -n '/^REQUIRED_FILES=(/,/^)/p' "$script" |
        sed '1d;$d' |
        tr -d ' ' |
        sort -u
)"
grep -qx "/usr/lib/systemd/boot/efi/systemd-bootx64.efi" <<<"$script_files" ||
    fail "the QEMU guest contract must require systemd-boot's exact EFI binary"
grep -Fq 'for path in ${REQUIRED_FILES[*]}; do' "$script" ||
    fail "the image build must preflight every REQUIRED_FILES entry"

# The pinned Fedora input must be internally consistent: the URL has to name the
# same file as the pinned image name, so a half-edited release bump fails here.
image_name="$(sed -n 's/^FEDORA_IMAGE="\(.*\)"$/\1/p' "$script")"
[[ -n "$image_name" ]] || fail "could not read FEDORA_IMAGE from $script"
grep -q "FEDORA_IMAGE_URL=.*\${FEDORA_IMAGE}" "$script" ||
    fail "FEDORA_IMAGE_URL should be derived from FEDORA_IMAGE so they cannot drift"
sha="$(sed -n 's/^FEDORA_IMAGE_SHA256="\(.*\)"$/\1/p' "$script")"
[[ "$sha" =~ ^[0-9a-f]{64}$ ]] || fail "FEDORA_IMAGE_SHA256 is not a sha256 hex digest: $sha"

# The build must discover firmware the same way the harness does, or an image
# can build here and fail to boot under conary-test.
qemu_engine="$repo_root/apps/conary-test/src/engine/qemu.rs"
while read -r path; do
    [[ -n "$path" ]] || continue
    grep -qF "$path" "$script" ||
        fail "conary-test looks for OVMF at $path but build-qemu-guest-image.sh does not"
done < <(sed -n '/^const OVMF_CODE_PATHS/,/^];/p' "$qemu_engine" |
    grep -o '"/[^"]*"' | tr -d '"')

printf 'PASS: build-qemu-guest-image.sh\n'
