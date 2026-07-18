#!/usr/bin/env bash
# scripts/test-release-matrix.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MATRIX="${REPO_ROOT}/scripts/release-matrix.sh"

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

assert_eq() {
    local expected="$1"
    local actual="$2"
    local context="${3:-expected [$expected], got [$actual]}"

    if [[ "$expected" != "$actual" ]]; then
        fail "$context"
    fi
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local context="${3:-expected output to contain [$needle]}"

    if [[ "$haystack" != *"$needle"* ]]; then
        fail "$context: $haystack"
    fi
}

cleanup() {
    find "$REPO_ROOT" -maxdepth 1 -type d -name '.tmp-release-matrix-test.*' -exec rm -rf {} +
}

trap cleanup EXIT

run_matrix() {
    bash "$MATRIX" "$@"
}

write_cargo_manifest() {
    local file="$1"
    local name="$2"
    local version="$3"

    cat > "$file" <<EOF
[package]
name = "$name"
version = "$version"
edition = "2024"
authors = ["Conary Contributors"]
license = "MIT"
EOF
}

create_release_fixture() {
    local repo

    repo="$(mktemp -d "${REPO_ROOT}/.tmp-release-matrix-test.XXXXXX")"

    mkdir -p \
        "$repo/scripts" \
        "$repo/apps/conary/man" \
        "$repo/apps/remi" \
        "$repo/apps/conaryd" \
        "$repo/apps/conary-test" \
        "$repo/crates/conary-core" \
        "$repo/crates/conary-bootstrap" \
        "$repo/crates/conary-mcp" \
        "$repo/packaging/rpm" \
        "$repo/packaging/arch" \
        "$repo/packaging/deb/debian" \
        "$repo/packaging/ccs" \
        "$repo/test-bin"

    cp "$REPO_ROOT/scripts/release.sh" "$repo/scripts/release.sh"
    cp "$REPO_ROOT/scripts/release-matrix.sh" "$repo/scripts/release-matrix.sh"
    chmod +x "$repo/scripts/release.sh" "$repo/scripts/release-matrix.sh"

    write_cargo_manifest "$repo/apps/conary/Cargo.toml" "conary" "0.7.0"
    write_cargo_manifest "$repo/crates/conary-core/Cargo.toml" "conary-core" "0.7.0"
    write_cargo_manifest "$repo/crates/conary-bootstrap/Cargo.toml" "conary-bootstrap" "0.7.0"
    write_cargo_manifest "$repo/apps/remi/Cargo.toml" "remi" "0.5.0"
    write_cargo_manifest "$repo/apps/conaryd/Cargo.toml" "conaryd" "0.5.0"
    write_cargo_manifest "$repo/apps/conary-test/Cargo.toml" "conary-test" "0.7.0"
    write_cargo_manifest "$repo/crates/conary-mcp/Cargo.toml" "conary-mcp" "0.7.0"

    printf 'fn main() {}\n' > "$repo/apps/conary/build.rs"
    printf '.TH conary 1 "" "conary 0.7.0"\n' > "$repo/apps/conary/man/conary.1"
    printf '# release fixture lockfile\n' > "$repo/Cargo.lock"
    printf '/apps/conary/man/\n' > "$repo/.gitignore"

    cat > "$repo/test-bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
    update)
        ;;
    build)
        version="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)"/\1/p' apps/conary/Cargo.toml | head -n1)"
        if [[ "${RELEASE_FIXTURE_STALE_MAN:-0}" == "1" ]]; then
            version="0.7.0"
        fi
        printf '.TH conary 1 "" "conary %s" \n' "$version" > apps/conary/man/conary.1
        ;;
    *)
        printf 'unexpected cargo fixture command: %s\n' "$*" >&2
        exit 1
        ;;
esac
EOF
    chmod +x "$repo/test-bin/cargo"

    cat > "$repo/packaging/rpm/conary.spec" <<'EOF'
Name:           conary
Version:        0.7.0
Release:        1
EOF

    cat > "$repo/packaging/arch/PKGBUILD" <<'EOF'
pkgname=conary
pkgver=0.7.0
pkgrel=1
EOF

    cat > "$repo/packaging/deb/debian/changelog" <<'EOF'
conary (0.7.0-1) unstable; urgency=medium

  * Release 0.7.0

 -- Conary Contributors <contributors@conary.io>  Thu, 09 Apr 2026 00:00:00 +0000
EOF

    cat > "$repo/packaging/ccs/ccs.toml" <<'EOF'
version = "0.7.0"
EOF

    printf 'initial conary fixture\n' > "$repo/apps/conary/changes.txt"
    printf 'initial remi fixture\n' > "$repo/apps/remi/changes.txt"
    printf 'initial conaryd fixture\n' > "$repo/apps/conaryd/changes.txt"
    printf 'initial conary-test fixture\n' > "$repo/apps/conary-test/changes.txt"

    (
        cd "$repo"
        git init -q
        git config user.name "Release Matrix Test"
        git config user.email "release-matrix@test"
        git add .
        git commit -q -m "chore: initial fixture"
    )

    printf '%s\n' "$repo"
}

tag_head() {
    local repo="$1"
    local tag="$2"

    (
        cd "$repo"
        git tag "$tag"
    )
}

commit_change() {
    local repo="$1"
    local path="$2"
    local message="$3"

    printf '%s\n' "$message" >> "$repo/$path"
    (
        cd "$repo"
        git add "$path"
        git commit -q -m "$message"
    )
}

commit_empty() {
    local repo="$1"
    local message="$2"

    (
        cd "$repo"
        git commit --allow-empty -q -m "$message"
    )
}

run_release_dry_run() {
    local repo="$1"
    local product="$2"

    (
        cd "$repo"
        ./scripts/release.sh "$product" --dry-run
    )
}

run_release() {
    local repo="$1"
    local product="$2"
    local stale_man="${RELEASE_FIXTURE_STALE_MAN:-0}"

    (
        cd "$repo"
        export PATH="$repo/test-bin:$PATH"
        export RELEASE_FIXTURE_STALE_MAN="$stale_man"
        ./scripts/release.sh "$product"
    )
}

run_repo_matrix() {
    local repo="$1"
    shift

    (
        cd "$repo"
        ./scripts/release-matrix.sh "$@"
    )
}

create_release_policy_fixture() {
    local repo

    repo="$(mktemp -d "${REPO_ROOT}/.tmp-release-matrix-test.XXXXXX")"
    mkdir -p \
        "$repo/scripts" \
        "$repo/.github/workflows" \
        "$repo/docs/operations" \
        "$repo/packaging/rpm" \
        "$repo/packaging/deb" \
        "$repo/packaging/arch" \
        "$repo/packaging/ccs"
    cp "$REPO_ROOT/scripts/release-matrix.sh" "$repo/scripts/release-matrix.sh"
    cp "$REPO_ROOT/.github/workflows/release-build.yml" "$repo/.github/workflows/release-build.yml"
    cp "$REPO_ROOT/.github/workflows/deploy-and-verify.yml" "$repo/.github/workflows/deploy-and-verify.yml"
    cp "$REPO_ROOT/.github/workflows/merge-validation.yml" "$repo/.github/workflows/merge-validation.yml"
    cp "$REPO_ROOT/docs/operations/release-artifact-matrix.md" "$repo/docs/operations/release-artifact-matrix.md"
    cp "$REPO_ROOT/packaging/rpm/Containerfile.build" "$repo/packaging/rpm/Containerfile.build"
    cp "$REPO_ROOT/packaging/deb/Containerfile.build" "$repo/packaging/deb/Containerfile.build"
    cp "$REPO_ROOT/packaging/arch/Containerfile.build" "$repo/packaging/arch/Containerfile.build"
    cp "$REPO_ROOT/packaging/rpm/build.sh" "$repo/packaging/rpm/build.sh"
    cp "$REPO_ROOT/packaging/deb/build.sh" "$repo/packaging/deb/build.sh"
    cp "$REPO_ROOT/packaging/arch/build.sh" "$repo/packaging/arch/build.sh"
    cp "$REPO_ROOT/packaging/ccs/build.sh" "$repo/packaging/ccs/build.sh"
    chmod +x "$repo/scripts/release-matrix.sh"
    printf '%s\n' "$repo"
}

replace_fixture_text_once() {
    local file="$1"
    local old="$2"
    local new="$3"

    python3 - "$file" "$old" "$new" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
old = sys.argv[2]
new = sys.argv[3]
text = path.read_text()
if old not in text:
    raise SystemExit(f"fixture could not find text to replace in {path}: {old}")
path.write_text(text.replace(old, new, 1))
PY
}

assert_check_release_matrix_fails() {
    local repo="$1"
    local expected="$2"
    local output status

    set +e
    output="$(bash "$REPO_ROOT/scripts/check-release-matrix.sh" "$repo" 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "check-release-matrix should fail for fixture containing $expected"
    fi

    assert_contains "$output" "$expected" "check-release-matrix failure should name $expected"
}

test_resolve_tag_remi_canonical() {
    local output
    output="$(run_matrix resolve-tag remi-v0.5.0 --format shell)"
    assert_contains "$output" "product=remi" "canonical remi tag should resolve to remi"
}

test_resolve_tag_remi_legacy() {
    local output
    output="$(run_matrix resolve-tag server-v0.5.0 --format shell)"
    assert_contains "$output" "product=remi" "legacy server tag should resolve to remi"
}

test_resolve_tag_conary_test_legacy() {
    local output
    output="$(run_matrix resolve-tag test-v0.3.0 --format shell)"
    assert_contains "$output" "product=conary-test" "legacy test tag should resolve to conary-test"
}

test_latest_version_from_list_mixed_prefixes() {
    local output
    output="$(run_matrix latest-version-from-list remi server-v0.5.0 remi-v0.4.0 remi-v0.6.0)"
    assert_eq "0.6.0" "$output" "mixed-prefix comparison should choose the highest numeric version"
}

test_field_conary_test_deploy_mode() {
    local output
    output="$(run_matrix field conary-test deploy_mode)"
    assert_eq "none" "$output" "conary-test should not deploy automatically"
}

test_field_conaryd_deploy_mode_paused() {
    local output
    output="$(run_matrix field conaryd deploy_mode)"
    assert_eq "none" "$output" "conaryd staging deploy should remain paused while Forge is retired"
}

test_field_conary_bundle_name() {
    local output
    output="$(run_matrix field conary bundle_name)"
    assert_eq "release-bundle" "$output" "conary should use the release bundle name"
}

test_unknown_tag_prefix_fails() {
    local output status

    set +e
    output="$(run_matrix resolve-tag foo-v1.0.0 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "unknown tag prefix should fail"
    fi

    assert_contains "$output" "unknown tag prefix: foo-v1.0.0" "unknown tag prefix should fail clearly"
}

test_latest_version_from_git_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "server-v1.0.0"
    commit_empty "$repo" "chore: canonical release point"
    tag_head "$repo" "remi-v2.0.0"

    output="$(run_repo_matrix "$repo" latest-version-from-git remi)"
    assert_eq "2.0.0" "$output" "fixture repo should prefer the highest numeric remi version"
}

test_max_owned_version_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    output="$(run_repo_matrix "$repo" max-owned-version conary-test)"
    assert_eq "0.7.0" "$output" "fixture repo should report the highest owned conary-test version"
}

test_assert_owned_version_accepts_matching_manifests() {
    local repo

    repo="$(create_release_fixture)"
    run_repo_matrix "$repo" assert-owned-version conary 0.7.0
}

test_assert_owned_version_rejects_mismatched_manifest() {
    local repo output status

    repo="$(create_release_fixture)"
    write_cargo_manifest "$repo/crates/conary-core/Cargo.toml" "conary-core" "0.7.1"

    set +e
    output="$(run_repo_matrix "$repo" assert-owned-version conary 0.7.0 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "assert-owned-version should reject a mismatched owned manifest"
    fi
    assert_contains \
        "$output" \
        "crates/conary-core/Cargo.toml is 0.7.1, expected 0.7.0" \
        "owned-version mismatch should identify the manifest and versions"
}

test_release_dry_run_remi_legacy_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "server-v0.5.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" remi)"
    assert_contains "$output" "Tag: remi-v0.5.1" "remi should emit canonical tags after legacy history"
}

test_release_dry_run_remi_prefers_highest_numeric_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "server-v1.0.0"
    commit_empty "$repo" "chore: canonical release point"
    tag_head "$repo" "remi-v2.0.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" remi)"
    assert_contains "$output" "Current: remi-v2.0.0" "mixed remi history should choose the highest numeric baseline"
}

test_release_dry_run_conaryd_canonical_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "conaryd-v0.5.0"
    commit_change "$repo" "apps/conaryd/changes.txt" "fix(conaryd): tighten daemon health checks"

    output="$(run_release_dry_run "$repo" conaryd)"
    assert_contains "$output" "Tag: conaryd-v0.5.1" "conaryd should continue on its canonical release line"
}

test_release_dry_run_conary_test_uses_owned_manifest_baseline() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "test-v0.3.0"
    commit_change "$repo" "apps/conary-test/changes.txt" "fix(test): update bundle layout"

    output="$(run_release_dry_run "$repo" conary-test)"
    assert_contains "$output" "Current: conary-test-v0.7.0" "conary-test should respect owned manifest versions"
    assert_contains "$output" "Tag: conary-test-v0.7.1" "conary-test should bump from the owned-manifest baseline"
}

test_release_conary_regenerates_and_stages_man_page() {
    local repo
    local output
    local committed_files
    local tags

    repo="$(create_release_fixture)"
    assert_eq \
        "apps/conary/man/conary.1" \
        "$(git -C "$repo" check-ignore apps/conary/man/conary.1)" \
        "release fixture must reproduce the repository's ignored generated man page"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/conary/changes.txt" "fix(conary): refresh command surface"

    output="$(run_release "$repo" conary)"
    assert_contains "$output" \
        "Regenerated apps/conary/man/conary.1 for 0.7.1" \
        "Conary release should regenerate the versioned man page"

    assert_contains \
        "$(<"$repo/apps/conary/man/conary.1")" \
        "conary 0.7.1" \
        "generated man page should contain the release version"
    if grep -Eq '[[:blank:]]$' "$repo/apps/conary/man/conary.1"; then
        fail "generated man page should not contain trailing whitespace"
    fi

    committed_files="$(git -C "$repo" show --pretty=format: --name-only HEAD)"
    assert_contains "$committed_files" \
        "apps/conary/man/conary.1" \
        "Conary release commit should stage the generated man page"

    tags="$(git -C "$repo" tag --points-at HEAD)"
    assert_contains "$tags" "v0.7.1" "Conary release should tag the regenerated man page commit"
}

test_release_conary_rejects_stale_generated_man_page() {
    local repo
    local output
    local status

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/conary/changes.txt" "fix(conary): refresh command surface"

    set +e
    output="$(RELEASE_FIXTURE_STALE_MAN=1 run_release "$repo" conary 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "Conary release should reject a generated man page with the old version"
    fi

    assert_contains "$output" \
        "does not contain Conary version 0.7.1" \
        "stale generated man page should fail before commit and tag"
    assert_eq "" "$(git -C "$repo" tag --list v0.7.1)" \
        "stale generated man page should not create the release tag"
}

test_check_release_matrix_rejects_conaryd_deploy_jobs_when_paused() {
    local repo
    repo="$(create_release_policy_fixture)"
    cat >> "$repo/.github/workflows/deploy-and-verify.yml" <<'YAML'

  deploy-conaryd:
    name: deploy-conaryd
    runs-on: ubuntu-latest
    steps:
      - run: echo deploy
YAML

    assert_check_release_matrix_fails "$repo" "deploy-conaryd"
}

test_check_release_matrix_rejects_conary_test_deploy_jobs() {
    local repo
    repo="$(create_release_policy_fixture)"
    cat >> "$repo/.github/workflows/deploy-and-verify.yml" <<'YAML'

  verify-conary-test:
    name: verify-conary-test
    runs-on: ubuntu-latest
    steps:
      - run: echo verify
YAML

    assert_check_release_matrix_fails "$repo" "verify-conary-test"
}

test_check_release_matrix_rejects_unpinned_rpm_builder_image() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'image: registry.fedoraproject.org/fedora@sha256:765b2260aa4b4eff379b9a6f983f15fcf41a6f9dda9b272b790e23e92fcbaafb' \
        'image: registry.fedoraproject.org/fedora:44'

    assert_check_release_matrix_fails "$repo" "release-build RPM builder must use the pinned Fedora 44 image"
}

test_check_release_matrix_rejects_unpinned_deb_builder_image() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'image: docker.io/library/ubuntu@sha256:3131b4cc82a783df6c9df078f86e01819a13594b865c2cad47bd1bca2b7063bb' \
        'image: docker.io/library/ubuntu:26.04'

    assert_check_release_matrix_fails "$repo" "release-build DEB builder must use the pinned Ubuntu 26.04 image"
}

test_check_release_matrix_rejects_unpinned_arch_builder_image() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'image: docker.io/library/archlinux@sha256:fe6972d4dc1f660c0c10f4c41b2de8986bab89e7e2955378f8beadb8ebcd7433' \
        'image: docker.io/library/archlinux:latest'

    assert_check_release_matrix_fails "$repo" "release-build Arch builder must use the pinned Arch image"
}

test_check_release_matrix_rejects_unverified_rustup_init() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c  /tmp/rustup-init' \
        '0000000000000000000000000000000000000000000000000000000000000000  /tmp/rustup-init'

    assert_check_release_matrix_fails "$repo" "release-build RPM builder checksum-pinned rustup-init flow"
}

test_check_release_matrix_rejects_unpinned_ccs_toolchain() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'toolchain: 1.96.0' \
        'toolchain: stable'

    assert_check_release_matrix_fails "$repo" "release-build CCS builder pinned Rust toolchain"
}

test_check_release_matrix_rejects_unpinned_arch_toolchain() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'rustup default 1.96.0' \
        'rustup default stable'

    assert_check_release_matrix_fails "$repo" "release-build Arch builder pinned Rust toolchain"
}

test_check_release_matrix_rejects_missing_live_version_assertion() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'bash scripts/release-matrix.sh assert-owned-version "$product" "$version"' \
        'echo "owned version assertion removed"'

    assert_check_release_matrix_fails "$repo" "live tag preparation must match every owned manifest"
}

test_check_release_matrix_rejects_non_failing_artifact_upload() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'if-no-files-found: error' \
        'if-no-files-found: warn'

    assert_check_release_matrix_fails "$repo" "fail-closed release artifact uploads"
}

test_check_release_matrix_rejects_missing_exact_ccs_asset_assertion() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '"release-packages/conary-${VERSION}.ccs"' \
        '"release-packages/conary-${VERSION}.ccs.unchecked"'

    assert_check_release_matrix_fails "$repo" "exact version-matching CCS release asset assertion"
}

test_check_release_matrix_rejects_unpinned_tester_guide_link() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'blob/${TAG_NAME}/docs/guides/agent-assisted-tester-loop.md' \
        'blob/main/docs/guides/agent-assisted-tester-loop.md'

    assert_check_release_matrix_fails "$repo" "tag-pinned tester guide release-note link"
}

test_check_release_matrix_rejects_missing_signer_trust_match() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'release signing key does not match an embedded trusted update key' \
        'release signing key check removed'

    assert_check_release_matrix_fails "$repo" "live signing key must match an embedded trusted update key"
}

test_check_release_matrix_rejects_stale_native_output_policy() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/rpm/build.sh" \
        'find "$OUTPUT" -maxdepth 1 -name '\''*.rpm'\'' -delete' \
        'echo "stale RPM output retained"'

    assert_check_release_matrix_fails "$repo" "RPM build must clean stale package output"
}

test_check_release_matrix_rejects_direct_release_publication() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'gh release create "$TAG_NAME" --draft --generate-notes --verify-tag' \
        'gh release create "$TAG_NAME" release-packages/* --generate-notes --verify-tag'

    assert_check_release_matrix_fails "$repo" "immutable-compatible draft-upload-publish sequence for bundle-conary"
}

test_check_release_matrix_rejects_late_conary_release_notes() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'gh release edit "$TAG_NAME" --notes-file "$release_notes"' \
        'gh release view "$TAG_NAME" --json body'

    assert_check_release_matrix_fails "$repo" "Conary release notes must be finalized before immutable asset publication"
}

test_check_release_matrix_rejects_missing_artifact_row() {
    local repo
    repo="$(create_release_policy_fixture)"
    grep -v '^| `remi` |' "$repo/docs/operations/release-artifact-matrix.md" > "$repo/docs/operations/release-artifact-matrix.md.tmp"
    mv "$repo/docs/operations/release-artifact-matrix.md.tmp" "$repo/docs/operations/release-artifact-matrix.md"

    assert_check_release_matrix_fails "$repo" "release artifact matrix missing remi row"
}

test_check_release_matrix_rejects_unknown_deploy_route_pair() {
    local repo
    repo="$(create_release_policy_fixture)"
    python3 - "$repo/.github/workflows/deploy-and-verify.yml" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
text = path.read_text()
old = "conary:release_bundle|remi:remote_bundle|conaryd:none|conary-test:none)"
new = "conary:release_bundle|remi:remote_bundle|conaryd:none|conary-test:none|remi:none)"
if old not in text:
    raise SystemExit("fixture could not find deploy routing case arm")
path.write_text(text.replace(old, new))
PY

    assert_check_release_matrix_fails "$repo" "unexpected deploy routing pair"
}

main() {
    local -a tests=(
        test_resolve_tag_remi_canonical
        test_resolve_tag_remi_legacy
        test_resolve_tag_conary_test_legacy
        test_latest_version_from_list_mixed_prefixes
        test_field_conary_test_deploy_mode
        test_field_conaryd_deploy_mode_paused
        test_field_conary_bundle_name
        test_unknown_tag_prefix_fails
        test_latest_version_from_git_in_fixture
        test_max_owned_version_in_fixture
        test_assert_owned_version_accepts_matching_manifests
        test_assert_owned_version_rejects_mismatched_manifest
        test_release_dry_run_remi_legacy_history
        test_release_dry_run_remi_prefers_highest_numeric_history
        test_release_dry_run_conaryd_canonical_history
        test_release_dry_run_conary_test_uses_owned_manifest_baseline
        test_release_conary_regenerates_and_stages_man_page
        test_release_conary_rejects_stale_generated_man_page
        test_check_release_matrix_rejects_conaryd_deploy_jobs_when_paused
        test_check_release_matrix_rejects_conary_test_deploy_jobs
        test_check_release_matrix_rejects_unpinned_rpm_builder_image
        test_check_release_matrix_rejects_unpinned_deb_builder_image
        test_check_release_matrix_rejects_unpinned_arch_builder_image
        test_check_release_matrix_rejects_unverified_rustup_init
        test_check_release_matrix_rejects_unpinned_ccs_toolchain
        test_check_release_matrix_rejects_unpinned_arch_toolchain
        test_check_release_matrix_rejects_missing_live_version_assertion
        test_check_release_matrix_rejects_non_failing_artifact_upload
        test_check_release_matrix_rejects_missing_exact_ccs_asset_assertion
        test_check_release_matrix_rejects_unpinned_tester_guide_link
        test_check_release_matrix_rejects_missing_signer_trust_match
        test_check_release_matrix_rejects_stale_native_output_policy
        test_check_release_matrix_rejects_direct_release_publication
        test_check_release_matrix_rejects_late_conary_release_notes
        test_check_release_matrix_rejects_missing_artifact_row
        test_check_release_matrix_rejects_unknown_deploy_route_pair
    )

    local test_name
    for test_name in "${tests[@]}"; do
        "$test_name"
        printf 'ok - %s\n' "$test_name"
    done
}

main "$@"
