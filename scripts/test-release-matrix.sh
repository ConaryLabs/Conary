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
        "$repo/crates/conary-agent-contract" \
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
    write_cargo_manifest \
        "$repo/crates/conary-agent-contract/Cargo.toml" \
        "conary-agent-contract" \
        "0.7.0"

    printf 'fn main() {}\n' > "$repo/apps/conary/build.rs"
    printf '.TH conary 1 "" "conary 0.7.0"\n' > "$repo/apps/conary/man/conary.1"
    printf '# release fixture lockfile\n' > "$repo/Cargo.lock"
    printf '/apps/conary/man/\n' > "$repo/.gitignore"
    printf '%s' $'# Changelog\n\nFixture release history.\n\nEntries are newest first.\n\n## [fixture] - 2026-01-01\n' > "$repo/CHANGELOG.md"

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
    shift 2

    (
        cd "$repo"
        ./scripts/release.sh "$product" --dry-run "$@"
    )
}

run_release() {
    local repo="$1"
    local product="$2"
    local stale_man="${RELEASE_FIXTURE_STALE_MAN:-0}"
    shift 2

    (
        cd "$repo"
        export PATH="$repo/test-bin:$PATH"
        export RELEASE_FIXTURE_STALE_MAN="$stale_man"
        ./scripts/release.sh "$product" "$@"
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
        "$repo/.github/actions/setup-exact-ownership-tests" \
        "$repo/.github/ISSUE_TEMPLATE" \
        "$repo/.github/workflows" \
        "$repo/docs/operations" \
        "$repo/site/src/lib" \
        "$repo/site/src/routes/install" \
        "$repo/packaging/rpm" \
        "$repo/packaging/deb" \
        "$repo/packaging/arch" \
        "$repo/packaging/ccs"
    cp "$REPO_ROOT/scripts/release-matrix.sh" "$repo/scripts/release-matrix.sh"
    cp "$REPO_ROOT/.github/workflows/release-build.yml" "$repo/.github/workflows/release-build.yml"
    cp "$REPO_ROOT/.github/workflows/deploy-and-verify.yml" "$repo/.github/workflows/deploy-and-verify.yml"
    cp "$REPO_ROOT/.github/workflows/release-artifact-proof.yml" "$repo/.github/workflows/release-artifact-proof.yml"
    cp "$REPO_ROOT/.github/workflows/merge-validation.yml" "$repo/.github/workflows/merge-validation.yml"
    cp "$REPO_ROOT/.github/workflows/pr-gate.yml" "$repo/.github/workflows/pr-gate.yml"
    cp "$REPO_ROOT/.github/actions/setup-exact-ownership-tests/action.yml" \
        "$repo/.github/actions/setup-exact-ownership-tests/action.yml"
    cp "$REPO_ROOT/.github/ISSUE_TEMPLATE/pre_alpha_feedback.md" "$repo/.github/ISSUE_TEMPLATE/pre_alpha_feedback.md"
    cp "$REPO_ROOT/docs/operations/release-artifact-matrix.md" "$repo/docs/operations/release-artifact-matrix.md"
    cp "$REPO_ROOT/site/src/lib/preview-release.ts" "$repo/site/src/lib/preview-release.ts"
    cp "$REPO_ROOT/site/src/routes/install/+page.svelte" "$repo/site/src/routes/install/+page.svelte"
    cp "$REPO_ROOT/packaging/rpm/Containerfile.build" "$repo/packaging/rpm/Containerfile.build"
    cp "$REPO_ROOT/packaging/rpm/conary.spec" "$repo/packaging/rpm/conary.spec"
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

test_latest_version_from_list_uses_canonical_tags() {
    local output
    output="$(run_matrix latest-version-from-list remi remi-v0.4.0 remi-v0.6.0)"
    assert_eq "0.6.0" "$output" "canonical tag comparison should choose the highest numeric version"
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

test_historical_tag_prefixes_are_rejected() {
    local tag output status

    for tag in server-v0.5.0 test-v0.3.0; do
        set +e
        output="$(run_matrix resolve-tag "$tag" 2>&1)"
        status=$?
        set -e

        if [[ "$status" -eq 0 ]]; then
            fail "historical tag prefix unexpectedly resolved: $tag"
        fi
        assert_contains "$output" "unknown tag prefix: $tag" "historical tag should fail clearly"
    done
}

test_latest_version_from_git_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "remi-v1.0.0"
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

test_release_dry_run_remi_canonical_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "remi-v0.5.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" remi)"
    assert_contains "$output" "Tag: remi-v0.5.1" "remi should continue its canonical release line"
}

test_release_dry_run_remi_prefers_highest_numeric_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "remi-v1.0.0"
    commit_empty "$repo" "chore: canonical release point"
    tag_head "$repo" "remi-v2.0.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" remi)"
    assert_contains "$output" "Current: remi-v2.0.0" "remi should choose the highest canonical numeric baseline"
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
    tag_head "$repo" "conary-test-v0.3.0"
    commit_change "$repo" "apps/conary-test/changes.txt" "fix(test): update bundle layout"

    output="$(run_release_dry_run "$repo" conary-test)"
    assert_contains "$output" "Current: conary-test-v0.7.0" "conary-test should respect owned manifest versions"
    assert_contains "$output" "Tag: conary-test-v0.7.1" "conary-test should bump from the owned-manifest baseline"
}

test_release_dry_run_accepts_explicit_target() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "remi-v0.5.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"
    commit_change "$repo" "apps/remi/changes.txt" "refactor(remi): hard-cut service schema"

    output="$(run_release_dry_run "$repo" remi --target remi=0.6.0)"
    assert_contains "$output" "Target authority: explicit" "explicit target should own the release decision"
    assert_contains "$output" "Tag: remi-v0.6.0" "explicit target should select the exact canonical tag"
    assert_contains "$output" "### Fixed" "scoped fixes should be categorized in release notes"
    assert_contains "$output" "- tighten deploy flow" "scoped fix prefixes should be removed"
    assert_contains "$output" "### Changed" "refactors should be categorized in release notes"
    assert_contains "$output" "- hard-cut service schema" "scoped refactor prefixes should be removed"
}

test_release_prepare_only_updates_all_conary_test_manifests() {
    local repo
    local committed_head
    local output
    local staged_files

    repo="$(create_release_fixture)"
    tag_head "$repo" "conary-test-v0.7.0"
    commit_change "$repo" "apps/conary-test/changes.txt" "feat(test): add exact lifecycle proof"
    committed_head="$(git -C "$repo" rev-parse HEAD)"

    run_release "$repo" conary-test --prepare-only --target conary-test=0.9.0

    assert_eq "0.9.0" \
        "$(run_repo_matrix "$repo" max-owned-version conary-test)" \
        "prepare-only should update the complete conary-test version set"
    run_repo_matrix "$repo" assert-owned-version conary-test 0.9.0
    assert_eq "$committed_head" "$(git -C "$repo" rev-parse HEAD)" \
        "prepare-only should not create a commit"
    assert_eq "" "$(git -C "$repo" tag --list conary-test-v0.9.0)" \
        "prepare-only should not create a tag"
    staged_files="$(git -C "$repo" diff --cached --name-only)"
    assert_contains "$staged_files" \
        "crates/conary-agent-contract/Cargo.toml" \
        "prepare-only should stage the agent contract version"
    assert_contains "$(<"$repo/CHANGELOG.md")" \
        $'- add exact lifecycle proof\n\n## [fixture]' \
        "release notes should leave a blank line before prior history"

    output="$(run_release_dry_run "$repo" conary-test --target conary-test=0.9.0)"
    assert_contains "$output" \
        "Tag: conary-test-v0.9.0" \
        "prepared explicit target should remain reproducible before publication"
}

test_release_rejects_target_for_unselected_product() {
    local repo
    local output
    local status

    repo="$(create_release_fixture)"
    tag_head "$repo" "remi-v0.5.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    set +e
    output="$(
        cd "$repo"
        ./scripts/release.sh remi --dry-run --target conary=0.8.0 2>&1
    )"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "release target for an unselected product should fail"
    fi
    assert_contains "$output" \
        "release target provided for unselected product: conary" \
        "unselected explicit target should fail clearly"
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

test_check_release_matrix_rejects_beta_maturity_drift() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/ISSUE_TEMPLATE/pre_alpha_feedback.md" \
        'name: Pre-Alpha Tester Feedback' \
        'name: Beta Feedback'

    assert_check_release_matrix_fails \
        "$repo" \
        "public maturity surfaces must identify this project as pre-alpha, not beta"
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

test_check_release_matrix_requires_shared_namespace_setup_in_every_workspace_lane() {
    local repo
    local workflow

    for workflow in pr-gate.yml merge-validation.yml release-build.yml; do
        repo="$(create_release_policy_fixture)"
        replace_fixture_text_once \
            "$repo/.github/workflows/$workflow" \
            '        uses: ./.github/actions/setup-exact-ownership-tests' \
            '        run: echo "exact ownership setup removed"'

        assert_check_release_matrix_fails "$repo" "shared exact ownership setup"
    done
}

test_check_release_matrix_rejects_unproven_namespace_action() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/actions/setup-exact-ownership-tests/action.yml" \
        '        unshare --user --map-root-user --mount --propagation private /bin/true' \
        '        /bin/true'

    assert_check_release_matrix_fails "$repo" "exact ownership namespace proof"
}

test_check_release_matrix_rejects_namespace_setup_after_workspace_tests() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '        uses: ./.github/actions/setup-exact-ownership-tests' \
        '        run: echo "namespace setup delayed"'
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '        run: cargo test --workspace --exclude conary-test --verbose' \
        $'        run: cargo test --workspace --exclude conary-test --verbose\n      - name: Delayed exact ownership setup\n        uses: ./.github/actions/setup-exact-ownership-tests'

    assert_check_release_matrix_fails "$repo" "release workspace validation exact ownership setup order"
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

test_check_release_matrix_rejects_unsigned_embedded_ccs_authority() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'target/release/examples/sign_hash --write-ccs-authority "$authority_dir"' \
        'echo "embedded CCS authority removed"'

    assert_check_release_matrix_fails "$repo" "CCS build must derive embedded authority from the configured release seed"
}

test_check_release_matrix_rejects_unverified_embedded_ccs_authority() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '          target/release/conary ccs verify \' \
        '          echo "embedded CCS verification removed" \'

    assert_check_release_matrix_fails "$repo" "CCS build must verify its embedded release authority"
}

test_check_release_matrix_rejects_unstable_ccs_release_name() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/ccs/build.sh" \
        'mv -- "$BUILT_CCS" "$EXPECTED_CCS"' \
        'echo "stable CCS release name removed"'

    assert_check_release_matrix_fails "$repo" "CCS wrapper must normalize one exact package-release name to the stable self-update asset"
}

test_check_release_matrix_rejects_ambiguous_ccs_target_directory() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/ccs/build.sh" \
        '    --target-dir "$TARGET_DIR"' \
        '    --target-dir target'

    assert_check_release_matrix_fails "$repo" "CCS wrapper must use one explicit Cargo target directory"
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

test_check_release_matrix_rejects_non_tag_static_site_checkout() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/deploy-and-verify.yml" \
        'ref: ${{ needs.resolve.outputs.tag_name }}' \
        'ref: main'

    assert_check_release_matrix_fails "$repo" "static-site checkout must use the serialized release tag"
}

test_check_release_matrix_rejects_single_static_site_deploy() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/deploy-and-verify.yml" \
        'bash deploy/deploy-sites.sh both' \
        'bash deploy/deploy-sites.sh site'

    assert_check_release_matrix_fails "$repo" "both-site deployment from the release tag"
}

main() {
    local -a tests=(
        test_resolve_tag_remi_canonical
        test_latest_version_from_list_uses_canonical_tags
        test_field_conary_test_deploy_mode
        test_field_conaryd_deploy_mode_paused
        test_field_conary_bundle_name
        test_unknown_tag_prefix_fails
        test_historical_tag_prefixes_are_rejected
        test_latest_version_from_git_in_fixture
        test_max_owned_version_in_fixture
        test_assert_owned_version_accepts_matching_manifests
        test_assert_owned_version_rejects_mismatched_manifest
        test_release_dry_run_remi_canonical_history
        test_release_dry_run_remi_prefers_highest_numeric_history
        test_release_dry_run_conaryd_canonical_history
        test_release_dry_run_conary_test_uses_owned_manifest_baseline
        test_release_dry_run_accepts_explicit_target
        test_release_prepare_only_updates_all_conary_test_manifests
        test_release_rejects_target_for_unselected_product
        test_release_conary_regenerates_and_stages_man_page
        test_release_conary_rejects_stale_generated_man_page
        test_check_release_matrix_rejects_beta_maturity_drift
        test_check_release_matrix_rejects_conaryd_deploy_jobs_when_paused
        test_check_release_matrix_rejects_conary_test_deploy_jobs
        test_check_release_matrix_rejects_unpinned_rpm_builder_image
        test_check_release_matrix_rejects_unpinned_deb_builder_image
        test_check_release_matrix_rejects_unpinned_arch_builder_image
        test_check_release_matrix_rejects_unverified_rustup_init
        test_check_release_matrix_rejects_unpinned_ccs_toolchain
        test_check_release_matrix_rejects_unpinned_arch_toolchain
        test_check_release_matrix_rejects_missing_live_version_assertion
        test_check_release_matrix_requires_shared_namespace_setup_in_every_workspace_lane
        test_check_release_matrix_rejects_unproven_namespace_action
        test_check_release_matrix_rejects_namespace_setup_after_workspace_tests
        test_check_release_matrix_rejects_non_failing_artifact_upload
        test_check_release_matrix_rejects_missing_exact_ccs_asset_assertion
        test_check_release_matrix_rejects_unpinned_tester_guide_link
        test_check_release_matrix_rejects_missing_signer_trust_match
        test_check_release_matrix_rejects_unsigned_embedded_ccs_authority
        test_check_release_matrix_rejects_unverified_embedded_ccs_authority
        test_check_release_matrix_rejects_unstable_ccs_release_name
        test_check_release_matrix_rejects_ambiguous_ccs_target_directory
        test_check_release_matrix_rejects_stale_native_output_policy
        test_check_release_matrix_rejects_direct_release_publication
        test_check_release_matrix_rejects_late_conary_release_notes
        test_check_release_matrix_rejects_missing_artifact_row
        test_check_release_matrix_rejects_unknown_deploy_route_pair
        test_check_release_matrix_rejects_non_tag_static_site_checkout
        test_check_release_matrix_rejects_single_static_site_deploy
    )

    local test_name
    for test_name in "${tests[@]}"; do
        "$test_name"
        printf 'ok - %s\n' "$test_name"
    done
}

main "$@"
